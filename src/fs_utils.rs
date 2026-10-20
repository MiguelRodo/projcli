use std::fs;
use camino::Utf8Path;
use anyhow::Context;

/// Replaces a disjoint destination directory with an existing source directory.
/// Preserves existing README, CHANGELOG and ignore files during destination cleanup.
/// Rejects missing sources, overlapping identities and symlinked write destinations
/// before deletion. Copy/removal failures propagate; this is not atomic promotion.
pub fn dir_move_exact(source: &Utf8Path, dest: &Utf8Path) -> anyhow::Result<()> {
    crate::path_safety::validate_directory_move(source, dest)?;

    if dest.exists() {
        // Clear destination except protected files
        if dest.is_dir() {
            {
                for entry in fs::read_dir(dest.as_std_path()).context("Failed to read promotion destination")? {
                    let entry = entry.context("Failed to read promotion destination entry")?;
                    let entry_path_utf8 = camino::Utf8PathBuf::try_from(entry.path())
                        .context("Encountered non-UTF-8 path during directory iteration")?;

                    let name_str = entry_path_utf8.file_name()
                        .context("Failed to get file name from path")?;

                    // Protected File Exclusion Guard
                    if name_str == "CHANGELOG.md" || name_str == ".gitignore" || name_str == "README.md" {
                        continue;
                    }
                    let is_dir = entry.file_type()
                        .context("Failed to read file type")?
                        .is_dir();

                    if is_dir {
                        fs::remove_dir_all(entry_path_utf8.as_std_path())
                            .context("Failed to remove directory")?;
                    } else {
                        fs::remove_file(entry_path_utf8.as_std_path())
                            .context("Failed to remove file")?;
                    }
                }
            }
        }
    } else {
        fs::create_dir_all(dest.as_std_path()).context("Failed to create destination directory")?;
    }

    // Now copy everything from source to dest
    copy_dir_recursive(source, dest)?;
    fs::remove_dir_all(source.as_std_path()).context("Failed to remove source directory")?;

    Ok(())
}

/// Copies a directory tree, rejecting symlinked destination paths before file writes.
/// Filesystem errors propagate; callers must provide transaction/rollback boundaries.
pub fn copy_dir_recursive(src: &Utf8Path, dst: &Utf8Path) -> anyhow::Result<()> {
    if !src.exists() {
        return Ok(());
    }
    if !dst.exists() {
        fs::create_dir_all(dst.as_std_path()).context("Failed to create destination directory")?;
    }

    for entry in fs::read_dir(src.as_std_path()).context("Failed to read source directory")? {
        let entry = entry.context("Failed to read directory entry")?;
        let ty = entry.file_type().context("Failed to read file type")?;

        let entry_path_utf8 = camino::Utf8PathBuf::try_from(entry.path())
            .context("Encountered non-UTF-8 path during directory copy iteration")?;

        let file_name = entry_path_utf8.file_name().context("Failed to get file name from path")?;
        let dest_path = dst.join(file_name);

        if ty.is_dir() {
            copy_dir_recursive(&entry_path_utf8, &dest_path)?;
        } else {
            crate::path_safety::resolve_path(dest_path.as_std_path(), true)?;
            fs::copy(entry_path_utf8.as_std_path(), dest_path.as_std_path())
                .context("Failed to copy file")?;
        }
    }
    Ok(())
}
