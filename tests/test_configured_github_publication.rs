
use std::{collections::BTreeMap,fs,io::{Read,Write},net::TcpListener,path::Path,process::{Command,Output},sync::{Arc,Mutex,atomic::{AtomicBool,Ordering}},thread,time::Duration};
use serde_json::json;
#[derive(Default)]
struct State {exists:bool,next:u64,assets:BTreeMap<String,(u64,Vec<u8>)>,uploads:Vec<String>,deleted:Vec<String>,fail_version:bool}
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
                    if data.fail_version&&name=="VERSION.zip" {status=503;b"{}".to_vec()}else{
                        assert!(!data.assets.contains_key(&name));data.next+=1;let id=data.next;
                        data.uploads.push(name.clone());let size=body.len();data.assets.insert(name.clone(),(id,body));
                        status=201;json!({"id":id,"name":name,"size":size}).to_string().into_bytes()
                    }
                }else if let Some(id)=path.strip_prefix("/repos/owner/repo/releases/assets/") {
                    let id:u64=id.parse().unwrap();let name=data.assets.iter().find(|(_,value)|value.0==id).map(|(name,_)|name.clone()).unwrap();
                    if method=="DELETE" {data.deleted.push(name.clone());data.assets.remove(&name);status=204;Vec::new()}else{data.assets[&name].1.clone()}
                }else {panic!("Unexpected fixture route {method} {path}")};
                write!(stream,"HTTP/1.1 {status} Fixture\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",response.len()).unwrap();
                stream.write_all(&response).unwrap();
            }
        });
        Self{base,state,stop,thread:Some(worker)}
    }
    fn finish(mut self){self.stop.store(true,Ordering::Relaxed);self.thread.take().unwrap().join().unwrap();}
}
fn command(root:&Path,program:&str)->Command {
    let mut command=Command::new(program);command.current_dir(root);
    for (key,_) in std::env::vars_os() {
        let text=key.to_string_lossy().to_ascii_uppercase();
        if text.starts_with("GIT_")||text.starts_with("GH_")||text.starts_with("GITHUB_")||text.starts_with("PROJR_")||text.starts_with("PROJCLI_")||text=="SSH_AUTH_SOCK"||text=="QUARTO_PROFILE" {command.env_remove(key);}
    }
    command.env_remove(proj::PROJECT_ROOT_ENV).env_remove(proj::RUN_CONTEXT_ENV)
        .env("HOME",root.join("home")).env("USERPROFILE",root.join("home")).env("XDG_CONFIG_HOME",root.join("home"))
        .env("GIT_CONFIG_NOSYSTEM","1").env("GIT_CONFIG_GLOBAL",root.join("home/gitconfig")).env("GIT_TERMINAL_PROMPT","0");
    command
}
fn setup(server:&Server)->(tempfile::TempDir,std::path::PathBuf) {
    let temporary=tempfile::tempdir().unwrap();let root=temporary.path().canonicalize().unwrap();
    fs::create_dir(root.join("home")).unwrap();fs::create_dir(root.join("results")).unwrap();
    fs::write(root.join("results/result.csv"),"actual fixture bytes\n").unwrap();fs::write(root.join("VERSION"),"v1.2.3\n").unwrap();
    fs::write(root.join("CHANGELOG.md"),"curated release\n").unwrap();
    fs::write(root.join("_proj.yml"),"directories:\n  output: {path: results}\nbuild:\n  git: false\n  scripts: []\n  github:\n    data:\n      content: [output]\n      send: {inspect: none}\n").unwrap();
    for args in [vec!["init"],vec!["remote","add","origin","https://127.0.0.1/owner/repo.git"]] {
        assert!(command(&root,"git").args(args).output().unwrap().status.success());
    }
    assert!(server.base.starts_with("http://127.0.0.1:"));
    (temporary,root)
}
fn invoke(server:&Server,root:&Path,args:&[&str])->Output {
    command(root,env!("CARGO_BIN_EXE_proj")).env("GITHUB_API_URL",&server.base).env("GITHUB_PAT","fixture-token")
        .args(["--project"]).arg(root).args(args).output().unwrap()
}
fn success(output:Output){assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stderr));}
fn members(bytes:&[u8])->BTreeMap<String,Vec<u8>> {
    let mut archive=zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();let mut result=BTreeMap::new();
    for index in 0..archive.len(){let mut file=archive.by_index(index).unwrap();if file.is_dir(){continue;}let name=file.name().unwrap().into_owned();let mut bytes=Vec::new();file.read_to_end(&mut bytes).unwrap();result.insert(name,bytes);}
    result
}
#[test]
fn actual_build_creates_release_data_and_metadata_then_restores_and_retains_old_archives() {
    let server=Server::start();let (_temporary,root)=setup(&server);
    success(invoke(&server,&root,&["build","--non-interactive"]));
    {
        let state=server.state.lock().unwrap();assert_eq!(state.uploads,["output-v1.2.4.zip","manifest.zip","VERSION.zip","CHANGELOG.zip"]);
        assert_eq!(members(&state.assets["output-v1.2.4.zip"].1)["result.csv"],b"actual fixture bytes\n");
        assert_eq!(members(&state.assets["VERSION.zip"].1)["VERSION"],b"Project: v1.2.4\noutput: v1.2.4\n");
        assert_eq!(members(&state.assets["manifest.zip"].1)["manifest.csv"],fs::read(root.join("manifest.csv")).unwrap());
    }
    fs::write(root.join("results/result.csv"),"second fixture\n").unwrap();success(invoke(&server,&root,&["build","--non-interactive"]));
    {
        let state=server.state.lock().unwrap();assert!(state.assets.contains_key("output-v1.2.4.zip"));assert!(state.assets.contains_key("output-v1.2.5.zip"));
        assert_eq!(state.deleted,["manifest.zip","VERSION.zip","CHANGELOG.zip"]);
    }
    fs::write(root.join("results/result.csv"),"damaged").unwrap();
    success(invoke(&server,&root,&["remote","restore","--label","output","--clear","--repository","owner/repo","--max-bytes","1000000","--max-expanded-bytes","1000000"]));
    assert_eq!(fs::read(root.join("results/result.csv")).unwrap(),b"second fixture\n");
    assert_eq!(fs::read_to_string(root.join("VERSION")).unwrap().trim(),"Version: v1.2.5.1");
    server.finish();
}
#[test]
fn metadata_upload_failure_preserves_local_release_and_reports_partial_publication() {
    let server=Server::start();let (_temporary,root)=setup(&server);server.state.lock().unwrap().fail_version=true;
    let output=invoke(&server,&root,&["build","--non-interactive"]);assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("partially published"));
    assert_eq!(fs::read_to_string(root.join("VERSION")).unwrap().trim(),"Version: v1.2.4");
    assert_eq!(fs::read(root.join("results/result.csv")).unwrap(),b"actual fixture bytes\n");
    {let state=server.state.lock().unwrap();assert!(state.assets.contains_key("output-v1.2.4.zip"));assert!(state.assets.contains_key("manifest.zip"));assert!(!state.assets.contains_key("VERSION.zip"));}
    server.finish();
}
#[test]
fn existing_current_archive_is_rejected_before_any_upload_or_deletion() {
    let server=Server::start();let (_temporary,root)=setup(&server);
    {let mut state=server.state.lock().unwrap();state.exists=true;state.next=1;state.assets.insert("output-v1.2.4.zip".into(),(1,b"existing bytes".to_vec()));}
    let output=invoke(&server,&root,&["build","--non-interactive"]);assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Current GitHub archive already exists"));
    {let state=server.state.lock().unwrap();assert!(state.uploads.is_empty());assert!(state.deleted.is_empty());assert_eq!(state.assets["output-v1.2.4.zip"].1,b"existing bytes");}
    server.finish();
}

#[test]
fn empty_archives_use_native_sentinel_and_preview_makes_no_requests() {
    let server=Server::start();let (_temporary,root)=setup(&server);
    success(invoke(&server,&root,&["build-dev","--non-interactive"]));
    assert!(!server.state.lock().unwrap().exists);
    fs::remove_file(root.join("results/result.csv")).unwrap();
    success(invoke(&server,&root,&["build","--non-interactive"]));
    let state=server.state.lock().unwrap();
    let (name,(_,bytes))=state.assets.iter().find(|(name,_)|name.starts_with("output-v")).unwrap();
    assert!(name.ends_with("-empty.zip"));
    assert_eq!(members(bytes),BTreeMap::from([("projr-empty".into(),Vec::new())]));
    drop(state);server.finish();
}
#[test]
fn unsupported_decisions_fail_before_hooks_and_catalog_aliases_never_upload() {
    for state in ["manifest","strategy","latest","alias"] {
        let server=Server::start();let (_temporary,root)=setup(&server);
        let mut config=fs::read_to_string(root.join("_proj.yml")).unwrap();
        config=match state {
            "manifest"=>config.replace("inspect: none","inspect: manifest"),
            "strategy"=>config.replace("inspect: none","inspect: none, strategy: sync-diff"),
            "latest"=>config.replace("      content:","      structure: latest\n      content:"),
            _=>config,
        };
        if state!="alias" {
            config=config.replace("build:\n","build:\n  hooks: {pre: [start.py]}\n");
            fs::write(root.join("start.py"),"from pathlib import Path\nPath('hook-ran').write_text('bad')\n").unwrap();
        }else{
            let mut data=server.state.lock().unwrap();data.exists=true;data.assets.insert("MANIFEST.zip".into(),(1,b"keep".to_vec()));
        }
        fs::write(root.join("_proj.yml"),config).unwrap();
        let output=invoke(&server,&root,&["build","--non-interactive"]);assert!(!output.status.success(),"{state}");
        assert!(!root.join("hook-ran").exists());
        if state!="alias" {assert!(!root.join("manifest.csv").exists());assert_eq!(fs::read(root.join("VERSION")).unwrap(),b"v1.2.3\n");}
        let data=server.state.lock().unwrap();assert!(data.uploads.is_empty());assert!(data.deleted.is_empty());drop(data);server.finish();
    }
}

#[test]
fn enterprise_api_never_uses_default_public_github_credential_helpers() {
    let server=Server::start();let (_temporary,root)=setup(&server);
    let helper="!f() { printf 'username=fixture\\npassword=public-only\\n'; echo called > helper-called; }; f";
    assert!(command(&root,"git").args(["config","credential.helper",helper]).output().unwrap().status.success());
    let output=command(&root,env!("CARGO_BIN_EXE_proj")).env("GITHUB_API_URL",&server.base)
        .args(["--project"]).arg(&root).args(["build","--non-interactive"]).output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("requires an explicit environment token"));
    assert!(!root.join("helper-called").exists());
    assert!(!server.state.lock().unwrap().exists);
    assert_eq!(fs::read(root.join("VERSION")).unwrap(),b"v1.2.3\n");
    server.finish();
}
