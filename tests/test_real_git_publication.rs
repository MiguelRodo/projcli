//! Real local Git repositories: no hosting account, remote service or credentials.
use std::{ffi::OsStr, fs, path::{Path, PathBuf}, process::{Command, Output}};
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

    fn command(&self, program: impl AsRef<OsStr>) -> Command {
        let mut command = Command::new(program);
        // Remove inherited repository, index, object, config and credential routing.
        // Enumerate keys only for removal; never mutate the parallel test process.
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().to_ascii_uppercase().starts_with("GIT_") {
                command.env_remove(key);
            }
        }
        for key in ["GITHUB_PAT", "GH_TOKEN", "GITHUB_TOKEN", "SSH_ASKPASS", "SSH_AUTH_SOCK",
                    "PROJCLI_GIT_TOKEN", "PROJCLI_GIT_TOKEN_OVERRIDE",
                    "PROJCLI_TEST_PROVIDER_ROOT", "PROJCLI_TEST_PROVIDER_MESSAGE",
                    proj::PROJECT_ROOT_ENV, proj::RUN_CONTEXT_ENV, "PROJR_PROFILE",
                    "QUARTO_PROFILE", "PROJR_CLEAR_OUTPUT"] {
            command.env_remove(key);
        }
        command.current_dir(&self.root)
            .env("GIT_CONFIG_NOSYSTEM", "1").env("GIT_CONFIG_SYSTEM", &self.global)
            .env("GIT_CONFIG_GLOBAL", &self.global).env("GIT_CONFIG_COUNT", "0")
            .env("GIT_TERMINAL_PROMPT", "0").env("GIT_ALLOW_PROTOCOL", "file");
        command
    }

    fn provider_commit(&self, message: &str) {
        let output = self.command(std::env::current_exe().unwrap())
            .args(["--ignored", "--exact", "provider_commit_in_isolated_child", "--nocapture"])
            .env("PROJCLI_TEST_PROVIDER_ROOT", &self.root)
            .env("PROJCLI_TEST_PROVIDER_MESSAGE", message).output().unwrap();
        assert!(output.status.success(), "{}\\n{}", String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr));
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
        let mut actual = refs.lines().map(str::to_owned).collect::<Vec<_>>();
        let mut expected = vec![format!("{TARGET} {target}"), format!("refs/heads/main {main}")];
        actual.sort();
        expected.sort();
        assert_eq!(actual, expected);
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

    fn pending_receipt(&self) -> (String, serde_json::Value) {
        let pending: Vec<_> = fs::read_dir(self.root.join(".proj/log")).unwrap()
            .map(|entry|entry.unwrap().path())
            .filter(|path|path.file_name().unwrap().to_string_lossy().ends_with(".git.json"))
            .filter(|path|!self.command("git").args(["ls-files","--error-unmatch","--"])
                .arg(path.strip_prefix(&self.root).unwrap()).output().unwrap().status.success()).collect();
        assert_eq!(pending.len(),1);
        let name=pending[0].file_name().unwrap().to_str().unwrap().to_owned();
        let bytes=fs::read(&pending[0]).unwrap();
        let receipt:serde_json::Value=serde_json::from_slice(&bytes).unwrap();
        let id=receipt["attempt_id"].as_str().unwrap();
        let snapshot=format!(".proj/log/{id}.json");
        let committed=self.git(&["show",&format!("HEAD:./{snapshot}")]);
        assert_eq!(fs::read(self.root.join(&snapshot)).unwrap(),committed);
        assert_eq!(receipt["snapshot_hash"],blake3::hash(&committed).to_hex().as_str());
        (name,receipt)
    }

    fn assert_pending_git_state(&self,version_dirty:bool) {
        let (name,receipt)=self.pending_receipt();
        let reference=&receipt["final_outputs"]["rows"];
        let csv=reference["file"].as_str().unwrap();
        let bytes=fs::read(self.root.join(".proj/log").join(csv)).unwrap();
        assert_eq!(reference["bytes"],bytes.len());
        assert_eq!(reference["blake3"],blake3::hash(&bytes).to_hex().as_str());
        assert!(!proj::manifest_csv::decode(&bytes).unwrap().is_empty());
        let mut expected=vec![format!("?? .proj/log/{name}"),format!("?? .proj/log/{csv}")];
        if version_dirty {expected.push(" M VERSION".into());}
        expected.sort();
        // Preserve initial whitespace in exact porcelain records.
        let mut actual:Vec<String>=String::from_utf8(self.git(&["status","--porcelain","--untracked-files=all"])).unwrap()
            .lines().map(str::to_owned).collect();
        actual.sort();
        assert_eq!(actual,expected);
    }

    fn assert_release_files(&self) {
        let expected = b"result\n84\n";
        assert_eq!(fs::read(self.root.join("released/results.csv")).unwrap(), expected);
        assert_eq!(fs::read(self.root.join("cache/projr/v1.2.4/output/results.csv")).unwrap(), expected);
        let manifest_path = self.root.join("manifest.csv").canonicalize().unwrap();
        let rows = proj::manifest::read(camino::Utf8Path::from_path(&manifest_path).unwrap()).unwrap();
        assert!(rows.iter().any(|row| row.label == "output" && row.r#fn == "results.csv"
            && row.version == "v1.2.4" && row.hash == "a70ebd90b8339a208d1573da9c1a700f"));
        assert_eq!(fs::read(self.root.join("released/previous.csv")).unwrap(), b"previous\n21\n");
    }

    fn assert_local_release(&self) {
        self.assert_release_files();
        assert_eq!(self.git(&["show", "HEAD:released/results.csv"]), b"result\n84\n");
        assert_eq!(self.git(&["show", "HEAD:VERSION"]), b"Version: v1.2.4");
        assert_eq!(self.git(&["show", "HEAD:manifest.csv"]),
                   fs::read(self.root.join("manifest.csv")).unwrap());
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
        fixture.assert_pending_git_state(true);
    }
}

#[test]
fn no_remote_allows_local_commit_but_requested_push_fails_before_analysis() {
    for push in [false, true] {
        let fixture = Fixture::new(push);
        fixture.git(&["remote", "remove", "fixture-upstream"]);
        assert_eq!(fixture.text(&["remote"]), "");
        let initial = fixture.text(&["rev-parse", "HEAD"]);
        let result = fixture.build(false);
        if push {
            assert!(!result.status.success());
            let diagnostic = String::from_utf8_lossy(&result.stderr);
            assert!(diagnostic.contains("tracking remote"), "{diagnostic}");
            assert!(!diagnostic.contains("Local build succeeded"), "{diagnostic}");
            assert_eq!(fixture.text(&["rev-parse", "HEAD"]), initial);
            assert_eq!(fs::read(fixture.root.join("VERSION")).unwrap(), b"v1.2.3-4\n");
            assert_eq!(fs::read(fixture.root.join("released/previous.csv")).unwrap(), b"previous\n21\n");
            assert!(!fixture.root.join("released/results.csv").exists());
            assert!(!fixture.root.join("cache").exists(), "push preflight must precede analysis/cache writes");
            assert!(!fixture.root.join("manifest.csv").exists());
            assert_eq!(fixture.text(&["status", "--porcelain"]), "");
        } else {
            assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
            assert_ne!(fixture.text(&["rev-parse", "HEAD"]), initial);
            fixture.assert_local_release();
            assert_eq!(fs::read(fixture.root.join("VERSION")).unwrap(), b"Version: v1.2.4.1");
        }
        fixture.assert_upstream_refs(&initial, &initial);
    }
}

#[test]
fn rejecting_real_pre_commit_hook_preserves_local_release_without_commit_or_push() {
    let fixture = Fixture::new(true);
    let initial = fixture.text(&["rev-parse", "HEAD"]);
    let hook = fixture.root.join(".git/hooks/pre-commit");
    fs::write(&hook, "#!/bin/sh\nprintf rejected > .git/pre-commit-witness\nexit 17\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let result = fixture.build(false);
    assert!(!result.status.success());
    let diagnostic = String::from_utf8_lossy(&result.stderr);
    assert_eq!(fs::read(fixture.root.join(".git/pre-commit-witness")).unwrap(), b"rejected");
    assert!(diagnostic.contains("Git operation failed"), "{diagnostic}");
    assert!(diagnostic.contains("Local build succeeded"), "{diagnostic}");
    assert!(diagnostic.contains("local release preserved"), "{diagnostic}");
    fixture.assert_release_files();
    assert_eq!(fs::read(fixture.root.join("VERSION")).unwrap(), b"Version: v1.2.4");
    assert_eq!(fixture.text(&["rev-parse", "HEAD"]), initial);
    assert_eq!(fixture.git(&["show", "HEAD:VERSION"]), b"v1.2.3-4\n");
    // Git staged the successful local release before the hook rejected its commit.
    // Do not claim that index state was rolled back or that publication succeeded.
    assert_eq!(fixture.git(&["show", ":released/results.csv"]), b"result\n84\n");
    assert_eq!(fixture.git(&["show", ":VERSION"]), b"Version: v1.2.4");
    assert_eq!(fixture.git(&["show", ":manifest.csv"]),
               fs::read(fixture.root.join("manifest.csv")).unwrap());
    fixture.assert_upstream_refs(&initial, &initial);
}

#[test]
fn actual_noop_commit_succeeds_without_creating_a_commit_but_real_changes_commit() {
    let fixture = Fixture::new(false);
    let initial = fixture.text(&["rev-parse", "HEAD"]);
    fixture.provider_commit("no changes");
    assert_eq!(fixture.text(&["rev-parse", "HEAD"]), initial);
    fs::write(fixture.root.join("tracked-result.txt"), b"actual new bytes\n").unwrap();
    fixture.provider_commit("real change");
    let changed = fixture.text(&["rev-parse", "HEAD"]);
    assert_ne!(changed, initial);
    assert_eq!(fixture.git(&["show", "HEAD:tracked-result.txt"]), b"actual new bytes\n");
    fixture.provider_commit("still no changes");
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
    fixture.assert_pending_git_state(false);
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


// Existing provider API inherits its process environment; run it only in the
// fixture-controlled child rather than changing global environment in parallel.
#[test]
#[ignore = "subprocess entrypoint invoked by the no-op and hostile-environment witnesses"]
fn provider_commit_in_isolated_child() {
    for key in ["GIT_DIR", "GIT_WORK_TREE", "GIT_COMMON_DIR", "GIT_INDEX_FILE",
                "GIT_OBJECT_DIRECTORY", "GIT_ALTERNATE_OBJECT_DIRECTORIES", "GIT_CONFIG",
                "GIT_CONFIG_PARAMETERS", "GIT_CONFIG_KEY_0", "GIT_CONFIG_VALUE_0",
                "GITHUB_PAT", "GH_TOKEN", "GITHUB_TOKEN", "PROJCLI_GIT_TOKEN"] {
        assert!(std::env::var_os(key).is_none(), "Inherited routing/credential override: {key}");
    }
    assert_eq!(std::env::var("GIT_ALLOW_PROTOCOL").unwrap(), "file");
    let path = PathBuf::from(std::env::var_os("PROJCLI_TEST_PROVIDER_ROOT").unwrap());
    let root = camino::Utf8Path::from_path(&path).unwrap().to_path_buf();
    SystemGit::new(root).commit_all(&std::env::var("PROJCLI_TEST_PROVIDER_MESSAGE").unwrap()).unwrap();
}

#[test]
#[ignore = "subprocess entrypoint invoked with deliberately hostile inherited Git routing"]
fn hostile_git_environment_child() {
    assert!(std::env::var_os("GIT_DIR").is_some());
    assert!(std::env::var_os("GIT_WORK_TREE").is_some());
    assert!(std::env::var_os("GIT_INDEX_FILE").is_some());
    let fixture = Fixture::new(true);
    let initial = fixture.text(&["rev-parse", "HEAD"]);
    fixture.provider_commit("isolated no-op under hostile inheritance");
    assert_eq!(fixture.text(&["rev-parse", "HEAD"]), initial);
    let result = fixture.build(false);
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    fixture.assert_local_release();
    fixture.assert_upstream_refs(&initial, &fixture.text(&["rev-parse", "HEAD"]));
}

#[test]
fn hostile_inherited_git_routing_cannot_modify_another_temporary_repository() {
    fn bytes(root: &Path) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
        let mut result = std::collections::BTreeMap::new();
        let mut pending = vec![root.to_owned()];
        while let Some(directory) = pending.pop() {
            for entry in fs::read_dir(directory).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() { pending.push(path); }
                else { result.insert(path.strip_prefix(root).unwrap().to_owned(), fs::read(path).unwrap()); }
            }
        }
        result
    }
    let victim = Fixture::new(false);
    let before = bytes(victim._temporary.path());
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", "hostile_git_environment_child", "--nocapture"])
        .env("GIT_DIR", victim.root.join(".git"))
        .env("GIT_COMMON_DIR", victim.root.join(".git"))
        .env("GIT_WORK_TREE", &victim.root)
        .env("GIT_INDEX_FILE", victim.root.join(".git/index"))
        .env("GIT_OBJECT_DIRECTORY", victim.root.join(".git/objects"))
        .env("GIT_ALTERNATE_OBJECT_DIRECTORIES", victim.root.join(".git/objects"))
        .env("GIT_NAMESPACE", "hostile-fixture")
        .env("GIT_CONFIG", &victim.global)
        .env("GIT_CONFIG_GLOBAL", &victim.global)
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "core.worktree")
        .env("GIT_CONFIG_VALUE_0", &victim.root)
        .env("GIT_CONFIG_PARAMETERS", "'core.bare=true'")
        .env("GIT_ASKPASS", victim.root.join("must-not-run"))
        .env("GIT_ALLOW_PROTOCOL", "https:ssh")
        .env("GH_TOKEN", "not-a-real-token")
        .output().unwrap();
    assert!(output.status.success(), "{}\\n{}", String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr));
    assert!(bytes(victim._temporary.path()) == before,
            "Hostile Git inheritance modified the other temporary project, Git metadata or upstream");
}

#[test]
fn successful_git_build_never_commits_live_writer_claim() {
    let fixture = Fixture::new(false);
    let result = fixture.build(false);
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    assert!(!fixture.root.join(".proj/build.lock").exists());
    let paths = fixture.git(&["ls-tree", "-r", "--name-only", "HEAD"]);
    assert!(!String::from_utf8_lossy(&paths).lines().any(|path| path == ".proj/build.lock"));
}

#[test]
fn nested_project_claim_exclusion_is_literal_and_keeps_whole_repository_scope() {
    let mut fixture = Fixture::new(false);
    let repository = fixture.root.clone();
    let entries: Vec<_> = fs::read_dir(&repository).unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.file_name().unwrap() != ".git").collect();
    let nested = repository.join(" child[1]");
    fs::create_dir(&nested).unwrap();
    for path in entries { fs::rename(&path, nested.join(path.file_name().unwrap())).unwrap(); }
    fixture.root = nested;
    let analysis = fs::read_to_string(fixture.root.join("analysis.py")).unwrap();
    fs::write(fixture.root.join("analysis.py"), analysis + "\nPath('../outside-generated').write_bytes(b'outside')\nPath('.proj/log').mkdir(exist_ok=True)\nPath('.proj/log/retained.json').write_text('{}')\n").unwrap();
    fixture.git(&["add", "-A"]);
    fixture.git(&["commit", "-m", "nested fixture"]);
    let result = fixture.build(false);
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    let paths = fixture.text(&["ls-tree", "--full-tree", "-r", "--name-only", "HEAD"]);
    assert!(paths.lines().any(|path| path == "outside-generated"), "{paths}");
    assert!(paths.lines().any(|path| path == " child[1]/.proj/log/retained.json"), "{paths}");
    assert!(!paths.lines().any(|path| path.ends_with(".proj/build.lock")), "{paths}");
    assert_eq!(fixture.git(&["show", "HEAD:./VERSION"]), b"Version: v1.2.4");
    assert_eq!(fs::read(fixture.root.join("VERSION")).unwrap(), b"Version: v1.2.4.1");
    let repeated = fixture.build(false);
    assert!(repeated.status.success(), "{}", String::from_utf8_lossy(&repeated.stderr));
    assert_eq!(fixture.git(&["show", "HEAD:./VERSION"]), b"Version: v1.2.5");
    assert_eq!(fs::read(fixture.root.join("VERSION")).unwrap(), b"Version: v1.2.5.1");
}

#[test]
fn sibling_project_claims_are_preserved_but_never_staged() {
    let fixture = Fixture::new(false);
    let analysis = fs::read_to_string(fixture.root.join("analysis.py")).unwrap();
    fs::write(fixture.root.join("analysis.py"), analysis + "\nPath('sibling/.proj').mkdir(parents=True)\nPath('sibling/.proj/build.lock').write_bytes(b'other active attempt')\nPath('sibling/result').write_bytes(b'whole repository scope')\n").unwrap();
    fixture.git(&["add", "-A"]);
    fixture.git(&["commit", "-m", "sibling ownership fixture"]);
    let result = fixture.build(false);
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    let paths = fixture.text(&["ls-tree", "--full-tree", "-r", "--name-only", "HEAD"]);
    assert!(!paths.lines().any(|path| path.ends_with(".proj/build.lock")), "{paths}");
    assert!(paths.lines().any(|path| path == "sibling/result"), "{paths}");
    assert_eq!(fs::read(fixture.root.join("sibling/.proj/build.lock")).unwrap(), b"other active attempt");
}

#[test]
fn tracked_sibling_claim_is_rejected_before_analysis() {
    let fixture = Fixture::new(false);
    fs::create_dir_all(fixture.root.join("sibling/.proj")).unwrap();
    fs::write(fixture.root.join("sibling/.proj/build.lock"), b"tracked reserved claim").unwrap();
    fixture.git(&["add", "-A"]);
    fixture.git(&["commit", "-m", "invalid tracked claim"]);
    let before = fs::read(fixture.root.join("VERSION")).unwrap();
    let result = fixture.build(false);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("must not be tracked"));
    assert_eq!(fs::read(fixture.root.join("VERSION")).unwrap(), before);
    assert_eq!(fs::read(fixture.root.join("sibling/.proj/build.lock")).unwrap(), b"tracked reserved claim");
    assert!(!fixture.root.join(".proj/build.lock").exists());
    assert!(!fixture.root.join("released/results.csv").exists());
}

#[test]
fn attempt_snapshot_and_receipt_follow_two_normal_commits_without_recursion() {
    let fixture=Fixture::new(false);
    let initial=fixture.text(&["rev-parse","HEAD"]);
    let first=fixture.build(false);
    assert!(first.status.success(),"{}",String::from_utf8_lossy(&first.stderr));
    assert_eq!(fixture.text(&["rev-list","--count",&format!("{initial}..HEAD")]),"1");
    let (name,receipt)=fixture.pending_receipt();
    assert_eq!(receipt["status"],"completed");
    assert_eq!(receipt["git"]["commit"],"completed");
    assert_eq!(receipt["git"]["push"],"not_requested");
    assert_eq!(receipt["git"]["head"],fixture.text(&["rev-parse","HEAD"]));
    assert_eq!(receipt["version_after"],"v1.2.4.1");
    let id=receipt["attempt_id"].as_str().unwrap();
    let snapshot:serde_json::Value=serde_json::from_slice(&fixture.git(&["show",&format!("HEAD:./.proj/log/{id}.json")])).unwrap();
    assert_eq!(receipt["final_outputs"]["status"],"completed");
    assert_eq!(receipt["final_outputs"]["inventory"],snapshot["inventory"]["release_outputs"]);
    assert_eq!(snapshot["startup_provenance"]["source_head"],initial);
    assert_eq!(snapshot["startup_provenance"]["source_state"],"git_preflight_accepted");
    assert_eq!(snapshot["startup_provenance"]["configuration"]["policy"]["git_commit"],true);
    let rows=proj::manifest_csv::decode(&fixture.git(&["show","HEAD:./manifest.csv"])).unwrap();
    let expected=blake3::hash(&proj::manifest_csv::encode(&rows).unwrap()).to_hex().to_string();
    assert_eq!(snapshot["inventory"]["release_outputs"]["canonical_csv_blake3"],expected);
    let bytes=fs::read(fixture.root.join(".proj/log").join(&name)).unwrap();
    let csv=receipt["final_outputs"]["rows"]["file"].as_str().unwrap();
    let csv_bytes=fs::read(fixture.root.join(".proj/log").join(csv)).unwrap();
    let second=fixture.build(false);
    assert!(second.status.success(),"{}",String::from_utf8_lossy(&second.stderr));
    assert_eq!(fixture.text(&["rev-list","--count",&format!("{initial}..HEAD")]),"2");
    assert_eq!(fixture.git(&["show",&format!("HEAD:./.proj/log/{name}")]),bytes);
    assert_eq!(fixture.git(&["show",&format!("HEAD:./.proj/log/{csv}")]),csv_bytes);
    fixture.assert_pending_git_state(true);
}

#[test]
fn edited_forged_and_unwitnessed_pending_receipts_fail_closed() {
    for change in ["valid-field-edit","forged-name","missing-witness","edited-csv","missing-csv"] {
        let fixture=Fixture::new(false);
        assert!(fixture.build(false).status.success());
        let initial=fixture.text(&["rev-parse","HEAD"]);
        let (name,mut receipt)=fixture.pending_receipt();
        let path=fixture.root.join(".proj/log").join(&name);
        if change=="valid-field-edit" {
            receipt["version_after"]=serde_json::json!("v8.8.8.8");
            fs::write(&path,serde_json::to_vec(&receipt).unwrap()).unwrap();
        }else if change=="forged-name" {
            fs::write(fixture.root.join(".proj/log/attempt-AAAAAAAAAAAAAAAAAAAAAAAA.git.json"),
                serde_json::to_vec(&receipt).unwrap()).unwrap();
        }else if change=="edited-csv"||change=="missing-csv" {
            let csv=fixture.root.join(".proj/log").join(receipt["final_outputs"]["rows"]["file"].as_str().unwrap());
            if change=="missing-csv"{fs::remove_file(csv).unwrap()}else{fs::write(csv,b"edited").unwrap()}
        }else{
            let id=receipt["attempt_id"].as_str().unwrap();
            fs::remove_file(fixture.root.join(".git/projcli-attempt-receipts").join(format!("{id}.json"))).unwrap();
        }
        let before=fs::read(fixture.root.join("VERSION")).unwrap();
        let result=fixture.build(false);
        assert!(!result.status.success(),"{change}");
        assert!(String::from_utf8_lossy(&result.stderr).contains("already contains changes"),"{change}");
        assert_eq!(fixture.text(&["rev-parse","HEAD"]),initial);
        assert_eq!(fs::read(fixture.root.join("VERSION")).unwrap(),before);
    }
}

#[cfg(unix)]
#[test]
fn failed_commit_and_failed_receipt_write_preserve_both_diagnostics_and_snapshot() {
    use std::os::unix::fs::PermissionsExt;
    let fixture=Fixture::new(false);
    let initial=fixture.text(&["rev-parse","HEAD"]);
    let hook=fixture.root.join(".git/hooks/pre-commit");
    fs::write(&hook,"#!/bin/sh\nchmod 500 .proj/log\nexit 17\n").unwrap();
    fs::set_permissions(&hook,fs::Permissions::from_mode(0o755)).unwrap();
    let result=fixture.build(false);
    fs::set_permissions(fixture.root.join(".proj/log"),fs::Permissions::from_mode(0o700)).unwrap();
    assert!(!result.status.success());
    let diagnostic=String::from_utf8_lossy(&result.stderr);
    assert!(diagnostic.contains("Git operation failed"),"{diagnostic}");
    assert!(diagnostic.contains("final attempt evidence could not be written"),"{diagnostic}");
    assert!(diagnostic.to_lowercase().contains("permission denied"),"{diagnostic}");
    assert_eq!(fixture.text(&["rev-parse","HEAD"]),initial);
    assert_eq!(fs::read(fixture.root.join("VERSION")).unwrap(),b"Version: v1.2.4");
    let logs:Vec<_>=fs::read_dir(fixture.root.join(".proj/log")).unwrap().map(|entry|entry.unwrap().path()).collect();
    assert_eq!(logs.len(),1);
    let snapshot:serde_json::Value=serde_json::from_slice(&fs::read(&logs[0]).unwrap()).unwrap();
    assert_eq!(snapshot["status"],"pre_git_snapshot");
    assert_eq!(snapshot["distribution_may_have_started"],true);
}

#[test]
fn git_receipt_observes_post_hook_changes_without_rewriting_the_committed_snapshot() {
    let fixture=Fixture::new(false);
    let config=fs::read_to_string(fixture.root.join("_proj.yml")).unwrap();
    fs::write(fixture.root.join("_proj.yml"),format!("{config}  hooks:\n    post: [end.py]\n")).unwrap();
    fs::write(fixture.root.join("end.py"),"from pathlib import Path\nPath('released/results.csv').rename('released/renamed.csv')\n").unwrap();
    fixture.git(&["add","-A"]);fixture.git(&["commit","-m","fixture end hook"]);
    let output=fixture.build(false);
    assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stderr));
    let (_,receipt)=fixture.pending_receipt();
    let id=receipt["attempt_id"].as_str().unwrap();
    let snapshot:serde_json::Value=serde_json::from_slice(&fixture.git(&["show",&format!("HEAD:./.proj/log/{id}.json")])).unwrap();
    assert_ne!(receipt["final_outputs"]["inventory"],snapshot["inventory"]["release_outputs"]);
    assert_eq!(receipt["git"]["commit"],"completed");
    assert_eq!(fixture.git(&["show","HEAD:released/results.csv"]),b"result\n84\n");
    assert_eq!(fs::read(fixture.root.join("released/renamed.csv")).unwrap(),b"result\n84\n");
    assert!(!fixture.root.join("released/results.csv").exists());
    let released=fixture.root.join("released").canonicalize().unwrap();
    let rows=proj::manifest::capture_directory("output",camino::Utf8Path::from_path(&released).unwrap(),"v1.2.4",false).unwrap();
    let expected=blake3::hash(&proj::manifest_csv::encode(&rows).unwrap()).to_hex().to_string();
    assert_eq!(receipt["final_outputs"]["inventory"]["canonical_csv_blake3"],expected);
}
