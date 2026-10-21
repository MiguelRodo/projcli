use std::{fs,path::Path,process::{Command,Output}};

fn query(root:&Path,caller:&Path,extra:&[&str])->Output {
    Command::new(env!("CARGO_BIN_EXE_proj")).args(["--machine","path","get","--project"]).arg(root)
        .args(extra).current_dir(caller).env_remove(proj::PROJECT_ROOT_ENV).env_remove(proj::RUN_CONTEXT_ENV)
        .env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE").output().unwrap()
}
fn returned(output:Output)->std::path::PathBuf {
    assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stdout));
    let value:serde_json::Value=serde_json::from_slice(&output.stdout).unwrap();
    value["result"]["path"].as_str().unwrap().into()
}

#[test]
fn safe_auto_defaults_keep_one_target_and_explicit_final_absolute_overrides() {
    let temp=tempfile::tempdir().unwrap();let directory=temp.path().canonicalize().unwrap();
    let root=directory.join("project");let caller=directory.join("caller");fs::create_dir(&root).unwrap();fs::create_dir(&caller).unwrap();
    fs::write(root.join("VERSION"),"v1.2.3-4\n").unwrap();
    fs::write(root.join("_proj.yml"),"directories:\n  cache: {path: scratch}\n  output: {path: released}\n").unwrap();
    let relative=returned(query(&root,&caller,&["--label","output","figures","result.png"]));
    assert!(!relative.is_absolute());
    assert_eq!(caller.join(relative),caller.join("../project/scratch/projr/v1.2.3-4/output/figures/result.png"));
    let absolute=returned(query(&root,&caller,&["--label","output","--format","absolute","figures","result.png"]));
    assert_eq!(absolute,root.join("scratch/projr/v1.2.3-4/output/figures/result.png"));
    assert_eq!(returned(query(&root,&caller,&["--label","output","--target","final","--format","absolute"])),root.join("released"));
    assert!(!root.join("scratch").exists());assert!(!root.join("released").exists());
}

#[test]
fn merged_representation_settings_and_explicit_cli_obey_precedence() {
    let temp=tempfile::tempdir().unwrap();let root=temp.path().canonicalize().unwrap();
    fs::write(root.join("VERSION"),"v1.2.3-4\n").unwrap();
    fs::write(root.join("_proj.yml"),"directories:\n  cache: {path: cache}\n  output: {path: released}\nsettings:\n  path-representation: auto\nbuild:\n  profile: active\n").unwrap();
    fs::write(root.join("_projr-active.yml"),"settings:\n  path-representation: absolute\n").unwrap();
    assert!(returned(query(&root,&root,&["--label","output"])).is_absolute());
    fs::write(root.join("_projr-local.yml"),"settings:\n  path-representation: relative\n").unwrap();
    assert_eq!(returned(query(&root,&root,&["--label","output"])),Path::new("cache/projr/v1.2.3-4/output"));
    assert!(returned(query(&root,&root,&["--label","output","--format","absolute"])).is_absolute());
    assert_eq!(returned(query(&root,&root,&["--label","output","--relative-to","project"])),Path::new("cache/projr/v1.2.3-4/output"));
}

#[test]
fn run_settings_are_frozen_while_cli_execution_override_and_fresh_caller_cwd_work() {
    for (configured,explicit) in [("project",None),("project",Some("native")),("native",Some("project"))] {
      for format in ["auto","absolute"] {
        let temp=tempfile::tempdir().unwrap();let directory=temp.path().canonicalize().unwrap();
        let root=directory.join("project");let caller=directory.join("caller");fs::create_dir(&root).unwrap();fs::create_dir(&caller).unwrap();
        fs::create_dir(root.join("raw")).unwrap();fs::create_dir(root.join("cache")).unwrap();
        fs::write(root.join("raw/input"),"sentinel").unwrap();fs::write(root.join("VERSION"),"v1.2.3-4\n").unwrap();
        let config=format!("directories:\n  cache: {{path: cache}}\n  raw: {{path: raw}}\n  output: {{path: released}}\nsettings:\n  execution-directory: {configured}\n  path-representation: {format}\nbuild:\n  scripts: [analysis.py]\n  git: false\n");
        fs::write(root.join("_proj.yml"),&config).unwrap();
        let expected=if explicit.unwrap_or(configured)=="project" {&root} else {&caller};
        fs::write(root.join("analysis.py"),format!(r#"import os,json,subprocess
from pathlib import Path
root=Path(os.environ['PROJ_PROJECT_ROOT'])
assert Path.cwd().samefile(Path({expected}))
(root/'_proj.yml').write_text('invalid: configuration changed during execution')
os.environ['PROJR_PROFILE']='changed'
def lookup(label,*parts):
 value=json.loads(subprocess.check_output([{binary},'--machine','path','get','--label',label,*parts],text=True))
 path=Path(value['result']['path'])
 assert path.is_absolute()=={absolute}
 return path
for cwd in [Path.cwd(),root/'raw']:
 os.chdir(cwd)
 assert lookup('raw','input').read_text()=='sentinel'
 target=lookup('output','retained.txt');target.parent.mkdir(parents=True,exist_ok=True);target.write_text('retained safe output')
"#,expected=serde_json::to_string(&expected.to_string_lossy()).unwrap(),binary=serde_json::to_string(env!("CARGO_BIN_EXE_proj")).unwrap(),absolute=if format=="absolute" {"True"} else {"False"})).unwrap();
        let mut cmd=Command::new(env!("CARGO_BIN_EXE_proj"));cmd.args(["build-dev","--non-interactive","--project"]).arg(&root).current_dir(&caller)
            .env_remove(proj::PROJECT_ROOT_ENV).env_remove(proj::RUN_CONTEXT_ENV).env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE");
        if let Some(explicit)=explicit {cmd.args(["--execution-dir",explicit]);}
        let result=cmd.output().unwrap();assert!(result.status.success(),"{}",String::from_utf8_lossy(&result.stderr));
        assert_eq!(fs::read_to_string(root.join("_proj.yml")).unwrap(),config);
        assert_eq!(fs::read_to_string(root.join("cache/projr/v1.2.3-4/output/retained.txt")).unwrap(),"retained safe output");
        assert!(!root.join("released").exists());
      }
    }
}

#[test]
fn invalid_settings_fail_per_active_layer_before_hooks() {
    for key in ["execution-directory","path-representation"] {
      for invalid in ["null","false","unsupported"] {
       for layer in ["_proj.yml","_projr-local.yml","_projr-active.yml"] {
        let temp=tempfile::tempdir().unwrap();let root=temp.path().canonicalize().unwrap();
        fs::write(root.join("_proj.yml"),"settings:\n  execution-directory: native\n  path-representation: auto\nbuild:\n  profile: active\n  scripts: []\n  hooks:\n    pre: [hook.py]\n").unwrap();
        fs::write(root.join("hook.py"),"raise RuntimeError('hook must not execute')\n").unwrap();
        fs::write(root.join(layer),format!("settings:\n  {key}: {invalid}\n")).unwrap();
        let result=Command::new(env!("CARGO_BIN_EXE_proj")).args(["build-dev","--non-interactive","--project"]).arg(&root).current_dir(&root)
            .env_remove(proj::PROJECT_ROOT_ENV).env_remove(proj::RUN_CONTEXT_ENV).env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE").output().unwrap();
        assert!(!result.status.success(),"{key}/{invalid}/{layer}");
        assert!(!String::from_utf8_lossy(&result.stderr).contains("hook must not execute"));
        assert!(!root.join("VERSION").exists());assert!(!root.join(".gitignore").exists());
       }
      }
    }
}
