use std::{fs, path::Path, process::Command};

fn invoke(root: &Path, mode: &str, profile: Option<&str>) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_proj"));
    command.args([mode, "--non-interactive", "--project"]).arg(root).current_dir(root)
        .env_remove(proj::PROJECT_ROOT_ENV).env_remove(proj::RUN_CONTEXT_ENV)
        .env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE").env_remove("PROJR_CLEAR_OUTPUT");
    if let Some(profile) = profile { command.args(["--profile", profile]); }
    command.output().unwrap()
}

#[test]
fn child_queries_use_captured_mapping_profile_and_active_version_after_cwd_and_file_changes() {
    for mode in ["build-dev", "build"] {
      for failing in [false, true] {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("project with spaces"); fs::create_dir(&root).unwrap();
        fs::create_dir(root.join("cache")).unwrap();
        fs::create_dir(root.join("released")).unwrap();
        let external = parent.path().join("external raw"); fs::create_dir(&external).unwrap();
        let other = parent.path().join("other"); fs::create_dir(&other).unwrap();
        fs::write(other.join("VERSION"), "v9.9.9").unwrap();
        fs::write(root.join("VERSION"), "v1.2.3-4\n").unwrap();
        let config = "directories:\n  cache:\n    path: cache\n  output:\n    path: released\nbuild:\n  profile: alpha\n  scripts: [wrong.py]\n  git: false\n  restrictions:\n    not_behind: false\n";
        fs::write(root.join("_proj.yml"), config).unwrap();
        fs::write(root.join("_projr-alpha.yml"), format!("directories:\n  raw-data:\n    path: {}\nbuild:\n  profile: ignored-redirect\n  scripts: [analysis.py]\n", serde_json::to_string(&external.to_string_lossy()).unwrap())).unwrap();
        let script = format!(r#"import json, os, subprocess
from pathlib import Path
root = Path(os.environ['PROJ_PROJECT_ROOT'])
binary = {binary}
other = Path({other})
(root / '_proj.yml').write_text('directories: {{output: {{path: changed}}}}\n')
(root / 'VERSION').write_text('v8.8.8\n')
os.chdir(other)
def query(*args):
 return json.loads(subprocess.check_output([binary, '--machine', *args], text=True))['result']
output = query('path','get','--label','output')['path']
raw = query('path','get','--label','raw-data')['path']
assert Path(output).samefile(root / 'released')
assert Path(raw).samefile(Path({external}))
human = subprocess.check_output([binary,'path','get','--label','output','--target','final','--format','absolute'],text=True).strip()
assert Path(human).samefile(root / 'released')
version = query('version','get')['version']
explicit = query('--project',str(other),'version','get')['version']
assert explicit['major'] == 9
(root/'cache/observed.json').write_text(json.dumps({{'version':version,'profile':os.environ['PROJR_PROFILE'],'context':os.environ['PROJ_RUN_CONTEXT']}}))
raise SystemExit({exit_code})
"#, binary=serde_json::to_string(env!("CARGO_BIN_EXE_proj")).unwrap(), other=serde_json::to_string(&other.to_string_lossy()).unwrap(), external=serde_json::to_string(&external.to_string_lossy()).unwrap(), exit_code=if failing {17} else {0});
        fs::write(root.join("analysis.py"), script).unwrap();
        let result = invoke(&root, mode, None);
        assert_eq!(result.status.success(), !failing, "{} {}", String::from_utf8_lossy(&result.stdout), String::from_utf8_lossy(&result.stderr));
        let seen: serde_json::Value = serde_json::from_slice(&fs::read(root.join("cache/observed.json")).unwrap()).unwrap();
        assert_eq!(seen["profile"], "alpha");
        assert_eq!(seen["version"]["patch"], if mode == "build" {4} else {3});
        assert_eq!(seen["version"]["dev"], if mode == "build" {0} else {4});
        assert!(!Path::new(seen["context"].as_str().unwrap()).exists(), "private context leaked after build");
        if mode == "build-dev" || failing {
            assert_eq!(fs::read_to_string(root.join("VERSION")).unwrap(), "v1.2.3-4\n");
            assert_eq!(fs::read_to_string(root.join("_proj.yml")).unwrap(), config);
        } else {
            let version = proj::version_get_from(camino::Utf8Path::from_path(&root).unwrap()).unwrap();
            assert_eq!(version.cache_key(), "v1.2.4-1");
        }
      }
    }
}

#[test]
fn malformed_context_is_an_error_not_a_discovery_fallback() {
    let project = tempfile::tempdir().unwrap();
    fs::write(project.path().join("VERSION"), "v1.2.3").unwrap();
    let bad = project.path().join("context.json"); fs::write(&bad, "{broken").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_proj")).args(["--machine","version","get"])
        .current_dir(project.path()).env(proj::RUN_CONTEXT_ENV, bad).env_remove(proj::PROJECT_ROOT_ENV).output().unwrap();
    assert!(!output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["ok"], false);
}

#[test]
fn profile_sources_have_one_priority_for_config_and_environment() {
    for (cli, projr, quarto, expected) in [
        (Some("alpha"), "beta", "beta", "alpha"),
        (None, "alpha", "beta", "alpha"),
        (None, "", "beta", "beta"),
        (None, "", "", "alpha"),
    ] {
        let project = tempfile::tempdir().unwrap(); let root = project.path();
        fs::create_dir(root.join("cache")).unwrap();
        fs::write(root.join("_proj.yml"), "directories:\n  cache:\n    path: cache\nbuild:\n  profile: alpha\n  scripts: [wrong.py]\n").unwrap();
        for profile in ["alpha","beta"] {
            fs::write(root.join(format!("_projr-{profile}.yml")), "dev:\n  scripts: [analysis.py]\n").unwrap();
            fs::write(root.join(format!("_environment-{profile}")), format!("PROJCLI_SELECTED_VALUE={profile}\n")).unwrap();
        }
        fs::write(root.join("analysis.py"), "import os,json\nfrom pathlib import Path\nPath('cache/seen').write_text(json.dumps([os.environ['PROJR_PROFILE'],os.environ['PROJCLI_SELECTED_VALUE']]))\n").unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_proj"));
        command.args(["build-dev","--non-interactive","--project"]).arg(root).current_dir(root)
            .env_remove(proj::PROJECT_ROOT_ENV).env_remove(proj::RUN_CONTEXT_ENV)
            .env_remove("PROJCLI_SELECTED_VALUE").env("PROJR_PROFILE", projr).env("QUARTO_PROFILE", quarto);
        if let Some(cli) = cli { command.args(["--profile",cli]); }
        let output = command.output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        let seen: Vec<String> = serde_json::from_slice(&fs::read(root.join("cache/seen")).unwrap()).unwrap();
        assert_eq!(seen, [expected,expected]);
    }
}

#[test]
fn concurrent_library_contexts_keep_child_queries_and_parent_environment_separate() {
    let parent = tempfile::tempdir().unwrap();
    let original = std::env::var_os(proj::RUN_CONTEXT_ENV);
    let mut workers = Vec::new();
    for name in ["alpha", "beta"] {
        let root = parent.path().join(name); fs::create_dir(&root).unwrap();
        fs::create_dir(root.join("cache")).unwrap(); fs::create_dir(root.join("raw")).unwrap();
        fs::write(root.join("_proj.yml"), "directories:\n  cache:\n    path: cache\n  raw-data:\n    path: raw\ndev:\n  scripts: [analysis.py]\n").unwrap();
        let script = format!(r#"import json,os,subprocess,time
from pathlib import Path
root = Path(os.environ['PROJ_PROJECT_ROOT'])
time.sleep(0.25)
value = json.loads(subprocess.check_output([{binary},'--machine','path','get','--label','raw-data','--target','final','--format','absolute'],text=True))
assert Path(value['result']['path']).samefile(root/'raw')
(root/'cache/context').write_text(os.environ['PROJ_RUN_CONTEXT'])
"#,binary=serde_json::to_string(env!("CARGO_BIN_EXE_proj")).unwrap());
        fs::write(root.join("analysis.py"),script).unwrap();
        let build_root = root.clone();
        workers.push((root,std::thread::spawn(move || proj::build_project_with_execution(camino::Utf8Path::from_path(&build_root).unwrap(),proj::BuildMode::Dev,Some(name),None,proj::ExecutionDirectory::Project))));
    }
    let mut contexts = Vec::new();
    for (root,worker) in workers {
        worker.join().unwrap().unwrap();
        let context = fs::read_to_string(root.join("cache/context")).unwrap();
        assert!(!Path::new(&context).exists()); contexts.push(context);
    }
    assert_ne!(contexts[0],contexts[1]);
    assert_eq!(std::env::var_os(proj::RUN_CONTEXT_ENV),original);
}
