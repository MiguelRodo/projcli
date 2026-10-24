
use std::{collections::BTreeMap,fs,io::{Read,Write},net::TcpListener,path::Path,process::{Command,Output},sync::{Arc,Mutex,atomic::{AtomicBool,Ordering}},thread,time::Duration};
use serde_json::json;
#[derive(Default)]
struct State {exists:bool,next:u64,assets:BTreeMap<String,(u64,Vec<u8>)>,uploads:Vec<String>,deleted:Vec<String>,fail_version:bool,fail_data:bool}
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
    let logs:Vec<_>=fs::read_dir(root.join(".proj/log")).unwrap().map(|entry|entry.unwrap().path()).collect();
    assert_eq!(logs.len(),2);
    let logs:Vec<_>=logs.into_iter().filter(|path|path.extension().is_some_and(|value|value=="json")).collect();
    assert_eq!(logs.len(),1);
    let bytes=fs::read(&logs[0]).unwrap();
    let record:serde_json::Value=serde_json::from_slice(&bytes).unwrap();
    assert_eq!(record["status"],"failed");
    assert_eq!(record["stage"],"distribution");
    assert_eq!(record["distribution_may_have_started"],true);
    assert_eq!(record["operations"][0]["provider"],"github_release");
    assert_eq!(record["operations"][0]["status"],"failed");
    assert_eq!(record["version_after"],"v1.2.4");
    assert_eq!(record["git"]["commit"],"not_requested");
    assert!(!String::from_utf8(bytes).unwrap().contains("http"));
    server.finish();
}
#[test]
fn corrupt_current_archive_is_rejected_before_any_upload_or_deletion() {
    let server=Server::start();let (_temporary,root)=setup(&server);
    {let mut state=server.state.lock().unwrap();state.exists=true;state.next=1;state.assets.insert("output-v1.2.4.zip".into(),(1,b"existing bytes".to_vec()));}
    let output=invoke(&server,&root,&["build","--non-interactive"]);assert!(!output.status.success());
    assert!(!String::from_utf8_lossy(&output.stderr).is_empty());
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
    for state in ["inspection","strategy","layout","alias"] {
        let server=Server::start();let (_temporary,root)=setup(&server);
        let mut config=fs::read_to_string(root.join("_proj.yml")).unwrap();
        config=match state {
            "inspection"=>config.replace("inspect: none","inspect: unknown"),
            "strategy"=>config.replace("inspect: none","inspect: none, strategy: unknown"),
            "layout"=>config.replace("      content:","      structure: unknown\n      content:"),
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

#[test]
fn numeric_current_archive_aliases_are_preserved_without_post_or_delete() {
    for name in ["output-v01.2.4.zip","output-v1.2.4.0.zip","output-v1.2.4.0-empty.zip","OUTPUT-V01.2.4.ZIP"] {
        let server=Server::start();let (_temporary,root)=setup(&server);
        {let mut data=server.state.lock().unwrap();data.exists=true;data.next=1;data.assets.insert(name.into(),(1,b"prior alias bytes".to_vec()));}
        let output=invoke(&server,&root,&["build","--non-interactive"]);
        assert!(!output.status.success(),"{name}");
        assert!(String::from_utf8_lossy(&output.stderr).contains("Current GitHub archive already exists"),"{}",String::from_utf8_lossy(&output.stderr));
        let data=server.state.lock().unwrap();assert!(data.uploads.is_empty());assert!(data.deleted.is_empty());assert_eq!(data.assets[name].1,b"prior alias bytes");drop(data);server.finish();
    }
}
#[test]
fn prefixed_no_label_r_data_archives_do_not_conflict_with_their_own_history() {
    let server=Server::start();let (_temporary,root)=setup(&server);
    fs::create_dir(root.join("data")).unwrap();fs::write(root.join("data/table.csv"),"R data\n").unwrap();
    let config=fs::read_to_string(root.join("_proj.yml")).unwrap()
        .replace("directories:\n","directories:\n  output-r-data: {}\n")
        .replace("      content: [output]","      path: bundle\n      path-append-label: false\n      content: [output-r-data]");
    fs::write(root.join("_proj.yml"),config).unwrap();
    success(invoke(&server,&root,&["build","--non-interactive"]));
    success(invoke(&server,&root,&["build","--non-interactive"]));
    let data=server.state.lock().unwrap();
    for name in ["bundle-v1.2.4.zip","bundle-v1.2.5.zip"] {assert_eq!(members(&data.assets[name].1)["table.csv"],b"R data\n");}
    drop(data);server.finish();
}
#[test]
fn all_space_release_title_fails_before_hooks_and_network() {
    let server=Server::start();let (_temporary,root)=setup(&server);
    let config=fs::read_to_string(root.join("_proj.yml")).unwrap().replace("    data:","    '   ':")
        .replace("build:\n","build:\n  hooks: {pre: [start.py]}\n");
    fs::write(root.join("_proj.yml"),config).unwrap();
    fs::write(root.join("start.py"),"from pathlib import Path\nPath('hook-ran').write_text('bad')\n").unwrap();
    let output=invoke(&server,&root,&["build","--non-interactive"]);assert!(!output.status.success());
    assert!(!root.join("hook-ran").exists());assert!(!root.join("manifest.csv").exists());
    assert!(!server.state.lock().unwrap().exists);server.finish();
}

#[test]
fn repeated_zip_suffix_prefix_cannot_hide_a_numeric_current_archive_alias() {
    let server=Server::start();let (_temporary,root)=setup(&server);
    let config=fs::read_to_string(root.join("_proj.yml")).unwrap()
        .replace("      content:","      path: bundle.zip\n      path-append-label: false\n      content:");
    fs::write(root.join("_proj.yml"),config).unwrap();
    {let mut data=server.state.lock().unwrap();data.exists=true;data.assets.insert("bundle.zip-v01.2.4.zip".into(),(1,b"preserve alias".to_vec()));}
    let output=invoke(&server,&root,&["build","--non-interactive"]);assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Current GitHub archive already exists"));
    let data=server.state.lock().unwrap();assert!(data.uploads.is_empty());assert!(data.deleted.is_empty());assert_eq!(data.assets["bundle.zip-v01.2.4.zip"].1,b"preserve alias");drop(data);server.finish();
}
#[cfg(unix)]
#[test]
fn public_gh_fallback_pins_hostname_despite_an_inherited_enterprise_host() {
    use std::os::unix::fs::PermissionsExt;
    let server=Server::start();let (_temporary,root)=setup(&server);
    assert!(command(&root,"git").args(["remote","set-url","origin","https://github.com/owner/repo.git"]).output().unwrap().status.success());
    let bin=root.join("bin");fs::create_dir(&bin).unwrap();
    let gh=bin.join("gh");fs::write(&gh,"#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$GH_ARGS_FILE\"\nexit 17\n").unwrap();
    fs::set_permissions(&gh,fs::Permissions::from_mode(0o755)).unwrap();
    let path=std::env::join_paths(std::iter::once(bin).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap()))).unwrap();
    let output=command(&root,env!("CARGO_BIN_EXE_proj")).env("GITHUB_API_URL","https://api.github.com")
        .env("GH_HOST","enterprise.example").env("GH_ARGS_FILE",root.join("gh-args")).env("PATH",path)
        .args(["--project"]).arg(&root).args(["build","--non-interactive"]).output().unwrap();
    assert!(!output.status.success(),"fake helper intentionally returns no token, so no network request is made");
    assert_eq!(fs::read_to_string(root.join("gh-args")).unwrap(),"auth\ntoken\n--hostname\ngithub.com\n");
    assert!(!server.state.lock().unwrap().exists);server.finish();
}


// Construct independent ZIP witnesses rather than relying on the publisher under test.
fn fixture_zip(files:&[(&str,&[u8])])->Vec<u8> {
    let mut cursor=std::io::Cursor::new(Vec::new());
    {
        let mut writer=zip::ZipWriter::new(&mut cursor);
        for (name,bytes) in files {
            writer.start_file(*name,zip::write::SimpleFileOptions::default()).unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.finish().unwrap();
    }
    cursor.into_inner()
}
fn latest(root:&Path) {
    let path=root.join("_proj.yml");
    let config=fs::read_to_string(&path).unwrap().replace("      content:","      structure: latest\n      content:");
    fs::write(path,config).unwrap();
}
fn seed(server:&Server,name:&str,bytes:Vec<u8>) {
    let mut state=server.state.lock().unwrap();state.exists=true;state.next+=1;
    let id=state.next;state.assets.insert(name.into(),(id,bytes));
}
#[test]
fn latest_replaces_current_selected_bytes_keeps_extras_and_restores_through_cli() {
    for cue in ["if-change","always"] {
        let server=Server::start();let (_temporary,root)=setup(&server);latest(&root);
        let config=fs::read_to_string(root.join("_proj.yml")).unwrap()
            .replace("inspect: none",&format!("inspect: none, cue: {cue}"))
            .replace("content: [output]","content: true");
        fs::write(root.join("_proj.yml"),config).unwrap();
        seed(&server,"output.zip",fixture_zip(&[("result.csv",b"old"),("extra/old.csv",b"keep")]));
        success(invoke(&server,&root,&["build","--non-interactive"]));
        {
            let state=server.state.lock().unwrap();
            assert_eq!(members(&state.assets["output.zip"].1),BTreeMap::from([
                ("result.csv".into(),b"actual fixture bytes\n".to_vec()),("extra/old.csv".into(),b"keep".to_vec())]));
            assert_eq!(members(&state.assets["VERSION.zip"].1)["VERSION"],b"Project: v1.2.4\noutput: v1.2.4*\n");
            assert_eq!(state.deleted,["output.zip"]);
        }
        fs::write(root.join("results/result.csv"),"second").unwrap();
        success(invoke(&server,&root,&["build","--non-interactive"]));
        fs::write(root.join("results/result.csv"),"damaged").unwrap();
        success(invoke(&server,&root,&["remote","restore","--label","output","--clear","--repository","owner/repo","--max-bytes","1000000","--max-expanded-bytes","1000000"]));
        assert_eq!(fs::read(root.join("results/result.csv")).unwrap(),b"second");
        assert_eq!(fs::read(root.join("results/extra/old.csv")).unwrap(),b"keep");
        server.finish();
    }
}
#[test]
fn canonical_current_archive_replacement_keeps_extras_and_older_versions() {
    let server=Server::start();let (_temporary,root)=setup(&server);
    let old=fixture_zip(&[("historical.csv",b"history")]);seed(&server,"output-v1.2.3.zip",old.clone());
    seed(&server,"output-v1.2.4.zip",fixture_zip(&[("prior.csv",b"prior")]));
    success(invoke(&server,&root,&["build","--non-interactive"]));
    {
        let state=server.state.lock().unwrap();
        assert_eq!(state.assets["output-v1.2.3.zip"].1,old);
        assert_eq!(members(&state.assets["output-v1.2.4.zip"].1),BTreeMap::from([
            ("prior.csv".into(),b"prior".to_vec()),("result.csv".into(),b"actual fixture bytes\n".to_vec())]));
        assert_eq!(state.deleted,["output-v1.2.4.zip"]);
        assert_eq!(members(&state.assets["VERSION.zip"].1)["VERSION"],b"Project: v1.2.4\noutput: v1.2.4*\n");
    }
    server.finish();
}
#[test]
fn latest_empty_source_keeps_existing_data_and_its_advertised_version() {
    let server=Server::start();let (_temporary,root)=setup(&server);latest(&root);
    success(invoke(&server,&root,&["build","--non-interactive"]));
    let prior=server.state.lock().unwrap().assets["output.zip"].1.clone();
    fs::remove_file(root.join("results/result.csv")).unwrap();
    success(invoke(&server,&root,&["build","--non-interactive"]));
    {
        let state=server.state.lock().unwrap();assert_eq!(state.assets["output.zip"].1,prior);
        assert!(!state.assets.contains_key("output-empty.zip"));
        assert_eq!(state.uploads.iter().filter(|name|name.as_str()=="output.zip").count(),1);
        assert_eq!(members(&state.assets["VERSION.zip"].1)["VERSION"],b"Project: v1.2.5\noutput: v1.2.4\n");
    }
    server.finish();
}
#[test]
fn latest_empty_variant_is_validated_and_removed_only_after_full_upload() {
    for valid in [true,false] {
        let server=Server::start();let (_temporary,root)=setup(&server);latest(&root);
        let bytes=fixture_zip(if valid {&[("projr-empty",b"")]}else{&[("surprise.csv",b"preserve")]});
        seed(&server,"output-empty.zip",bytes.clone());
        let output=invoke(&server,&root,&["build","--non-interactive"]);
        let state=server.state.lock().unwrap();
        if valid {
            assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stderr));
            assert!(!state.assets.contains_key("output-empty.zip"));assert_eq!(state.deleted,["output-empty.zip"]);
            assert_eq!(members(&state.assets["output.zip"].1)["result.csv"],b"actual fixture bytes\n");
        }else{
            assert!(!output.status.success());assert!(state.uploads.is_empty());assert!(state.deleted.is_empty());
            assert_eq!(state.assets["output-empty.zip"].1,bytes);
        }
        drop(state);server.finish();
    }
}
#[test]
fn latest_data_replacement_failure_reports_deleted_asset_and_keeps_local_release() {
    let server=Server::start();let (_temporary,root)=setup(&server);latest(&root);
    seed(&server,"output.zip",fixture_zip(&[("prior.csv",b"prior")]));
    server.state.lock().unwrap().fail_data=true;
    let output=invoke(&server,&root,&["build","--non-interactive"]);
    assert!(!output.status.success());let stderr=String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("partially published"));assert!(stderr.contains("Previous asset was deleted"));
    assert_eq!(fs::read_to_string(root.join("VERSION")).unwrap().trim(),"Version: v1.2.4");
    assert_eq!(fs::read(root.join("results/result.csv")).unwrap(),b"actual fixture bytes\n");
    let state=server.state.lock().unwrap();assert!(!state.assets.contains_key("output.zip"));assert!(state.uploads.is_empty());
    assert_eq!(state.deleted,["output.zip"]);drop(state);server.finish();
}

#[test]
fn empty_transition_guards_preserve_old_assets_before_mutation() {
    for case in ["invalid-empty-source","failed-full-upload","both-variants"] {
        let server=Server::start();let (_temporary,root)=setup(&server);latest(&root);
        let empty=fixture_zip(if case=="invalid-empty-source" {&[("surprise",b"keep")]}else{&[("projr-empty",b"")]});
        seed(&server,"output-empty.zip",empty.clone());
        if case=="invalid-empty-source" {fs::remove_file(root.join("results/result.csv")).unwrap();}
        if case=="failed-full-upload" {server.state.lock().unwrap().fail_data=true;}
        if case=="both-variants" {seed(&server,"output.zip",fixture_zip(&[("prior.csv",b"prior")]));}
        let output=invoke(&server,&root,&["build","--non-interactive"]);assert!(!output.status.success(),"{case}");
        let state=server.state.lock().unwrap();
        assert_eq!(state.assets["output-empty.zip"].1,empty,"{case}");
        assert!(state.uploads.is_empty(),"{case}");assert!(state.deleted.is_empty(),"{case}");
        if case=="failed-full-upload" {assert!(String::from_utf8_lossy(&output.stderr).contains("partially published"));}
        if case=="both-variants" {assert_eq!(members(&state.assets["output.zip"].1)["prior.csv"],b"prior");}
        drop(state);server.finish();
    }
}


#[test]
fn no_inspection_strategy_cue_layout_matrix_uses_real_cli_and_exact_zip_bytes() {
    for layout in ["latest","archive"] {for strategy in ["upload-all","upload-missing","sync-diff","sync-purge"] {for cue in ["if-change","always"] {
        let server=Server::start();let (_temporary,root)=setup(&server);
        if layout=="latest" {latest(&root);}
        let config=fs::read_to_string(root.join("_proj.yml")).unwrap().replace("inspect: none",&format!("inspect: none, strategy: {strategy}, cue: {cue}"));
        fs::write(root.join("_proj.yml"),config).unwrap();
        let asset=if layout=="latest" {"output.zip"}else{"output-v1.2.4.zip"};
        seed(&server,asset,fixture_zip(&[("result.csv",b"old"),("extra.csv",b"retained unless purge")]));
        success(invoke(&server,&root,&["build","--non-interactive"]));
        {
            let state=server.state.lock().unwrap();
            let mut expected=BTreeMap::from([("result.csv".into(),b"actual fixture bytes\n".to_vec())]);
            if strategy!="sync-purge" {expected.insert("extra.csv".into(),b"retained unless purge".to_vec());}
            assert_eq!(members(&state.assets[asset].1),expected,"{layout}/{strategy}/{cue}");
            assert_eq!(state.deleted,[asset],"{layout}/{strategy}/{cue}");
            let version=format!("Project: v1.2.4\noutput: v1.2.4{}\n",if strategy=="sync-purge"{""}else{"*"});
            assert_eq!(members(&state.assets["VERSION.zip"].1)["VERSION"],version.as_bytes(),"{layout}/{strategy}/{cue}");
            assert_eq!(members(&state.assets["manifest.zip"].1)["manifest.csv"],fs::read(root.join("manifest.csv")).unwrap());
        }
        success(invoke(&server,&root,&["remote","restore","--label","output","--clear","--repository","owner/repo","--max-bytes","1000000","--max-expanded-bytes","1000000"]));
        assert_eq!(fs::read(root.join("results/result.csv")).unwrap(),b"actual fixture bytes\n");
        assert_eq!(root.join("results/extra.csv").exists(),strategy!="sync-purge");
        server.finish();
    }}}
}
#[test]
fn none_empty_source_does_not_purge_remote_extras_and_diff_metadata_stays_untrusted() {
    for strategy in ["upload-all","upload-missing","sync-diff","sync-purge"] {for cue in ["if-change","always"] {
        let server=Server::start();let (_temporary,root)=setup(&server);latest(&root);
        let config=fs::read_to_string(root.join("_proj.yml")).unwrap().replace("inspect: none",&format!("inspect: none, strategy: {strategy}, cue: {cue}"));
        fs::write(root.join("_proj.yml"),config).unwrap();
        fs::remove_file(root.join("results/result.csv")).unwrap();
        let prior=fixture_zip(&[("extra.csv",b"preserve")]);seed(&server,"output.zip",prior.clone());
        seed(&server,"VERSION.zip",fixture_zip(&[("VERSION",b"Project: v1.2.3\noutput: v1.2.3\n")]));
        success(invoke(&server,&root,&["build","--non-interactive"]));
        let state=server.state.lock().unwrap();assert_eq!(state.assets["output.zip"].1,prior);
        assert!(!state.deleted.iter().any(|name|name.starts_with("output")));assert!(!state.uploads.iter().any(|name|name.starts_with("output")));
        let version=if strategy=="sync-diff" {"v1.2.4*"}else{"v1.2.3"};
        assert_eq!(members(&state.assets["VERSION.zip"].1)["VERSION"],format!("Project: v1.2.4\noutput: {version}\n").as_bytes(),"{strategy}/{cue}");
        drop(state);server.finish();
    }}
}


#[test]
fn complete_inspection_strategy_cue_layout_matrix_uses_actual_cli() {
    for layout in ["latest","archive"] {for inspect in ["none","file","manifest"] {
        for strategy in ["upload-all","upload-missing","sync-diff","sync-purge"] {for cue in ["if-change","always"] {
            let server=Server::start();let (_temporary,root)=setup(&server);
            if layout=="latest" {latest(&root);}
            let config=fs::read_to_string(root.join("_proj.yml")).unwrap()
                .replace("inspect: none",&format!("inspect: {inspect}, strategy: {strategy}, cue: {cue}"));
            fs::write(root.join("_proj.yml"),config).unwrap();
            let asset=if layout=="latest" {"output.zip"}else{"output-v1.2.4.zip"};
            seed(&server,asset,fixture_zip(&[("result.csv",b"old"),("extra.csv",b"extra")]));
            success(invoke(&server,&root,&["build","--non-interactive"]));
            let mut expected=BTreeMap::new();
            let keep_old=inspect!="none"&&strategy=="upload-missing";
            expected.insert("result.csv".into(),if keep_old {b"old".to_vec()}else{b"actual fixture bytes\n".to_vec()});
            if strategy=="upload-all"||strategy=="upload-missing"||(inspect=="none"&&strategy=="sync-diff") {
                expected.insert("extra.csv".into(),b"extra".to_vec());
            }
            {
                let state=server.state.lock().unwrap();
                assert_eq!(members(&state.assets[asset].1),expected,"{layout}/{inspect}/{strategy}/{cue}");
                let changed=!(inspect!="none"&&strategy=="upload-missing");
                assert_eq!(state.deleted.iter().filter(|name|name.as_str()==asset).count(),usize::from(changed),"{layout}/{inspect}/{strategy}/{cue}");
                let untrusted=strategy=="upload-all"||strategy=="upload-missing"||(inspect=="none"&&strategy=="sync-diff");
                let label_version=if !changed {"Project: v1.2.4\n".to_owned()}else{format!("Project: v1.2.4\noutput: v1.2.4{}\n",if untrusted {"*"}else{""})};
                assert_eq!(members(&state.assets["VERSION.zip"].1)["VERSION"],label_version.as_bytes(),"{layout}/{inspect}/{strategy}/{cue}");
                assert_eq!(members(&state.assets["manifest.zip"].1)["manifest.csv"],fs::read(root.join("manifest.csv")).unwrap());
            }
            // With upload-missing and no label advertisement, preserve the
            // native lack of a restore selection instead of inventing one.
            if !(inspect!="none"&&strategy=="upload-missing") {
                success(invoke(&server,&root,&["remote","restore","--label","output","--clear","--repository","owner/repo","--max-bytes","1000000","--max-expanded-bytes","1000000"]));
                for (name,bytes) in expected {assert_eq!(fs::read(root.join("results").join(name)).unwrap(),bytes);}
            }
            server.finish();
        }}
    }}
}
fn digest(bytes:&[u8])->md5::Digest {let mut context=md5::Context::new();context.consume(bytes);context.finalize()}
fn row(name:&str,version:&str,bytes:&[u8])->String {
    format!("output,{name},{version},{:x}\n",digest(bytes))
}
#[test]
fn manifest_matching_history_preserves_unselected_actual_bytes_and_untrusted_evidence_falls_back() {
    for evidence in ["matching","starred","mismatch","missing","malformed"] {
        let server=Server::start();let (_temporary,root)=setup(&server);latest(&root);
        let config=fs::read_to_string(root.join("_proj.yml")).unwrap().replace("send: {inspect: none}","send: {}");
        fs::write(root.join("_proj.yml"),config).unwrap();
        fs::write(root.join("results/kept.csv"),b"recorded kept").unwrap();
        let history="label,fn,version,hash\n".to_owned()+&row("result.csv","v1.2.3",b"old")+&row("kept.csv","v1.2.3",b"recorded kept");
        fs::write(root.join("manifest.csv"),&history).unwrap();
        let remote=if evidence=="mismatch" {history.replace(&format!("{:x}",digest(b"recorded kept")),&format!("{:x}",digest(b"different")))}
            else if evidence=="malformed" {"bad,csv\n".into()}else{history.clone()};
        seed(&server,"manifest.zip",fixture_zip(&[("manifest.csv",remote.as_bytes())]));
        if evidence!="missing" {
            let version=if evidence=="starred" {b"Project: v1.2.3\noutput: v1.2.3*\n".as_slice()}else{b"Project: v1.2.3\noutput: v1.2.3\n".as_slice()};
            seed(&server,"VERSION.zip",fixture_zip(&[("VERSION",version)]));
        }
        let prior=fixture_zip(&[("result.csv",b"old"),("kept.csv",b"actual drift")]);
        seed(&server,"output.zip",prior.clone());
        let output=invoke(&server,&root,&["build","--non-interactive"]);
        let state=server.state.lock().unwrap();
        if evidence=="malformed" {
            assert!(!output.status.success());assert!(state.uploads.is_empty());assert!(state.deleted.is_empty());assert_eq!(state.assets["output.zip"].1,prior);
        }else{
            assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stderr));
            let files=members(&state.assets["output.zip"].1);
            assert_eq!(files["result.csv"],b"actual fixture bytes\n");
            assert_eq!(files["kept.csv"],if evidence=="matching"{b"actual drift".as_slice()}else{b"recorded kept".as_slice()},"{evidence}");
            assert_eq!(members(&state.assets["VERSION.zip"].1)["VERSION"],b"Project: v1.2.4\noutput: v1.2.4\n");
        }
        drop(state);server.finish();
    }
}
#[test]
fn inspected_empty_source_removes_extras_and_retires_full_only_after_empty_upload() {
    for inspect in ["file","manifest"] {for strategy in ["sync-diff","sync-purge"] {for cue in ["if-change","always"] {
        let server=Server::start();let (_temporary,root)=setup(&server);latest(&root);
        let config=fs::read_to_string(root.join("_proj.yml")).unwrap().replace("inspect: none",&format!("inspect: {inspect}, strategy: {strategy}, cue: {cue}"));
        fs::write(root.join("_proj.yml"),config).unwrap();
        fs::remove_file(root.join("results/result.csv")).unwrap();
        seed(&server,"output.zip",fixture_zip(&[("extra.csv",b"remove")]));
        success(invoke(&server,&root,&["build","--non-interactive"]));
        let state=server.state.lock().unwrap();
        assert!(!state.assets.contains_key("output.zip"));
        assert_eq!(members(&state.assets["output-empty.zip"].1),BTreeMap::from([("projr-empty".into(),Vec::new())]));
        assert_eq!(state.deleted,["output.zip"]);
        assert_eq!(members(&state.assets["VERSION.zip"].1)["VERSION"],b"Project: v1.2.4\noutput: v1.2.4\n");
        drop(state);server.finish();
    }}}
}


#[test]
fn archive_history_matrix_reuses_only_eligible_unchanged_versions_and_new_snapshots_are_complete() {
    for inspect in ["none","file","manifest"] {for strategy in ["upload-all","upload-missing","sync-diff","sync-purge"] {for cue in ["if-change","always"] {
        let server=Server::start();let (_temporary,root)=setup(&server);
        let config=fs::read_to_string(root.join("_proj.yml")).unwrap().replace("inspect: none",&format!("inspect: {inspect}, strategy: {strategy}, cue: {cue}"));
        fs::write(root.join("_proj.yml"),config).unwrap();
        fs::write(root.join("results/unchanged.csv"),b"keep unchanged").unwrap();
        success(invoke(&server,&root,&["build","--non-interactive"]));
        let first=server.state.lock().unwrap().assets["output-v1.2.4.zip"].1.clone();
        success(invoke(&server,&root,&["build","--non-interactive"]));
        let reuse=inspect!="none"&&cue=="if-change"&&matches!(strategy,"upload-missing"|"sync-diff");
        {
            let state=server.state.lock().unwrap();assert_eq!(state.assets["output-v1.2.4.zip"].1,first);
            assert_eq!(state.assets.contains_key("output-v1.2.5.zip"),!reuse,"{inspect}/{strategy}/{cue}");
            let label=if reuse {"v1.2.4"}else{"v1.2.5"};
            assert_eq!(members(&state.assets["VERSION.zip"].1)["VERSION"],format!("Project: v1.2.5\noutput: {label}\n").as_bytes(),"{inspect}/{strategy}/{cue}");
        }
        fs::write(root.join("results/result.csv"),b"changed result").unwrap();
        success(invoke(&server,&root,&["build","--non-interactive"]));
        {
            let state=server.state.lock().unwrap();assert_eq!(state.assets["output-v1.2.4.zip"].1,first);
            assert_eq!(members(&state.assets["output-v1.2.6.zip"].1),BTreeMap::from([
                ("result.csv".into(),b"changed result".to_vec()),("unchanged.csv".into(),b"keep unchanged".to_vec())]),"{inspect}/{strategy}/{cue}");
        }
        fs::write(root.join("results/result.csv"),b"damage").unwrap();
        success(invoke(&server,&root,&["remote","restore","--label","output","--clear","--repository","owner/repo","--max-bytes","1000000","--max-expanded-bytes","1000000"]));
        assert_eq!(fs::read(root.join("results/result.csv")).unwrap(),b"changed result");
        assert_eq!(fs::read(root.join("results/unchanged.csv")).unwrap(),b"keep unchanged");
        server.finish();
    }}}
}
#[test]
fn archive_ineligible_history_is_not_downloaded_or_reused() {
    for inspect in ["file","manifest"] {for evidence in ["star","mismatch","missing","future"] {
        let server=Server::start();let (_temporary,root)=setup(&server);
        let config=fs::read_to_string(root.join("_proj.yml")).unwrap().replace("inspect: none",&format!("inspect: {inspect}"));
        fs::write(root.join("_proj.yml"),config).unwrap();
        success(invoke(&server,&root,&["build","--non-interactive"]));
        {
            let mut state=server.state.lock().unwrap();
            // A deliberately invalid unselected ZIP must not block a complete
            // new snapshot when historical evidence makes it ineligible.
            state.assets.get_mut("output-v1.2.4.zip").unwrap().1=b"not a ZIP".to_vec();
            match evidence {
                "star"=>state.assets.get_mut("VERSION.zip").unwrap().1=fixture_zip(&[("VERSION",b"Project: v1.2.4\noutput: v1.2.4*\n")]),
                "mismatch"=>{
                    let csv="label,fn,version,hash\n".to_owned()+&row("result.csv","v1.2.4",b"mismatched history");
                    state.assets.get_mut("manifest.zip").unwrap().1=fixture_zip(&[("manifest.csv",csv.as_bytes())]);
                },
                "missing"=>{state.assets.remove("VERSION.zip");},
                _=>{state.next+=1;let id=state.next;state.assets.insert("output-v9.0.0.zip".into(),(id,b"future invalid ZIP".to_vec()));},
            }
        }
        success(invoke(&server,&root,&["build","--non-interactive"]));
        let state=server.state.lock().unwrap();
        assert_eq!(state.assets["output-v1.2.4.zip"].1,b"not a ZIP");
        assert_eq!(members(&state.assets["output-v1.2.5.zip"].1)["result.csv"],b"actual fixture bytes\n","{inspect}/{evidence}");
        assert_eq!(members(&state.assets["VERSION.zip"].1)["VERSION"],b"Project: v1.2.5\noutput: v1.2.5\n");
        drop(state);server.finish();
    }}
}
#[test]
fn archive_file_drift_and_changed_then_reverted_history_require_complete_current_snapshots() {
    for inspect in ["file","manifest"] {for scenario in ["drift","revert"] {
        let server=Server::start();let (_temporary,root)=setup(&server);
        let config=fs::read_to_string(root.join("_proj.yml")).unwrap().replace("inspect: none",&format!("inspect: {inspect}"));
        fs::write(root.join("_proj.yml"),config).unwrap();
        fs::write(root.join("results/kept.csv"),b"keep").unwrap();
        success(invoke(&server,&root,&["build","--non-interactive"]));
        if scenario=="drift" {
            server.state.lock().unwrap().assets.get_mut("output-v1.2.4.zip").unwrap().1=fixture_zip(&[("result.csv",b"actual fixture bytes\n"),("kept.csv",b"drift")]);
            success(invoke(&server,&root,&["build","--non-interactive"]));
            let state=server.state.lock().unwrap();
            assert_eq!(members(&state.assets["output-v1.2.4.zip"].1)["kept.csv"],b"drift");
            assert_eq!(state.assets.contains_key("output-v1.2.5.zip"),inspect=="file");
            if inspect=="file" {
                assert_eq!(members(&state.assets["output-v1.2.5.zip"].1),BTreeMap::from([
                    ("result.csv".into(),b"actual fixture bytes\n".to_vec()),("kept.csv".into(),b"keep".to_vec())]));
            }
            drop(state);
        }else{
            let old=server.state.lock().unwrap().assets["output-v1.2.4.zip"].1.clone();
            fs::write(root.join("results/result.csv"),b"intervening change").unwrap();
            success(invoke(&server,&root,&["build","--non-interactive"]));
            fs::write(root.join("results/result.csv"),b"actual fixture bytes\n").unwrap();
            success(invoke(&server,&root,&["build","--non-interactive"]));
            let state=server.state.lock().unwrap();
            assert_eq!(state.assets["output-v1.2.4.zip"].1,old);
            assert_eq!(members(&state.assets["output-v1.2.6.zip"].1),BTreeMap::from([
                ("result.csv".into(),b"actual fixture bytes\n".to_vec()),("kept.csv".into(),b"keep".to_vec())]));
            drop(state);
        }
        server.finish();
    }}
}
#[test]
fn historical_empty_archive_sentinel_is_validated_before_reuse_or_metadata_changes() {
    for inspect in ["file","manifest"] {
        let server=Server::start();let (_temporary,root)=setup(&server);
        let config=fs::read_to_string(root.join("_proj.yml")).unwrap().replace("inspect: none",&format!("inspect: {inspect}"));
        fs::write(root.join("_proj.yml"),config).unwrap();fs::remove_file(root.join("results/result.csv")).unwrap();
        success(invoke(&server,&root,&["build","--non-interactive"]));
        success(invoke(&server,&root,&["build","--non-interactive"]));
        let (prior,uploads,deletes)={
            let mut state=server.state.lock().unwrap();assert!(!state.assets.contains_key("output-v1.2.5-empty.zip"));
            state.assets.get_mut("output-v1.2.4-empty.zip").unwrap().1=fixture_zip(&[("unexpected.csv",b"preserve")]);
            (state.assets.clone(),state.uploads.clone(),state.deleted.clone())
        };
        let output=invoke(&server,&root,&["build","--non-interactive"]);assert!(!output.status.success());
        let state=server.state.lock().unwrap();assert_eq!(state.assets,prior);assert_eq!(state.uploads,uploads);assert_eq!(state.deleted,deletes);
        drop(state);server.finish();
    }
}


#[cfg(target_os="linux")]
#[test]
fn final_zip_native_aliases_fail_before_remote_mutations() {
    for case in ["source-alias","merged-alias"] {
        let server=Server::start();let (_temporary,root)=setup(&server);latest(&root);
        fs::write(root.join("results/A.csv"),b"uppercase").unwrap();
        let old=if case=="source-alias" {
            fs::write(root.join("results/a.csv"),b"lowercase").unwrap();
            fixture_zip(&[("prior.csv",b"preserve")])
        }else{fixture_zip(&[("a.csv",b"remote lowercase")])};
        seed(&server,"output.zip",old.clone());
        let output=invoke(&server,&root,&["build","--non-interactive"]);assert!(!output.status.success(),"{case}");
        let state=server.state.lock().unwrap();assert!(state.uploads.is_empty());assert!(state.deleted.is_empty());
        assert_eq!(state.assets["output.zip"].1,old);
        drop(state);server.finish();
    }
}


#[test]
fn registered_github_definition_remains_restore_source_after_uploads_are_disabled() {
    let server=Server::start();let (_temporary,root)=setup(&server);
    let mut config=json!({"directories":{"output":{"path":"results"}},
        "remotes":{"github":{"data":{"content":["output"],"send":{"inspect":"none"}}}},
        "build":{"scripts":[],"git":false,"dest":{"data":{}}}});
    fs::write(root.join("_proj.yml"),serde_json::to_vec_pretty(&config).unwrap()).unwrap();
    success(invoke(&server,&root,&["build","--non-interactive"]));
    let before={let state=server.state.lock().unwrap();
        assert_eq!(state.uploads,["output-v1.2.4.zip","manifest.zip","VERSION.zip","CHANGELOG.zip"]);
        assert_eq!(members(&state.assets["output-v1.2.4.zip"].1)["result.csv"],b"actual fixture bytes\n");
        (state.assets.clone(),state.uploads.clone(),state.deleted.clone())};
    config["build"]["dest"]=json!({});
    fs::write(root.join("_proj.yml"),serde_json::to_vec_pretty(&config).unwrap()).unwrap();
    success(invoke(&server,&root,&["build","--non-interactive"]));
    {let state=server.state.lock().unwrap();assert_eq!(state.assets,before.0);assert_eq!(state.uploads,before.1);assert_eq!(state.deleted,before.2);}
    let manifest=fs::read(root.join("manifest.csv")).unwrap();let version=fs::read(root.join("VERSION")).unwrap();
    fs::write(root.join("results/result.csv"),b"damaged").unwrap();
    success(invoke(&server,&root,&["remote","restore","--label","output","--title","data","--position","source",
        "--remote-version","v1.2.4","--clear","--repository","owner/repo","--max-bytes","1000000","--max-expanded-bytes","1000000"]));
    assert_eq!(fs::read(root.join("results/result.csv")).unwrap(),b"actual fixture bytes\n");
    assert_eq!(fs::read(root.join("manifest.csv")).unwrap(),manifest);assert_eq!(fs::read(root.join("VERSION")).unwrap(),version);
    {let state=server.state.lock().unwrap();assert_eq!(state.assets,before.0);assert_eq!(state.uploads,before.1);assert_eq!(state.deleted,before.2);}
    server.finish();
}
