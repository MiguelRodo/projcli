//! Per-build environment values. Never mutates the parent process environment.
use std::{collections::BTreeMap, ffi::{OsStr, OsString}, fs, process::Command};
use anyhow::Context;
use camino::Utf8Path;

// Intentionally no Debug implementation: values may include credentials.
#[derive(Clone)]
pub(crate) struct RunEnvironment {
    values: BTreeMap<OsString, OsString>,
}

impl RunEnvironment {
    pub(crate) fn inherited() -> Self {
        Self { values: std::env::vars_os().map(|(key, value)| (normalize_key(&key), value)).collect() }
    }

    pub(crate) fn resolve(explicit: Option<&str>, root: &Utf8Path) -> anyhow::Result<Self> {
        let mut environment = Self::inherited();
        if let Some(profile) = explicit { environment.set("PROJR_PROFILE", profile); }
        // The caller has selected one effective proj profile list before any file reads.
        let profiles = crate::profile::parse_active_profiles(environment.text("PROJR_PROFILE").unwrap_or_default());
        let mut files = vec![root.join("_environment.local")];
        files.extend(profiles.iter().map(|profile| root.join(format!("_environment-{profile}"))));
        files.push(root.join("_environment"));
        for file in files {
            if crate::yml::configuration_entry_exists(&file)? {
                for (key, value) in crate::env::read_environment_file(&file)? {
                    if environment.value(&key).is_none() { environment.set(&key, value); }
                }
            }
        }
        let required = root.join("_environment.required");
        if crate::yml::configuration_entry_exists(&required)? {
            let text = fs::read_to_string(&required).context("Cannot read required environment file")?;
            for (index, key) in crate::env::required_environment_keys(&text)? {
                if environment.value(&key).is_none() { anyhow::bail!("Required environment variable is missing (entry {} in {required})", index + 1); }
            }
        }
        Ok(environment)
    }

    pub(crate) fn value(&self, key: &str) -> Option<&OsStr> {
        self.values.get(&normalize_key(OsStr::new(key))).map(OsString::as_os_str)
    }

    pub(crate) fn text(&self, key: &str) -> Option<&str> { self.value(key).and_then(OsStr::to_str) }

    pub(crate) fn set(&mut self, key: &str, value: impl Into<OsString>) {
        self.values.insert(normalize_key(OsStr::new(key)), value.into());
    }

    pub(crate) fn command(&self, program: impl AsRef<OsStr>) -> Command {
        let mut command = Command::new(program);
        command.env_clear().envs(&self.values);
        if self.non_interactive() { command.stdin(std::process::Stdio::null()); }
        command
    }

    pub(crate) fn non_interactive(&self) -> bool {
        self.text(crate::NON_INTERACTIVE_ENV).is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true"))
    }
}

fn normalize_key(key: &OsStr) -> OsString {
    #[cfg(windows)] {
        use std::os::windows::ffi::{OsStrExt, OsStringExt};
        // Preserve non-Unicode native keys; normalize the conventional ASCII key alphabet.
        OsString::from_wide(&key.encode_wide().map(|value| if (97..=122).contains(&value) { value - 32 } else { value }).collect::<Vec<_>>())
    }
    #[cfg(not(windows))] { key.to_owned() }
}
