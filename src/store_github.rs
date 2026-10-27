//! Shared immutable whole-store asset transfer. Backend readers prove store integrity.
use crate::github_release::GithubReleaseClient;
use anyhow::{Context, Result};
use camino::Utf8Path;
pub(crate) struct Download {
    directory: tempfile::TempDir,
}
impl Download {
    pub(crate) fn path(&self) -> camino::Utf8PathBuf {
        camino::Utf8PathBuf::try_from(self.directory.path().join("store.zip"))
            .expect("validated UTF-8 temporary path")
    }
}
/// Upload a new name, or leave an existing immutable candidate for validation.
pub(crate) fn upload(
    client: &GithubReleaseClient,
    tag: &str,
    name: &str,
    source: &Utf8Path,
    create: bool,
) -> Result<()> {
    let existing = client.assets_if_exists(tag)?.unwrap_or_default();
    let matches: Vec<_> = existing.iter().filter(|asset| asset.name == name).collect();
    anyhow::ensure!(
        matches.len() <= 1,
        "Ambiguous whole-store asset; no overwrite performed"
    );
    if matches.is_empty() {
        client.upload_asset(tag,name,source,create)
        .context("Store upload was not verified; keep prior reference and inspect remote state before retry")?;
    }
    Ok(())
}
/// Download to scratch and check trusted transport bytes before backend parsing.
pub(crate) fn download(
    project: &Utf8Path,
    client: &GithubReleaseClient,
    tag: &str,
    name: &str,
    hash: &str,
    length: u64,
    max_bytes: u64,
) -> Result<Download> {
    crate::cas::validate_digest(hash)?;
    anyhow::ensure!(length <= max_bytes, "Store exceeds download budget");
    let directory = crate::store_archive::scratch()?;
    let root = Utf8Path::from_path(directory.path()).context("Store download path is not UTF-8")?;
    let archive = root.join("store.zip");
    client.download_asset(project, tag, name, &archive, max_bytes)?;
    anyhow::ensure!(
        std::fs::metadata(&archive)?.len() == length,
        "Store transport length mismatch"
    );
    anyhow::ensure!(
        crate::cas::hash_file(&archive)? == hash,
        "Store transport digest mismatch"
    );
    Ok(Download { directory })
}
