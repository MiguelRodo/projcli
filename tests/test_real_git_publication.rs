//! Real local Git repositories: no hosting account, remote service or credentials.
use std::{fs, path::{Path, PathBuf}, process::{Command, Output}};
use proj::git::{GitProvider, SystemGit};

const TARGET: &str = "refs/heads/reviewed-fixture";

struct Fixture {
    _temporary: tempfile::TempDir,
    root: PathBuf,
    upstream: PathBuf,
    global: PathBuf,
}

impl Fixture {
    fn new(push: bool) -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("project with spaces");
        let upstream = temporary.path().join("bare upstream.git");
        let global = temporary.path().join("empty-git-config");
        fs::create_dir(&root).unwrap();
        fs::write(&global, "").unwrap();
        let fixture = Self { _temporary: temporary, root, upstream, global };
        fixture.git_at(&fixture.root, &["init", "-b", "analysis-fixture"]);
        fixture.git_at(&fixture.root, &["init", "--bare", fixture.upstream.to_str().unwrap()]);
        for (key, value) in [("user.name", "Fixture"), ("user.email", "fixture@example.invalid"),
                             ("commit.gpgsign", "false"), ("core.autocrlf", "false")] {
            fixture.git(&["config", key, value]);
        }
        fs::write(fixture.root.join("_proj.yml"), format!(r#"directories:
  cache: {{path: cache}}
  output: {{path: released, ignore: []}}
build:
  scripts: [analysis.py]
  git: {{commit: true, push: {push}}}
  restrictions:
    not_behind: false
"#)).unwrap();
        fs::write(fixture.root.join("VERSION"), b"v1.2.3-4\n").unwrap();
        fs::write(fixture.root.join(".gitignore"), b"cache/\n").unwrap();
        fs::create_dir(fixture.root.join("released")).unwrap();
        fs::write(fixture.root.join("released/previous.csv"), b"previous\n21\n").unwrap();
        fs::write(fixture.root.join("analysis.py"), r#"import json, os, subprocess
from pathlib import Path
result=json.loads(subprocess.check_output([os.environ['PROJCLI_TEST_BINARY'],
    '--machine','path','get','--label','output','results.csv'],text=True))
target=Path(result['result']['path'])
target.parent.mkdir(parents=True,exist_ok=True)
target.write_bytes(b'result\n84\n')
if os.environ['PROJCLI_TEST_FAIL_ANALYSIS']=='1':
    raise RuntimeError('DELIBERATE-ANALYSIS-FAILURE')
"#).unwrap();
        fixture.git(&["add", "-A"]);
        fixture.git(&["commit", "-m", "initial fixture"]);
        fixture.git(&["remote", "add", "fixture-upstream", fixture.upstream.to_str().unwrap()]);
        // Seed main as a sentinel, then track a differently named non-main ref.
        fixture.git(&["push", "fixture-upstream", "HEAD:refs/heads/main"]);
        fixture.git(&["push", "--set-upstream", "fixture-upstream", &format!("HEAD:{TARGET}")]);
        fixture
    }

    fn command(&self, program: &str) -> Command {
        let mut command = Command::new(program);
        command.current_dir(&self.root)
            .env("GIT_CONFIG_NOSYSTEM", "1").env("GIT_CONFIG_SYSTEM", &self.global)
            .env("GIT_CONFIG_GLOBAL", &self.global).env("GIT_CONFIG_COUNT", "0")
            .env("GIT_TERMINAL_PROMPT", "0").env("GIT_ALLOW_PROTOCOL", "file");
        for key in ["GIT_CONFIG_PARAMETERS", "GITHUB_PAT", "GH_TOKEN", "GITHUB_TOKEN",
                    "GIT_ASKPASS", "SSH_ASKPASS", "SSH_AUTH_SOCK", "GIT_SSH", "GIT_SSH_COMMAND",
                    proj::PROJECT_ROOT_ENV, proj::RUN_CONTEXT_ENV, "PROJR_PROFILE",
                    "QUARTO_PROFILE", "PROJR_CLEAR_OUTPUT"] {
            command.env_remove(key);
        }
        command
    }

    fn git_at(&self, directory: &Path, args: &[&str]) -> Vec<u8> {
        let output = self.command("git").current_dir(directory).args(args).output().unwrap();
        assert!(output.status.success(), "{args:?}: {}", String::from_utf8_lossy(&output.stderr));
        output.stdout
    }

    fn git(&self, args: &[&str]) -> Vec<u8> { self.git_at(&self.root, args) }

    fn text(&self, args: &[&str]) -> String {
        String::from_utf8(self.git(args)).unwrap().trim().to_owned()
    }

    fn assert_upstream_refs(&self, main: &str, target: &str) {
        let output = self.git_at(&self.upstream,
            &["for-each-ref", "--format=%(refname) %(objectname)", "refs/heads"]);
        let refs = String::from_utf8(output).unwrap();
        assert_eq!(refs.lines().collect::<Vec<_>>(),
                   [format!("{TARGET} {target}"), format!("refs/heads/main {main}")]);
    }

    fn build(&self, fail_analysis: bool) -> Output {
        let output = self.command(env!("CARGO_BIN_EXE_proj"))
            .args(["--machine", "--non-interactive", "--project"]).arg(&self.root)
            .args(["build", "--description", "real local Git witness"])
            .env("PROJCLI_TEST_BINARY", env!("CARGO_BIN_EXE_proj"))
            .env("PROJCLI_TEST_FAIL_ANALYSIS", if fail_analysis { "1" } else { "0" })
            .output().unwrap();
        let envelope: serde_json::Value = serde_json::from_slice(&output.stdout)
            .unwrap_or_else(|error| panic!("{error}: {}", String::from_utf8_lossy(&output.stdout)));
        assert_eq!(envelope["protocol_version"], 1);
        assert_eq!(envelope["ok"], output.status.success());
        if !output.status.success() { assert_eq!(envelope["error"]["code"], "command_failed"); }
        output
    }

    fn assert_local_release(&self) {
        let expected = b"result\n84\n";
        assert_eq!(fs::read(self.root.join("released/results.csv")).unwrap(), expected);
        assert_eq!(fs::read(self.root.join("cache/projr/v1.2.4/output/results.csv")).unwrap(), expected);
        assert_eq!(self.git(&["show", "HEAD:released/results.csv"]), expected);
        assert_eq!(self.git(&["show", "HEAD:VERSION"]), b"Version: v1.2.4");
        assert_eq!(fs::read(self.root.join("released/previous.csv")).unwrap(), b"previous\n21\n");
    }
}

#[test]
fn actual_push_uses_only_the_configured_non_main_upstream_and_remains_optional() {
    for push in [false, true] {
        let fixture = Fixture::new(push);
        let initial = fixture.text(&["rev-parse", "HEAD"]);
        let result = fixture.build(false);
        assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
        let released = fixture.text(&["rev-parse", "HEAD"]);
        assert_ne!(released, initial);
        assert_eq!(fixture.text(&["rev-list", "--count", &format!("{initial}..HEAD")]), "1");
        assert_eq!(fixture.text(&["symbolic-ref", "--short", "HEAD"]), "analysis-fixture");
        fixture.assert_upstream_refs(&initial, if push { &released } else { &initial });
        if push {
            assert_eq!(fixture.git_at(&fixture.upstream, &["show", &format!("{TARGET}:released/results.csv")]), b"result\n84\n");
        }
        fixture.assert_local_release();
        assert_eq!(fs::read(fixture.root.join("VERSION")).unwrap(), b"Version: v1.2.4.1");
        let status = String::from_utf8(fixture.git(&["status", "--porcelain"])).unwrap();
        assert_eq!(status.lines().collect::<Vec<_>>(), [" M VERSION"]);
    }
}

#[test]
fn actual_noop_commit_succeeds_without_creating_a_commit_but_real_changes_commit() {
    let fixture = Fixture::new(false);
    let root = camino::Utf8Path::from_path(&fixture.root).unwrap().to_path_buf();
    let provider = SystemGit::new(root);
    let initial = fixture.text(&["rev-parse", "HEAD"]);
    provider.commit_all("no changes").unwrap();
    assert_eq!(fixture.text(&["rev-parse", "HEAD"]), initial);
    fs::write(fixture.root.join("tracked-result.txt"), b"actual new bytes\n").unwrap();
    provider.commit_all("real change").unwrap();
    let changed = fixture.text(&["rev-parse", "HEAD"]);
    assert_ne!(changed, initial);
    assert_eq!(fixture.git(&["show", "HEAD:tracked-result.txt"]), b"actual new bytes\n");
    provider.commit_all("still no changes").unwrap();
    assert_eq!(fixture.text(&["rev-parse", "HEAD"]), changed);
    assert_eq!(fixture.text(&["rev-list", "--count", &format!("{initial}..HEAD")]), "1");
    fixture.assert_upstream_refs(&initial, &initial);
    assert_eq!(fixture.text(&["status", "--porcelain"]), "");
}

#[test]
fn rejected_real_push_preserves_successful_local_release_and_skips_next_dev_bump() {
    let fixture = Fixture::new(true);
    let initial = fixture.text(&["rev-parse", "HEAD"]);
    let tree = fixture.text(&["rev-parse", "HEAD^{tree}"]);
    // Advance only the bare branch with a sibling of the forthcoming build commit.
    let divergent = fixture.text(&["commit-tree", &tree, "-p", &initial, "-m", "remote divergence"]);
    fixture.git(&["push", "fixture-upstream", &format!("{divergent}:{TARGET}")]);
    let result = fixture.build(false);
    assert!(!result.status.success());
    let diagnostic = String::from_utf8_lossy(&result.stderr);
    assert!(diagnostic.contains("Local build succeeded"), "{diagnostic}");
    assert!(diagnostic.contains("local release preserved"), "{diagnostic}");
    let released = fixture.text(&["rev-parse", "HEAD"]);
    assert_ne!(released, initial);
    assert_ne!(released, divergent);
    assert_eq!(fixture.text(&["rev-list", "--count", &format!("{initial}..HEAD")]), "1");
    fixture.assert_local_release();
    assert_eq!(fs::read(fixture.root.join("VERSION")).unwrap(), b"Version: v1.2.4");
    fixture.assert_upstream_refs(&initial, &divergent);
    assert_eq!(fixture.text(&["status", "--porcelain"]), "");
}

#[test]
fn analysis_failure_is_not_publication_failure_and_changes_neither_git_reference() {
    let fixture = Fixture::new(true);
    let initial = fixture.text(&["rev-parse", "HEAD"]);
    let result = fixture.build(true);
    assert!(!result.status.success());
    let diagnostic = String::from_utf8_lossy(&result.stderr);
    assert!(diagnostic.contains("DELIBERATE-ANALYSIS-FAILURE"), "{diagnostic}");
    assert!(!diagnostic.contains("Local build succeeded"), "{diagnostic}");
    assert_eq!(fixture.text(&["rev-parse", "HEAD"]), initial);
    fixture.assert_upstream_refs(&initial, &initial);
    assert_eq!(fs::read(fixture.root.join("VERSION")).unwrap(), b"v1.2.3-4\n");
    assert_eq!(fs::read(fixture.root.join("released/previous.csv")).unwrap(), b"previous\n21\n");
    assert!(!fixture.root.join("released/results.csv").exists());
    assert_eq!(fs::read(fixture.root.join("cache/projr/v1.2.4/output/results.csv")).unwrap(), b"result\n84\n");
    assert_eq!(fixture.text(&["status", "--porcelain"]), "");
}
