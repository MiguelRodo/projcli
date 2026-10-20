use anyhow::Context;
use crate::run_environment::RunEnvironment;
use std::process::Command;

use std::io::Write;

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
        let output = self.environment.command("git")
            .current_dir(&self.repo_path)
            .args(args)
            .output()
            .context("Failed to execute system git process")?;

        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
        } else {
            anyhow::bail!("{}", String::from_utf8_lossy(&output.stderr).trim())
        }
    }
}

impl GitProvider for SystemGit {
    fn is_available(&self) -> bool {
        // Quick check to see if the executable exists in PATH and runs
        self.environment.command("git").arg("--version").output().is_ok()
    }

    fn get_user_name(&self) -> Option<String> {
        self.run_cmd(&["config", "user.name"]).ok()
    }

    fn get_user_email(&self) -> Option<String> {
        self.run_cmd(&["config", "user.email"]).ok()
    }

    fn commit_all(&self, message: &str) -> anyhow::Result<()> {
        self.run_cmd(&["add", "-A"])?;
        self.run_cmd(&["commit", "-m", message])?;
        Ok(())
    }

    fn push(&self, remote: &str, branch: &str) -> anyhow::Result<()> {
        self.run_cmd(&["push", remote, branch])?;
        Ok(())
    }

    fn is_behind_remote(&self, remote: &str, branch: &str) -> anyhow::Result<bool> {
        // Fetch tracking info silently first
        let _ = self.run_cmd(&["fetch", remote]);

        // Count the commits the local branch is behind the remote tracking branch
        let remote_target = format!("{}/{}", remote, branch);
        let count_str = self.run_cmd(&["rev-list", "--count", &format!("HEAD..{}", remote_target)])?;

        let count: usize = count_str.parse().unwrap_or(0);
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
    let mut child = environment.command("git")
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
    if let Ok(output) = environment.command("gh").args(["auth", "token"]).output() {
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

pub fn execute_authenticated_git(args: &[&str], token: &str, current_dir: Option<&camino::Utf8Path>) -> anyhow::Result<()> {
    execute_authenticated_git_in(args, token, current_dir, &RunEnvironment::inherited())
}

pub(crate) fn execute_authenticated_git_in(args: &[&str], token: &str, current_dir: Option<&camino::Utf8Path>, environment: &RunEnvironment) -> anyhow::Result<()> {
    // Construct an inline script helper string that Git will execute to read the password.
    // Git credential helpers expect output formatted as key=value lines.
    let inline_helper = format!("!f() {{ echo \"password={}\"; }}; f", token);

    let mut cmd = environment.command("git");

    // Wipe out standard global credential helpers for this command context only
    cmd.arg("-c")
       .arg(format!("credential.helper={}", inline_helper))
       // Bind environment to fail fast instead of freezing on interactive prompts
       .env("GIT_TERMINAL_PROMPT", "0")
       // Append target arguments, e.g., ["push", "origin", "main"] or ["fetch"]
       .args(args);

    if let Some(dir) = current_dir {
        cmd.current_dir(dir);
    }

    let status = cmd.status().context("Failed to execute system Git subprocess")?;

    match status {
        s if s.success() => Ok(()),
        s => anyhow::bail!("Git command exited with failure status code: {}", s),
    }
}

/// Returns true if executing `git --version` succeeds.
pub fn is_git_installed() -> bool {
    is_git_installed_in(&RunEnvironment::inherited())
}

pub(crate) fn is_git_installed_in(environment: &RunEnvironment) -> bool {
    environment.command("git")
        .arg("--version")
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

/// Queries local, global, and system variables via `git config --get user.name` and `git config --get user.email`.
/// If either stdout buffer returns blank or throws an error, return an explicit error string detailing exactly what config parameter is missing and how the user can configure it.
pub fn check_git_profile(current_dir: Option<&camino::Utf8Path>) -> anyhow::Result<()> {
    check_git_profile_in(current_dir, &RunEnvironment::inherited())
}

pub(crate) fn check_git_profile_in(current_dir: Option<&camino::Utf8Path>, environment: &RunEnvironment) -> anyhow::Result<()> {
    let check_config = |key: &str| -> anyhow::Result<()> {
        let mut cmd = environment.command("git");
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
    use std::env;
    use crate::yml::GitEngine;

    #[test]
    fn test_system_git_fails_gracefully_without_path() {
        // Temporarily clear path variables to simulate an environment missing git
        let original_path = env::var("PATH").unwrap_or_default();
        unsafe { env::set_var("PATH", "") };

        let result = create_git_provider(GitEngine::Auto, camino::Utf8PathBuf::from("."));

        // Restore environment safety
        unsafe { env::set_var("PATH", original_path) };

        assert!(result.is_err());
        match result {
            Err(e) => assert_eq!(e.to_string(), "System Git executable could not be resolved in the current environment PATH."),
            Ok(_) => panic!("Expected an error when git is not in PATH"),
        }
    }
}
