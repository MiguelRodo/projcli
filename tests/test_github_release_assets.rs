use std::{io::{Read, Write}, net::TcpListener, process::Command, thread, time::{Duration, Instant}};
use camino::Utf8PathBuf;
use proj::github_release::GithubReleaseClient;
use serde_json::json;

struct Reply { method: &'static str, path: String, status: u16, body: Vec<u8>, headers: Vec<(String,String)>, auth: bool, request: Option<Vec<u8>>, zip_request: Option<Vec<(String,Vec<u8>)>> }
fn reply(method: &'static str, path: &str, status: u16, body: impl AsRef<[u8]>) -> Reply {
    Reply { method, path: path.into(), status, body: body.as_ref().to_vec(), headers: vec![], auth: true, request: None, zip_request: None }
}
struct Server { base: String, thread: Option<thread::JoinHandle<()>> }
impl Server {
    fn start(build: impl FnOnce(&str) -> Vec<Reply>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base = format!("http://{}",listener.local_addr().unwrap());
        let replies = build(&base);
        let thread = thread::spawn(move || {
            for expected in replies {
                let deadline=Instant::now()+Duration::from_secs(15);
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream,_)) => break stream,
                        Err(error) if error.kind()==std::io::ErrorKind::WouldBlock && Instant::now()<deadline => thread::sleep(Duration::from_millis(5)),
                        other => panic!("Fixture did not receive expected request: {other:?}"),
                    }
                };
                stream.set_nonblocking(false).unwrap();
                stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                let mut bytes=Vec::new();
                while !bytes.ends_with(b"\r\n\r\n") {
                    let mut byte=[0]; stream.read_exact(&mut byte).unwrap(); bytes.push(byte[0]);
                    assert!(bytes.len()<65536);
                }
                let head=String::from_utf8(bytes).unwrap();
                assert_eq!(head.lines().next().unwrap(),format!("{} {} HTTP/1.1",expected.method,expected.path));
                let headers: Vec<_>=head.lines().skip(1).filter_map(|line| line.split_once(':'))
                    .map(|(k,v)|(k.to_ascii_lowercase(),v.trim().to_string())).collect();
                assert_eq!(headers.iter().find(|(k,_)| k=="authorization").map(|(_,v)|v.as_str()),
                    if expected.auth {Some("Bearer fixture-token")} else {None});
                let length: usize=headers.iter().find(|(k,_)|k=="content-length").map(|(_,v)|v.parse().unwrap()).unwrap_or(0);
                let mut body=vec![0;length];stream.read_exact(&mut body).unwrap();
                if let Some(request)=expected.request { assert_eq!(body,request); }
                if let Some(expected)=expected.zip_request {
                    let mut zip=zip::ZipArchive::new(std::io::Cursor::new(&body)).unwrap();let mut actual=Vec::new();
                    for index in 0..zip.len() {
                        let mut member=zip.by_index(index).unwrap();if member.is_dir() {continue;}
                        let name=member.name().unwrap().into_owned();let mut bytes=Vec::new();member.read_to_end(&mut bytes).unwrap();
                        actual.push((name,bytes));
                    }
                    actual.sort();assert_eq!(actual,expected);
                }
                write!(stream,"HTTP/1.1 {} Fixture\r\nContent-Length: {}\r\nConnection: close\r\n",expected.status,expected.body.len()).unwrap();
                for (key,value) in expected.headers { write!(stream,"{key}: {value}\r\n").unwrap(); }
                stream.write_all(b"\r\n").unwrap();stream.write_all(&expected.body).unwrap();
            }
        });
        Self {base,thread:Some(thread)}
    }
    fn client(&self) -> GithubReleaseClient { GithubReleaseClient::new("owner/repo",&self.base,Some("fixture-token".into())).unwrap() }
    fn finish(mut self) { self.thread.take().unwrap().join().unwrap(); }
}
fn release(base:&str) -> String { json!({"id":1,"tag_name":"data","upload_url":format!("{base}/upload{{?name,label}}")}).to_string() }
fn asset(name:&str,size:usize)->String {json!({"id":2,"name":name,"size":size}).to_string()}
fn selected(base:&str,name:&str,size:usize)->Vec<Reply> {vec![
    reply("GET","/repos/owner/repo/releases/tags/data",200,release(base)),
    reply("GET","/repos/owner/repo/releases/1/assets?per_page=100&page=1",200,format!("[{}]",asset(name,size))),
]}
fn root(temp:&tempfile::TempDir)->Utf8PathBuf {Utf8PathBuf::from_path_buf(temp.path().canonicalize().unwrap()).unwrap()}

#[test]
fn actual_upload_and_redirected_download_preserve_exact_bytes_and_strip_auth() {
    let temp=tempfile::tempdir().unwrap();let root=root(&temp);
    let source=root.join("source.zip");let destination=root.join("download.zip");
    let bytes=b"PK\x03\x04opaque fixture bytes \0";
    std::fs::write(&source,bytes).unwrap();std::fs::write(&destination,"old").unwrap();
    let binary_server=Server::start(|_| {
        let mut binary=reply("GET","/signed-binary?signature=private",200,bytes);binary.auth=false;vec![binary]
    });
    let server=Server::start(|base| {
        let mut upload=reply("POST","/upload?name=output-v1.2.3.zip",201,asset("output-v1.2.3.zip",bytes.len()));
        upload.request=Some(bytes.to_vec());
        let mut redirects=selected(base,"output-v1.2.3.zip",bytes.len());
        let mut redirect=reply("GET","/repos/owner/repo/releases/assets/2",302,"");
        redirect.headers.push(("Location".into(),format!("{}/signed-binary?signature=private",binary_server.base)));
        let mut replies=vec![reply("GET","/repos/owner/repo/releases/tags/data",200,release(base)),
            reply("GET","/repos/owner/repo/releases/1/assets?per_page=100&page=1",200,"[]"),upload];
        replies.append(&mut redirects);replies.push(redirect);replies
    });
    server.client().upload_asset("data","output-v1.2.3.zip",&source,false).unwrap();
    server.client().download_asset(&root,"data","output-v1.2.3.zip",&destination,1024).unwrap();
    assert_eq!(std::fs::read(destination).unwrap(),bytes);
    server.finish();binary_server.finish();
}

#[test]
fn duplicate_upload_and_bad_download_preserve_existing_state() {
    let temp=tempfile::tempdir().unwrap();let root=root(&temp);
    let source=root.join("source.zip");let target=root.join("target.zip");
    std::fs::write(&source,"new").unwrap();std::fs::write(&target,"old").unwrap();
    let server=Server::start(|base| {
        let mut replies=selected(base,"data.zip",4);
        replies.extend(selected(base,"data.zip",4));
        replies.push(reply("GET","/repos/owner/repo/releases/assets/2",200,"bad"));replies
    });
    assert!(server.client().upload_asset("data","data.zip",&source,false).unwrap_err().to_string().contains("already exists"));
    assert!(server.client().download_asset(&root,"data","data.zip",&target,10).is_err());
    assert_eq!(std::fs::read(&target).unwrap(),b"old");
    assert_eq!(std::fs::read_dir(&root).unwrap().count(),2);
    server.finish();
}

#[test]
fn cli_creates_release_only_explicitly_and_emits_machine_metadata() {
    let temp=tempfile::tempdir().unwrap();let root=root(&temp);std::fs::write(root.join("asset.zip"),"zip").unwrap();
    let server=Server::start(|base| vec![
        reply("GET","/repos/owner/repo/releases/tags/data",404,"{}"),
        reply("POST","/repos/owner/repo/releases",201,release(base)),
        reply("GET","/repos/owner/repo/releases/1/assets?per_page=100&page=1",200,"[]"),
        reply("POST","/upload?name=manifest.zip",201,asset("manifest.zip",3)),
        reply("GET","/repos/owner/repo/releases/tags/data",200,release(base)),
        reply("GET","/repos/owner/repo/releases/1/assets?per_page=100&page=1",200,format!("[{}]",asset("manifest.zip",3))),
    ]);
    let mut command=Command::new(env!("CARGO_BIN_EXE_proj"));
    command.env("GITHUB_API_URL",&server.base).env("GITHUB_PAT","fixture-token")
        .args(["--project",root.as_str(),"remote","github","upload","--repository","owner/repo","--tag","data",
            "--name","manifest.zip","--source","asset.zip","--create-release"]);
    let output=command.output().unwrap();
    assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stderr));
    let output=Command::new(env!("CARGO_BIN_EXE_proj")).env("GITHUB_API_URL",&server.base)
        .env("GITHUB_PAT","fixture-token").args(["--project",root.as_str(),"--machine","remote","github","assets",
            "--repository","owner/repo","--tag","data"]).output().unwrap();
    assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stderr));
    let value:serde_json::Value=serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["result"]["assets"][0]["name"],"manifest.zip");
    assert!(!String::from_utf8_lossy(&output.stdout).contains("fixture-token"));
    server.finish();
}

#[test]
fn permissions_protected_paths_size_limit_and_untrusted_upload_fail_without_writes() {
    let temp=tempfile::tempdir().unwrap();let root=root(&temp);let target=root.join("asset.zip");
    std::fs::write(&target,"old").unwrap();
    let server=Server::start(|base| {
        let mut replies=selected(base,"asset.zip",11);
        replies.push(reply("GET","/repos/owner/repo/releases/tags/data",200,
            json!({"id":1,"tag_name":"data","upload_url":"https://untrusted.example/upload"}).to_string()));
        replies.push(reply("GET","/repos/owner/repo/releases/1/assets?per_page=100&page=1",200,"[]"));replies
    });
    let client=server.client();
    for path in ["VERSION","version","manifest.csv","_proj.yml",".git/config",".projr/state"] {
        assert!(client.download_asset(&root,"data","asset.zip",&root.join(path),100).is_err());
    }
    assert!(client.download_asset(&root,"data","asset.zip",&target,10).is_err());
    assert!(client.upload_asset("data","asset.zip",&target,false).unwrap_err().to_string().contains("Untrusted"));
    assert_eq!(std::fs::read(target).unwrap(),b"old");
    server.finish();
}
#[cfg(unix)]
#[test]
fn linked_and_special_sources_fail_before_network_or_blocking_open() {
    use std::os::unix::fs::symlink;
    let temp=tempfile::tempdir().unwrap();let root=root(&temp);
    std::fs::write(root.join("original"),"old").unwrap();symlink("original",root.join("link")).unwrap();
    let fifo=root.join("fifo");
    assert!(Command::new("mkfifo").arg(&fifo).status().unwrap().success());
    let client=GithubReleaseClient::new("owner/repo","http://127.0.0.1:1",Some("fixture-token".into())).unwrap();
    for path in ["link","fifo"] {
        assert!(client.upload_asset("data","asset.zip",&root.join(path),false).is_err());
        assert!(client.download_asset(&root,"data","asset.zip",&root.join(path),100).is_err());
    }
    assert_eq!(std::fs::read(root.join("original")).unwrap(),b"old");
}

#[test]
fn paginated_assets_and_anonymous_enterprise_api_path_are_preserved() {
    let server=Server::start(|_| {
        let assets:Vec<_>=(1..=100).map(|id|json!({"id":id,"name":format!("asset-{id}.zip"),"size":0})).collect();
        let mut replies=vec![
            reply("GET","/api/v3/repos/owner/repo/releases/tags/archive%2Fv1.2",200,
                json!({"id":1,"tag_name":"archive/v1.2"}).to_string()),
            reply("GET","/api/v3/repos/owner/repo/releases/1/assets?per_page=100&page=1",200,json!(assets).to_string()),
            reply("GET","/api/v3/repos/owner/repo/releases/1/assets?per_page=100&page=2",200,
                json!([{"id":101,"name":"last.zip","size":3}]).to_string()),
        ];
        for reply in &mut replies {reply.auth=false;}replies
    });
    let client=GithubReleaseClient::new("owner/repo",&format!("{}/api/v3",server.base),None).unwrap();
    let assets=client.assets("archive/v1.2").unwrap();
    assert_eq!(assets.len(),101);assert_eq!(assets[100].name,"last.zip");
    server.finish();
}

#[test]
fn ambiguous_upload_and_creation_errors_report_uncertainty_without_response_secrets() {
    let temp=tempfile::tempdir().unwrap();let root=root(&temp);
    let source=root.join("source.zip");std::fs::write(&source,"zip").unwrap();
    for (status,body) in [(502,"fixture-token signed-url-secret"),(201,"{bad-json signed-url-secret")] {
        let server=Server::start(|base|vec![
            reply("GET","/repos/owner/repo/releases/tags/data",200,release(base)),
            reply("GET","/repos/owner/repo/releases/1/assets?per_page=100&page=1",200,"[]"),
            reply("POST","/upload?name=data.zip",status,body),
        ]);
        let error=format!("{:#}",server.client().upload_asset("data","data.zip",&source,false).unwrap_err());
        assert!(error.contains("inspect remote state")||error.contains("may have completed"));
        assert!(!error.contains("fixture-token")&&!error.contains("signed-url-secret"));
        assert_eq!(std::fs::read(&source).unwrap(),b"zip");server.finish();
    }
    let server=Server::start(|_|vec![
        reply("GET","/repos/owner/repo/releases/tags/data",404,"{}"),
        reply("POST","/repos/owner/repo/releases",201,"bad signed-url-secret"),
    ]);
    let error=format!("{:#}",server.client().upload_asset("data","data.zip",&source,true).unwrap_err());
    assert!(error.contains("may have completed"));assert!(!error.contains("signed-url-secret"));
    server.finish();
}

#[cfg(windows)]
#[test]
fn windows_authority_aliases_fail_before_network() {
    let temp=tempfile::tempdir().unwrap();let root=root(&temp);
    for name in ["VERSION","_projr-local.yml"] {std::fs::write(root.join(name),"authority").unwrap();}
    let client=GithubReleaseClient::new("owner/repo","http://127.0.0.1:1",None).unwrap();
    for name in ["VERSION.","_projr-local.yml "] {
        let alias=root.join(name);
        // A non-verbatim path exercises Win32 trailing-dot/space normalisation.
        let text=alias.as_str().strip_prefix(r"\\?\").unwrap_or(alias.as_str());
        let error=client.download_asset(&root,"data","data.zip",camino::Utf8Path::new(text),100).unwrap_err();
        assert!(error.to_string().contains("native path alias"),"{error:#}");
    }
    assert_eq!(std::fs::read(root.join("VERSION")).unwrap(),b"authority");
    assert_eq!(std::fs::read(root.join("_projr-local.yml")).unwrap(),b"authority");
}

#[test]
fn directory_zip_upload_and_safe_download_merge_clear_and_empty_variants() {
    use proj::github_zip::{directory_zip,directory_asset_name,GithubStructure,download_directory};
    let temp=tempfile::tempdir().unwrap();let root=root(&temp);
    let source=root.join("source");let destination=root.join("restored");
    std::fs::create_dir(&source).unwrap();std::fs::create_dir(&destination).unwrap();
    std::fs::write(source.join("report.txt"),"new-report").unwrap();std::fs::write(destination.join("old.txt"),"retain").unwrap();
    let (zip,empty)=directory_zip(&source).unwrap();assert!(!empty);
    let name=directory_asset_name(None,true,"output",GithubStructure::Archive,Some("1.2.3"),empty).unwrap();
    let bytes=std::fs::read(zip.path()).unwrap();
    let server=Server::start(|base| {
        let mut upload=reply("POST","/upload?name=output-v1.2.3.zip",201,asset(&name,bytes.len()));upload.request=Some(bytes.clone());
        let mut replies=vec![
            reply("GET","/repos/owner/repo/releases/tags/data",200,release(base)),
            reply("GET","/repos/owner/repo/releases/1/assets?per_page=100&page=1",200,"[]"),upload];
        for _ in 0..2 {replies.extend(selected(base,&name,bytes.len()));
            replies.push(reply("GET","/repos/owner/repo/releases/assets/2",200,&bytes));}replies
    });
    server.client().upload_asset("data",&name,camino::Utf8Path::from_path(zip.path()).unwrap(),false).unwrap();
    download_directory(&server.client(),&root,"data",&name,&destination,10000,10000,false,false).unwrap();
    assert_eq!(std::fs::read(destination.join("report.txt")).unwrap(),b"new-report");
    assert_eq!(std::fs::read(destination.join("old.txt")).unwrap(),b"retain");
    download_directory(&server.client(),&root,"data",&name,&destination,10000,10000,true,false).unwrap();
    assert!(!destination.join("old.txt").exists());server.finish();
    let empty_source=tempfile::tempdir().unwrap();
    let (empty_zip,empty)=directory_zip(camino::Utf8Path::from_path(&empty_source.path().canonicalize().unwrap()).unwrap()).unwrap();assert!(empty);
    let bytes=std::fs::read(empty_zip.path()).unwrap();
    let server=Server::start(|base| {let mut replies=selected(base,"output-empty.zip",bytes.len());
        replies.push(reply("GET","/repos/owner/repo/releases/assets/2",200,&bytes));replies});
    download_directory(&server.client(),&root,"data","output-empty.zip",&destination,10000,10000,true,true).unwrap();
    assert_eq!(std::fs::read_dir(destination).unwrap().count(),0);server.finish();
}

#[test]
fn new_zip_cli_upload_routes_package_exact_members_and_expose_help() {
    use proj::github_zip::{directory_zip,metadata_zip,MetadataKind};
    let temp=tempfile::tempdir().unwrap();let root=root(&temp);
    let source=root.join("source");std::fs::create_dir(&source).unwrap();
    std::fs::write(source.join("report.txt"),"report").unwrap();std::fs::write(root.join("manifest.csv"),"label,fn,version,hash\n").unwrap();
    let (directory,_)=directory_zip(&source).unwrap();let directory_size=directory.as_file().metadata().unwrap().len() as usize;
    let metadata=metadata_zip(&root,MetadataKind::Manifest).unwrap();let metadata_size=metadata.as_file().metadata().unwrap().len() as usize;
    let server=Server::start(|base| {
        let mut replies=Vec::new();
        for (name,size,members) in [
            ("output-v1.2.3.zip",directory_size,vec![("report.txt".into(),b"report".to_vec())]),
            ("manifest.zip",metadata_size,vec![("manifest.csv".into(),b"label,fn,version,hash\n".to_vec())]),
        ] {
            replies.push(reply("GET","/repos/owner/repo/releases/tags/data",200,release(base)));
            replies.push(reply("GET","/repos/owner/repo/releases/1/assets?per_page=100&page=1",200,"[]"));
            let mut upload=reply("POST",&format!("/upload?name={name}"),201,asset(name,size));upload.zip_request=Some(members);replies.push(upload);
        }replies
    });
    for arguments in [
        vec!["upload-directory","--source","source","--label","output","--remote-version","1.2.3"],
        vec!["upload-metadata","--source",".","--kind","manifest"],
    ] {
        let output=Command::new(env!("CARGO_BIN_EXE_proj")).env("GITHUB_API_URL",&server.base).env("GITHUB_PAT","fixture-token")
            .args(["--project",root.as_str(),"--machine","remote","github"]).args(arguments)
            .args(["--repository","owner/repo","--tag","data"]).output().unwrap();
        assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stderr));
        let value:serde_json::Value=serde_json::from_slice(&output.stdout).unwrap();assert_eq!(value["result"]["completed"],true);
    }
    for command in ["upload-directory","upload-metadata","download-directory"] {
        assert!(Command::new(env!("CARGO_BIN_EXE_proj")).args(["remote","github",command,"--help"]).output().unwrap().status.success());
    }
    server.finish();
}
