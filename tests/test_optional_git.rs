use std::{fs, process::Command};

#[test]
fn omitted_or_disabled_git_never_requires_git_gh_or_tokens_even_inside_a_repository() {
    for git in ["", "  git: false\n", "  git: {}\n"] {
        let project = tempfile::tempdir().unwrap();
        let root = project.path();
        fs::create_dir(root.join(".git")).unwrap();
        fs::write(root.join("_proj.yml"), format!("build:\n  scripts: []\n{git}")).unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_proj"))
            .args(["build", "--patch", "--non-interactive", "--project"]).arg(root)
            .env("PATH", "").env_remove("GITHUB_PAT").env_remove("GH_TOKEN").env_remove("GITHUB_TOKEN")
            .env_remove(proj::PROJECT_ROOT_ENV).env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE")
            .output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        assert!(root.join("VERSION").is_file());
    }
}

#[cfg(unix)]
mod mocked {
    use super::*;
    use std::{path::Path, os::unix::fs::PermissionsExt};

    fn fixture(root: &Path) -> std::ffi::OsString {
        fs::create_dir(root.join("bin")).unwrap();
        fs::create_dir(root.join(".git")).unwrap();
        let fake = r#"#!/usr/bin/env python3
import os, sys, json
from pathlib import Path
root = Path(os.environ['PROJ_PROJECT_ROOT'])
original = sys.argv[1:]
with (root / 'git-calls.jsonl').open('a') as log:
 log.write(json.dumps({'args': original, 'private_token': bool(os.environ.get('PROJCLI_GIT_TOKEN'))}) + '\n')
args = original[:]
while args[:1] == ['-c']: args = args[2:]
action = args[0] if args else ''
if action == 'config':
 key = args[-1]
 if key.endswith('.pushRemote') or key == 'remote.pushDefault' or key.endswith('.push') or key.endswith('.pushurl'):
  if os.environ.get('TEST_CUSTOM_PUSH') == '1' or (key.endswith('.pushurl') and os.environ.get('TEST_PUSHURL') == '1'): print('custom-route')
  else: raise SystemExit(1)
 elif key.endswith('.remote'): print('upstream')
 elif key.endswith('.merge'):
  print('refs/heads/release-target')
  if os.environ.get('TEST_MULTIPLE_MERGE') == '1': print('refs/heads/other')
 elif key.endswith('.mirror'): print('true' if os.environ.get('TEST_MIRROR') == '1' else 'false')
 else: print('fixture')
elif action == 'symbolic-ref': print('feature/analysis')
elif action == 'remote':
 if os.environ.get('TEST_MISSING_REMOTE') == '1': raise SystemExit(2)
 if '--push' in args and os.environ.get('TEST_PUSH_INSTEADOF') == '1': print('https://other.example/redirected.git')
 else: print(os.environ.get('TEST_REMOTE_URL', 'https://github.com/example/repo.git'))
 if os.environ.get('TEST_MULTIPLE_URLS') == '1': print('https://other.example/second.git')
elif action == 'status':
 if os.environ.get('TEST_DIRTY') == '1': print(' M unrelated.txt')
 elif os.environ.get('TEST_HIDDEN_UNTRACKED') == '1' and '--untracked-files=all' in args: print('?? private.txt')
elif action == 'diff': raise SystemExit(1 if os.environ.get('TEST_STAGED') == '1' else 0)
elif action == 'rev-list': print('0')
elif action in ['push', 'fetch'] and os.environ.get('TEST_NETWORK_FAIL') == '1':
 print(os.environ.get('GITHUB_PAT', 'fixture'), file=sys.stderr)
 raise SystemExit(23)
"#;
        for (name, content) in [("git", fake), ("gh", "#!/bin/sh\necho called > gh-called\nexit 99\n")] {
            let path = root.join("bin").join(name);
            fs::write(&path, content).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let mut paths = vec![root.join("bin")];
        paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()));
        std::env::join_paths(paths).unwrap()
    }

    fn calls(root: &Path) -> Vec<serde_json::Value> {
        fs::read_to_string(root.join("git-calls.jsonl")).unwrap().lines()
            .map(|line| serde_json::from_str(line).unwrap()).collect()
    }
    fn action(call: &serde_json::Value) -> &str {
        let args = call["args"].as_array().unwrap();
        let mut index = 0;
        while args[index] == "-c" { index += 2; }
        args[index].as_str().unwrap()
    }
    fn command(root: &Path, path: &std::ffi::OsStr) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_proj"));
        command.args(["build", "--patch", "--non-interactive", "--project"]).arg(root).current_dir(root)
            .env("PATH", path).env_remove(proj::PROJECT_ROOT_ENV).env_remove("PROJR_PROFILE").env_remove("QUARTO_PROFILE")
            .env_remove("GITHUB_PAT").env_remove("GH_TOKEN").env_remove("GITHUB_TOKEN");
        command
    }

    #[test]
    fn commit_is_explicit_noop_is_success_and_network_is_not_implicit() {
        for staged in ["0", "1"] {
            let project = tempfile::tempdir().unwrap(); let root = project.path(); let path = fixture(root);
            fs::write(root.join("_proj.yml"), "build:\n  scripts: []\n  git: true\n").unwrap();
            let output = command(root, &path).env("TEST_STAGED", staged).output().unwrap();
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
            let calls = calls(root);
            assert_eq!(calls.iter().filter(|call| action(call) == "commit").count(), usize::from(staged == "1"));
            assert!(!calls.iter().any(|call| ["fetch", "push", "credential"].contains(&action(call))));
            assert!(!root.join("gh-called").exists());
        }
    }

    #[test]
    fn fetch_is_only_requested_and_uses_the_actual_tracking_remote() {
        let project = tempfile::tempdir().unwrap(); let root = project.path(); let path = fixture(root);
        fs::write(root.join("_proj.yml"), "build:\n  scripts: []\n  git: false\n  restrictions:\n    not_behind: true\n").unwrap();
        let output = command(root, &path).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        let calls = calls(root);
        let fetch = calls.iter().find(|call| action(call) == "fetch").unwrap();
        assert_eq!(fetch["args"], serde_json::json!(["fetch", "--recurse-submodules=no", "--", "upstream"]));
        assert!(!calls.iter().any(|call| ["add", "commit", "push", "credential"].contains(&action(call))));
        assert!(!root.join("gh-called").exists());
    }

    #[test]
    fn push_uses_current_upstream_native_auth_by_default_and_scoped_override_only_when_enabled() {
        for (override_enabled, url, expected_token) in [(false, "https://github.com/example/repo.git", false), (true, "https://github.com/example/repo.git", true), (true, "https://other.example/repo.git", false)] {
            let project = tempfile::tempdir().unwrap(); let root = project.path(); let path = fixture(root);
            fs::write(root.join("_proj.yml"), format!("config:\n  git:\n    use_proj_cred_helper: {override_enabled}\nbuild:\n  scripts: []\n  git:\n    commit: true\n    push: true\n")).unwrap();
            let token = "fixture-token-sentinel";
            let output = command(root, &path).env("GITHUB_PAT", token).env("TEST_REMOTE_URL", url).env("TEST_STAGED", "1").output().unwrap();
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
            let calls = calls(root);
            let push = calls.iter().find(|call| action(call) == "push").unwrap();
            let args = push["args"].as_array().unwrap();
            assert_eq!(&args[args.len()-3..], &serde_json::json!(["--", "upstream", "HEAD:refs/heads/release-target"]).as_array().unwrap()[..]);
            assert!(args.iter().any(|value| value == "--no-follow-tags"));
            assert!(args.iter().any(|value| value == "--recurse-submodules=no"));
            assert_eq!(push["private_token"], expected_token);
            assert!(!fs::read_to_string(root.join("git-calls.jsonl")).unwrap().contains(token));
            assert!(!calls.iter().any(|call| action(call) == "fetch"));
            assert_eq!(calls.iter().filter(|call| action(call) == "commit").count(), 1);
            assert!(!root.join("gh-called").exists());
        }
    }

    #[test]
    fn preexisting_dirty_work_fails_before_build_mutations_and_publication_errors_keep_local_success() {
        for dirty in [false, true] {
            let project = tempfile::tempdir().unwrap(); let root = project.path(); let path = fixture(root);
            fs::write(root.join("VERSION"), "v1.2.3\n").unwrap();
            fs::write(root.join("unrelated.txt"), "keep").unwrap();
            fs::write(root.join("_proj.yml"), "build:\n  scripts: []\n  git:\n    commit: true\n    push: true\n").unwrap();
            let output = command(root, &path).env("TEST_DIRTY", if dirty { "1" } else { "0" })
                .env("TEST_NETWORK_FAIL", "1").env("GITHUB_PAT", "fixture-token-sentinel").output().unwrap();
            assert!(!output.status.success());
            let diagnostic = String::from_utf8_lossy(&output.stderr);
            assert!(!diagnostic.contains("fixture-token-sentinel"));
            if dirty { assert_eq!(fs::read_to_string(root.join("VERSION")).unwrap(), "v1.2.3\n"); assert!(!root.join(".gitignore").exists()); }
            else { assert!(diagnostic.contains("Local build succeeded")); }
            assert_eq!(fs::read_to_string(root.join("unrelated.txt")).unwrap(), "keep");
        }
    }

    #[test]
    fn unsupported_custom_push_routing_fails_before_authoritative_writes() {
        let project = tempfile::tempdir().unwrap(); let root = project.path(); let path = fixture(root);
        fs::write(root.join("VERSION"), "v1.2.3\n").unwrap();
        fs::write(root.join("_proj.yml"), "build:\n  scripts: []\n  git:\n    commit: true\n    push: true\n").unwrap();
        let output = command(root, &path).env("TEST_CUSTOM_PUSH", "1").output().unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("Custom Git push routing"));
        assert_eq!(fs::read_to_string(root.join("VERSION")).unwrap(), "v1.2.3\n");
        assert!(!root.join(".gitignore").exists());
        assert!(!calls(root).iter().any(|call| ["add", "commit", "push", "fetch"].contains(&action(call))));
    }


    #[test]
    fn hidden_untracked_content_and_extra_push_destinations_fail_before_mutation() {
        for setting in ["TEST_HIDDEN_UNTRACKED", "TEST_PUSHURL", "TEST_MULTIPLE_URLS", "TEST_MIRROR", "TEST_PUSH_INSTEADOF", "TEST_MISSING_REMOTE", "TEST_MULTIPLE_MERGE"] {
            let project = tempfile::tempdir().unwrap(); let root = project.path(); let path = fixture(root);
            fs::write(root.join("VERSION"), "v1.2.3\n").unwrap();
            fs::write(root.join("private.txt"), "untracked private fixture").unwrap();
            fs::write(root.join("_proj.yml"), "build:\n  scripts: []\n  git:\n    commit: true\n    push: true\n").unwrap();
            let output = command(root, &path).env(setting, "1").output().unwrap();
            assert!(!output.status.success(), "{setting}");
            assert_eq!(fs::read_to_string(root.join("VERSION")).unwrap(), "v1.2.3\n");
            assert_eq!(fs::read_to_string(root.join("private.txt")).unwrap(), "untracked private fixture");
            assert!(!root.join(".gitignore").exists());
            assert!(!calls(root).iter().any(|call| ["add", "commit", "push", "fetch"].contains(&action(call))));
        }
    }

}
