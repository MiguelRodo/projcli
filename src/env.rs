use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use camino::Utf8Path;

/// A Scoped Transaction guard for environment variables.
///
/// Records the original state of the environment and automatically restores or unsets
/// modified keys when the guard goes out of scope, mimicking R's `on.exit()` behavior.
///
/// # Examples
///
/// ```rust
/// use proj::env::EnvGuard;
/// use camino::Utf8Path;
///
/// let mut guard = EnvGuard::new();
/// guard.track_and_set("DUMMY_VAR_TEST", "123");
/// assert_eq!(std::env::var("DUMMY_VAR_TEST").unwrap(), "123");
/// drop(guard);
/// assert!(std::env::var("DUMMY_VAR_TEST").is_err());
/// ```
pub struct EnvGuard {
    original_state: HashMap<String, Option<String>>,
}

impl Default for EnvGuard {
    fn default() -> Self {
        Self::new()
    }
}

impl EnvGuard {
    /// Creates a new, empty EnvGuard.
    pub fn new() -> Self {
        Self {
            original_state: HashMap::new(),
        }
    }

    /// Resolves active profiles by reading `QUARTO_PROFILE` and `PROJR_PROFILE`.
    /// Merges profiles with `QUARTO_PROFILE` taking priority, strips out "required",
    /// and removes duplicates while preserving evaluation order.
    pub fn resolve_profiles() -> Vec<String> {
        let mut profiles = Vec::new();
        let mut seen = std::collections::HashSet::new();

        let quarto_profiles = std::env::var("QUARTO_PROFILE").unwrap_or_default();
        let projr_profiles = std::env::var("PROJR_PROFILE").unwrap_or_default();

        let mut parse_and_add = |s: &str| {
            for p in s.split(|c| c == ',' || c == ';') {
                let p = p.trim();
                if !p.is_empty() && p != "required" && !seen.contains(p) {
                    seen.insert(p.to_string());
                    profiles.push(p.to_string());
                }
            }
        };

        parse_and_add(&quarto_profiles);
        parse_and_add(&projr_profiles);

        profiles
    }

    /// Parses a single line from an environment file.
    /// Returns `Some((key, value))` if the line is a valid assignment, `None` otherwise.
    fn parse_line(line: &str) -> Option<(String, String)> {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            return None;
        }

        if let Some((k, v)) = trimmed.split_once('=') {
            let key = k.trim().to_string();
            // Remove inline comments first (e.g. `value # comment`)
            let mut value_part = v;

            // Search for `#` that is preceded by whitespace
            let mut search_idx = 0;
            while let Some(relative_idx) = value_part[search_idx..].find('#') {
                let absolute_idx = search_idx + relative_idx;
                if absolute_idx == 0 || value_part[..absolute_idx].chars().last().unwrap().is_whitespace() {
                    value_part = &value_part[..absolute_idx];
                    break;
                }
                // Move search index past this '#'
                search_idx = absolute_idx + 1;
            }

            let mut value = value_part.trim();
            // Strip surrounding quotes
            if value.len() >= 2 && ((value.starts_with('"') && value.ends_with('"')) || (value.starts_with('\'') && value.ends_with('\''))) {
                value = &value[1..value.len() - 1];
            }
            return Some((key, value.to_string()));
        }
        None
    }

    /// Loads environment variables from the given file, setting them via `track_and_set`
    /// only if they are not already set in the current process.
    pub fn load_file(&mut self, path: &Utf8Path) {
        if let Ok(file) = File::open(path) {
            let reader = BufReader::new(file);
            for line in reader.lines().flatten() {
                if let Some((key, value)) = Self::parse_line(&line) {
                    if std::env::var(&key).is_err() {
                        self.track_and_set(&key, &value);
                    }
                }
            }
        }
    }

    /// Commences an environment block, parsing profiles and backing up state.
    pub fn activate(explicit_profile: Option<&str>, base_dir: &Utf8Path) -> anyhow::Result<Self> {
        Self::activate_inner(explicit_profile, base_dir, true)
    }

    /// Loads the usual environment cascade without writing project ignore files.
    ///
    /// Profile and required-variable rules match `activate`; missing required
    /// variables return an error. Dropping the guard restores the environment.
    ///
    /// ```rust
    /// let project = tempfile::tempdir().unwrap();
    /// let root = camino::Utf8Path::from_path(project.path()).unwrap();
    /// let guard = proj::EnvGuard::activate_for_preview(None, root).unwrap();
    /// drop(guard);
    /// assert!(!root.join(".gitignore").exists());
    /// ```
    pub fn activate_for_preview(explicit_profile: Option<&str>, base_dir: &Utf8Path) -> anyhow::Result<Self> {
        Self::activate_inner(explicit_profile, base_dir, false)
    }

    fn activate_inner(explicit_profile: Option<&str>, base_dir: &Utf8Path, update_ignores: bool) -> anyhow::Result<Self> {
        let mut guard = Self::new();

        // Temporarily handle PROJR_PROFILE swap if explicitly overridden for this build run
        if let Some(prof) = explicit_profile {
            guard.track_and_set("PROJR_PROFILE", prof);
        }

        let profiles = Self::resolve_profiles();

        // The cascade layers are parsed in decreasing priority.
        // However, because higher precedence files lock the variable, we can actually just evaluate them in highest to lowest priority order.
        // Wait, if we process them highest to lowest, and only set if `var(key).is_err()`, then the highest precedence file sets it,
        // and lower precedence files are ignored.
        // 1. _environment.local
        // 2. _environment-<profile> (sorted by profile precedence, which we get from `resolve_profiles`)
        // 3. _environment

        let local_env_file = base_dir.join("_environment.local");
        let mut files_to_load = Vec::new();
        files_to_load.push(local_env_file.clone());

        for profile in profiles {
            files_to_load.push(base_dir.join(format!("_environment-{}", profile)));
        }

        files_to_load.push(base_dir.join("_environment"));

        for file in files_to_load {
            if file.exists() {
                if update_ignores && file == local_env_file {
                    // Automatically add _environment.local to ignores
                    let _ = crate::ignore::add_manual_ignores(
                        base_dir,
                        &["_environment.local".to_string()],
                        true,
                        crate::ignore::IgnoreType::All,
                    );
                }
                guard.load_file(&file);
            }
        }

        // Validate _environment.required
        let required_file = base_dir.join("_environment.required");
        if required_file.exists() {
            if let Ok(file) = File::open(&required_file) {
                let reader = BufReader::new(file);
                for line in reader.lines().flatten() {
                    let trimmed = line.trim();
                    if trimmed.is_empty() || trimmed.starts_with('#') {
                        continue;
                    }

                    let key = if let Some((k, _)) = trimmed.split_once('=') {
                        k.trim().to_string()
                    } else {
                        // Extract just the key if there is no assignment
                        let mut k_part = trimmed;
                        if let Some(comment_idx) = k_part.find('#') {
                            let before_comment = &k_part[..comment_idx];
                            if before_comment.chars().last().map_or(false, |c| c.is_whitespace()) {
                                k_part = before_comment;
                            }
                        }
                        k_part.trim().to_string()
                    };

                    if !key.is_empty() && std::env::var(&key).is_err() {
                        anyhow::bail!(
                            "Required environment variable '{}' is missing from the environment.",
                            key
                        );
                    }
                }
            }
        }

        Ok(guard)
    }

    /// Safely updates `std::env` while snapshotting what it looked like before mutation.
    pub fn track_and_set(&mut self, key: &str, value: &str) {
        if !self.original_state.contains_key(key) {
            let old_val = std::env::var(key).ok();
            self.original_state.insert(key.to_string(), old_val);
        }
        unsafe {
            std::env::set_var(key, value);
        }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        // Runs automatically when the guard goes out of scope (even on function panic!)
        for (key, original_value) in &self.original_state {
            unsafe {
                match original_value {
                    Some(old_val) => std::env::set_var(key, old_val),
                    None => std::env::remove_var(key),
                }
            }
        }
    }
}

/// Environment flag inherited by managed children for explicit noninteractive runs.
///
/// ```rust
/// assert_eq!(proj::NON_INTERACTIVE_ENV, "PROJ_NON_INTERACTIVE");
/// ```
pub const NON_INTERACTIVE_ENV: &str = "PROJ_NON_INTERACTIVE";

/// Returns whether the inherited noninteractive flag is `true` or `1`.
///
/// Missing or other values leave ordinary interaction behavior enabled. This
/// reads process state without changing it; it does not isolate child filesystems.
///
/// ```rust
/// let enabled: bool = proj::is_non_interactive();
/// let _ = enabled;
/// ```
pub fn is_non_interactive() -> bool {
    std::env::var(NON_INTERACTIVE_ENV)
        .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}
