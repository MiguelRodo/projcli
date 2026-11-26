//! projr's destination placeholders (`@version`, `@branch`, `@uuid`), resolved once
//! per build before hooks (SATVILab/projr@5f915ef `R/template.R`).
//!
//! A placeholder ends at the end of the text or before `-`, `_`, `.`, `/`, a space
//! or a backslash, so `archive-@branching` stays literal. Git metadata is read from
//! the checkout's `HEAD` file without starting Git.
use anyhow::Context;
use camino::{Utf8Path, Utf8PathBuf};
use std::io::Read;

const TOKENS: [&str; 3] = ["version", "branch", "uuid"];

/// Frozen placeholder values for one build, restore or validation pass.
#[derive(Clone, Debug)]
pub(crate) struct Templates {
    version: Option<String>,
    head: Option<String>,
    uuid: Option<String>,
}

impl Templates {
    /// Values for one build: its run version, the checkout `HEAD` and a fresh UUIDv4.
    pub(crate) fn for_build(root: &Utf8Path, version: &str) -> anyhow::Result<Self> {
        Ok(Self {
            version: Some(version.to_owned()),
            head: checkout_head(root),
            uuid: Some(uuid_v4()?),
        })
    }

    /// Values for a later lookup: no build UUID exists, so `@uuid` fails clearly.
    pub(crate) fn for_lookup(root: &Utf8Path, version: Option<String>) -> Self {
        Self {
            version,
            head: checkout_head(root),
            uuid: None,
        }
    }

    /// Inert values for configuration validation outside a build.
    pub(crate) fn validation() -> Self {
        Self {
            version: Some("v0.0.0".into()),
            head: Some("ref: refs/heads/branch".into()),
            uuid: Some("00000000-0000-4000-8000-000000000000".into()),
        }
    }

    /// The build UUID, when one was generated.
    pub(crate) fn uuid(&self) -> Option<&str> {
        self.uuid.as_deref()
    }

    fn value(&self, token: &str) -> anyhow::Result<String> {
        match token {
            "version" => {
                let version = self.version.as_deref().context(
                    "@version requires a project version; set one or run a production build",
                )?;
                Ok(if version.starts_with('v') {
                    version.to_owned()
                } else {
                    format!("v{version}")
                })
            }
            "branch" => {
                let head = self.head.as_deref().unwrap_or("");
                if is_object_id(head) {
                    return Ok(format!("__detached-{head}"));
                }
                Ok(crate::manifest_store::encode_component(&self.raw_branch()?))
            }
            _ => self.uuid.clone().context(
                "@uuid names a single build's destination; use the destination recorded for that build",
            ),
        }
    }

    /// The checked-out branch name, unencoded, for a GitHub `branch: "@branch"`.
    pub(crate) fn raw_branch(&self) -> anyhow::Result<String> {
        let head = self.head.as_deref().unwrap_or("");
        match head.strip_prefix("ref: refs/heads/") {
            Some(name) if !name.is_empty() => Ok(name.to_owned()),
            _ => anyhow::bail!(
                "@branch needs a named Git branch for this operation ({})",
                if head.is_empty() {
                    "no Git checkout was found"
                } else {
                    "HEAD is detached"
                }
            ),
        }
    }

    /// Replace every placeholder in `text`; text without placeholders is unchanged.
    pub(crate) fn expand(&self, text: &str) -> anyhow::Result<String> {
        let mut out = String::new();
        let mut rest = text;
        while let Some(at) = rest.find('@') {
            out.push_str(&rest[..at]);
            let after = &rest[at + 1..];
            let token = TOKENS.into_iter().find(|token| {
                after.starts_with(token)
                    && after[token.len()..]
                        .chars()
                        .next()
                        .is_none_or(|next| matches!(next, '-' | '_' | '.' | '/' | ' ' | '\\'))
            });
            match token {
                Some(token) => {
                    out.push_str(&self.value(token)?);
                    rest = &after[token.len()..];
                }
                None => {
                    out.push('@');
                    rest = after;
                }
            }
        }
        out.push_str(rest);
        Ok(out)
    }
}

fn is_object_id(text: &str) -> bool {
    matches!(text.len(), 40 | 64) && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn read_first_line(path: &Utf8Path) -> Option<String> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(4096)
        .read_to_end(&mut bytes)
        .ok()?;
    Some(
        String::from_utf8(bytes)
            .ok()?
            .lines()
            .next()?
            .trim_end()
            .to_owned(),
    )
}

/// First line of the checkout's HEAD, following a worktree `.git` file's `gitdir:`.
fn checkout_head(root: &Utf8Path) -> Option<String> {
    for directory in root.ancestors() {
        let git = directory.join(".git");
        let Ok(metadata) = std::fs::metadata(&git) else {
            continue;
        };
        let git_dir = if metadata.is_dir() {
            git
        } else {
            let pointer = read_first_line(&git)?;
            let pointer = Utf8PathBuf::from(pointer.strip_prefix("gitdir: ")?.trim());
            if pointer.is_absolute() {
                pointer
            } else {
                directory.join(pointer)
            }
        };
        return read_first_line(&git_dir.join("HEAD"));
    }
    None
}

fn uuid_v4() -> anyhow::Result<String> {
    use ring::rand::SecureRandom;
    let mut bytes = [0u8; 16];
    ring::rand::SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| anyhow::anyhow!("Cannot generate the build UUID"))?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn values(head: &str) -> Templates {
        Templates {
            version: Some("v1.2.3".into()),
            head: Some(head.into()),
            uuid: Some("u".into()),
        }
    }
    #[test]
    fn placeholders_end_at_projr_separators() {
        let t = values("ref: refs/heads/feature/foo");
        assert_eq!(t.expand("archive-@version").unwrap(), "archive-v1.2.3");
        assert_eq!(
            t.expand("archive-@branch").unwrap(),
            "archive-feature_2ffoo"
        );
        assert_eq!(
            t.expand("@branch/@version-@uuid").unwrap(),
            "feature_2ffoo/v1.2.3-u"
        );
        assert_eq!(
            t.expand("archive-@branching").unwrap(),
            "archive-@branching"
        );
        assert_eq!(t.expand("a@b@version").unwrap(), "a@bv1.2.3");
        assert_eq!(t.raw_branch().unwrap(), "feature/foo");
    }
    #[test]
    fn detached_and_missing_checkouts_follow_projr() {
        let sha = "a".repeat(40);
        let t = values(&sha);
        assert_eq!(t.expand("@branch").unwrap(), format!("__detached-{sha}"));
        assert!(t.raw_branch().is_err());
        let none = Templates {
            head: None,
            ..values("")
        };
        assert!(none.expand("@branch").is_err());
        assert_eq!(none.expand("literal").unwrap(), "literal");
        let lookup = Templates {
            uuid: None,
            ..values("")
        };
        assert!(lookup.expand("x-@uuid").is_err());
    }
    #[test]
    fn uuid_is_version_four() {
        let id = uuid_v4().unwrap();
        assert_eq!(id.len(), 36);
        assert_eq!(&id[14..15], "4");
        assert!(matches!(&id[19..20], "8" | "9" | "a" | "b"));
        assert_ne!(id, uuid_v4().unwrap());
    }
}
