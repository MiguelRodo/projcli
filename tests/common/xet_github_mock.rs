
use std::{collections::BTreeMap,io::{Read,Write},net::TcpListener,sync::{Arc,Mutex,atomic::{AtomicBool,Ordering}},thread,time::Duration};
use serde_json::json;
#[derive(Default)]
pub(crate) struct State {pub(crate) exists:bool,pub(crate) next:u64,pub(crate) assets:BTreeMap<String,(u64,Vec<u8>)>,pub(crate) uploads:Vec<String>,pub(crate) deleted:Vec<String>,pub(crate) fail_version:bool,pub(crate) fail_data:bool,pub(crate) corrupt:bool,pub(crate) fail_xet:bool,pub(crate) disconnect_xet:bool}
pub(crate) struct Server {pub(crate) base:String,pub(crate) state:Arc<Mutex<State>>,stop:Arc<AtomicBool>,thread:Option<thread::JoinHandle<()>>}
impl Server {
    pub(crate) fn start()->Self {
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
                    if (data.fail_xet&&name.starts_with("proj-xet-store-"))||(data.fail_version&&name=="VERSION.zip")||(data.fail_data&&name.starts_with("output")) {status=503;b"{}".to_vec()}else{
                        assert!(!data.assets.contains_key(&name));data.next+=1;let id=data.next;
                        data.uploads.push(name.clone());let size=body.len();data.assets.insert(name.clone(),(id,body));
                        status=201;json!({"id":id,"name":name,"size":size}).to_string().into_bytes()
                    }
                }else if let Some(id)=path.strip_prefix("/repos/owner/repo/releases/assets/") {
                    let id:u64=id.parse().unwrap();let name=data.assets.iter().find(|(_,value)|value.0==id).map(|(name,_)|name.clone()).unwrap();
                    if method=="DELETE" {data.deleted.push(name.clone());data.assets.remove(&name);status=204;Vec::new()}else{let mut bytes=data.assets[&name].1.clone();if data.corrupt&&!bytes.is_empty(){let index=bytes.len()/2;bytes[index]^=0x80;}bytes}
                }else {panic!("Unexpected fixture route {method} {path}")};
                if data.disconnect_xet&&method=="POST"&&path.starts_with("/upload?name=proj-xet-store-")&&status==201 {continue;}
                write!(stream,"HTTP/1.1 {status} Fixture\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",response.len()).unwrap();
                stream.write_all(&response).unwrap();
            }
        });
        Self{base,state,stop,thread:Some(worker)}
    }
    pub(crate) fn finish(mut self){self.stop.store(true,Ordering::Relaxed);self.thread.take().unwrap().join().unwrap();}
}

