//! Startup observations use actual CLI builds; no global environment mutation.
use std::{fs,path::Path,process::Command};

fn fixture(root:&Path,scripts:&str) {
    fs::write(root.join("VERSION"),b"v1.2.3-19\r\n").unwrap();
    fs::write(root.join("_proj.yml"),format!("directories:\n  cache: {{path: cache}}\n  output: {{path: released}}\nparameters:\n  token: CONFIG-PRIVATE-TOKEN\n  endpoint: https://user:secret@private.invalid/api?token=CONFIG-QUERY\nbuild:\n  scripts: {scripts}\n  git: false\n")).unwrap();
    fs::create_dir(root.join("released")).unwrap();
}
fn build(root:&Path,empty_path:bool)->std::process::Output {
    let mut command=Command::new(env!("CARGO_BIN_EXE_proj"));
    command.current_dir(root).args(["build","--non-interactive","--project"]).arg(root)
        .env("GH_TOKEN","ENV-PRIVATE-TOKEN")
        .env("PROJCLI_PRIVATE_URL","https://user:secret@private.invalid/?token=ENV-QUERY")
        .env_remove(proj::PROJECT_ROOT_ENV).env_remove(proj::RUN_CONTEXT_ENV)
        .env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE").env_remove("PROJR_CLEAR_OUTPUT");
    if empty_path {command.env("PATH","");}
    command.output().unwrap()
}
fn success(output:std::process::Output) {
    assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stderr));
}
fn record(root:&Path,version:&str)->serde_json::Value {
    let records:Vec<serde_json::Value>=fs::read_dir(root.join(".proj/log")).unwrap()
        .map(|entry|entry.unwrap().path()).filter(|path|path.extension().is_some_and(|value|value=="json"))
        .map(|path|serde_json::from_slice(&fs::read(path).unwrap()).unwrap())
        .filter(|record:&serde_json::Value|record["release_version"]==version).collect();
    assert_eq!(records.len(),1);
    records.into_iter().next().unwrap()
}
fn file<'a>(items:&'a serde_json::Value,name:&str)->&'a serde_json::Value {
    items.as_array().unwrap().iter().find(|file|file["name"]==name).unwrap()
}
fn digest(bytes:&[u8])->String {blake3::hash(bytes).to_hex().to_string()}

#[test]
fn actual_python_build_binds_startup_sources_dependencies_policy_and_sanitized_runner() {
    let temporary=tempfile::tempdir().unwrap();let root=temporary.path().canonicalize().unwrap();
    fixture(&root,"[analysis.py]");
    let config=fs::read_to_string(root.join("_proj.yml")).unwrap();
    fs::write(root.join("_proj.yml"),format!("{config}  hooks:\n    pre: [start.py]\n")).unwrap();
    let analysis=b"from pathlib import Path\nPath('released/result').write_bytes(b'computed')\n";
    let start=b"from pathlib import Path\nPath('renv.lock').write_bytes(b'changed by start hook')\n";
    let dependency=b"dependency with https://user:secret@private.invalid/?token=DEPENDENCY-QUERY\n";
    fs::write(root.join("analysis.py"),analysis).unwrap();
    fs::write(root.join("start.py"),start).unwrap();
    fs::write(root.join("renv.lock"),dependency).unwrap();
    success(build(&root,false));
    assert_eq!(fs::read(root.join("released/result")).unwrap(),b"computed");
    assert_eq!(fs::read(root.join("renv.lock")).unwrap(),b"changed by start hook");
    let first=record(&root,"v1.2.4");
    let evidence=&first["startup_provenance"];
    assert_eq!(evidence["stage"],"before_start_hooks");
    assert_eq!(evidence["source_scope"],"selected_scripts_and_hooks_only");
    assert_eq!(evidence["source_state"],"git_not_queried");
    assert!(evidence["source_head"].is_null());
    assert_eq!(evidence["selected_sources_omitted"],0);
    let selected=&evidence["selected_sources"];
    assert_eq!(selected.as_array().unwrap().len(),2);
    for (name,bytes) in [("analysis.py",analysis.as_slice()),("start.py",start.as_slice())] {
        let source=file(selected,name);
        assert_eq!(source["status"],"observed");
        assert_eq!(source["blake3"],digest(bytes));
        assert_eq!(source["bytes"],bytes.len());
        assert_eq!(source["outside_project"],false);
        assert_eq!(source["name_redacted"],false);
    }
    assert_eq!(file(&evidence["dependencies"],"renv.lock")["blake3"],digest(dependency));
    assert_eq!(file(&evidence["dependencies"],"requirements.txt")["status"],"missing");
    let runner=&evidence["runners"][0];
    assert!(matches!(runner["name"].as_str().unwrap(),"python"|"python3"));
    assert_eq!(runner["status"],"reported");
    assert!(regex::Regex::new(r"^\d+\.\d+(?:\.\d+)?$").unwrap().is_match(runner["reported_version"].as_str().unwrap()));
    let configuration=&evidence["configuration"];
    assert_eq!(configuration["scope"],"validated_flags_and_counts_only");
    assert_eq!(configuration["policy"]["git_commit"],false);
    assert_eq!(configuration["policy"]["declared_directories"],2);
    assert_eq!(configuration["policy"]["cache_selected"],true);
    assert_eq!(configuration["policy_blake3"],digest(&serde_json::to_vec(&configuration["policy"]).unwrap()));
    let text=serde_json::to_string(&first).unwrap();
    for secret in ["CONFIG-PRIVATE-TOKEN","ENV-PRIVATE-TOKEN","CONFIG-QUERY","ENV-QUERY",
        "DEPENDENCY-QUERY","user:secret","private.invalid","PROJCLI_PRIVATE_URL"] {
        assert!(!text.contains(secret),"{secret}");
    }
    assert!(!text.contains(root.to_str().unwrap()),"personal absolute project path leaked");

    let changed=b"from pathlib import Path\nPath('released/result').write_bytes(b'new computed')\n";
    fs::write(root.join("analysis.py"),changed).unwrap();
    success(build(&root,false));
    let second=record(&root,"v1.2.5");
    assert_eq!(file(&second["startup_provenance"]["selected_sources"],"analysis.py")["blake3"],digest(changed));
    assert_eq!(file(&second["startup_provenance"]["dependencies"],"renv.lock")["blake3"],digest(b"changed by start hook"));
    assert_eq!(second["startup_provenance"]["configuration"],evidence["configuration"]);
    assert_eq!(record(&root,"v1.2.4"),first,"historical startup evidence changed");
}

#[test]
fn optional_dependency_limits_and_unknowns_keep_standalone_no_runner_builds_working() {
    let temporary=tempfile::tempdir().unwrap();let root=temporary.path().canonicalize().unwrap();
    fixture(&root,"[]");
    fs::File::create(root.join("renv.lock")).unwrap().set_len(8*1024*1024+1).unwrap();
    fs::write(root.join("requirements.txt"),b"known declarations").unwrap();
    fs::create_dir(root.join("poetry.lock")).unwrap();
    success(build(&root,true));
    let log=record(&root,"v1.2.4");
    let evidence=&log["startup_provenance"];
    assert_eq!(log["status"],"completed");
    assert_eq!(evidence["runners"],serde_json::json!([]));
    assert_eq!(evidence["selected_sources"],serde_json::json!([]));
    assert_eq!(file(&evidence["dependencies"],"renv.lock")["status"],"limit");
    assert!(file(&evidence["dependencies"],"renv.lock")["blake3"].is_null());
    assert_eq!(file(&evidence["dependencies"],"poetry.lock")["status"],"unavailable");
    assert_eq!(file(&evidence["dependencies"],"requirements.txt")["blake3"],digest(b"known declarations"));
    assert!(!root.join(".git").exists());
}

#[cfg(unix)]
#[test]
fn linked_and_special_dependency_declarations_are_not_followed_or_opened() {
    let temporary=tempfile::tempdir().unwrap();let root=temporary.path().canonicalize().unwrap();
    fixture(&root,"[]");
    fs::write(root.join("outside-private"),b"do not read or change").unwrap();
    std::os::unix::fs::symlink(root.join("outside-private"),root.join("renv.lock")).unwrap();
    assert!(Command::new("mkfifo").arg(root.join("requirements.txt")).status().unwrap().success());
    success(build(&root,false));
    let log=record(&root,"v1.2.4");
    for name in ["renv.lock","requirements.txt"] {
        let dependency=file(&log["startup_provenance"]["dependencies"],name);
        assert_eq!(dependency["status"],"unavailable");
        assert!(dependency["blake3"].is_null());
        assert!(dependency["bytes"].is_null());
    }
    assert_eq!(fs::read(root.join("outside-private")).unwrap(),b"do not read or change");
}


#[test]
fn parent_relative_external_source_names_are_redacted_using_physical_identity() {
    let temporary=tempfile::tempdir().unwrap();let base=temporary.path().canonicalize().unwrap();
    let root=base.join("project");fs::create_dir(&root).unwrap();
    fixture(&root,"['../private-machine-owner.py']");
    let bytes=b"from pathlib import Path\nPath('released/result').write_bytes(b'external computed')\n";
    fs::write(base.join("private-machine-owner.py"),bytes).unwrap();
    success(build(&root,false));
    let log=record(&root,"v1.2.4");
    let source=&log["startup_provenance"]["selected_sources"][0];
    assert_eq!(source["outside_project"],true);
    assert_eq!(source["name_redacted"],true);
    assert_eq!(source["name"],"source-1");
    assert_eq!(source["status"],"observed");
    assert_eq!(source["blake3"],digest(bytes));
    let text=serde_json::to_string(&log).unwrap();
    assert!(!text.contains("private-machine-owner.py"));
    assert!(!text.contains("../"));
    assert_eq!(fs::read(root.join("released/result")).unwrap(),b"external computed");
    assert_eq!(fs::read(base.join("private-machine-owner.py")).unwrap(),bytes);
}
