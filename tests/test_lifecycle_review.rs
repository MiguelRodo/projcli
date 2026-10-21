use std::{fs, path::Path, process::{Command, Output}};

fn run(root: &Path, mode: &str) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_proj"));
    cmd.args([mode,"--non-interactive","--project"]).arg(root).current_dir(root)
        .env_remove(proj::PROJECT_ROOT_ENV).env_remove(proj::RUN_CONTEXT_ENV)
        .env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE").env_remove("PROJR_CLEAR_OUTPUT");
    if root.join("bin").exists() {
        let mut paths=vec![root.join("bin")];paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()));
        cmd.env("PATH",std::env::join_paths(paths).unwrap());
    }
    cmd.output().unwrap()
}

fn setup(root: &Path, scripts: &str, extra: &str) {
    fs::create_dir_all(root.join("published")).unwrap();fs::create_dir_all(root.join("released")).unwrap();
    fs::create_dir_all(root.join("cache")).unwrap();
    fs::write(root.join("VERSION"),"v1.2.3-4\n").unwrap();
    fs::write(root.join("published/old.html"),"old published").unwrap();
    fs::write(root.join("released/old"),"old output").unwrap();
    fs::write(root.join("_proj.yml"),format!("directories:\n  cache: {{path: cache}}\n  output: {{path: released}}\n  docs: {{path: published}}\nbuild:\n  scripts: {scripts}\n  git: false\n{extra}")).unwrap();
}

#[cfg(unix)]
fn fake_renderer(root: &Path) {
    use std::os::unix::fs::PermissionsExt;
    fs::create_dir_all(root.join("bin")).unwrap();
    let source=r#"#!/usr/bin/env python3
import os,sys,json,re
from pathlib import Path
root=Path(os.environ.get('PROJ_PROJECT_ROOT','.'))
if sys.argv[1:2]==['--version']: raise SystemExit(0)
if sys.argv[1:2]==['-e'] and 'render' not in sys.argv[2]: raise SystemExit(0)
if sys.argv[1:2]==['render'] or (sys.argv[1:2]==['-e'] and 'render' in sys.argv[2]):
 (root/'cache/renderer-called').write_text('called')
 if (root/'cache/noop').exists(): raise SystemExit(0)
 context=json.loads(Path(os.environ['PROJ_RUN_CONTEXT']).read_text())
 document=context['render'].get('document')
 if document: Path(document).with_suffix('.html').write_text('fresh document')
 else:
  config=(root/('_quarto.yml' if (root/'_quarto.yml').exists() else '_bookdown.yml')).read_text()
  stage=Path(re.search(r'output[-_]dir:\s*[\"\x27]?([^\s\"\x27]+)',config)[1])
  stage.mkdir(parents=True,exist_ok=True);(stage/'index.html').write_text('fresh project')
"#;
    for name in ["quarto","Rscript"] {let path=root.join("bin").join(name);fs::write(&path,source).unwrap();fs::set_permissions(path,fs::Permissions::from_mode(0o755)).unwrap();}
}

#[cfg(unix)]
#[test]
fn controlled_metadata_writes_reject_hook_links_without_touching_external_bytes() {
    for metadata in ["VERSION",".gitignore",".Rbuildignore","_quarto.yml","_bookdown.yml"] {
        let temp=tempfile::tempdir().unwrap();let root=temp.path().join("project");fs::create_dir(&root).unwrap();
        let external=temp.path().join("external");fs::write(&external,"external sentinel").unwrap();
        let document=metadata.starts_with("_quarto")||metadata.starts_with("_bookdown");
        setup(&root,if document {"[report.qmd]"} else {"[analysis.py]"},"  hooks:\n    pre: [start.py]\n  dest: [archive]\nremotes:\n  local:\n    archive:\n      path: archive\n      structure: cas\n      inspect: manifest\n      content: [output]\n");
        fs::write(root.join("report.qmd"),"Report\n").unwrap();fs::write(root.join("analysis.py"),"pass\n").unwrap();
        if metadata=="_quarto.yml" {fs::write(root.join(metadata),"project:\n  type: default\n").unwrap();}
        if metadata=="_bookdown.yml" {fs::write(root.join(metadata),"book_filename: _main\n").unwrap();}
        fs::write(root.join("start.py"),format!("from pathlib import Path\np=Path({metadata:?})\nif p.exists(): p.unlink()\np.symlink_to({:?})\n",external.to_str().unwrap())).unwrap();
        fake_renderer(&root);
        let result=run(&root,"build");assert!(!result.status.success(),"{metadata}");
        assert_eq!(fs::read_to_string(&external).unwrap(),"external sentinel");
        assert!(!root.join("archive").exists());assert!(!root.join("cache/renderer-called").exists());
    }
}

#[cfg(unix)]
#[test]
fn promotion_rejects_symlink_and_fifo_sources_before_publication() {
    for kind in ["symlink","fifo"] {
        let temp=tempfile::tempdir().unwrap();let root=temp.path().canonicalize().unwrap();
        setup(&root,"[analysis.py]","  dest: [archive]\nremotes:\n  local:\n    archive:\n      path: archive\n      structure: cas\n      inspect: manifest\n      content: [output]\n");
        fs::write(root.join("external"),"external sentinel").unwrap();
        fs::write(root.join("analysis.py"),format!("import os\nfrom pathlib import Path\np=Path('cache/projr/v1.2.4/output');p.mkdir(parents=True)\n{}\n",if kind=="symlink" {"(p/'artifact').symlink_to(Path('external').resolve())"}else{"os.mkfifo(p/'artifact')"})).unwrap();
        let result=run(&root,"build");assert!(!result.status.success());
        assert_eq!(fs::read_to_string(root.join("released/old")).unwrap(),"old output");
        assert_eq!(fs::read_to_string(root.join("VERSION")).unwrap(),"v1.2.3-4\n");
        assert!(!root.join("archive").exists());
    }
}

#[cfg(unix)]
#[test]
fn frozen_layout_and_immediate_native_freshness_survive_ordered_scripts() {
    for case in ["before-layout","after-layout","planted-noop","book-name","book-hook","book-noop"] {
        let temp=tempfile::tempdir().unwrap();let root=temp.path().canonicalize().unwrap();
        let scripts=if case=="after-layout" {"[report.qmd, mutate.py]"} else {"[mutate.py, report.qmd]"};
        setup(&root,if case=="book-hook" {"[report.qmd]"} else {scripts},if case=="book-hook" {"  hooks:\n    pre: [mutate.py]\n"} else {""});fake_renderer(&root);
        let original="---\nformat: html\n---\nReport\n";fs::write(root.join("report.qmd"),original).unwrap();
        fs::write(root.join("other.html"),"unrelated sibling").unwrap();
        fs::write(root.join("report.html"),"original native").unwrap();
        fs::create_dir(root.join("_main_files")).unwrap();fs::write(root.join("_main_files/widget.html"),"old widget").unwrap();
        fs::create_dir(root.join("custom_files")).unwrap();fs::write(root.join("custom_files/sentinel"),"custom sentinel").unwrap();
        if case.starts_with("book") {fs::write(root.join("_bookdown.yml"),"book_filename: _main\n").unwrap();}
        let mutation=match case {
            "before-layout"|"after-layout" => "Path('report.qmd').write_text('---\\nformat: html\\noutput-file: other.html\\n---\\nReport\\n')",
            "planted-noop" => "Path('report.html').write_text('planted stale artifact');Path('cache/noop').touch()",
            "book-name"|"book-hook" => "Path('_bookdown.yml').write_text('book_filename: custom\\n')",
            _ => "Path('cache/noop').touch()",
        };
        fs::write(root.join("mutate.py"),format!("from pathlib import Path\n{mutation}\n")).unwrap();
        let result=run(&root,"build");let success=case=="after-layout";
        assert_eq!(result.status.success(),success,"{case}: {}",String::from_utf8_lossy(&result.stderr));
        assert_eq!(fs::read_to_string(root.join("other.html")).unwrap(),"unrelated sibling");
        assert_eq!(fs::read_to_string(root.join("report.html")).unwrap(),"original native");
        assert_eq!(fs::read_to_string(root.join("_main_files/widget.html")).unwrap(),"old widget");
        assert_eq!(fs::read_to_string(root.join("custom_files/sentinel")).unwrap(),"custom sentinel");
        if success {assert_eq!(fs::read_to_string(root.join("published/report.html")).unwrap(),"fresh document");}
        else {assert_eq!(fs::read_to_string(root.join("VERSION")).unwrap(),"v1.2.3-4\n");assert_eq!(fs::read_to_string(root.join("published/old.html")).unwrap(),"old published");}
    }
}

#[test]
fn two_managed_git_builds_accept_only_the_exact_recorded_dev_version_change() {
    let temp=tempfile::tempdir().unwrap();let root=temp.path().canonicalize().unwrap();
    setup(&root,"[]","");
    let config=fs::read_to_string(root.join("_proj.yml")).unwrap().replace("git: false","git: {commit: true, push: false}");
    fs::write(root.join("_proj.yml"),config).unwrap();
    let git=|args: &[&str]| {let result=Command::new("git").current_dir(&root).args(args).output().unwrap();assert!(result.status.success(),"{}",String::from_utf8_lossy(&result.stderr));result.stdout};
    git(&["init"]);git(&["config","user.name","Fixture"]);git(&["config","user.email","fixture@example.invalid"]);git(&["add","-A"]);git(&["commit","-m","initial"]);
    for version in ["1.2.4","1.2.5"] {
        let result=run(&root,"build");assert!(result.status.success(),"{}",String::from_utf8_lossy(&result.stderr));
        assert_eq!(String::from_utf8(git(&["show","HEAD:VERSION"])).unwrap(),format!("Version: v{version}"));
        assert_eq!(fs::read_to_string(root.join("VERSION")).unwrap(),format!("Version: v{version}.1"));
    }
    fs::write(root.join("VERSION"),"Version: v9.9.9.1").unwrap();assert!(!run(&root,"build").status.success());
    fs::write(root.join("VERSION"),"Version: v1.2.5.1").unwrap();fs::write(root.join("untracked-private"),"private").unwrap();assert!(!run(&root,"build").status.success());
}


#[cfg(unix)]
#[test]
fn project_selection_and_output_preparation_precede_final_output_pre_clear() {
    let temp=tempfile::tempdir().unwrap();let root=temp.path().canonicalize().unwrap();
    setup(&root,"[analysis.py, report.qmd]","  hooks:\n    pre: [start.py]\nsettings:\n  output-clear-stage: pre\n");fake_renderer(&root);
    fs::write(root.join("_quarto.yml"),"project:\n  type: default\n").unwrap();
    fs::write(root.join("report.qmd"),"Report\n").unwrap();
    fs::write(root.join("start.py"),"from pathlib import Path\nassert 'render:' not in Path('_quarto.yml').read_text()\nassert Path('released/old').read_text()=='old output'\nPath('cache/start').touch()\n").unwrap();
    fs::write(root.join("analysis.py"),"from pathlib import Path\ny=Path('_quarto.yml').read_text()\nassert 'render:' in y and 'report.qmd' in y and 'output-dir:' in y\nassert not Path('released/old').exists()\nassert Path('cache/start').exists()\nPath('cache/analysis').touch()\n").unwrap();
    let result=run(&root,"build");assert!(result.status.success(),"{}",String::from_utf8_lossy(&result.stderr));
    assert!(root.join("cache/analysis").exists());assert!(root.join("published/index.html").exists());
    assert_eq!(fs::read_to_string(root.join("_quarto.yml")).unwrap(),"project:\n  type: default\n");
}
