use std::{fs,path::Path,process::Command};

fn invoke(root:&Path, caller:&Path, project:bool)->std::process::Output {
    let mut cmd=Command::new(env!("CARGO_BIN_EXE_proj"));
    cmd.args(["build-dev","--non-interactive","--project"]).arg(root).current_dir(caller)
        .env_remove(proj::PROJECT_ROOT_ENV).env_remove(proj::RUN_CONTEXT_ENV).env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE");
    if project {cmd.args(["--execution-dir","project"]);}
    let mut paths=vec![root.join("bin")];paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()));
    cmd.env("PATH",std::env::join_paths(paths).unwrap()).output().unwrap()
}

#[test]
fn native_scripts_and_hooks_inherit_launch_cwd_while_project_paths_survive_cwd_changes() {
    for project in [false,true] {
        let temp=tempfile::tempdir().unwrap();let root=temp.path().join("project");let caller=temp.path().join("caller");
        fs::create_dir_all(root.join("analysis")).unwrap();fs::create_dir(root.join("cache")).unwrap();fs::create_dir(root.join("raw")).unwrap();fs::create_dir(&caller).unwrap();
        fs::write(root.join("raw/sentinel.txt"),"project data").unwrap();
        fs::write(root.join("_proj.yml"),"directories:\n  cache:\n    path: cache\n  raw:\n    path: raw\nbuild:\n  scripts: [analysis/first.py, analysis/second.py]\n  hooks:\n    pre: [analysis/pre.py]\n    post: [analysis/post.py]\n  git: false\n  restrictions:\n    not_behind: false\n").unwrap();
        for stage in ["pre","first","second","post"] {
            let expected=if project {&root} else {&caller};
            fs::write(root.join(format!("analysis/{stage}.py")),format!(r#"import os,json,subprocess
from pathlib import Path
root=Path(os.environ['PROJ_PROJECT_ROOT'])
assert Path.cwd().samefile(Path({expected}))
for cwd in [Path.cwd(), root/'analysis']:
 os.chdir(cwd)
 value=json.loads(subprocess.check_output([{binary},'--machine','path','get','--label','raw','--format','auto','sentinel.txt'],text=True))
 target=Path(value['result']['path'])
 assert not target.is_absolute()
 assert target.read_text()=='project data'
with (root/'cache/order').open('a') as stream: stream.write('{stage}\n')
"#,expected=serde_json::to_string(&expected.to_string_lossy()).unwrap(),binary=serde_json::to_string(env!("CARGO_BIN_EXE_proj")).unwrap())).unwrap();
        }
        let output=invoke(&root,&caller,project);
        assert!(output.status.success(),"{} {}",String::from_utf8_lossy(&output.stdout),String::from_utf8_lossy(&output.stderr));
        assert_eq!(fs::read_to_string(root.join("cache/order")).unwrap(),"pre\nfirst\nsecond\npost\n");
        assert!(!caller.join("order").exists());assert!(!root.join("analysis/order").exists());
    }
}

#[cfg(unix)]
#[test]
fn renderer_dispatch_retains_native_defaults_and_only_passes_explicit_root_overrides() {
    use std::os::unix::fs::PermissionsExt;
    for extension in ["qmd","Rmd"] {
      for project in [false,true] {
        let temp=tempfile::tempdir().unwrap();let root=temp.path().canonicalize().unwrap().join("project");let caller=temp.path().canonicalize().unwrap().join("caller");
        fs::create_dir_all(root.join("analysis")).unwrap();fs::create_dir(root.join("cache")).unwrap();fs::create_dir(root.join("bin")).unwrap();fs::create_dir(&caller).unwrap();
        let document=format!("analysis/report's é.{extension}");
        fs::write(root.join(&document),"---\ntitle: test\n---\n").unwrap();
        fs::write(root.join("_proj.yml"),format!("directories:\n  cache: {{path: cache}}\n  docs: {{path: published}}\nbuild:\n  scripts: [{}]\n",serde_json::to_string(&document).unwrap())).unwrap();
        let fake=r#"#!/usr/bin/env python3
import os,sys,json
from pathlib import Path
root=Path(os.environ['PROJ_PROJECT_ROOT'])
if '--version' in sys.argv: print('fixture 1.0'); raise SystemExit(0)
if sys.argv[1:2]==['-e'] and 'rmarkdown::render(' not in sys.argv[2]: raise SystemExit(0)
context=json.loads(Path(os.environ['PROJ_RUN_CONTEXT']).read_text())
(root/'cache/renderer.json').write_text(json.dumps({'args':sys.argv[1:],'cwd':str(Path.cwd()),'render':context['render']}))
"#;
        for name in ["quarto","Rscript"] {let executable=root.join("bin").join(name);fs::write(&executable,fake).unwrap();fs::set_permissions(executable,fs::Permissions::from_mode(0o755)).unwrap();}
        let output=invoke(&root,&caller,project);assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stderr));
        let observed:serde_json::Value=serde_json::from_slice(&fs::read(root.join("cache/renderer.json")).unwrap()).unwrap();
        assert_eq!(Path::new(observed["cwd"].as_str().unwrap()).canonicalize().unwrap(),if project {root.clone()} else {caller.clone()});
        assert_eq!(Path::new(observed["render"]["document"].as_str().unwrap()),root.join(&document));
        let args=observed["args"].as_array().unwrap();
        if extension=="qmd" {
            assert_eq!(args.iter().any(|arg|arg=="--execute-dir"),project);
            if project {assert_eq!(args.last().unwrap(),root.to_str().unwrap());}
        } else {
            assert_eq!(args[3],if project {"project"} else {"native"});
            assert!(args[1].as_str().unwrap().contains("else NULL"));
            assert!(!args[1].as_str().unwrap().contains("report's"));
        }
      }
    }
}
