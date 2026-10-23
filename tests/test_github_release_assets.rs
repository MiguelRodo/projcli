use std::{io::{Read, Write}, net::TcpListener, process::Command, thread, time::{Duration, Instant}};
use camino::Utf8PathBuf;
use proj::github_release::GithubReleaseClient;
use serde_json::json;

struct Reply { method: &'static str, path: String, status: u16, body: Vec<u8>, headers: Vec<(String,String)>, auth: bool, request: Option<Vec<u8>>, zip_request: Option<Vec<(String,Vec<u8>)>>, before_response: Option<Box<dyn FnOnce()+Send>>, disconnect: bool }
fn reply(method: &'static str, path: &str, status: u16, body: impl AsRef<[u8]>) -> Reply {
    Reply { method, path: path.into(), status, body: body.as_ref().to_vec(), headers: vec![], auth: true, request: None, zip_request: None, before_response: None, disconnect: false }
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
                if let Some(action)=expected.before_response {action();}
                if expected.disconnect {continue;}
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

fn replacement_asset(name:&str,size:usize)->String {json!({"id":3,"name":name,"size":size}).to_string()}
#[test]
fn explicit_replacement_stages_bytes_before_delete_and_uploads_once() {
    let temp=tempfile::tempdir().unwrap();let root=root(&temp);let source=root.join("data.zip");
    std::fs::write(&source,"stable-new").unwrap();
    let changed=source.clone();
    let server=Server::start(|base| {
        let mut delete=reply("DELETE","/repos/owner/repo/releases/assets/2",204,"");
        delete.before_response=Some(Box::new(move||std::fs::write(changed,"changed-after-delete").unwrap()));
        let mut upload=reply("POST","/upload?name=data.zip",201,replacement_asset("data.zip",10));
        upload.request=Some(b"stable-new".to_vec());
        let mut replies=selected(base,"data.zip",3);replies.extend([delete,upload]);replies
    });
    let asset=server.client().replace_asset("data","data.zip",&source,false).unwrap();
    assert_eq!(asset.id,3);assert_eq!(std::fs::read(source).unwrap(),b"changed-after-delete");server.finish();
}
#[test]
fn replacement_failures_report_the_irreversible_boundary_without_retry_or_secret_bodies() {
    let temp=tempfile::tempdir().unwrap();let root=root(&temp);let source=root.join("data.zip");
    std::fs::write(&source,"new").unwrap();
    for (delete_status,upload_status) in [(403,None),(204,Some(502)),(204,Some(201))] {
        let server=Server::start(|base| {
            let mut replies=selected(base,"data.zip",3);
            replies.push(reply("DELETE","/repos/owner/repo/releases/assets/2",delete_status,""));
            if let Some(status)=upload_status {replies.push(reply("POST","/upload?name=data.zip",status,"malformed fixture-token signed-url-secret"));}replies
        });
        let error=format!("{:#}",server.client().replace_asset("data","data.zip",&source,false).unwrap_err());
        if delete_status==204 {assert!(error.contains("Previous asset was deleted"),"{error}");}
        else {assert!(error.contains("replacement was not uploaded"),"{error}");}
        assert!(error.contains("inspect remote state"));assert!(!error.contains("fixture-token")&&!error.contains("signed-url-secret"));server.finish();
    }
    let server=Server::start(|base| {
        let mut replies=selected(base,"data.zip",3);
        let mut delete=reply("DELETE","/repos/owner/repo/releases/assets/2",204,"");delete.disconnect=true;replies.push(delete);replies
    });
    let error=format!("{:#}",server.client().replace_asset("data","data.zip",&source,false).unwrap_err());
    assert!(error.contains("may have been deleted")&&error.contains("replacement was not uploaded"));server.finish();
}
#[test]
fn unsafe_or_ambiguous_replacement_preflight_never_deletes() {
    let temp=tempfile::tempdir().unwrap();let root=root(&temp);let source=root.join("data.zip");std::fs::write(&source,"new").unwrap();
    let server=Server::start(|_|vec![
        reply("GET","/repos/owner/repo/releases/tags/data",200,json!({"id":1,"tag_name":"data","upload_url":"https://untrusted.example/upload"}).to_string()),
        reply("GET","/repos/owner/repo/releases/1/assets?per_page=100&page=1",200,format!("[{}]",asset("data.zip",3))),
    ]);
    assert!(server.client().replace_asset("data","data.zip",&source,false).unwrap_err().to_string().contains("Untrusted"));
    server.finish();
    let server=Server::start(|base|vec![
        reply("GET","/repos/owner/repo/releases/tags/data",200,release(base)),
        reply("GET","/repos/owner/repo/releases/1/assets?per_page=100&page=1",200,
            json!([{"id":2,"name":"data.zip","size":3},{"id":3,"name":"data.zip","size":3}]).to_string()),
    ]);
    assert!(server.client().replace_asset("data","data.zip",&source,false).unwrap_err().to_string().contains("ambiguous"));server.finish();
    let client=GithubReleaseClient::new("owner/repo","http://127.0.0.1:1",Some("fixture-token".into())).unwrap();
    assert!(client.replace_asset("data","data.zip",&root.join("absent"),false).is_err());
    #[cfg(unix)] {
        std::os::unix::fs::symlink("data.zip",root.join("link")).unwrap();
        assert!(client.replace_asset("data","data.zip",&root.join("link"),false).is_err());
    }
}
#[test]
fn cli_replacement_is_explicit_and_machine_result_stays_bounded() {
    let temp=tempfile::tempdir().unwrap();let root=root(&temp);std::fs::write(root.join("data.zip"),"new").unwrap();
    let server=Server::start(|base| {let mut replies=selected(base,"data.zip",3);
        let mut upload=reply("POST","/upload?name=data.zip",201,replacement_asset("data.zip",3));upload.request=Some(b"new".to_vec());
        replies.extend([reply("DELETE","/repos/owner/repo/releases/assets/2",204,""),upload]);replies});
    let output=Command::new(env!("CARGO_BIN_EXE_proj")).env("GITHUB_API_URL",&server.base).env("GITHUB_PAT","fixture-token")
        .args(["--project",root.as_str(),"--machine","remote","github","upload","--repository","owner/repo","--tag","data",
            "--name","data.zip","--source","data.zip","--replace"]).output().unwrap();
    assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stderr));
    let value:serde_json::Value=serde_json::from_slice(&output.stdout).unwrap();assert_eq!(value["result"]["completed"],true);
    assert!(!String::from_utf8_lossy(&output.stdout).contains("fixture-token"));server.finish();
}

#[test]
fn explicit_replacement_of_absent_name_uploads_without_delete_and_rejects_stale_identity() {
    let temp=tempfile::tempdir().unwrap();let root=root(&temp);let source=root.join("data.zip");std::fs::write(&source,"new").unwrap();
    let server=Server::start(|base|vec![
        reply("GET","/repos/owner/repo/releases/tags/data",200,release(base)),
        reply("GET","/repos/owner/repo/releases/1/assets?per_page=100&page=1",200,"[]"),
        reply("POST","/upload?name=data.zip",201,replacement_asset("data.zip",3)),
    ]);
    assert_eq!(server.client().replace_asset("data","data.zip",&source,false).unwrap().id,3);server.finish();
    let server=Server::start(|base| {let mut replies=selected(base,"data.zip",3);
        replies.extend([reply("DELETE","/repos/owner/repo/releases/assets/2",204,""),
            reply("POST","/upload?name=data.zip",201,asset("data.zip",3))]);replies});
    let error=format!("{:#}",server.client().replace_asset("data","data.zip",&source,false).unwrap_err());
    assert!(error.contains("Previous asset was deleted")&&error.contains("reused the deleted asset identity"));server.finish();
}

fn configure_restore(root:&camino::Utf8Path,layout:&str) {
    std::fs::create_dir(root.join("restored")).unwrap();std::fs::write(root.join("restored/old.txt"),"old-state").unwrap();
    std::fs::write(root.join("manifest.csv"),"label,fn,version,hash\nraw,data.csv,v1.2.3,900150983cd24fb0d6963f7d28e17f72\n").unwrap();
    std::fs::write(root.join("_proj.yml"),format!("directories:\n  raw:\n    path: restored\n    source:\n      github: data\nbuild:\n  github:\n    data:\n      structure: {layout}\n      path: analysis\n")).unwrap();
}
fn restore_command(root:&camino::Utf8Path,base:&str)->Command {
    let mut command=Command::new(env!("CARGO_BIN_EXE_proj"));
    command.env("GITHUB_API_URL",base).env("GITHUB_PAT","fixture-token")
        .env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE").env_remove(proj::PROJECT_ROOT_ENV).env_remove(proj::RUN_CONTEXT_ENV)
        .args(["--project",root.as_str(),"--machine","remote","restore","--label","raw",
            "--repository","owner/repo","--max-bytes","100000","--max-expanded-bytes","100000","--clear"]);command
}
#[test]
fn configured_github_archive_restore_selects_numeric_newest_and_exact_data_bytes() {
    let temp=tempfile::tempdir().unwrap();let root=root(&temp);configure_restore(&root,"archive");
    let source=root.join("source");std::fs::create_dir(&source).unwrap();std::fs::write(source.join("data.csv"),"selected-1.10").unwrap();
    let (zip,_)=proj::github_zip::directory_zip(&source).unwrap();let bytes=std::fs::read(zip.path()).unwrap();
    let server=Server::start(|base|vec![
        reply("GET","/repos/owner/repo/releases/tags/data",200,release(base)),
        reply("GET","/repos/owner/repo/releases/1/assets?per_page=100&page=1",200,
            json!([{"id":5,"name":"analysis-raw-v1.9.zip","size":1},{"id":2,"name":"analysis-raw-v1.10.zip","size":bytes.len()},
                {"id":6,"name":"analysis-raw-vnotes.zip","size":1}]).to_string()),
        reply("GET","/repos/owner/repo/releases/tags/data",200,release(base)),
        reply("GET","/repos/owner/repo/releases/1/assets?per_page=100&page=1",200,format!("[{}]",asset("analysis-raw-v1.10.zip",bytes.len()))),
        reply("GET","/repos/owner/repo/releases/assets/2",200,&bytes),
    ]);
    let output=restore_command(&root,&server.base).output().unwrap();
    assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stderr));
    let value:serde_json::Value=serde_json::from_slice(&output.stdout).unwrap();assert_eq!(value["result"]["restored"],true);assert_eq!(value["result"]["version"],"v1.10");
    assert_eq!(std::fs::read(root.join("restored/data.csv")).unwrap(),b"selected-1.10");assert!(!root.join("restored/old.txt").exists());server.finish();
}
#[test]
fn configured_github_latest_reads_metadata_and_preserves_final_on_corrupt_zip() {
    for corrupt in [false,true] {
        let temp=tempfile::tempdir().unwrap();let root=root(&temp);configure_restore(&root,"latest");
        let source=root.join("source");let metadata=root.join("metadata");std::fs::create_dir(&source).unwrap();std::fs::create_dir(&metadata).unwrap();
        std::fs::write(source.join("data.csv"),"latest-data").unwrap();std::fs::write(metadata.join("VERSION"),"raw: v1.10*\n").unwrap();
        let (zip,_)=proj::github_zip::directory_zip(&source).unwrap();
        let bytes=if corrupt {b"not-a-zip".to_vec()}else{std::fs::read(zip.path()).unwrap()};
        let metadata_zip=proj::github_zip::metadata_zip(&metadata,proj::github_zip::MetadataKind::Version).unwrap();let metadata_bytes=std::fs::read(metadata_zip.path()).unwrap();
        let server=Server::start(|base| {
            let listing=json!([{"id":2,"name":"analysis-raw.zip","size":bytes.len()},{"id":3,"name":"VERSION.zip","size":metadata_bytes.len()}]).to_string();
            let mut replies=vec![
                reply("GET","/repos/owner/repo/releases/tags/data",200,release(base)),reply("GET","/repos/owner/repo/releases/1/assets?per_page=100&page=1",200,&listing),
                reply("GET","/repos/owner/repo/releases/tags/data",200,release(base)),reply("GET","/repos/owner/repo/releases/1/assets?per_page=100&page=1",200,&listing),
                reply("GET","/repos/owner/repo/releases/assets/3",200,&metadata_bytes),
            ];
            replies.extend([reply("GET","/repos/owner/repo/releases/tags/data",200,release(base)),
                reply("GET","/repos/owner/repo/releases/1/assets?per_page=100&page=1",200,&listing),
                reply("GET","/repos/owner/repo/releases/assets/2",200,&bytes)]);replies
        });
        let output=restore_command(&root,&server.base).output().unwrap();
        let value:serde_json::Value=serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(output.status.success(),!corrupt,"{}",String::from_utf8_lossy(&output.stderr));assert_eq!(value["ok"],!corrupt);
        if corrupt {assert_eq!(std::fs::read(root.join("restored/old.txt")).unwrap(),b"old-state");assert!(!root.join("restored/data.csv").exists());}
        else {assert_eq!(value["result"]["version"],"v1.10");assert_eq!(std::fs::read(root.join("restored/data.csv")).unwrap(),b"latest-data");assert!(!root.join("restored/old.txt").exists());}
        assert!(!String::from_utf8_lossy(&output.stdout).contains("fixture-token"));server.finish();
    }
}

#[test]
fn configured_latest_empty_skips_and_repeated_trust_markers_fail_without_data_download() {
    for (version,lowercase) in [("v1.10*",false),("v1.10**",false),("v1.10*",true)] {
        let temp=tempfile::tempdir().unwrap();let root=root(&temp);configure_restore(&root,"latest");
        let metadata=root.join("metadata");std::fs::create_dir(&metadata).unwrap();
        std::fs::write(metadata.join("VERSION"),format!("raw: {version}\n")).unwrap();
        let zip=proj::github_zip::metadata_zip(&metadata,proj::github_zip::MetadataKind::Version).unwrap();
        let mut bytes=std::fs::read(zip.path()).unwrap();
        if lowercase {
            let offsets:Vec<_>=bytes.windows(7).enumerate().filter_map(|(index,part)|(part==b"VERSION").then_some(index)).collect();
            assert_eq!(offsets.len(),2);
            for offset in offsets {bytes[offset..offset+7].copy_from_slice(b"version");}
        }
        let server=Server::start(|base| {
            let listing=json!([{"id":2,"name":"analysis-raw-empty.zip","size":1},{"id":3,"name":"VERSION.zip","size":bytes.len()}]).to_string();
            vec![
                reply("GET","/repos/owner/repo/releases/tags/data",200,release(base)),reply("GET","/repos/owner/repo/releases/1/assets?per_page=100&page=1",200,&listing),
                reply("GET","/repos/owner/repo/releases/tags/data",200,release(base)),reply("GET","/repos/owner/repo/releases/1/assets?per_page=100&page=1",200,&listing),
                reply("GET","/repos/owner/repo/releases/assets/3",200,&bytes),
            ]
        });
        let output=restore_command(&root,&server.base).output().unwrap();
        let value:serde_json::Value=serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(output.status.success(),version=="v1.10*"&&!lowercase,"{}",String::from_utf8_lossy(&output.stderr));
        if version=="v1.10*"&&!lowercase {assert_eq!(value["result"]["skipped"],"empty-remote");assert_eq!(value["result"]["restored"],false);}
        else {assert_eq!(value["ok"],false);}
        assert_eq!(std::fs::read(root.join("restored/old.txt")).unwrap(),b"old-state");server.finish();
    }
}
#[test]
fn configured_legacy_data_archive_and_latest_use_existing_namespace_without_rewriting_history() {
    for layout in ["archive","latest"] {
        let temp=tempfile::tempdir().unwrap();let root=root(&temp);
        std::fs::create_dir(root.join("data")).unwrap();std::fs::write(root.join("data/old"),"old").unwrap();
        let history=b"label,fn,version,hash\ndata,data.csv,v1.2.3,900150983cd24fb0d6963f7d28e17f72\n";
        std::fs::write(root.join("manifest.csv"),history).unwrap();
        std::fs::write(root.join("_projr.yml"),format!("directories:\n  data:\n    source:\n      github: data\nbuild:\n  github:\n    data:\n      structure: {layout}\n      path: analysis\n      content: [data]\n      source: [data]\n")).unwrap();
        let source=root.join("source");let metadata=root.join("metadata");std::fs::create_dir(&source).unwrap();std::fs::create_dir(&metadata).unwrap();
        std::fs::write(source.join("data.csv"),"legacy data").unwrap();std::fs::write(metadata.join("VERSION"),"data: v1.10*\n").unwrap();
        let (zip,_)=proj::github_zip::directory_zip(&source).unwrap();let bytes=std::fs::read(zip.path()).unwrap();
        let meta=proj::github_zip::metadata_zip(&metadata,proj::github_zip::MetadataKind::Version).unwrap();let meta_bytes=std::fs::read(meta.path()).unwrap();
        let name=if layout=="archive" {"analysis-data-v1.10.zip"} else {"analysis-data.zip"};
        let server=Server::start(|base| {
            let listing=json!([{"id":2,"name":name,"size":bytes.len()},{"id":3,"name":"VERSION.zip","size":meta_bytes.len()}]).to_string();
            let mut replies=vec![reply("GET","/repos/owner/repo/releases/tags/data",200,release(base)),reply("GET","/repos/owner/repo/releases/1/assets?per_page=100&page=1",200,&listing)];
            if layout=="latest" {replies.extend([
                reply("GET","/repos/owner/repo/releases/tags/data",200,release(base)),reply("GET","/repos/owner/repo/releases/1/assets?per_page=100&page=1",200,&listing),
                reply("GET","/repos/owner/repo/releases/assets/3",200,&meta_bytes),
            ]);}
            replies.extend([reply("GET","/repos/owner/repo/releases/tags/data",200,release(base)),
                reply("GET","/repos/owner/repo/releases/1/assets?per_page=100&page=1",200,&listing),
                reply("GET","/repos/owner/repo/releases/assets/2",200,&bytes)]);replies
        });
        let mut command=Command::new(env!("CARGO_BIN_EXE_proj"));
        command.env("GITHUB_API_URL",&server.base).env("GITHUB_PAT","fixture-token")
            .env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE").env_remove(proj::PROJECT_ROOT_ENV).env_remove(proj::RUN_CONTEXT_ENV)
            .args(["--project",root.as_str(),"--machine","remote","restore","--label","output-r-data","--repository","owner/repo",
                "--max-bytes","100000","--max-expanded-bytes","100000","--clear"]);
        let output=command.output().unwrap();assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stderr));
        let value:serde_json::Value=serde_json::from_slice(&output.stdout).unwrap();assert_eq!(value["result"]["label"],"output-r-data");assert_eq!(value["result"]["version"],"v1.10");
        assert_eq!(std::fs::read(root.join("data/data.csv")).unwrap(),b"legacy data");assert!(!root.join("data/old").exists());
        assert_eq!(std::fs::read(root.join("manifest.csv")).unwrap(),history);server.finish();
    }
}
