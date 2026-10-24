//! Synthetic loopback Release assets only; no live data, credentials or Git configuration.
use std::{collections::BTreeMap,fs,io::{Read,Write},net::TcpListener,path::{Path,PathBuf},
    process::{Command,Output},sync::{Arc,Mutex,atomic::{AtomicBool,Ordering}},thread,time::Duration};
use serde_json::{json,Value};
use proj::catalogue::{Catalogue,Format,Inventory};
const META:&str="projcli-manifests-v1.zip";
#[derive(Default)]
struct State {
    tags:BTreeMap<String,u64>,next_release:u64,next:u64,
    assets:BTreeMap<(u64,String),(u64,Vec<u8>)>,
    uploads:Vec<(u64,String)>,deleted:Vec<(u64,String)>,downloads:Vec<(u64,String)>,
    fail_metadata:Option<u64>,race_metadata:bool,metadata_downloaded:bool,
}
struct Server {base:String,state:Arc<Mutex<State>>,stop:Arc<AtomicBool>,thread:Option<thread::JoinHandle<()>>}
impl Server {
    fn start()->Self {
        let listener=TcpListener::bind("127.0.0.1:0").unwrap();listener.set_nonblocking(true).unwrap();
        let base=format!("http://{}",listener.local_addr().unwrap());let origin=base.clone();
        let state=Arc::new(Mutex::new(State::default()));let captured=state.clone();
        let stop=Arc::new(AtomicBool::new(false));let stopped=stop.clone();
        let worker=thread::spawn(move||while !stopped.load(Ordering::Relaxed) {
            let (mut stream,_)=match listener.accept(){Ok(value)=>value,Err(error) if error.kind()==std::io::ErrorKind::WouldBlock=>{
                thread::sleep(Duration::from_millis(2));continue;},Err(error)=>panic!("{error}")};
            stream.set_nonblocking(false).unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
            let mut head=Vec::new();let mut byte=[0];
            while !head.ends_with(b"\r\n\r\n"){stream.read_exact(&mut byte).unwrap();head.push(byte[0]);}
            let text=String::from_utf8(head).unwrap();let mut lines=text.lines();let request:Vec<_>=lines.next().unwrap().split(' ').collect();
            let (method,path)=(request[0],request[1]);
            let headers:BTreeMap<_,_>=lines.filter_map(|line|line.split_once(':')).map(|(key,value)|(key.to_ascii_lowercase(),value.trim().to_owned())).collect();
            assert_eq!(headers.get("authorization").unwrap(),"Bearer fixture-token");
            let length=headers.get("content-length").map(|value|value.parse::<usize>().unwrap()).unwrap_or(0);
            let mut body=vec![0;length];stream.read_exact(&mut body).unwrap();
            let mut data=captured.lock().unwrap();let mut status=200;
            let response=if let Some(tag)=path.strip_prefix("/repos/owner/repo/releases/tags/") {
                if let Some(id)=data.tags.get(tag) {json!({"id":id,"tag_name":tag,"upload_url":format!("{origin}/upload/{id}{{?name,label}}")}).to_string().into_bytes()}
                else{status=404;b"{}".to_vec()}
            }else if method=="POST"&&path=="/repos/owner/repo/releases" {
                let value:Value=serde_json::from_slice(&body).unwrap();let tag=value["tag_name"].as_str().unwrap().to_owned();
                assert_eq!(value["draft"],false);data.next_release+=1;let id=data.next_release;assert!(data.tags.insert(tag.clone(),id).is_none());
                status=201;json!({"id":id,"tag_name":tag,"upload_url":format!("{origin}/upload/{id}{{?name,label}}")}).to_string().into_bytes()
            }else if let Some((id,_))=path.strip_prefix("/repos/owner/repo/releases/").and_then(|rest|rest.split_once("/assets?")) {
                let id:u64=id.parse().unwrap();
                if data.race_metadata&&data.metadata_downloaded {
                    data.race_metadata=false;data.next+=1;let next=data.next;
                    if let Some(value)=data.assets.get_mut(&(id,META.into())) {value.0=next;}
                }
                serde_json::to_vec(&data.assets.iter().filter(|((release,_),_)|*release==id)
                    .map(|((_,name),(id,bytes))|json!({"id":id,"name":name,"size":bytes.len()})).collect::<Vec<_>>()).unwrap()
            }else if method=="POST"&&path.starts_with("/upload/") {
                let (release,name)=path.strip_prefix("/upload/").unwrap().split_once("?name=").unwrap();
                let release:u64=release.parse().unwrap();let name=name.to_owned();let key=(release,name.clone());
                if data.fail_metadata==Some(release)&&name==META {status=503;b"{}".to_vec()}
                else if data.assets.contains_key(&key){status=422;b"{}".to_vec()}
                else {
                    data.next+=1;let id=data.next;data.uploads.push(key.clone());let size=body.len();data.assets.insert(key,(id,body));
                    status=201;json!({"id":id,"name":name,"size":size}).to_string().into_bytes()
                }
            }else if let Some(id)=path.strip_prefix("/repos/owner/repo/releases/assets/") {
                let id:u64=id.parse().unwrap();let key=data.assets.iter().find(|(_,value)|value.0==id).map(|(key,_)|key.clone());
                if let Some(key)=key {
                    if method=="DELETE"{data.deleted.push(key.clone());data.assets.remove(&key);status=204;Vec::new()}
                    else {
                        data.downloads.push(key.clone());if key.1==META{data.metadata_downloaded=true;}
                        data.assets[&key].1.clone()
                    }
                }else{status=404;b"{}".to_vec()}
            }else{panic!("Unexpected fixture route {method} {path}")};
            write!(stream,"HTTP/1.1 {status} Fixture\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",response.len()).unwrap();
            stream.write_all(&response).unwrap();
        });
        Self{base,state,stop,thread:Some(worker)}
    }
    fn insert(&self,tag:&str,name:&str,bytes:Vec<u8>)->u64 {
        let mut data=self.state.lock().unwrap();
        let release=if let Some(id)=data.tags.get(tag){*id}else{
            data.next_release+=1;let id=data.next_release;data.tags.insert(tag.into(),id);id
        };
        data.next+=1;let id=data.next;data.assets.insert((release,name.into()),(id,bytes));id
    }
    fn assets(&self,tag:&str)->BTreeMap<String,Vec<u8>> {
        let data=self.state.lock().unwrap();let Some(release)=data.tags.get(tag) else{return BTreeMap::new()};
        data.assets.iter().filter(|((id,_),_)|id==release).map(|((_,name),(_,bytes))|(name.clone(),bytes.clone())).collect()
    }
}
impl Drop for Server {fn drop(&mut self){self.stop.store(true,Ordering::Relaxed);self.thread.take().unwrap().join().unwrap();}}
fn command(root:&Path,program:&str)->Command {
    let mut command=Command::new(program);command.current_dir(root);
    for (key,_) in std::env::vars_os() {
        let name=key.to_string_lossy().to_ascii_uppercase();
        if ["GIT_","GH_","GITHUB_","PROJR_","PROJCLI_"].iter().any(|prefix|name.starts_with(prefix))
            ||matches!(name.as_str(),"SSH_AUTH_SOCK"|"QUARTO_PROFILE"){command.env_remove(key);}
    }
    command.env_remove(proj::PROJECT_ROOT_ENV).env_remove(proj::RUN_CONTEXT_ENV)
        .env("HOME",root.join("home")).env("USERPROFILE",root.join("home")).env("XDG_CONFIG_HOME",root.join("home"))
        .env("GIT_CONFIG_NOSYSTEM","1").env("GIT_CONFIG_GLOBAL",root.join("home/gitconfig")).env("GIT_TERMINAL_PROMPT","0");
    command
}
fn setup(mode:&str,placement:&str,layout:&str)->(tempfile::TempDir,PathBuf,Value) {
    let temp=tempfile::tempdir().unwrap();let root=temp.path().canonicalize().unwrap();
    for name in ["home","results"]{fs::create_dir(root.join(name)).unwrap();}
    fs::write(root.join("results/result.csv"),b"shared fixture bytes\n").unwrap();
    fs::write(root.join("results/caf\u{e9},notes.txt"),b"shared fixture bytes\n").unwrap();
    fs::write(root.join("VERSION"),b"v1.2.3\n").unwrap();
    let config=json!({"directories":{"output":{"path":"results"}},
        "remotes":{"github":{"data":{"structure":layout,"manifest":placement,"manifest-format":mode,"send":{"inspect":"file"}}}},
        "build":{"scripts":[],"git":false,"dest":{"data":{}}}});
    save(&root,&config);
    for args in [vec!["init"],vec!["remote","add","origin","https://127.0.0.1/owner/repo.git"]] {
        assert!(command(&root,"git").args(args).output().unwrap().status.success());
    }
    (temp,root,config)
}
fn save(root:&Path,value:&Value){fs::write(root.join("_proj.yml"),serde_json::to_vec_pretty(value).unwrap()).unwrap();}
fn invoke(server:&Server,root:&Path)->Output {
    command(root,env!("CARGO_BIN_EXE_proj")).env("GITHUB_API_URL",&server.base).env("GITHUB_PAT","fixture-token")
        .args(["--machine","--non-interactive","--project"]).arg(root).arg("build").output().unwrap()
}
fn build(server:&Server,root:&Path){let output=invoke(server,root);assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stderr));}
fn members(bytes:&[u8])->BTreeMap<String,Vec<u8>> {
    let mut archive=zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();let mut result=BTreeMap::new();
    for index in 0..archive.len(){let mut file=archive.by_index(index).unwrap();if file.is_dir(){continue;}
        let name=file.name().unwrap().into_owned();let mut bytes=Vec::new();file.read_to_end(&mut bytes).unwrap();assert!(result.insert(name,bytes).is_none());}
    result
}
fn fixture_zip(files:&[(&str,&[u8])])->Vec<u8> {
    let mut output=std::io::Cursor::new(Vec::new());{
        let mut writer=zip::ZipWriter::new(&mut output);
        for (name,bytes) in files {writer.start_file(*name,zip::write::SimpleFileOptions::default()).unwrap();writer.write_all(bytes).unwrap();}
        writer.finish().unwrap();
    }output.into_inner()
}
fn files(root:&Path)->BTreeMap<String,Vec<u8>> {
    fn collect(root:&Path,path:&Path,result:&mut BTreeMap<String,Vec<u8>>) {
        for entry in fs::read_dir(path).unwrap(){let entry=entry.unwrap();let path=entry.path();
            if entry.file_type().unwrap().is_dir(){collect(root,&path,result)}else{
                assert!(entry.file_type().unwrap().is_file());result.insert(path.strip_prefix(root).unwrap().to_string_lossy().replace(std::path::MAIN_SEPARATOR,"/"),fs::read(path).unwrap());}}
    }
    let mut result=BTreeMap::new();if root.exists(){collect(root,root,&mut result)}result
}
fn project(root:&Path,title:&str)->PathBuf {
    let mut hash=blake3::Hasher::new_derive_key("projcli.ordinary-remote-name.v1");
    hash.update(&serde_json::to_vec(&("github",title.to_uppercase())).unwrap());let id=hash.finalize().to_hex().to_string();
    root.join(".proj/remotes").join(&id[..2]).join(&id[2..])
}
fn filename(mode:&str)->&'static str {match mode{"csv"=>"catalogues/v1/manifest.csv","json"=>"catalogues/v1/manifest.json",
    "csv-dedupe-file"=>"catalogues/v1/manifest-dedupe-file.csv","json-dedupe-file"=>"catalogues/v1/manifest-dedupe-file.json",_=>panic!("mode")}}
fn history(rows:&BTreeMap<String,Vec<u8>>,mode:&str)->Catalogue {
    Catalogue::parse(&rows[filename(mode)],serde_json::from_value::<Format>(json!(mode)).unwrap()).unwrap()
}
fn bindings(rows:&BTreeMap<String,Vec<u8>>,namespace:&str)->Vec<Value> {
    rows.iter().filter(|(name,_)|name.starts_with(namespace)).map(|(name,bytes)|{
        let suffix=name.strip_prefix(namespace).unwrap();let (prefix,rest)=suffix.split_once('/').unwrap();
        assert_eq!(prefix.len(),2);assert_eq!(rest.len(),67);serde_json::from_slice(bytes).unwrap()
    }).collect()
}
fn assert_payload(history:&Catalogue,version:&str,payload:&BTreeMap<String,Vec<u8>>) {
    let snapshot=history.expanded_snapshot(version,&["output".into()]).unwrap();let directory=&snapshot.directories[0];
    let actual:BTreeMap<_,_>=directory.files.iter().map(|file|(file.path.clone(),file.hash.clone())).collect();
    let expected:BTreeMap<_,_>=payload.iter().filter(|(name,_)|name.as_str()!="projr-empty")
        .map(|(name,bytes)|(name.clone(),blake3::hash(bytes).to_hex().to_string())).collect();assert_eq!(actual,expected);
}
#[test]
fn all_formats_placements_and_layouts_record_exact_zip_bytes_and_unchanged_version_references() {
    let mut ids=BTreeMap::new();
    for layout in ["latest","archive"]{for mode in ["csv","json","csv-dedupe-file","json-dedupe-file"]{for placement in ["auto","project","remote","both"] {
        let server=Server::start();let (_temp,root,_config)=setup(mode,placement,layout);build(&server,&root);
        let first=server.assets("data");let payload=if layout=="latest"{"output.zip"}else{"output-v1.2.4.zip"};
        let expected=members(&first[payload]);let project_path=project(&root,"data");let project_rows=files(&project_path);
        assert_eq!(first.contains_key(META),placement!="project");
        assert_eq!(project_rows.contains_key(filename(mode)),placement!="remote");
        let full=if placement=="project"{project_rows.clone()}else{members(&first[META])};
        assert_payload(&history(&full,mode),"v1.2.4",&expected);
        let refs=bindings(&project_rows,"references/v1/");assert_eq!(refs.len(),1);
        assert_eq!(refs[0]["repository"],"owner/repo");assert_eq!(refs[0]["tag"],"data");
        assert_eq!(refs[0]["payloads"][0]["asset"],payload);assert_eq!(refs[0]["payloads"][0]["empty"],false);
        let id=project_rows.keys().find(|name|name.starts_with("references/v1/")).unwrap().clone();
        if let Some(previous)=ids.get(layout){assert_eq!(&id,previous)}else{ids.insert(layout,id);}
        build(&server,&root);let second=server.assets("data");assert_eq!(second[payload],first[payload]);
        if layout=="archive"{assert!(!second.contains_key("output-v1.2.5.zip"));}
        let project_rows=files(&project_path);let full=if placement=="project"{project_rows.clone()}else{members(&second[META])};
        let history=history(&full,mode);assert_eq!(history.records.len(),2);
        assert!(matches!(&history.records[1].inventory,Inventory::Reference{ref_version} if ref_version=="v1.2.4"));
        assert_payload(&history,"v1.2.5",&expected);assert_eq!(bindings(&project_rows,"references/v1/").len(),2);
        let data=server.state.lock().unwrap();
        if placement!="project"{assert_eq!(data.uploads.last().unwrap().1,META);}
        else{assert!(!data.uploads.iter().any(|(_,name)|name==META));}
    }}}
}
#[test]
fn actual_final_upload_strategy_union_is_recorded_without_claiming_source_agreement() {
    for strategy in ["upload-all","upload-missing","sync-diff","sync-purge"] {
        let server=Server::start();let (_temp,root,mut config)=setup("json-dedupe-file","both","latest");
        config["remotes"]["github"]["data"]["send"]["strategy"]=json!(strategy);save(&root,&config);
        server.insert("data","output.zip",fixture_zip(&[("result.csv",b"older"),("retained",b"remote only")]));
        build(&server,&root);let assets=server.assets("data");let actual=members(&assets["output.zip"]);let metadata=members(&assets[META]);
        assert_payload(&history(&metadata,"json-dedupe-file"),"v1.2.4",&actual);
        assert_eq!(actual.contains_key("retained"),strategy.starts_with("upload-"));
        assert_eq!(actual["result.csv"],if strategy=="upload-missing"{b"older".to_vec()}else{b"shared fixture bytes\n".to_vec()});
        let refs=bindings(&metadata,"snapshots/v1/");let version=refs[0]["payloads"][0]["advertised_version"].as_str().unwrap();
        assert_eq!(version.ends_with('*'),strategy.starts_with("upload-"));
    }
}
#[test]
fn archive_catalog_only_placeholder_never_becomes_an_empty_recorded_payload() {
    let server=Server::start();let (_temp,root,_config)=setup("csv","both","archive");build(&server,&root);
    server.insert("data","manifest.zip",fixture_zip(&[("manifest.csv",b"label,fn,version,hash\noutput,result.csv,v1.2.4,00000000000000000000000000000000\n")]));
    server.state.lock().unwrap().downloads.clear();build(&server,&root);
    let assets=server.assets("data");assert!(assets.contains_key("output-v1.2.5.zip"));
    assert_payload(&history(&members(&assets[META]),"csv"),"v1.2.5",&members(&assets["output-v1.2.5.zip"]));
    assert!(!server.state.lock().unwrap().downloads.iter().any(|(_,name)|name=="output-v1.2.4.zip"));
}
#[test]
fn project_remote_both_and_format_switch_preserve_earlier_complete_history() {
    let server=Server::start();let (_temp,root,mut config)=setup("csv","project","archive");build(&server,&root);
    let old=fs::read(project(&root,"data").join(filename("csv"))).unwrap();
    config["remotes"]["github"]["data"]["manifest"]=json!("remote");
    config["remotes"]["github"]["data"]["manifest-format"]=json!("json");save(&root,&config);build(&server,&root);
    assert_eq!(fs::read(project(&root,"data").join(filename("csv"))).unwrap(),old);
    assert_eq!(history(&members(&server.assets("data")[META]),"json").records.len(),2);
    config["remotes"]["github"]["data"]["manifest"]=json!("both");
    config["remotes"]["github"]["data"]["manifest-format"]=json!("csv-dedupe-file");save(&root,&config);build(&server,&root);
    let remote=history(&members(&server.assets("data")[META]),"csv-dedupe-file");
    assert_eq!(remote.records.len(),3);assert_eq!(history(&files(&project(&root,"data")),"csv-dedupe-file"),remote);
    assert_eq!(fs::read(project(&root,"data").join(filename("csv"))).unwrap(),old);
}
#[test]
fn failed_metadata_replacement_keeps_old_project_reference_and_local_release() {
    let server=Server::start();let (_temp,root,_config)=setup("json","both","latest");build(&server,&root);
    let before=files(&project(&root,"data"));server.state.lock().unwrap().fail_metadata=Some(1);
    fs::write(root.join("results/result.csv"),b"new result").unwrap();
    let output=invoke(&server,&root);assert!(!output.status.success());
    assert_eq!(files(&project(&root,"data")),before);assert!(!server.assets("data").contains_key(META));
    assert_eq!(members(&server.assets("data")["output.zip"])["result.csv"],b"new result");
    assert_eq!(fs::read_to_string(root.join("VERSION")).unwrap().trim(),"Version: v1.2.5");
}
#[test]
fn later_target_failure_prevents_all_new_project_success_bindings() {
    let server=Server::start();let (_temp,root,mut config)=setup("json","both","latest");
    let mut second=config["remotes"]["github"]["data"].clone();second["path"]=json!("other");
    config["remotes"]["github"]["second"]=second;config["build"]["dest"]["second"]=json!({});
    save(&root,&config);server.state.lock().unwrap().fail_metadata=Some(2);
    let output=invoke(&server,&root);assert!(!output.status.success());
    assert!(server.assets("data").contains_key(META));assert!(server.assets("second").contains_key("other-output.zip"));
    assert!(files(&project(&root,"data")).is_empty());assert!(files(&project(&root,"second")).is_empty());
}
#[test]
fn stale_metadata_identity_is_not_deleted_or_replaced() {
    let server=Server::start();let (_temp,root,_config)=setup("csv","both","latest");build(&server,&root);
    let project_before=files(&project(&root,"data"));let old=server.assets("data")[META].clone();
    {let mut state=server.state.lock().unwrap();state.race_metadata=true;state.metadata_downloaded=false;state.deleted.clear();state.uploads.clear();}
    let output=invoke(&server,&root);assert!(!output.status.success());
    assert_eq!(server.assets("data")[META],old);assert_eq!(files(&project(&root,"data")),project_before);
    assert!(!server.state.lock().unwrap().deleted.iter().any(|(_,name)|name==META));
    assert!(String::from_utf8_lossy(&output.stderr).contains("metadata asset changed"));
}
#[test]
fn unknown_or_corrupt_bundle_members_and_namespace_aliases_fail_without_remote_mutation() {
    for (name,zip) in [(META,fixture_zip(&[("unexpected.txt",b"untrusted")])),
        (META,fixture_zip(&[("catalogues/v1/manifest.json",b"{}")])),
        ("PROJCLI-MANIFESTS-V1.ZIP",fixture_zip(&[("preserved",b"old")]))] {
        let server=Server::start();let (_temp,root,_config)=setup("json","both","latest");
        server.insert("data",name,zip);let before=server.assets("data");
        assert!(!invoke(&server,&root).status.success());assert_eq!(server.assets("data"),before);
        let data=server.state.lock().unwrap();assert!(data.uploads.is_empty());assert!(data.deleted.is_empty());
        assert!(files(&project(&root,"data")).is_empty());
    }
}

#[test]
fn named_default_csv_and_legacy_inline_without_new_policy_remain_distinct() {
    let server=Server::start();let (_temp,root,mut config)=setup("csv","auto","latest");
    for key in ["manifest","manifest-format"]{config["remotes"]["github"]["data"].as_object_mut().unwrap().remove(key);}
    save(&root,&config);build(&server,&root);
    assert_eq!(history(&members(&server.assets("data")[META]),"csv").records.len(),1);
    let server=Server::start();let (_temp,root,mut config)=setup("csv","auto","latest");
    let mut definition=config["remotes"]["github"]["data"].take();
    for key in ["manifest","manifest-format"]{definition.as_object_mut().unwrap().remove(key);}
    definition["content"]=json!(true);config.as_object_mut().unwrap().remove("remotes");
    config["build"].as_object_mut().unwrap().remove("dest");config["build"]["github"]=json!({"data":definition});
    save(&root,&config);build(&server,&root);assert!(!server.assets("data").contains_key(META));assert!(!project(&root,"data").exists());
}
#[test]
fn uploaded_empty_children_and_transport_sentinel_are_not_invented_as_user_files() {
    let server=Server::start();let (_temp,root,mut config)=setup("json","both","latest");
    config["remotes"]["github"]["data"]["send"]["strategy"]=json!("upload-all");save(&root,&config);
    let mut bytes=std::io::Cursor::new(Vec::new());{
        let mut zip=zip::ZipWriter::new(&mut bytes);let options=zip::write::SimpleFileOptions::default();
        zip.add_directory("unretained-empty/",options).unwrap();
        zip.start_file("retained",options).unwrap();zip.write_all(b"kept").unwrap();zip.finish().unwrap();
    }
    server.insert("data","output.zip",bytes.into_inner());build(&server,&root);
    let metadata=members(&server.assets("data")[META]);let first_history=history(&metadata,"json");
    let snapshot=first_history.expanded_snapshot("v1.2.4",&["output".into()]).unwrap();
    assert!(snapshot.directories[0].empty_directories.is_empty());
    assert_payload(&first_history,"v1.2.4",&members(&server.assets("data")["output.zip"]));
    config["remotes"]["github"]["data"]["send"]["strategy"]=json!("sync-purge");save(&root,&config);
    for entry in fs::read_dir(root.join("results")).unwrap(){fs::remove_file(entry.unwrap().path()).unwrap();}
    build(&server,&root);let assets=server.assets("data");assert!(assets.contains_key("output-empty.zip"));assert!(!assets.contains_key("output.zip"));
    let metadata=members(&assets[META]);assert_payload(&history(&metadata,"json"),"v1.2.5",&BTreeMap::new());
    let refs=bindings(&metadata,"snapshots/v1/");let empty=refs.iter().find(|value|value["version"]=="v1.2.5").unwrap();
    assert_eq!(empty["payloads"][0]["empty"],true);assert_eq!(empty["payloads"][0]["asset"],"output-empty.zip");
}
#[test]
fn new_flat_config_still_restores_via_ordinary_compatible_metadata_without_changing_project_history() {
    for layout in ["latest","archive"] {
        let server=Server::start();let (_temp,root,_config)=setup("json-dedupe-file","remote",layout);build(&server,&root);
        let assets=server.assets("data");let payload=if layout=="latest"{"output.zip"}else{"output-v1.2.4.zip"};
        let expected=members(&assets[payload]);let before=files(&project(&root,"data"));
        let csv=fs::read(root.join("manifest.csv")).unwrap();let version=fs::read(root.join("VERSION")).unwrap();
        fs::write(root.join("results/result.csv"),b"damage").unwrap();fs::write(root.join("results/local-only"),b"remove").unwrap();
        let output=command(&root,env!("CARGO_BIN_EXE_proj")).env("GITHUB_API_URL",&server.base).env("GITHUB_PAT","fixture-token")
            .args(["--machine","--non-interactive","--project"]).arg(&root)
            .args(["remote","restore","--label","output","--title","data","--provider","github","--repository","owner/repo",
                "--clear","--max-bytes","1000000","--max-expanded-bytes","1000000"]).output().unwrap();
        assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stderr));
        assert_eq!(files(&root.join("results")),expected);assert_eq!(files(&project(&root,"data")),before);
        assert_eq!(fs::read(root.join("manifest.csv")).unwrap(),csv);assert_eq!(fs::read(root.join("VERSION")).unwrap(),version);
    }
}
#[test]
fn native_zip_member_budget_counts_parent_directories_before_extracting_any_files() {
    let temp=tempfile::tempdir().unwrap();let archive=temp.path().canonicalize().unwrap().join("members.zip");
    {
        let mut zip=zip::ZipWriter::new(fs::File::create(&archive).unwrap());
        let options=zip::write::SimpleFileOptions::default();
        for index in 0..50_001 {
            zip.add_directory(format!("d{index}/"),options).unwrap();
            zip.start_file(format!("d{index}/file"),options).unwrap();
        }
        zip.finish().unwrap();
    }
    let error=proj::github_zip::extract_zip(camino::Utf8Path::from_path(&archive).unwrap(),128*1024*1024).err().expect("member cap must reject before extraction");
    assert!(format!("{error:#}").contains("member limit"));
}

// Tree fixtures reuse the same isolated CLI and loopback transport; no live service.\nfn structured_failure(output:Output) {
    assert!(!output.status.success());
    let value:Value=serde_json::from_slice(&output.stdout).expect("structured failure required, not a panic");
    assert_eq!(value["protocol_version"],1);assert_eq!(value["ok"],false);
    assert!(matches!(value["error"]["code"].as_str(),Some("command_failed"|"invalid_arguments")));
}

fn enable_tree(root:&Path,config:&mut Value) {
    config["remotes"]["github"]["data"]["manifest-style"]=json!("tree");
    config["remotes"]["github"]["data"].as_object_mut().unwrap().remove("manifest-format");
    save(root,config);
}
fn assert_tree(metadata:&BTreeMap<String,Vec<u8>>,binding:&Value,payload:&BTreeMap<String,Vec<u8>>)->String {
    use proj::snapshot_format::{DirectoryTree,EntryKind};
    fn collect(metadata:&BTreeMap<String,Vec<u8>>,id:&str,prefix:&str,depth:usize,files:&mut BTreeMap<String,String>) {
        assert!(depth<=64);
        let tree=DirectoryTree::parse(&metadata[&format!("trees/v1/{}/{}.json",&id[..2],&id[2..])]).unwrap();
        assert_eq!(tree.identity().unwrap(),id);
        for entry in tree.entries {
            let path=if prefix.is_empty(){entry.name}else{format!("{prefix}/{}",entry.name)};
            match entry.kind {
                EntryKind::File=>{assert!(files.insert(path,entry.hash).is_none());},
                EntryKind::Tree=>collect(metadata,&entry.hash,&path,depth+1,files),
            }
        }
    }
    assert_eq!(binding["schema"],"projcli.github-tree-snapshot.v1");
    let id=binding["payloads"][0]["tree"].as_str().unwrap();let mut actual=BTreeMap::new();
    collect(metadata,id,"",0,&mut actual);
    let expected:BTreeMap<_,_>=payload.iter().filter(|(path,_)|path.as_str()!="projr-empty")
        .map(|(path,bytes)|(path.clone(),blake3::hash(bytes).to_hex().to_string())).collect();
    assert_eq!(actual,expected);id.into()
}
#[test]
fn github_tree_layout_placement_and_cue_matrix_has_pure_roots_exact_zip_members_and_small_references() {
    let mut expected_root=None;
    for layout in ["latest","archive"] {for placement in ["auto","project","remote","both"] {for cue in ["always","if-change"] {
        let server=Server::start();let (_temp,root,mut config)=setup("json",placement,layout);
        enable_tree(&root,&mut config);config["remotes"]["github"]["data"]["send"]["cue"]=json!(cue);save(&root,&config);
        fs::create_dir_all(root.join("results/nested/empty")).unwrap();
        fs::write(root.join("results/nested/other"),b"nested bytes").unwrap();
        build(&server,&root);
        let first=server.assets("data");let asset=if layout=="latest"{"output.zip"}else{"output-v1.2.4.zip"};
        let payload=members(&first[asset]);assert!(!payload.keys().any(|path|path.contains("empty")));
        let project_rows=files(&project(&root,"data"));let refs=bindings(&project_rows,"references/v1/");assert_eq!(refs.len(),1);
        assert_eq!(first.contains_key(META),placement!="project");
        assert_eq!(project_rows.keys().any(|name|name.starts_with("trees/v1/")),placement!="remote");
        let full=if placement=="project"{project_rows}else{members(&first[META])};
        let tree=assert_tree(&full,&refs[0],&payload);
        if let Some(expected)=&expected_root{assert_eq!(&tree,expected)}else{expected_root=Some(tree.clone());}
        assert_payload(&history(&full,"json"),"v1.2.4",&payload);
        let old_nodes:BTreeMap<_,_>=full.iter().filter(|(path,_)|path.starts_with("trees/v1/")).map(|(path,bytes)|(path.clone(),bytes.clone())).collect();
        assert!(!old_nodes.is_empty());assert!(!full.keys().any(|path|path.starts_with("objects/")));
        build(&server,&root);
        let second=server.assets("data");let rows=files(&project(&root,"data"));let refs=bindings(&rows,"references/v1/");
        let later=refs.iter().find(|row|row["version"]=="v1.2.5").unwrap();
        assert_eq!(later["payloads"][0]["tree"],tree);
        let later_asset=if layout=="archive"&&cue=="always"{"output-v1.2.5.zip"}else{asset};
        assert_eq!(later["payloads"][0]["asset"],later_asset);
        let full=if placement=="project"{rows.clone()}else{members(&second[META])};
        assert_eq!(assert_tree(&full,later,&members(&second[later_asset])),tree);
        for (path,bytes) in old_nodes{assert_eq!(full[&path],bytes);}
        assert!(matches!(&history(&full,"json").records[1].inventory,Inventory::Reference{ref_version} if ref_version=="v1.2.4"));
    }}}
}
#[test]
fn github_flat_tree_flat_changes_keep_older_bindings_histories_and_tree_nodes() {
    let server=Server::start();let (_temp,root,mut config)=setup("csv","both","archive");build(&server,&root);
    let first=members(&server.assets("data")[META]);let old=files(&project(&root,"data"));
    enable_tree(&root,&mut config);build(&server,&root);
    let middle=members(&server.assets("data")[META]);let refs=bindings(&middle,"snapshots/v1/");
    let tree_ref=refs.iter().find(|row|row["version"]=="v1.2.5").unwrap();
    let tree=assert_tree(&middle,tree_ref,&members(&server.assets("data")["output-v1.2.4.zip"]));
    config["remotes"]["github"]["data"]["manifest-style"]=json!("flat");
    config["remotes"]["github"]["data"]["manifest-format"]=json!("json-dedupe-file");save(&root,&config);build(&server,&root);
    let final_rows=members(&server.assets("data")[META]);
    for (path,bytes) in &first {assert_eq!(&final_rows[path],bytes);}
    for (path,bytes) in middle.iter().filter(|(path,_)|path.starts_with("trees/v1/")){assert_eq!(&final_rows[path],bytes);}
    for (path,bytes) in old{assert_eq!(files(&project(&root,"data"))[&path],bytes);}
    let refs=bindings(&final_rows,"snapshots/v1/");let newest=refs.iter().find(|row|row["version"]=="v1.2.6").unwrap();
    assert_eq!(newest["schema"],"projcli.github-snapshot.v1");assert!(newest["payloads"][0].get("tree").is_none());
    assert_eq!(history(&final_rows,"json-dedupe-file").records.len(),3);assert_eq!(tree.len(),64);
}
#[test]
fn missing_or_corrupt_github_directory_objects_fail_before_any_remote_write() {
    for damage in ["missing","corrupt","unknown"] {
        let server=Server::start();let (_temp,root,mut config)=setup("json","both","latest");
        enable_tree(&root,&mut config);build(&server,&root);
        let mut rows=members(&server.assets("data")[META]);let path=rows.keys().find(|path|path.starts_with("trees/v1/")).unwrap().clone();
        match damage {
            "missing"=>{rows.remove(&path);},
            "corrupt"=>{rows.insert(path,b"{}".to_vec());},
            _=>{rows.insert("trees/v1/../untrusted.json".into(),b"{}".to_vec());},
        }
        let zip=fixture_zip(&rows.iter().map(|(path,bytes)|(path.as_str(),bytes.as_slice())).collect::<Vec<_>>());
        {let mut state=server.state.lock().unwrap();state.assets.get_mut(&(1,META.into())).unwrap().1=zip;state.uploads.clear();state.deleted.clear();}
        let before=server.assets("data");let local=files(&project(&root,"data"));
        structured_failure(invoke(&server,&root));
        assert_eq!(server.assets("data"),before);assert_eq!(files(&project(&root,"data")),local);
        let state=server.state.lock().unwrap();assert!(state.uploads.is_empty());assert!(state.deleted.is_empty());
    }
}
#[test]
fn github_tree_metadata_failure_never_writes_project_success_references() {
    let server=Server::start();let (_temp,root,mut config)=setup("json","both","latest");enable_tree(&root,&mut config);
    let mut second=config["remotes"]["github"]["data"].clone();second["path"]=json!("other");
    config["remotes"]["github"]["second"]=second;config["build"]["dest"]["second"]=json!({});save(&root,&config);
    server.state.lock().unwrap().fail_metadata=Some(2);
    structured_failure(invoke(&server,&root));assert!(server.assets("data").contains_key(META));
    assert!(server.assets("second").contains_key("other-output.zip"));
    assert!(files(&project(&root,"data")).is_empty());assert!(files(&project(&root,"second")).is_empty());
}
#[test]
fn github_tree_requires_fixed_json_and_preserves_ordinary_restore() {
    for mode in ["csv","csv-dedupe-file","json-dedupe-file"] {
        let server=Server::start();let (_temp,root,mut config)=setup(mode,"both","latest");
        config["remotes"]["github"]["data"]["manifest-style"]=json!("tree");save(&root,&config);
        structured_failure(invoke(&server,&root));assert!(server.assets("data").is_empty());
        assert!(files(&project(&root,"data")).is_empty());
    }
    for layout in ["latest","archive"] {
        let server=Server::start();let (_temp,root,mut config)=setup("json","remote",layout);enable_tree(&root,&mut config);build(&server,&root);
        let assets=server.assets("data");let name=if layout=="latest"{"output.zip"}else{"output-v1.2.4.zip"};
        let expected=members(&assets[name]);let before=files(&project(&root,"data"));
        fs::write(root.join("results/result.csv"),b"damaged").unwrap();fs::write(root.join("results/extra"),b"remove").unwrap();
        let output=command(&root,env!("CARGO_BIN_EXE_proj")).env("GITHUB_API_URL",&server.base).env("GITHUB_PAT","fixture-token")
            .args(["--machine","--non-interactive","--project"]).arg(&root)
            .args(["remote","restore","--label","output","--title","data","--provider","github","--repository","owner/repo",
                "--clear","--max-bytes","1000000","--max-expanded-bytes","1000000"]).output().unwrap();
        assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stderr));
        assert_eq!(files(&root.join("results")),expected);assert_eq!(files(&project(&root,"data")),before);
    }
}

#[test]
fn reformatted_project_directory_nodes_reuse_canonical_identity_without_overwriting_bytes() {
    let server=Server::start();let (_temp,root,mut config)=setup("json","both","latest");enable_tree(&root,&mut config);build(&server,&root);
    let project_path=project(&root,"data");let rows=files(&project_path);
    let path=rows.keys().find(|path|path.starts_with("trees/v1/")).unwrap();
    let parsed:Value=serde_json::from_slice(&rows[path]).unwrap();let compact=serde_json::to_vec(&parsed).unwrap();
    assert_ne!(compact,rows[path]);fs::write(project_path.join(path),&compact).unwrap();
    build(&server,&root);assert_eq!(fs::read(project_path.join(path)).unwrap(),compact);
    let refs=bindings(&files(&project_path),"references/v1/");assert_eq!(refs.len(),2);
    assert_eq!(refs[0]["payloads"][0]["tree"],refs[1]["payloads"][0]["tree"]);
}
