#![cfg(unix)]
use std::{fs, process::Command, os::unix::fs::PermissionsExt};

#[test]
fn project_embedding_headers_restore_exact_bytes_on_success_failure_and_source_change() {
    for case in ["success","render-failure","source-change"] {
        let temporary=tempfile::tempdir().unwrap();
        let root=temporary.path().canonicalize().unwrap();
        for directory in ["analysis","cache","published","bin"] { fs::create_dir(root.join(directory)).unwrap(); }
        let source=b"---\r\nformat: html\r\n---\r\n![literal](../image.png)\r\n";
        fs::write(root.join("analysis/report.qmd"),source).unwrap();
        fs::write(root.join("published/previous.html"),b"old final report").unwrap();
        fs::write(root.join("VERSION"),"v1.2.3-4\n").unwrap();
        let project=b"# retain exact bytes\nproject:\n  type: default\n";
        fs::write(root.join("_quarto.yml"),project).unwrap();
        let scripts=if case=="source-change" {"[change.py, analysis/report.qmd]"} else {"[analysis/report.qmd]"};
        fs::write(root.join("_proj.yml"),format!("directories:\n  cache: {{path: cache}}\n  docs: {{path: published}}\nbuild:\n  scripts: {scripts}\n  git: false\n")).unwrap();
        fs::write(root.join("change.py"),"from pathlib import Path\nPath('analysis/report.qmd').write_text('---\\nformat: html\\n---\\nchanged after preparation\\n')\n").unwrap();
        let fake=r#"#!/usr/bin/env python3
import os,sys,json,re
from pathlib import Path
root=Path(os.environ['PROJ_PROJECT_ROOT'])
if '--version' in sys.argv: print('fixture');raise SystemExit(0)
if sys.argv[1:2]==['inspect']:
 path=Path(sys.argv[2])
 print(json.dumps({'files':{'input':[str(root/'analysis/report.qmd')]}} if path.is_dir() else {'formats':{'html':{'pandoc':{}}}}))
 raise SystemExit(0)
if sys.argv[1:2]==['render']:
 source=(root/'analysis/report.qmd').read_text()
 assert 'embed-resources: true' in source and '![literal](../image.png)' in source
 (root/'cache/prepared-header-observed').write_text('yes')
 stage=Path(re.search(r'output-dir:\s*["\x27]?([^\s"\x27]+)',(root/'_quarto.yml').read_text())[1])
 (stage/'report.html').write_text('fresh rendered report')
 raise SystemExit(17 if os.environ.get('FIXTURE_RENDER_FAILURE')=='1' else 0)
"#;
        fs::write(root.join("bin/quarto"),fake).unwrap();
        fs::set_permissions(root.join("bin/quarto"),fs::Permissions::from_mode(0o755)).unwrap();
        let mut paths=vec![root.join("bin")];paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()));
        let output=Command::new(env!("CARGO_BIN_EXE_proj")).args(["build-dev","--non-interactive","--project"]).arg(&root).current_dir(&root)
            .env("PATH",std::env::join_paths(paths).unwrap()).env("FIXTURE_RENDER_FAILURE",if case=="render-failure" {"1"} else {"0"})
            .env_remove(proj::PROJECT_ROOT_ENV).env_remove(proj::RUN_CONTEXT_ENV).env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE")
            .output().unwrap();
        let log=String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.success(),case=="success","{case}: {log}");
        if case=="render-failure" {assert!(log.contains("17"),"{log}");}
        if case=="source-change" {assert!(log.contains("source changed after embedding preparation"),"{log}");}
        assert_eq!(fs::read(root.join("analysis/report.qmd")).unwrap(),source);
        assert_eq!(fs::read(root.join("_quarto.yml")).unwrap(),project);
        assert_eq!(fs::read(root.join("published/previous.html")).unwrap(),b"old final report");
        assert_eq!(root.join("cache/prepared-header-observed").exists(),case!="source-change");
    }
}
