
use std::{collections::BTreeMap,fs,io::{Read,Write},net::TcpListener,sync::{Arc,Mutex,atomic::{AtomicBool,Ordering}},thread,time::Duration};
use serde_json::json;
#[derive(Default)]
struct State {exists:bool,next:u64,assets:BTreeMap<String,(u64,Vec<u8>)>,uploads:Vec<String>,deleted:Vec<String>,fail_version:bool,fail_data:bool,corrupt:bool}
struct Server {base:String,state:Arc<Mutex<State>>,stop:Arc<AtomicBool>,thread:Option<thread::JoinHandle<()>>}
impl Server {
    fn start()->Self {
        let listener=TcpListener::bind("127.0.0.1:0").unwrap();listener.set_nonblocking(true).unwrap();
        let base=format!("http://{}",listener.local_addr().unwrap());
        let state=Arc::new(Mutex::new(State::default()));let stop=Arc::new(AtomicBool::new(false));
        let captured=state.clone();let halt=stop.clone();let origin=base.clone();
        let worker=thread::spawn(move||{
            while !halt.load(Ordering::Relaxed) {
                let (mut stream,_)=match listener.accept(){Ok(value)=>value,Err(error) if error.kind()==std::io::ErrorKind::WouldBlock=>{thread::sleep(Duration::from_millis(5));continue;},Err(error)=>panic!("{error}")};
                stream.set_nonblocking(false).unwrap();
                stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
                let mut head=Vec::new();while !head.ends_with(b"\r\n\r\n") {let mut byte=[0];stream.read_exact(&mut byte).unwrap();head.push(byte[0]);assert!(head.len()<65536);}
                let text=String::from_utf8(head).unwrap();let mut first=text.lines().next().unwrap().split_whitespace();
                let method=first.next().unwrap();let path=first.next().unwrap();
                let headers=text.lines().skip(1).filter_map(|line|line.split_once(':')).map(|(k,v)|(k.to_ascii_lowercase(),v.trim().to_string())).collect::<BTreeMap<_,_>>();
                assert_eq!(headers.get("authorization").map(String::as_str),Some("Bearer fixture-token"));
                let length=headers.get("content-length").map(|v|v.parse::<usize>().unwrap()).unwrap_or(0);
                let mut body=vec![0;length];stream.read_exact(&mut body).unwrap();
                let mut data=captured.lock().unwrap();let mut status=200;
                let response=if path.starts_with("/repos/owner/repo/releases/tags/") {
                    if data.exists {json!({"id":1,"tag_name":"data","upload_url":format!("{origin}/upload{{?name,label}}")}).to_string().into_bytes()}else{status=404;b"{}".to_vec()}
                }else if method=="POST"&&path=="/repos/owner/repo/releases" {
                    let value:serde_json::Value=serde_json::from_slice(&body).unwrap();assert_eq!(value["tag_name"],"data");assert_eq!(value["draft"],false);
                    data.exists=true;status=201;json!({"id":1,"tag_name":"data","upload_url":format!("{origin}/upload{{?name,label}}")}).to_string().into_bytes()
                }else if path.starts_with("/repos/owner/repo/releases/1/assets?") {
                    serde_json::to_vec(&data.assets.iter().map(|(name,(id,bytes))|json!({"id":id,"name":name,"size":bytes.len()})).collect::<Vec<_>>()).unwrap()
                }else if method=="POST"&&path.starts_with("/upload?name=") {
                    let name=path.strip_prefix("/upload?name=").unwrap().to_owned();
                    if (data.fail_version&&name=="VERSION.zip")||(data.fail_data&&name.starts_with("output")) {status=503;b"{}".to_vec()}else{
                        assert!(!data.assets.contains_key(&name));data.next+=1;let id=data.next;
                        data.uploads.push(name.clone());let size=body.len();data.assets.insert(name.clone(),(id,body));
                        status=201;json!({"id":id,"name":name,"size":size}).to_string().into_bytes()
                    }
                }else if let Some(id)=path.strip_prefix("/repos/owner/repo/releases/assets/") {
                    let id:u64=id.parse().unwrap();let name=data.assets.iter().find(|(_,value)|value.0==id).map(|(name,_)|name.clone()).unwrap();
                    if method=="DELETE" {data.deleted.push(name.clone());data.assets.remove(&name);status=204;Vec::new()}else{let mut bytes=data.assets[&name].1.clone();if data.corrupt&&!bytes.is_empty(){let index=bytes.len()/2;bytes[index]^=0x80;}bytes}
                }else {panic!("Unexpected fixture route {method} {path}")};
                write!(stream,"HTTP/1.1 {status} Fixture\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",response.len()).unwrap();
                stream.write_all(&response).unwrap();
            }
        });
        Self{base,state,stop,thread:Some(worker)}
    }
    fn finish(mut self){self.stop.store(true,Ordering::Relaxed);self.thread.take().unwrap().join().unwrap();}
}

fn path(value:impl AsRef<std::path::Path>)->camino::Utf8PathBuf {
    camino::Utf8PathBuf::from_path_buf(value.as_ref().to_owned()).unwrap()
}
#[test]
fn actual_xet_github_whole_store_round_trip_retains_prior_export_on_failure() {
    let program=std::env::var_os("PROJCLI_TEST_XET_ENGINE");
    assert!(program.is_some()||std::env::var_os("PROJCLI_TEST_XET_REQUIRED").is_none());
    let Some(program)=program else{return};
    let engine=proj::xet::Engine::new(program).unwrap();
    let temp=tempfile::tempdir().unwrap();let root=path(temp.path());let project=root.join("project");
    let source=project.join("raw");let store=root.join("store");fs::create_dir_all(&source).unwrap();
    fs::write(source.join("data.csv"),b"original bytes\n").unwrap();
    let first=proj::xet::publish(&project,&store,&[("raw-data".into(),source.clone())],"v1.0.0",&engine).unwrap();
    let server=Server::start();
    let client=proj::github_release::GithubReleaseClient::new("owner/repo",&server.base,Some("fixture-token".into())).unwrap();
    let reference=proj::xet_github::upload(&project,&store,&client,"data",true,64*1024*1024,&engine).unwrap();
    assert_eq!(reference.structure,"xet");
    assert!(reference.snapshots.contains(&first));
    let prior={let state=server.state.lock().unwrap();assert_eq!(state.uploads.len(),1);state.assets.clone()};
    fs::remove_dir_all(&store).unwrap();fs::remove_dir_all(&source).unwrap();
    let imported=proj::xet_github::download(&project,&client,&reference,64*1024*1024,64*1024*1024,&engine).unwrap();
    let restored=project.join("restored");
    proj::xet::restore(&project,imported.path(),&first,"raw-data",&restored,true,&engine).unwrap();
    assert_eq!(fs::read(restored.join("data.csv")).unwrap(),b"original bytes\n");
    fs::create_dir_all(&source).unwrap();fs::write(source.join("data.csv"),b"updated bytes\n").unwrap();
    let second=proj::xet::publish(&project,imported.path(),&[("raw-data".into(),source)],"v1.0.1",&engine).unwrap();
    // Return corrupt downloadable bytes after accepting the unique new upload.
    server.state.lock().unwrap().corrupt=true;
    assert!(proj::xet_github::upload(&project,imported.path(),&client,"data",true,64*1024*1024,&engine).is_err());
    {let state=server.state.lock().unwrap();assert!(state.deleted.is_empty());for (name,value) in &prior {assert_eq!(&state.assets[name],value);}}
    assert_eq!(fs::read(restored.join("data.csv")).unwrap(),b"original bytes\n");
    server.state.lock().unwrap().corrupt=false;
    // Retry uses the existing immutable new ZIP rather than deleting either version.
    let updated=proj::xet_github::upload(&project,imported.path(),&client,"data",true,64*1024*1024,&engine).unwrap();
    assert!(updated.snapshots.contains(&first)&&updated.snapshots.contains(&second));
    let fresh=proj::xet_github::download(&project,&client,&updated,64*1024*1024,64*1024*1024,&engine).unwrap();
    proj::xet::restore(&project,fresh.path(),&second,"raw-data",&restored,true,&engine).unwrap();
    assert_eq!(fs::read(restored.join("data.csv")).unwrap(),b"updated bytes\n");
    {let state=server.state.lock().unwrap();assert!(state.deleted.is_empty());assert_eq!(state.assets.len(),2);}
    println!("Mocked GitHub complete-store download/local-update/upload verified both snapshots and retained prior ZIP on failed download verification.");
    server.finish();
}
