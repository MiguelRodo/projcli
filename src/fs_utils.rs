use std::fs;
use camino::Utf8Path;
use anyhow::Context;

/// Replaces a disjoint destination directory with an existing source directory.
/// Preserves existing README, CHANGELOG and ignore files during destination cleanup.
/// Rejects missing sources, overlapping identities and symlinked write destinations
/// before deletion. Use resolved parents: symlinked write paths, including parent
/// aliases, are rejected. Copy/removal failures propagate; this is not atomic promotion.
pub fn dir_move_exact(source: &Utf8Path, dest: &Utf8Path) -> anyhow::Result<()> {
    crate::path_safety::validate_directory_move(source, dest)?;
    validate_copy_source_tree(source)?;

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
    copy_dir_contents(source, dest)?;
    fs::remove_dir_all(source.as_std_path()).context("Failed to remove source directory")?;

    Ok(())
}

/// Copies a directory tree, rejecting symlinked destination paths before file writes.
/// Filesystem errors propagate; callers must provide transaction/rollback boundaries.
pub fn copy_dir_recursive(src: &Utf8Path, dst: &Utf8Path) -> anyhow::Result<()> {
    crate::path_safety::validate_directory_move(src, dst)?;
    validate_copy_source_tree(src)?;
    copy_dir_contents(src, dst)
}

fn copy_dir_contents(src: &Utf8Path, dst: &Utf8Path) -> anyhow::Result<()> {
    crate::path_safety::validate_directory_move(src, dst)?;
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
            copy_dir_contents(&entry_path_utf8, &dest_path)?;
        } else if ty.is_file() {
            crate::path_safety::resolve_path(entry_path_utf8.as_std_path(), true)?;
            crate::path_safety::resolve_path(dest_path.as_std_path(), true)?;
            fs::copy(entry_path_utf8.as_std_path(), dest_path.as_std_path())
                .context("Failed to copy file")?;
        } else { anyhow::bail!("Promotion source must contain only real files/directories: {entry_path_utf8}"); }
    }
    Ok(())
}

/// Revalidates every existing component immediately before a controlled write.
pub(crate) fn write_managed_file(path: impl AsRef<std::path::Path>, bytes: impl AsRef<[u8]>) -> anyhow::Result<()> {
    let path = path.as_ref();
    crate::path_safety::resolve_path(path, true)?;
    match fs::symlink_metadata(path) {
        Ok(metadata) => anyhow::ensure!(metadata.is_file(), "Managed write requires a regular file or absent target: {}", path.display()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
        Err(error) => return Err(error.into()),
    }
    fs::write(path, bytes).with_context(|| format!("Cannot write managed file {}", path.display()))
}

/// Replace a guarded source with a new inode in its own directory, preserving
/// existing permissions. Hard-linked aliases keep their original bytes.
/// Failed temporary writes/replacements are cleaned up by NamedTempFile.
pub(crate) fn replace_managed_file(path: &Utf8Path, bytes: impl AsRef<[u8]>) -> anyhow::Result<()> {
    use std::io::Write;
    crate::path_safety::resolve_path(path.as_std_path(), true)?;
    let metadata = fs::symlink_metadata(path)?;
    anyhow::ensure!(metadata.is_file(), "Managed replacement requires a regular file: {path}");
    let parent = path.parent().context("Managed replacement has no parent")?;
    crate::path_safety::resolve_path(parent.as_std_path(), true)?;
    let mut staged = tempfile::Builder::new().prefix(".proj-write-").tempfile_in(parent)?;
    staged.write_all(bytes.as_ref())?;
    fs::set_permissions(staged.path(), metadata.permissions())?;
    staged.flush()?;
    crate::path_safety::resolve_path(path.as_std_path(), true)?;
    anyhow::ensure!(fs::symlink_metadata(path)?.is_file(), "Managed replacement target changed: {path}");
    staged.persist(path).map_err(|error| error.error).context("Cannot replace prepared source file")?;
    Ok(())
}

pub(crate) fn read_managed_text(path: impl AsRef<std::path::Path>) -> anyhow::Result<String> {
    let path = path.as_ref();
    crate::path_safety::resolve_path(path, true)?;
    anyhow::ensure!(fs::symlink_metadata(path)?.is_file(), "Managed read requires a regular file: {}", path.display());
    fs::read_to_string(path).with_context(|| format!("Cannot read managed file {}", path.display()))
}

fn validate_copy_source_tree(source: &Utf8Path) -> anyhow::Result<()> {
    crate::path_safety::resolve_path(source.as_std_path(), true)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let path = camino::Utf8PathBuf::try_from(entry.path())?;
        if kind.is_dir() { validate_copy_source_tree(&path)?; }
        else if !kind.is_file() { anyhow::bail!("Promotion source must contain only real files/directories: {path}"); }
    }
    Ok(())
}

/// Make a private copied tree removable, without following links. Never use on
/// installed data or retained originals: Windows file flags may affect aliases.
pub(crate) fn prepare_owned_cleanup(path:&std::path::Path)->anyhow::Result<()> {
    let metadata=fs::symlink_metadata(path)?;
    anyhow::ensure!(!metadata.file_type().is_symlink(),"Linked private staging cleanup entry");
    if metadata.is_dir() {
        #[cfg(unix)] {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path,fs::Permissions::from_mode(metadata.permissions().mode()|0o700))?;
        }
        #[cfg(windows)] {
            let mut mode=metadata.permissions();mode.set_readonly(false);fs::set_permissions(path,mode)?;
        }
        for entry in fs::read_dir(path)? {prepare_owned_cleanup(&entry?.path())?;}
    } else {
        anyhow::ensure!(metadata.is_file(),"Special private staging cleanup entry");
        #[cfg(windows)] {
            let mut mode=metadata.permissions();mode.set_readonly(false);fs::set_permissions(path,mode)?;
        }
    }
    Ok(())
}

/// Read bounded regular bytes, rejecting linked path components and special files before open.
pub(crate) fn read_managed_bounded(path:&Utf8Path,limit:u64)->anyhow::Result<Vec<u8>> {
    use std::io::Read;
    crate::path_safety::resolve_path(path.as_std_path(), true)?;
    let metadata = fs::symlink_metadata(path)?;
    anyhow::ensure!(metadata.is_file() && metadata.len() <= limit, "Managed input must be a bounded regular file");
    let mut bytes=Vec::new();
    fs::File::open(path).with_context(||format!("Cannot read bounded managed file at {path}"))?.take(limit+1).read_to_end(&mut bytes)?;
    anyhow::ensure!(bytes.len() as u64 <= limit, "Managed input is too large");
    Ok(bytes)
}
