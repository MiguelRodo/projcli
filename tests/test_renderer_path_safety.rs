#![cfg(unix)]
use std::{fs, path::Path, process::{Command, Output}};
use std::os::unix::fs::{symlink, PermissionsExt};

fn fixture(root: &Path, docs: &str, extension: &str) {
    fs::write(root.join("VERSION"), "v1.2.3\n").unwrap();
    fs::write(root.join(format!("report.{extension}")), "---\nformat: html\n---\nReport\n").unwrap();
    fs::write(root.join("_proj.yml"), format!("directories:\n  cache:\n    path: _tmp\n  raw:\n    path: _raw_data\n  output:\n    path: _output\n  docs:\n    path: '{}'\nconfig:\n  git:\n    use_proj_cred_helper: false\nbuild:\n  scripts: [report.{extension}]\n  git: false\n  restrictions:\n    not_behind: false\n", docs.replace('\'', "''"))).unwrap();
    fs::create_dir(root.join("bin")).unwrap();
    let fake = r#"#!/usr/bin/env python3
import os, sys
from pathlib import Path
if '--version' in sys.argv:
    print('fake renderer 1.0')
    raise SystemExit(0)
if sys.argv[1:2] == ['render'] or any('rmarkdown::render(' in arg for arg in sys.argv):
    root = Path(os.environ['PROJ_PROJECT_ROOT'])
    (root / 'renderer-called').write_text('yes')
    (root / 'report.html').write_text('new rendered bytes')
"#;
    for name in ["quarto", "Rscript"] {
        let path = root.join("bin").join(name);
        fs::write(&path, fake).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }
}

fn build(root: &Path) -> Output {
    let mut paths = vec![root.join("bin")];
    paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()));
    Command::new(env!("CARGO_BIN_EXE_proj"))
        .args(["build", "--patch", "--non-interactive", "--project"]).arg(root).current_dir(root)
        .env("PATH", std::env::join_paths(paths).unwrap())
        .env_remove(proj::PROJECT_ROOT_ENV).env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE")
        .env_remove("PROJR_CLEAR_OUTPUT").output().unwrap()
}

#[test]
fn cli_promotes_to_configured_docs_and_never_uses_the_linked_default_docs_path() {
    for extension in ["qmd", "Rmd"] {
        let project = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        let root = project.path().canonicalize().unwrap();
        fs::write(external.path().join("report.html"), "external sentinel").unwrap();
        symlink(external.path(), root.join("docs")).unwrap();
        fixture(&root, "published", extension);
        let output = build(&root);
        assert!(output.status.success(), "{} {}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        assert_eq!(fs::read_to_string(root.join("published/report.html")).unwrap(), "new rendered bytes");
        assert_eq!(fs::read_to_string(external.path().join("report.html")).unwrap(), "external sentinel");
        assert!(root.join("renderer-called").exists());
    }
}

#[test]
fn cli_actually_promotes_to_a_disjoint_external_docs_directory() {
    for extension in ["qmd", "Rmd"] {
        let project = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        let root = project.path().canonicalize().unwrap();
        let docs = external.path().canonicalize().unwrap().join("published");
        fixture(&root, docs.to_str().unwrap(), extension);
        let output = build(&root);
        assert!(output.status.success(), "{} {}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        assert_eq!(fs::read_to_string(docs.join("report.html")).unwrap(), "new rendered bytes");
        assert!(!root.join("docs").exists());
    }
}

#[test]
fn concrete_render_output_and_promotion_links_fail_before_renderer_or_state_writes() {
    for extension in ["qmd", "Rmd"] {
        for link_at in ["report.html", "published/report.html"] {
            let project = tempfile::tempdir().unwrap();
            let external = tempfile::tempdir().unwrap();
            let root = project.path().canonicalize().unwrap();
            let sentinel = external.path().join("report.html");
            fs::write(&sentinel, "external sentinel").unwrap();
            fs::create_dir(root.join("published")).unwrap();
            symlink(&sentinel, root.join(link_at)).unwrap();
            fixture(&root, "published", extension);
            let output = build(&root);
            assert!(!output.status.success());
            assert!(!root.join("renderer-called").exists());
            assert_eq!(fs::read_to_string(&sentinel).unwrap(), "external sentinel");
            assert_eq!(fs::read_to_string(root.join("VERSION")).unwrap(), "v1.2.3\n");
            assert!(!root.join(".gitignore").exists());
        }
    }
}


#[test]
fn invalid_output_file_relationships_fail_before_fake_renderer_execution() {
    for extension in ["qmd", "Rmd"] {
        for name in ["../outside.html", "/absolute.html", "VERSION"] {
            let project = tempfile::tempdir().unwrap();
            let root = project.path().canonicalize().unwrap();
            fixture(&root, "published", extension);
            fs::write(root.join(format!("report.{extension}")), format!("---\nformat: html\noutput-file: '{name}'\n---\n")).unwrap();
            let output = build(&root);
            assert!(!output.status.success());
            assert!(!root.join("renderer-called").exists());
            assert_eq!(fs::read_to_string(root.join("VERSION")).unwrap(), "v1.2.3\n");
            assert!(!root.join(".gitignore").exists());
            assert!(!root.join("published").exists());
        }
    }
}


#[test]
fn renderer_outputs_cannot_overwrite_other_selected_scripts_or_hooks() {
    for extension in ["qmd", "Rmd"] {
        for selection in ["script", "hook", "asset-hook"] {
            let project = tempfile::tempdir().unwrap();
            let root = project.path().canonicalize().unwrap();
            fixture(&root, "published", extension);
            let config_path = root.join("_proj.yml");
            let mut config = fs::read_to_string(&config_path).unwrap();
            if selection == "script" {
                config = config.replace(&format!("scripts: [report.{extension}]"), &format!("scripts: [report.{extension}, post.py]"));
            } else if selection == "hook" { config.push_str("  hooks:\n    post: [post.py]\n"); }
            else { config.push_str("  hooks:\n    post: [report_files/post.py]\n"); }
            fs::write(&config_path, config).unwrap();
            let protected = if selection == "asset-hook" { root.join("report_files/post.py") } else { root.join("post.py") };
            fs::create_dir_all(protected.parent().unwrap()).unwrap();
            fs::write(&protected, "raise RuntimeError('must not run')\n").unwrap();
            let output_name = if selection == "asset-hook" { "report.html" } else { "post.py" };
            fs::write(root.join(format!("report.{extension}")), format!("---\nformat: html\noutput-file: {output_name}\n---\n")).unwrap();
            let output = build(&root);
            assert!(!output.status.success());
            assert!(String::from_utf8_lossy(&output.stderr).contains("overwrites protected input"), "{}", String::from_utf8_lossy(&output.stderr));
            assert_eq!(fs::read_to_string(&protected).unwrap(), "raise RuntimeError('must not run')\n");
            assert!(!root.join("renderer-called").exists());
            assert_eq!(fs::read_to_string(root.join("VERSION")).unwrap(), "v1.2.3\n");
            assert!(!root.join(".gitignore").exists());
        }
    }
}

#[test]
fn renderer_outputs_cannot_replace_authoritative_configuration_or_state() {
    for extension in ["qmd", "Rmd"] {
        for name in ["DESCRIPTION", "_environment.local", "_projr-extra.yml", "pyproject.toml"] {
            let project = tempfile::tempdir().unwrap();
            let root = project.path().canonicalize().unwrap();
            fixture(&root, "published", extension);
            fs::write(root.join(name), "# original configuration\n").unwrap();
            fs::write(root.join(format!("report.{extension}")), format!("---\nformat: html\noutput-file: {name}\n---\n")).unwrap();
            let output = build(&root);
            assert!(!output.status.success());
            assert!(!root.join("renderer-called").exists());
            assert_eq!(fs::read_to_string(root.join(name)).unwrap(), "# original configuration\n");
            assert_eq!(fs::read_to_string(root.join("VERSION")).unwrap(), "v1.2.3\n");
            assert!(!root.join(".gitignore").exists());
        }
    }
}
