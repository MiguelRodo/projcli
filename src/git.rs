use anyhow::Context;
use crate::run_environment::RunEnvironment;
use std::process::Command;

use std::io::{Read,Write};

/// Resolve Git explicitly: Windows otherwise may search the parent's PATH.
pub(crate) fn git_command_in(root: Option<&camino::Utf8Path>, environment: &RunEnvironment) -> anyhow::Result<Command> {
    let directory = match root {
        Some(root) => root.as_std_path().to_owned(),
        None => environment.value(crate::PROJECT_ROOT_ENV).map(std::path::PathBuf::from)
            .unwrap_or(std::env::current_dir()?),
    };
    let directory = if directory.is_absolute() { directory } else { std::env::current_dir()?.join(directory) };
    let search = environment.value("PATH").unwrap_or(std::ffi::OsStr::new(""));
    for entry in std::env::split_paths(search) {
        let entry = if entry.is_absolute() { entry } else { directory.join(entry) };
        let candidate = entry.join(if cfg!(windows) { "git.exe" } else { "git" });
        if !candidate.is_file() { continue; }
        #[cfg(unix)] {
            use std::os::unix::fs::PermissionsExt;
            if std::fs::metadata(&candidate)?.permissions().mode() & 0o111 == 0 { continue; }
        }
        return Ok(environment.command(candidate));
    }
    anyhow::bail!("Git executable is missing from the resolved PATH")
}

pub trait GitProvider {
    fn is_available(&self) -> bool;
    fn get_user_name(&self) -> Option<String>;
    fn get_user_email(&self) -> Option<String>;
    fn commit_all(&self, message: &str) -> anyhow::Result<()>;
    fn push(&self, remote: &str, branch: &str) -> anyhow::Result<()>;
    fn is_behind_remote(&self, remote: &str, branch: &str) -> anyhow::Result<bool>;
}

pub struct SystemGit {
    repo_path: camino::Utf8PathBuf,
    environment: RunEnvironment,
}

impl SystemGit {
    pub fn new(repo_path: camino::Utf8PathBuf) -> Self {
        Self { repo_path, environment: RunEnvironment::inherited() }
    }

    // Helper to execute standard git actions safely with uniform string handling
    fn run_cmd(&self, args: &[&str]) -> anyhow::Result<String> {
        let output = git_command_in(Some(&self.repo_path), &self.environment)?
            .current_dir(&self.repo_path)
            .args(args)
            .output()
            .context("Failed to execute system git process")?;

        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
        } else {
            anyhow::bail!("Git operation failed with status {}", output.status)
        }
    }

    // Keep whole-repository scope, excluding reserved live claims at every depth.
    fn writer_exclusion(&self) -> anyhow::Result<&'static str> {
        let tracked = git_command_in(Some(&self.repo_path), &self.environment)?
            .current_dir(&self.repo_path)
            .args(["ls-files", "--error-unmatch", "--", ":(top,glob)**/.proj/build.lock"]).output()?;
        anyhow::ensure!(tracked.status.code() == Some(1),
            "Build writer claims must not be tracked by Git");
        Ok(":(top,exclude,glob)**/.proj/build.lock")
    }

    fn network_command(&self, action: &str, remote: &str, reference: Option<&str>) -> anyhow::Result<()> {
        let mut arguments = if action == "push" {
            vec![action, "--no-follow-tags", "--recurse-submodules=no", "--", remote]
        } else { vec![action, "--recurse-submodules=no", "--", remote] };
        if let Some(reference) = reference { arguments.push(reference); }
        let url = if action == "push" { self.run_cmd(&["remote", "get-url", "--push", "--all", "--", remote])? }
            else { self.run_cmd(&["remote", "get-url", "--all", "--", remote])? };
        if url.lines().count() != 1 { anyhow::bail!("Multiple Git network destinations are unsupported"); }
        let github_https = url.strip_prefix("https://").and_then(|tail| tail.split('/').next())
            .is_some_and(|host| host.eq_ignore_ascii_case("github.com"));
        let override_enabled = self.environment.text("PROJCLI_GIT_TOKEN_OVERRIDE") == Some("true");
        let token = if override_enabled && github_https {
            ["GITHUB_PAT", "GH_TOKEN", "GITHUB_TOKEN"].iter()
                .find_map(|key| self.environment.text(key).filter(|value| !value.is_empty()))
        } else { None };
        if let Some(token) = token {
            execute_authenticated_git_in(&arguments, token, Some(&self.repo_path), &self.environment)
        } else {
            self.run_cmd(&arguments).map(|_| ())
        }
    }

}

impl GitProvider for SystemGit {
    fn is_available(&self) -> bool {
        git_command_in(Some(&self.repo_path), &self.environment).and_then(|mut command| Ok(command.arg("--version").output()?))
            .map(|output| output.status.success()).unwrap_or(false)
    }

    fn get_user_name(&self) -> Option<String> {
        self.run_cmd(&["config", "user.name"]).ok()
    }

    fn get_user_email(&self) -> Option<String> {
        self.run_cmd(&["config", "user.email"]).ok()
    }

    fn commit_all(&self, message: &str) -> anyhow::Result<()> {
        let exclusion = self.writer_exclusion()?;
        self.run_cmd(&["add", "-A", "--", ":/", &exclusion])?;
        let output = git_command_in(Some(&self.repo_path), &self.environment)?.current_dir(&self.repo_path)
            .args(["diff", "--cached", "--quiet"]).output()?;
        match output.status.code() {
            Some(0) => return Ok(()),
            Some(1) => {}
            _ => anyhow::bail!("Cannot determine staged Git changes"),
        }
        self.run_cmd(&["commit", "-m", message])?;
        Ok(())
    }

    fn push(&self, remote: &str, branch: &str) -> anyhow::Result<()> {
        self.network_command("push", remote, Some(branch))
    }

    fn is_behind_remote(&self, remote: &str, branch: &str) -> anyhow::Result<bool> {
        // Fetch tracking info silently first
        self.network_command("fetch", remote, None)?;

        // Count the commits the local branch is behind the remote tracking branch
        let remote_target = format!("{}/{}", remote, branch);
        let count_str = self.run_cmd(&["rev-list", "--count", &format!("HEAD..{}", remote_target)])?;

        let count: usize = count_str.parse().context("Invalid Git upstream comparison result")?;
        Ok(count > 0)
    }
}

// Unified Factory pattern isolated away from your pipeline logic
pub fn create_git_provider(engine: crate::yml::GitEngine, repo_path: camino::Utf8PathBuf) -> anyhow::Result<Box<dyn GitProvider>> {
    create_git_provider_in(engine, repo_path, &RunEnvironment::inherited())
}

pub(crate) fn create_git_provider_in(engine: crate::yml::GitEngine, repo_path: camino::Utf8PathBuf, environment: &RunEnvironment) -> anyhow::Result<Box<dyn GitProvider>> {
    let provider = Box::new(SystemGit { repo_path, environment: environment.clone() });

    match engine {
        crate::yml::GitEngine::System | crate::yml::GitEngine::Auto => {
            if provider.is_available() {
                Ok(provider)
            } else {
                anyhow::bail!("System Git executable could not be resolved in the current environment PATH.")
            }
        }
    }
}

pub fn get_github_token() -> anyhow::Result<String> {
    get_github_token_in(&RunEnvironment::inherited())
}

pub(crate) fn get_github_token_in(environment: &RunEnvironment) -> anyhow::Result<String> {
    // Track A: Environment Context Inspection
    let env_vars = ["GITHUB_PAT", "GH_TOKEN", "GITHUB_TOKEN"];
    for var in &env_vars {
        if let Some(val) = environment.text(var) {
            let trimmed = val.trim();
            if !trimmed.is_empty() {
                return Ok(trimmed.to_string());
            }
        }
    }

    // Track B: System Git Credential Helper Interrogation
    let mut child = git_command_in(None, environment)?
        .args(["credential", "fill"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("Failed to spawn git credential fill")?;

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(b"protocol=https\nhost=github.com\n\n");
    }

    let output = child.wait_with_output().context("Failed to wait on git credential fill")?;
    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            if let Some(password) = line.strip_prefix("password=") {
                let trimmed = password.trim();
                if !trimmed.is_empty() {
                    return Ok(trimmed.to_string());
                }
            }
        }
    }

    // Track C: GitHub CLI Engine Query
    if let Ok(output) = environment.command("gh").args(["auth", "token", "--hostname", "github.com"]).output() {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let trimmed = stdout.trim();
            if !trimmed.is_empty() {
                return Ok(trimmed.to_string());
            }
        }
    }

    // Output structured error if token is missing
    anyhow::bail!("Error: GitHub authentication token not found.\nAuthentication is required to interact with remote repositories via git push or fetch.\n\nTo resolve this, please execute one of the following options:\nOption A (Environment Variable):\n    Set the GITHUB_PAT environment variable in your active terminal profile.\nOption B (GitHub CLI):\n    Install the 'gh' utility and run 'gh auth login' to authenticate your host.\nOption C (Git Helper Setup):\n    Approve host access directly inside your local system credential helper:\n    git credential approve < echo -e \"protocol=https\\nhost=github.com\\nusername=user\\npassword=YOUR_PAT\"")
}

const TOKEN_HELPER: &str = r#"!f() { [ "$1" = get ] || exit 0; protocol=; host=; while IFS= read -r line && [ -n "$line" ]; do case "$line" in protocol=*) protocol=${line#protocol=};; host=*) host=${line#host=};; esac; done; if [ "$protocol" = https ] && [ "$host" = github.com ]; then printf 'username=x-access-token\npassword=%s\n' "$PROJCLI_GIT_TOKEN"; fi; }; f"#;

pub fn execute_authenticated_git(args: &[&str], token: &str, current_dir: Option<&camino::Utf8Path>) -> anyhow::Result<()> {
    execute_authenticated_git_in(args, token, current_dir, &RunEnvironment::inherited())
}

pub(crate) fn execute_authenticated_git_in(args: &[&str], token: &str, current_dir: Option<&camino::Utf8Path>, environment: &RunEnvironment) -> anyhow::Result<()> {
    if token.contains(['\0', '\n', '\r']) {
        anyhow::bail!("Git token must be a single non-NUL line");
    }
    // Fixed code only. Secret bytes are passed as environment data, never argv/code.

    let mut command = git_command_in(current_dir, environment)?;
    command.args(["-c", "credential.helper=", "-c"])
        .arg(format!("credential.helper={TOKEN_HELPER}"))
        .env("PROJCLI_GIT_TOKEN", token).env("GIT_TERMINAL_PROMPT", "0").args(args);
    if let Some(directory) = current_dir { command.current_dir(directory); }
    let output = command.output().context("Cannot execute authenticated Git operation")?;
    if !output.status.success() { anyhow::bail!("Authenticated Git operation failed with status {}", output.status); }
    Ok(())
}

pub(crate) fn tracking_target_in(root: &camino::Utf8Path, environment: &RunEnvironment) -> anyhow::Result<(String, String)> {
    let git = SystemGit { repo_path: root.to_owned(), environment: environment.clone() };
    let branch = git.run_cmd(&["symbolic-ref", "--quiet", "--short", "HEAD"])
        .context("Git publication requires a checked-out branch")?;
    let remote = git.run_cmd(&["config", "--get-all", &format!("branch.{branch}.remote")])
        .context("Configured Git operation requires the current branch's tracking remote")?;
    let reference = git.run_cmd(&["config", "--get-all", &format!("branch.{branch}.merge")])
        .context("Configured Git operation requires the current branch's tracking reference")?;
    if remote.is_empty() || remote.lines().count() != 1 || reference.lines().count() != 1 || !reference.starts_with("refs/heads/") || reference == "refs/heads/" {
        anyhow::bail!("Invalid current-branch tracking configuration");
    }
    if remote.contains(['\n', '\r', '\0']) {
        anyhow::bail!("Invalid tracking remote name");
    }
    let checked = git_command_in(Some(root), environment)?.current_dir(root).args(["check-ref-format", &reference]).output()?;
    if !checked.status.success() { anyhow::bail!("Invalid tracking branch reference"); }
    Ok((remote, reference))
}

// No Debug implementation: endpoint URLs may contain user-supplied credentials.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct PushTarget {
    branch: String,
    pub(crate) remote: String,
    pub(crate) reference: String,
    endpoint: String,
}

pub(crate) fn push_target_in(root: &camino::Utf8Path, environment: &RunEnvironment) -> anyhow::Result<PushTarget> {
    let target = tracking_target_in(root, environment)?;
    let git = SystemGit { repo_path: root.to_owned(), environment: environment.clone() };
    let branch = git.run_cmd(&["symbolic-ref", "--quiet", "--short", "HEAD"])?;
    // ponytail: one current-upstream push; reject custom routes until explicitly modeled.
    for key in [format!("branch.{branch}.pushRemote"), "remote.pushDefault".to_string(), format!("remote.{}.push", target.0), format!("remote.{}.pushurl", target.0)] {
        let output = git_command_in(Some(root), environment)?.current_dir(root).args(["config", "--get", &key]).output()?;
        match output.status.code() {
            Some(1) => {}
            Some(0) if output.stdout.iter().all(u8::is_ascii_whitespace) => {}
            Some(0) => anyhow::bail!("Custom Git push routing is not supported by this build; configure a plain current-branch upstream"),
            _ => anyhow::bail!("Cannot inspect Git push routing"),
        }
    }
    let mirror = git_command_in(Some(root), environment)?.current_dir(root)
        .args(["config", "--type=bool", "--get-all", &format!("remote.{}.mirror", target.0)]).output()?;
    match mirror.status.code() {
        Some(1) => {}
        Some(0) if String::from_utf8_lossy(&mirror.stdout).trim() == "false" => {}
        Some(0) => anyhow::bail!("Git mirror publication is unsupported"),
        _ => anyhow::bail!("Cannot inspect Git mirror configuration"),
    }
    let fetch = git.run_cmd(&["remote", "get-url", "--all", "--", &target.0])?;
    let destinations = git.run_cmd(&["remote", "get-url", "--push", "--all", "--", &target.0])?;
    if fetch.lines().count() != 1 || destinations.lines().count() != 1 {
        anyhow::bail!("Multiple Git push destinations are unsupported");
    }
    if fetch != destinations { anyhow::bail!("Separate effective Git push endpoints are unsupported"); }
    Ok(PushTarget { branch, remote: target.0, reference: target.1, endpoint: destinations })
}

pub(crate) fn validate_push_target_in(root: &camino::Utf8Path, environment: &RunEnvironment, captured: &PushTarget) -> anyhow::Result<()> {
    let current = push_target_in(root, environment).context("Git publication target is no longer supported")?;
    if &current != captured { anyhow::bail!("Git publication target changed after the local build"); }
    Ok(())
}

pub(crate) fn fetch_remote_in(root: &camino::Utf8Path, remote: &str, environment: &RunEnvironment) -> anyhow::Result<()> {
    SystemGit { repo_path: root.to_owned(), environment: environment.clone() }.network_command("fetch", remote, None)
}

pub(crate) fn require_clean_git_in(root: &camino::Utf8Path, environment: &RunEnvironment) -> anyhow::Result<()> {
    let git = SystemGit { repo_path: root.to_owned(), environment: environment.clone() };
    let exclusion = git.writer_exclusion()?;
    let output = git_command_in(Some(root), environment)?.current_dir(root)
        .args(["status", "--porcelain", "-z", "--untracked-files=all", "--ignore-submodules=none", "--", ":/", &exclusion]).output()?;
    anyhow::ensure!(output.status.success(), "Cannot inspect Git worktree state");
    if !output.stdout.is_empty() {
        let prefix = git_command_in(Some(root), environment)?.current_dir(root)
            .args(["rev-parse", "--show-prefix"]).output()?;
        anyhow::ensure!(prefix.status.success(), "Cannot resolve project path within Git repository");
        let prefix = prefix.stdout.strip_suffix(b"\n").unwrap_or(&prefix.stdout);
        let prefix = prefix.strip_suffix(b"\r").unwrap_or(prefix);
        let mut expected_path = b" M ".to_vec();
        expected_path.extend_from_slice(prefix);
        expected_path.extend_from_slice(b"VERSION\0");
        let mut version_dirty=false;
        let mut allowed=true;
        for entry in output.stdout.split(|byte|*byte==0).filter(|entry|!entry.is_empty()) {
            if entry==&expected_path[..expected_path.len()-1] {version_dirty=true;continue;}
            let receipt=entry.strip_prefix(b"?? ").and_then(|path|path.strip_prefix(prefix))
                .and_then(|path|std::str::from_utf8(path).ok());
            let owned=if let Some(relative)=receipt {
                crate::attempt_log::owned_receipt(root,relative,|object|git_object_bytes(&git,object),
                    |id,snapshot,receipt|pending_receipt_matches(root,&git,id,snapshot,receipt))?
            }else{false};
            if !owned {allowed=false;break;}
        }
        anyhow::ensure!(allowed && (!version_dirty||owned_dev_transition_matches(root,&git)?),
            "Git commit requested but the repository already contains changes; resolve them before building");
    }
    Ok(())
}

/// Returns true if executing `git --version` succeeds.
pub fn is_git_installed() -> bool {
    is_git_installed_in(&RunEnvironment::inherited())
}

pub(crate) fn is_git_installed_in(environment: &RunEnvironment) -> bool {
    git_command_in(None, environment).and_then(|mut command| Ok(command.arg("--version").output()?))
        .map(|output| output.status.success()).unwrap_or(false)
}

/// Queries local, global, and system variables via `git config --get user.name` and `git config --get user.email`.
/// If either stdout buffer returns blank or throws an error, return an explicit error string detailing exactly what config parameter is missing and how the user can configure it.
pub fn check_git_profile(current_dir: Option<&camino::Utf8Path>) -> anyhow::Result<()> {
    check_git_profile_in(current_dir, &RunEnvironment::inherited())
}

pub(crate) fn check_git_profile_in(current_dir: Option<&camino::Utf8Path>, environment: &RunEnvironment) -> anyhow::Result<()> {
    let check_config = |key: &str| -> anyhow::Result<()> {
        let mut cmd = git_command_in(current_dir, environment)?;
        cmd.args(["config", "--get", key]);
        if let Some(dir) = current_dir {
            cmd.current_dir(dir);
        }
        let output = cmd.output()
            .context("Failed to execute git config check")?;

        if !output.status.success() || String::from_utf8_lossy(&output.stdout).trim().is_empty() {
            anyhow::bail!(
                "Git configuration missing: '{}' is not set.\nPlease configure it using: git config --global {} \"Your Value\"",
                key, key
            );
        }
        Ok(())
    };

    check_config("user.name")?;
    check_config("user.email")?;

    Ok(())
}

/// Runs `git add -A` to stage modified and untracked changes.
/// Runs `git commit -m "<message>"`. Squelch errors gracefully if there are no modifications staged to be committed.
pub fn git_commit_all(message: &str, current_dir: Option<&camino::Utf8Path>) -> anyhow::Result<()> {
    // git add -A
    let mut add_cmd = Command::new("git");
    add_cmd.args(["add", "-A"]);
    if let Some(dir) = current_dir {
        add_cmd.current_dir(dir);
    }

    let add_output = add_cmd.output()
        .context("Failed to execute 'git add -A'")?;

    if !add_output.status.success() {
        anyhow::bail!("'git add -A' failed: {}", String::from_utf8_lossy(&add_output.stderr));
    }

    // git commit -m message
    let mut commit_cmd = Command::new("git");
    commit_cmd.args(["commit", "-m", message]);
    if let Some(dir) = current_dir {
        commit_cmd.current_dir(dir);
    }

    let commit_output = commit_cmd.output()
        .context("Failed to execute 'git commit'")?;

    // Squelch errors gracefully if there are no modifications staged to be committed
    let stdout = String::from_utf8_lossy(&commit_output.stdout);
    let stderr = String::from_utf8_lossy(&commit_output.stderr);

    if !commit_output.status.success() {
        let combined_output = format!("{}\n{}", stdout, stderr).to_lowercase();
        // Check for typical "nothing to commit" messages
        if combined_output.contains("nothing to commit") ||
           combined_output.contains("working tree clean") ||
           combined_output.contains("no changes added to commit") {
            return Ok(()); // Squelch error
        }
        anyhow::bail!("'git commit' failed: {}", stderr);
    }

    Ok(())
}

/// Runs `git push` to upload tracking offsets upstream.
pub fn git_push(current_dir: Option<&camino::Utf8Path>, token: Option<&str>) -> anyhow::Result<()> {
    if let Some(t) = token {
        execute_authenticated_git(&["push"], t, current_dir)
    } else {
        let mut push_cmd = Command::new("git");
        push_cmd.arg("push");
        if let Some(dir) = current_dir {
            push_cmd.current_dir(dir);
        }

        let output = push_cmd.output()
            .context("Failed to execute 'git push'")?;

        if !output.status.success() {
            anyhow::bail!("'git push' failed: {}", String::from_utf8_lossy(&output.stderr));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::yml::GitEngine;

    #[test]
    fn test_system_git_fails_gracefully_without_path() {
        let mut environment = RunEnvironment::inherited();
        environment.set("PATH", "");
        let result = create_git_provider_in(GitEngine::Auto, camino::Utf8PathBuf::from("."), &environment);

        assert!(result.is_err());
        match result {
            Err(e) => assert_eq!(e.to_string(), "System Git executable could not be resolved in the current environment PATH."),
            Ok(_) => panic!("Expected an error when git is not in PATH"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn fixed_token_helper_returns_literal_data_only_for_its_exact_https_host() {
        use std::process::Stdio;
        let project = tempfile::tempdir().unwrap();
        let token = "fixture-$(touch injected)-'\";$value";
        for (protocol, host, allowed) in [("https", "github.com", true), ("http", "github.com", false), ("https", "other.example", false)] {
            let mut child = Command::new("/bin/sh").current_dir(project.path())
                .args(["-c", &format!("{} get", &TOKEN_HELPER[1..])])
                .env("PROJCLI_GIT_TOKEN", token)
                .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
            child.stdin.take().unwrap().write_all(format!("protocol={protocol}\nhost={host}\n\n").as_bytes()).unwrap();
            let output = child.wait_with_output().unwrap();
            assert!(output.status.success());
            if allowed { assert_eq!(String::from_utf8(output.stdout).unwrap(), format!("username=x-access-token\npassword={token}\n")); }
            else { assert!(output.stdout.is_empty()); }
            assert!(!project.path().join("injected").exists());
        }
    }

    #[cfg(unix)]
    #[test]
    fn token_is_absent_from_argv_and_failure_diagnostics_and_helper_chain_is_reset() {
        use std::os::unix::fs::PermissionsExt;
        let project = tempfile::tempdir().unwrap();
        let root = camino::Utf8Path::from_path(project.path()).unwrap();
        let bin = root.join("bin"); std::fs::create_dir(&bin).unwrap();
        let git = bin.join("git");
        std::fs::write(&git, "#!/bin/sh\nprintf '%s\\n' \"$@\" > argv\nprintf '%s' \"$PROJCLI_GIT_TOKEN\" >&2\nexit 23\n").unwrap();
        std::fs::set_permissions(&git, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut environment = RunEnvironment::inherited();
        environment.set("PATH", bin.as_str());
        environment.set("GIT_CONFIG_COUNT", "1");
        environment.set("GIT_CONFIG_KEY_0", "credential.helper");
        environment.set("GIT_CONFIG_VALUE_0", "existing-helper");
        let token = "fixture-sensitive-token";
        let error = execute_authenticated_git_in(&["fetch", "--", "upstream"], token, Some(root), &environment).unwrap_err();
        let argv = std::fs::read_to_string(root.join("argv")).unwrap();
        assert!(!argv.contains(token));
        assert!(!format!("{error:#}").contains(token));
        let lines: Vec<_> = argv.lines().collect();
        assert_eq!(&lines[..3], &["-c", "credential.helper=", "-c"]);
        assert!(lines[3].starts_with("credential.helper=!"));
    }


    #[cfg(unix)]
    #[test]
    fn relative_repo_and_path_execute_the_resolved_absolute_git_file() {
        use std::os::unix::fs::PermissionsExt;
        let cwd = std::env::current_dir().unwrap().canonicalize().unwrap();
        let container = tempfile::Builder::new().prefix("relative-git-").tempdir_in(&cwd).unwrap();
        let repo = container.path().join("project");
        std::fs::create_dir_all(repo.join("bin")).unwrap();
        let git = repo.join("bin/git");
        std::fs::write(&git, "#!/bin/sh\nprintf '%s\\n' \"$0\"\n").unwrap();
        std::fs::set_permissions(&git, std::fs::Permissions::from_mode(0o755)).unwrap();
        let relative = camino::Utf8PathBuf::try_from(repo.strip_prefix(&cwd).unwrap().to_owned()).unwrap();
        let mut environment = RunEnvironment::inherited(); environment.set("PATH", "bin");
        let provider = create_git_provider_in(GitEngine::Auto, relative, &environment).unwrap();
        assert!(provider.is_available());
        assert_eq!(std::path::PathBuf::from(provider.get_user_name().unwrap()), git.canonicalize().unwrap());
    }


    #[cfg(unix)]
    #[test]
    fn git_symlink_dispatcher_keeps_its_invocation_name() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let project = tempfile::tempdir().unwrap();
        let physical = project.path().canonicalize().unwrap();
        let root = camino::Utf8Path::from_path(&physical).unwrap();
        let bin = root.join("bin"); std::fs::create_dir(&bin).unwrap();
        let dispatcher = bin.join("dispatcher");
        std::fs::write(&dispatcher, "#!/bin/sh\ncase \"$0\" in */git) printf 'git-shim\\n' ;; *) exit 31 ;; esac\n").unwrap();
        std::fs::set_permissions(&dispatcher, std::fs::Permissions::from_mode(0o755)).unwrap();
        symlink("dispatcher", bin.join("git")).unwrap();
        let mut environment = RunEnvironment::inherited(); environment.set("PATH", bin.as_str());
        let provider = create_git_provider_in(GitEngine::Auto, root.to_owned(), &environment).unwrap();
        assert_eq!(provider.get_user_name().unwrap(), "git-shim");
    }

}

// This is one exact framework-owned transition, not permission for arbitrary dirty
// VERSION edits. The record lives in this worktree's Git metadata, never a commit.
#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct DevTransition {
    schema: u32,
    root: String,
    head: String,
    release: String,
    development: String,
}

fn transition_path(root: &camino::Utf8Path, git: &SystemGit) -> anyhow::Result<std::path::PathBuf> {
    let directory = std::path::PathBuf::from(git.run_cmd(&["rev-parse", "--absolute-git-dir"])?);
    anyhow::ensure!(directory.is_absolute() && directory.is_dir(), "Git metadata directory is invalid");
    crate::path_safety::resolve_path(&directory, true)?;
    let _ = root;
    Ok(directory.join("projcli-dev-transition.json"))
}

fn git_object_bytes(git:&SystemGit,object:&str)->anyhow::Result<Vec<u8>> {
    let output=git_command_in(Some(&git.repo_path),&git.environment)?.current_dir(&git.repo_path)
        .args(["show",object]).output()?;
    anyhow::ensure!(output.status.success(),"Cannot inspect committed project evidence");
    Ok(output.stdout)
}

fn git_file_bytes(git:&SystemGit)->anyhow::Result<Vec<u8>> {
    git_object_bytes(git,"HEAD:./VERSION").context("Cannot inspect committed VERSION")
}

/// Read only an object ID, without configuration, identities, credentials or stderr.
pub(crate) fn head_in(root:&camino::Utf8Path,environment:&RunEnvironment)->Option<String> {
    let output=git_command_in(Some(root),environment).ok()?.current_dir(root)
        .args(["rev-parse","HEAD"]).output().ok()?;
    if !output.status.success(){return None;}
    let head=std::str::from_utf8(&output.stdout).ok()?.trim();
    if !matches!(head.len(),40|64)||!head.bytes().all(|byte|byte.is_ascii_hexdigit()){return None;}
    Some(head.to_owned())
}

fn owned_dev_transition_matches(root: &camino::Utf8Path, git: &SystemGit) -> anyhow::Result<bool> {
    let path = transition_path(root, git)?;
    crate::path_safety::resolve_path(&path, true)?;
    let metadata = match std::fs::metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    if !metadata.is_file() || metadata.len() > 4096 { return Ok(false); }
    let record: DevTransition = match serde_json::from_str(&crate::fs_utils::read_managed_text(path)?) {
        Ok(record) => record, Err(_) => return Ok(false),
    };
    crate::path_safety::resolve_path(root.join("VERSION").as_std_path(), true)?;
    Ok(record.schema == 1 && record.root == root.as_str()
        && record.head == git.run_cmd(&["rev-parse", "HEAD"])?
        && record.release.as_bytes() == git_file_bytes(git)?
        && record.development.as_bytes() == std::fs::read(root.join("VERSION"))?)
}

pub(crate) fn capture_dev_transition(root: &camino::Utf8Path, environment: &RunEnvironment,
    release: &crate::version::ProjVersion, development: &crate::version::ProjVersion) -> anyhow::Result<Option<DevTransition>> {
    let git = SystemGit { repo_path: root.to_owned(), environment: environment.clone() };
    let tracked = git.run_cmd(&["ls-tree", "HEAD", "--", "VERSION"])?;
    if tracked.is_empty() { return Ok(None); }
    anyhow::ensure!(tracked.starts_with("100644 blob ") || tracked.starts_with("100755 blob "), "Committed VERSION must be a regular file");
    let release = crate::version::version_file_content(release);
    anyhow::ensure!(release.as_bytes() == git_file_bytes(&git)?, "Git commit did not retain the planned release VERSION");
    Ok(Some(DevTransition { schema: 1, root: root.to_string(), head: git.run_cmd(&["rev-parse", "HEAD"])?,
        release, development: crate::version::version_file_content(development) }))
}

pub(crate) fn record_dev_transition(root: &camino::Utf8Path, environment: &RunEnvironment, record: &DevTransition) -> anyhow::Result<()> {
    let git = SystemGit { repo_path: root.to_owned(), environment: environment.clone() };
    anyhow::ensure!(record.head == git.run_cmd(&["rev-parse", "HEAD"])? && record.release.as_bytes() == git_file_bytes(&git)?,
        "Git state changed during end hooks; development transition was not applied");
    crate::fs_utils::write_managed_file(transition_path(root, &git)?, serde_json::to_vec(record)?)
}

#[derive(serde::Serialize,serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ReceiptWitness {schema:u32,project_identity:String,attempt_id:String,snapshot_hash:String,receipt_hash:String}

fn receipt_witness_path(root:&camino::Utf8Path,git:&SystemGit,id:&str)->anyhow::Result<std::path::PathBuf> {
    anyhow::ensure!(id.starts_with("attempt-")&&id.len()==32
        &&id.bytes().all(|byte|byte.is_ascii_alphanumeric()||byte==b'-'),"Invalid attempt identity");
    let control=transition_path(root,git)?;
    let parent=control.parent().context("Missing Git metadata directory")?;
    let parent=camino::Utf8Path::from_path(parent).context("Git metadata path must be UTF-8")?;
    crate::path_safety::validate_native_names(parent,&format!("projcli-attempt-receipts/{id}.json"),false)?;
    let path=parent.join("projcli-attempt-receipts").join(format!("{id}.json")).into_std_path_buf();
    crate::path_safety::resolve_path(&path,true)?;
    Ok(path)
}

/// Store one immutable local ownership witness outside Git staging, not a catalogue.
pub(crate) fn record_pending_receipt_in(root:&camino::Utf8Path,environment:&RunEnvironment,
    id:&str,snapshot:&str,receipt:&str)->anyhow::Result<()> {
    let git=SystemGit {repo_path:root.to_owned(),environment:environment.clone()};
    let path=receipt_witness_path(root,&git,id)?;
    std::fs::create_dir_all(path.parent().unwrap())?;
    crate::path_safety::resolve_path(&path,true)?;
    let witness=ReceiptWitness {schema:1,project_identity:blake3::hash(root.as_str().as_bytes()).to_hex().to_string(),
        attempt_id:id.into(),snapshot_hash:snapshot.into(),receipt_hash:receipt.into()};
    let mut file=std::fs::OpenOptions::new().write(true).create_new(true).open(path)
        .context("Cannot create pending Git receipt ownership witness")?;
    file.write_all(&serde_json::to_vec(&witness)?)?;
    file.sync_all()?;
    Ok(())
}

fn pending_receipt_matches(root:&camino::Utf8Path,git:&SystemGit,id:&str,snapshot:&str,receipt:&str)->anyhow::Result<bool> {
    let path=receipt_witness_path(root,git,id)?;
    let metadata=match std::fs::symlink_metadata(&path) {
        Ok(metadata)=>metadata,
        Err(error) if error.kind()==std::io::ErrorKind::NotFound=>return Ok(false),
        Err(error)=>return Err(error).context("Cannot inspect pending Git receipt witness"),
    };
    if !metadata.is_file()||metadata.len()>8192{return Ok(false);}
    let mut bytes=Vec::new();std::fs::File::open(path)?.take(8193).read_to_end(&mut bytes)?;
    if bytes.len()>8192{return Ok(false);}
    let witness:ReceiptWitness=match serde_json::from_slice(&bytes){Ok(witness)=>witness,Err(_)=>return Ok(false)};
    Ok(witness.schema==1&&witness.project_identity==blake3::hash(root.as_str().as_bytes()).to_hex().as_str()
        &&witness.attempt_id==id&&witness.snapshot_hash==snapshot&&witness.receipt_hash==receipt)
}
