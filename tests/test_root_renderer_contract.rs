use std::{fs, path::Path, process::{Command, Output}};

fn config(root: &Path, scripts: &str, extra: &str) {
    fs::write(root.join("VERSION"), "v1.2.3\n").unwrap();
    fs::write(root.join("_proj.yml"), format!("directories:\n  cache:\n    path: cache\n  output:\n    path: output\n  docs:\n    path: published\nbuild:\n  scripts: [{scripts}]\n{extra}")).unwrap();
}

fn invoke(root: &Path, production: bool) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_proj"));
    command.arg(if production { "build" } else { "build-dev" });
    if production { command.arg("--patch"); }
    command.args(["--non-interactive", "--project"]).arg(root)
        .env_remove(proj::PROJECT_ROOT_ENV).env_remove("PROJR_PROFILE")
        .env_remove("QUARTO_PROFILE").env_remove("PROJR_CLEAR_OUTPUT");
    let mut paths = vec![root.join("bin")];
    paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()));
    command.env("PATH", std::env::join_paths(paths).unwrap());
    command.output().unwrap()
}

#[test]
fn nested_scripts_and_hooks_execute_in_root_in_selected_order() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap().join("project with spaces é");
    fs::create_dir_all(root.join("nested")).unwrap();
    config(&root, "nested/first.py, nested/second.py", "  hooks:\n    pre: [nested/pre.py]\n    post: [nested/post.py]\n");
    for name in ["pre", "first", "second", "post"] {
        fs::write(root.join(format!("nested/{name}.py")), format!("from pathlib import Path\nimport os\nassert Path.cwd().samefile(Path(os.environ['PROJ_PROJECT_ROOT']))\nwith open('order.txt', 'a') as f: f.write('{name}\\n')\n")).unwrap();
    }
    let output = invoke(&root, false);
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(fs::read_to_string(root.join("order.txt")).unwrap().lines().collect::<Vec<_>>(), vec!["pre", "first", "second", "post"]);
    assert!(!root.join("nested/order.txt").exists());
}

#[test]
fn invalid_control_values_fail_before_analysis_or_managed_writes() {
    for extra in ["  clear_output: typo\n", ""] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        config(&root, "analysis.py", extra);
        if extra.is_empty() {
            let path = root.join("_proj.yml");
            let original = fs::read_to_string(&path).unwrap();
            fs::write(path, original.replace("path: output", "path: output\n    ignore: [typo]")).unwrap();
        }
        fs::write(root.join("analysis.py"), "open('analysis-called', 'w').write('bad')\n").unwrap();
        let before = fs::read(root.join("_proj.yml")).unwrap();
        let output = invoke(&root, true);
        assert!(!output.status.success());
        assert!(!root.join("analysis-called").exists());
        assert!(!root.join(".gitignore").exists());
        assert!(!root.join("cache").exists());
        assert_eq!(fs::read(root.join("_proj.yml")).unwrap(), before);
        assert_eq!(fs::read_to_string(root.join("VERSION")).unwrap(), "v1.2.3\n");
    }
}

#[cfg(unix)]
mod renderer {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn fake(root: &Path) {
        fs::create_dir(root.join("bin")).unwrap();
        let script = r#"#!/usr/bin/env python3
import os, sys, json
from pathlib import Path
root = Path(os.environ['PROJ_PROJECT_ROOT'])
if '--version' in sys.argv:
    print('fixture 1.0')
    raise SystemExit(0)
if sys.argv[1:2] == ['-e'] and 'rmarkdown::render(' not in sys.argv[2]:
    raise SystemExit(0)
(root / 'renderer-argv.json').write_text(json.dumps(sys.argv[1:]))
assert Path.cwd() == root
if sys.argv[1:2] == ['render']:
    assert sys.argv[sys.argv.index('--execute-dir') + 1] == str(root)
else:
    assert len(sys.argv) == 5, sys.argv
    assert sys.argv[4] == str(root)
name = (root / 'expected-output').read_text()
target = root / name
target.parent.mkdir(parents=True, exist_ok=True)
target.write_text('fresh artifact')
"#;
        for name in ["quarto", "Rscript"] {
            let path = root.join("bin").join(name);
            fs::write(&path, script).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    #[test]
    fn modeled_individual_formats_produce_real_expected_artifacts() {
        for (extension, key, format, suffix) in [
            ("qmd", "format", "html", "html"), ("qmd", "format", "pdf", "pdf"),
            ("qmd", "format", "docx", "docx"), ("Rmd", "output", "html_document", "html"),
            ("Rmd", "output", "pdf_document", "pdf"), ("Rmd", "output", "word_document", "docx")
        ] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().canonicalize().unwrap();
            fs::create_dir(root.join("nested")).unwrap();
            let input = format!("nested/report's é.{extension}");
            config(&root, &format!("\"{input}\""), "");
            fs::write(root.join(&input), format!("---\n{key}: {format}\n---\nReport\n")).unwrap();
            fs::write(root.join("expected-output"), format!("nested/report's é.{suffix}")).unwrap();
            fake(&root);
            let output = invoke(&root, true);
            assert!(output.status.success(), "{format}: {} {}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
            assert_eq!(fs::read_to_string(root.join(format!("published/report's é.{suffix}"))).unwrap(), "fresh artifact");
            let args: Vec<String> = serde_json::from_slice(&fs::read(root.join("renderer-argv.json")).unwrap()).unwrap();
            if extension == "Rmd" {
                assert!(!args[1].contains("report's"));
                assert_eq!(args[2], root.join(&input).to_str().unwrap());
                assert_eq!(args[3], root.to_str().unwrap());
            }
        }
    }

    #[test]
    fn effective_quarto_root_must_match_explicit_proj_root_for_all_quarto_documents() {
        for case in ["ancestor", "nested-rmd-yml", "nested-rmd-yaml"] {
            let temp = tempfile::tempdir().unwrap();
            let outer = temp.path().canonicalize().unwrap();
            let root = outer.join("inner");
            fs::create_dir_all(root.join("nested")).unwrap();
            let extension = if case == "ancestor" { "qmd" } else { "Rmd" };
            config(&root, &format!("nested/report.{extension}"), "");
            fs::write(root.join(format!("nested/report.{extension}")), "---\nformat: html\n---\n").unwrap();
            if case == "ancestor" {
                fs::write(outer.join("_quarto.yml"), "project: {}\n").unwrap();
            } else {
                fs::write(root.join("_quarto.yml"), "project: {}\n").unwrap();
                fs::write(root.join(if case.ends_with("yaml") { "nested/_quarto.yaml" } else { "nested/_quarto.yml" }), "project: {}\n").unwrap();
            }
            fake(&root);
            let output = invoke(&root, true);
            assert!(!output.status.success(), "{case}");
            assert!(!root.join("renderer-argv.json").exists());
            assert!(!root.join(".gitignore").exists());
            assert!(!root.join("cache").exists());
            assert_eq!(fs::read_to_string(root.join("VERSION")).unwrap(), "v1.2.3\n");
        }
    }

    #[test]
    fn unsupported_layouts_and_nested_quarto_roots_fail_before_writes() {
        for case in ["nested-yml", "nested-yaml", "unknown", "multi", "malformed", "inherited", "rmd-custom", "qmd-ext", "qmd-format-ext", "rmd-lib", "rmd-format-lib"] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().canonicalize().unwrap();
            fs::create_dir(root.join("nested")).unwrap();
            let extension = if case.starts_with("rmd-") { "Rmd" } else { "qmd" };
            config(&root, &format!("nested/report.{extension}"), "");
            let body = match case {
                "qmd-ext" => "---\nformat: html\noutput-ext: txt\n---\n",
                "qmd-format-ext" => "---\nformat: {html: {output-ext: txt}}\n---\n",
                "rmd-lib" => "---\noutput: html_document\nlib_dir: external\n---\n",
                "rmd-format-lib" => "---\noutput: {html_document: {lib_dir: external}}\n---\n",
                "unknown" => "---\nformat: mystery\n---\n",
                "multi" => "---\nformat: {html: default, pdf: default}\n---\n",
                "malformed" => "---\nformat: [\n---\n",
                "rmd-custom" => "---\noutput: html_document\noutput_file: custom.html\n---\n",
                _ => "---\nformat: html\n---\n",
            };
            fs::write(root.join(format!("nested/report.{extension}")), body).unwrap();
            match case {
                "nested-yml" => fs::write(root.join("nested/_quarto.yml"), "project: {}\n").unwrap(),
                "nested-yaml" => fs::write(root.join("nested/_quarto.yaml"), "project: {}\n").unwrap(),
                "inherited" => fs::write(root.join("nested/_metadata.yml"), "format: pdf\n").unwrap(),
                _ => {}
            }
            fake(&root);
            let before = fs::read(root.join("_proj.yml")).unwrap();
            let output = invoke(&root, true);
            assert!(!output.status.success(), "{case}");
            assert!(!root.join("renderer-argv.json").exists(), "{case}");
            assert!(!root.join(".gitignore").exists(), "{case}");
            assert!(!root.join("cache").exists(), "{case}");
            assert_eq!(fs::read(root.join("_proj.yml")).unwrap(), before);
            assert_eq!(fs::read_to_string(root.join("VERSION")).unwrap(), "v1.2.3\n");
        }
    }
}
