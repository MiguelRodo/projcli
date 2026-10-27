//! Bounded physical-file inventory shared by portable store envelopes.
use anyhow::Result;
use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fs};
fn path(value: impl AsRef<std::path::Path>) -> Result<Utf8PathBuf> {
    Utf8PathBuf::from_path_buf(value.as_ref().to_owned())
        .map_err(|_| anyhow::anyhow!("Store archive path is not UTF-8"))
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct File {
    pub(crate) hash: String,
    pub(crate) length: u64,
}
pub(crate) fn inventory(root: &Utf8Path) -> Result<BTreeMap<String, File>> {
    fn visit(
        root: &Utf8Path,
        current: &Utf8Path,
        depth: usize,
        work: &mut usize,
        result: &mut BTreeMap<String, File>,
    ) -> Result<()> {
        anyhow::ensure!(depth <= 64, "Xet export exceeds depth bound");
        for entry in fs::read_dir(current)? {
            *work += 1;
            anyhow::ensure!(*work <= 100_000, "Xet export exceeds entry bound");
            let entry = entry?;
            let item = path(entry.path())?;
            crate::path_safety::resolve_path(item.as_std_path(), true)?;
            let relative = item.strip_prefix(root)?.as_str().to_owned();
            crate::path_safety::portable_relative(&relative)?;
            let metadata = fs::symlink_metadata(&item)?;
            anyhow::ensure!(
                metadata.is_dir() || metadata.is_file(),
                "Xet export contains linked/special entry"
            );
            if metadata.is_dir() {
                visit(root, &item, depth + 1, work, result)?;
            } else {
                result.insert(
                    relative,
                    File {
                        hash: crate::cas::hash_file(&item)?,
                        length: metadata.len(),
                    },
                );
            }
        }
        Ok(())
    }
    let mut result = BTreeMap::new();
    visit(root, root, 0, &mut 0, &mut result)?;
    Ok(result)
}
