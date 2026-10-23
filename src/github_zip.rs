//! Projr-compatible flat asset names and guarded ZIP preparation/extraction.
//! No HTTP publication or configured source-selection policy lives here.
use std::{collections::{BTreeSet,BTreeMap}, fs, io::{Read, Write}};
use anyhow::Context;
use camino::{Utf8Path, Utf8PathBuf};

/// Ordinary GitHub asset layout; CAS is a separate structure.
#[derive(Clone, Copy)]
pub enum GithubStructure {
    /// One label ZIP, with remote metadata advertising its version.
    Latest,
    /// One label/version ZIP per archived version.
    Archive,
}

/// Builds the exact flat projr asset name (remote.R525-560).
/// A missing/empty prefix always uses label, even when append_label is false.
/// Archive versions keep their numeric spelling and gain exactly one leading v.
/// The empty variant appends -empty before .zip. Portable names are required.
pub fn directory_asset_name(prefix: Option<&str>, append_label: bool, label: &str,
    structure: GithubStructure, version: Option<&str>, empty: bool) -> anyhow::Result<String> {
    crate::path_safety::validate_directory_label(label)?;
    let mut name=match prefix.filter(|value| !value.is_empty()) {
        Some(prefix) => {
            crate::path_safety::validate_directory_label(prefix)?;
            if append_label {format!("{prefix}-{label}")} else {prefix.to_owned()}
        },
        None => label.to_owned(),
    };
    if matches!(structure,GithubStructure::Archive) {
        let version=version.context("Archive asset naming requires a version")?;
        let bare=version.strip_prefix('v').unwrap_or(version);
        anyhow::ensure!(bare.split(['.','-']).count()>=2 && bare.split(['.','-'])
            .all(|part| !part.is_empty() && part.bytes().all(|byte|byte.is_ascii_digit())),
            "Archive version must have numeric components");
        name.push_str(&format!("-v{bare}"));
    }
    if empty {name.push_str("-empty");}
    name.push_str(".zip");Ok(name)
}

/// The three shared metadata ZIP assets implemented by pinned projr.
#[derive(Clone, Copy)]
pub enum MetadataKind {
    /// manifest.zip containing manifest.csv.
    Manifest,
    /// VERSION.zip containing VERSION.
    Version,
    /// CHANGELOG.zip containing CHANGELOG.md.
    Changelog,
}
impl MetadataKind {
    /// Exact asset and member names; metadata names have no directory prefix.
    pub fn names(self) -> (&'static str,&'static str) {
        match self {
            Self::Manifest => ("manifest.zip","manifest.csv"),
            Self::Version => ("VERSION.zip","VERSION"),
            Self::Changelog => ("CHANGELOG.zip","CHANGELOG.md"),
        }
    }
}

fn portable_member(name:&str)->anyhow::Result<Utf8PathBuf> {
    anyhow::ensure!(!name.is_empty() && !name.starts_with('/') && !name.contains('\\'),
        "ZIP member must be a relative portable path");
    let name=name.strip_suffix('/').unwrap_or(name);
    for part in name.split('/') {
        crate::path_safety::validate_directory_label(part)?;
        let stem=part.split('.').next().unwrap().to_ascii_uppercase();
        anyhow::ensure!(!matches!(stem.as_str(),"CON"|"PRN"|"AUX"|"NUL"|"CONIN$"|"CONOUT$"|"CLOCK$")
            && !(stem.len()==4 && (stem.starts_with("COM")||stem.starts_with("LPT"))
                && matches!(stem.as_bytes()[3],b'1'..=b'9'))
            && !["COM","LPT"].iter().any(|prefix| matches!(stem.strip_prefix(prefix),Some("\u{b9}"|"\u{b2}"|"\u{b3}"))),
            "ZIP member contains a reserved native device name");
    }
    Ok(name.into())
}

fn write_members(source:&Utf8Path, members:&[(String,String)], empty:bool)->anyhow::Result<tempfile::NamedTempFile> {
    let mut output=tempfile::Builder::new().prefix("proj-github-").suffix(".zip").tempfile_in(fs::canonicalize(std::env::temp_dir())?)?;
    {
        let mut writer=zip::ZipWriter::new(output.as_file_mut());
        let options=zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        if empty { writer.start_file("projr-empty",options)?; }
        // Preserve nonempty parent-directory privacy, without adding empty dirs.
        let mut directories=BTreeSet::new();
        for (relative,_) in members {
            let mut parent=Utf8Path::new(relative).parent();
            while let Some(path)=parent.filter(|path| !path.as_str().is_empty()) {
                directories.insert(path.to_owned());parent=path.parent();
            }
        }
        for relative in directories {
            let path=source.join(&relative);
            crate::path_safety::resolve_path(path.as_std_path(),true)?;
            anyhow::ensure!(fs::symlink_metadata(&path)?.is_dir(),"ZIP parent must be a real directory");
            #[cfg(unix)]
            let options={
                use std::os::unix::fs::PermissionsExt;
                options.unix_permissions(fs::metadata(path)?.permissions().mode()&0o777)
            };
            writer.add_directory(format!("{relative}/"),options)?;
        }
        for (relative,expected) in members {
            portable_member(relative)?;
            let path=source.join(relative);
            crate::path_safety::resolve_path(path.as_std_path(),true)?;
            anyhow::ensure!(fs::symlink_metadata(&path)?.is_file(),"ZIP source must be a regular file");
            let mut input=fs::File::open(&path)?;
            let mut digest=md5::Context::new();
            #[cfg(unix)]
            let options={
                use std::os::unix::fs::PermissionsExt;
                options.unix_permissions(input.metadata()?.permissions().mode()&0o777)
            };
            writer.start_file(relative,options)?;
            let mut buffer=[0;65536];
            loop {
                let count=input.read(&mut buffer)?;if count==0 {break;}
                digest.consume(&buffer[..count]);writer.write_all(&buffer[..count])?;
            }
            anyhow::ensure!(format!("{:x}",digest.finalize())==*expected,"ZIP source changed after inventory");
        }
        writer.finish()?;
    }
    output.as_file_mut().flush()?;Ok(output)
}

/// Creates a private ZIP with source-relative members and no enclosing directory.
/// Empty directories produce the projr-empty sentinel. Existing streaming inventory
/// rejects links/special files; copied bytes must match that inventory. Empty
/// subdirectories are omitted. Nonempty parent directory entries preserve Unix
/// privacy modes, a safety addition to projr's file-list packaging.
/// Returns the private ZIP and whether it represents an empty source.
/// The caller owns cleanup and decides the final asset name/publication policy.
pub fn directory_zip(source:&Utf8Path)->anyhow::Result<(tempfile::NamedTempFile,bool)> {
    crate::path_safety::resolve_path(source.as_std_path(),true)?;
    anyhow::ensure!(fs::symlink_metadata(source)?.is_dir(),"ZIP source must be a real directory");
    let members=crate::manifest::inventory_directory(source,false)?;
    let empty=members.is_empty();
    let zip=write_members(source,&members,empty)?;
    anyhow::ensure!(crate::manifest::inventory_directory(source,false)?==members,
        "ZIP source inventory changed during preparation");
    Ok((zip,empty))
}

/// Packages exactly the canonical metadata member from source, not other files.
/// This preserves bytes; parsing/merging VERSION/CSV history remains the caller's
/// responsibility. Missing, linked or special metadata sources are errors.
pub fn metadata_zip(source:&Utf8Path,kind:MetadataKind)->anyhow::Result<tempfile::NamedTempFile> {
    let (_,member)=kind.names();
    let path=source.join(member);
    crate::path_safety::resolve_path(path.as_std_path(),true)?;
    anyhow::ensure!(fs::symlink_metadata(&path)?.is_file(),"Metadata source must be a regular file");
    let mut input=fs::File::open(path)?;let mut digest=md5::Context::new();let mut buffer=[0;65536];
    loop {let count=input.read(&mut buffer)?;if count==0 {break;}digest.consume(&buffer[..count]);}
    write_members(source,&[(member.into(),format!("{:x}",digest.finalize()))],false)
}

/// Private extracted tree with explicit, permission-aware cleanup.
/// Call close to observe cleanup failures. Drop restores owner access to owned
/// directories before best-effort cleanup, without following links.
pub struct ExtractedZip { directory: Option<tempfile::TempDir> }
impl ExtractedZip {
    /// Location of the owned private tree; contents are not installed yet.
    pub fn path(&self)->&std::path::Path {self.directory.as_ref().unwrap().path()}
    /// Restores directory owner access for cleanup, removes the private tree,
    /// and reports any retained path instead of silently losing the outcome.
    pub fn close(mut self)->anyhow::Result<()> {
        let directory=self.directory.take().unwrap();let path=directory.path().to_owned();
        if let Err(error)=writable_owned_directories(&path) {
            let retained=directory.keep();
            anyhow::bail!("Cannot prepare private extraction cleanup: {error:#}; retained at {}",retained.display());
        }
        directory.close().with_context(||format!("Private extraction cleanup failed at {}",path.display()))
    }
}
impl Drop for ExtractedZip {
    fn drop(&mut self) {
        if let Some(directory)=&self.directory {let _=writable_owned_directories(directory.path());}
    }
}
fn writable_owned_directories(path:&std::path::Path)->anyhow::Result<()> {
    let metadata=fs::symlink_metadata(path)?;
    if metadata.is_dir()&&!metadata.file_type().is_symlink() {
        #[cfg(unix)] {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path,fs::Permissions::from_mode(metadata.permissions().mode()|0o700))?;
        }
        for entry in fs::read_dir(path)? {
            let entry=entry?;
            if entry.file_type()?.is_dir() {writable_owned_directories(&entry.path())?;}
        }
    }
    Ok(())
}

/// Extracts into a newly owned private temporary directory, never an existing tree.
/// Enforces a total expanded-byte limit,100000member ceiling and CRC checks.
/// Rejects traversal, absolute/backslash/native-alias paths, duplicates/case aliases,
/// encrypted entries, links/special entries and parent/file collisions.
/// It does not install into a project, interpret an empty marker or verify CSV.
/// The caller must validate the expected contents before guarded installation.
pub fn extract_zip(archive:&Utf8Path,max_bytes:u64)->anyhow::Result<ExtractedZip> {
    crate::path_safety::resolve_path(archive.as_std_path(),true)?;
    anyhow::ensure!(fs::symlink_metadata(archive)?.is_file(),"ZIP archive must be a regular file");
    let input=fs::File::open(archive)?;let mut zip=zip::ZipArchive::new(input)?;
    anyhow::ensure!(zip.len()<=100000,"ZIP exceeds member limit");
    let output=ExtractedZip {directory:Some(tempfile::Builder::new().prefix("proj-unzip-").tempdir_in(fs::canonicalize(std::env::temp_dir())?)?)};
    let root=Utf8Path::from_path(output.path()).context("ZIP temporary path must be UTF-8")?;
    let result=(|| -> anyhow::Result<()> {
    let mut names=BTreeSet::new();let mut prefixes=BTreeMap::<String,(String,bool)>::new();let mut total=0u64;
    #[cfg(unix)]
    let mut directory_modes=Vec::new();
    for index in 0..zip.len() {
        let mut member=zip.by_index(index)?;
        let name=member.name()?.into_owned();let relative=portable_member(&name)?;
        anyhow::ensure!(names.insert(relative.as_str().to_uppercase()),"ZIP contains duplicate/native-alias members");
        anyhow::ensure!(!member.encrypted()&&!member.is_symlink(),"ZIP contains an encrypted/linked member");
        let mode=member.unix_mode().unwrap_or(0);
        anyhow::ensure!(matches!(mode&0o170000,0|0o040000|0o100000),"ZIP contains a special member");
        let parts:Vec<_>=relative.as_str().split('/').collect();
        for end in 1..=parts.len() {
            let prefix=parts[..end].join("/");
            let directory=end<parts.len()||member.is_dir();
            if let Some((previous,was_directory))=prefixes.insert(prefix.to_uppercase(),(prefix.clone(),directory)) {
                anyhow::ensure!(prefix==previous && directory && was_directory,
                    "ZIP contains conflicting member paths or native parent aliases");
            }
        }
        crate::path_safety::validate_native_names(root,relative.as_str(),false)?;
        let destination=root.join(&relative);
        if member.is_dir() {
            anyhow::ensure!(member.size()==0,"ZIP directory member has a payload");
            let mut probe=[0];
            anyhow::ensure!(member.read(&mut probe)?==0,"ZIP directory member has a payload");
            fs::create_dir_all(&destination)?;
            crate::path_safety::validate_native_names(root,relative.as_str(),false)?;
            #[cfg(unix)]
            if mode!=0 {directory_modes.push((destination,mode&0o777));}
            continue;
        }
        total=total.checked_add(member.size()).context("ZIP expanded size overflow")?;
        anyhow::ensure!(total<=max_bytes,"ZIP exceeds expanded-byte limit");
        fs::create_dir_all(destination.parent().context("ZIP member has no parent")?)?;
        // Filesystems may rewrite Unicode spelling during creation. Reject that
        // immediately rather than accepting a later alias as a different parent.
        crate::path_safety::validate_native_names(root,relative.as_str(),false)?;
        // create_new catches file/directory collisions even on native aliases.
        let mut file=fs::OpenOptions::new().write(true).create_new(true).open(&destination)?;
        let expected=member.size();
        let actual=std::io::copy(&mut (&mut member).take(expected),&mut file)?;
        anyhow::ensure!(actual==expected,"ZIP member size mismatch");
        let mut probe=[0];
        anyhow::ensure!(member.read(&mut probe)?==0,"ZIP member exceeds advertised size");
        #[cfg(unix)] {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&destination,fs::Permissions::from_mode(if mode==0 {0o600}else{mode&0o777}))?;
        }
    }
    #[cfg(unix)] {
        use std::os::unix::fs::PermissionsExt;
        directory_modes.sort_by_key(|(path,_)|std::cmp::Reverse(path.components().count()));
        for (path,mode) in directory_modes {fs::set_permissions(path,fs::Permissions::from_mode(mode))?;}
    }
    Ok(())
    })();
    if let Err(error)=result {
        return match output.close() {
            Ok(())=>Err(error),
            Err(cleanup)=>Err(error.context(format!("Extraction failed; cleanup also failed: {cleanup:#}"))),
        };
    }
    Ok(output)
}

/// Downloads and extracts an explicitly selected directory ZIP before invoking
/// the existing guarded merge/clear installer. Compressed/expanded byte limits
/// are independent. Empty variants must contain only an empty projr-empty file.
/// Destination-only files survive unless clear is explicit. Publication metadata,
/// configured source lookup and historical CSV verification are not inferred.
pub fn download_directory(client:&crate::github_release::GithubReleaseClient,
    root:&Utf8Path,tag:&str,name:&str,destination:&Utf8Path,max_bytes:u64,
    max_expanded_bytes:u64,clear:bool,empty:bool)->anyhow::Result<crate::github_release::ReleaseAsset> {
    let transfer=tempfile::Builder::new().prefix("proj-github-transfer-").tempdir_in(fs::canonicalize(std::env::temp_dir())?)?;
    let result=(|| {
        let transfer_root=Utf8Path::from_path(transfer.path()).context("Transfer path must be UTF-8")?;
        crate::local_remote::validate_download_destination(root,transfer_root,destination)?;
        let archive=transfer_root.join("asset.zip");
        let asset=client.download_asset(root,tag,name,&archive,max_bytes)?;
        let extracted=extract_zip(&archive,max_expanded_bytes)?;
        let installed=(|| -> anyhow::Result<()> {
            let extracted_root=Utf8Path::from_path(extracted.path()).context("Extraction path must be UTF-8")?;
            if empty {
                let entries:Vec<_>=fs::read_dir(extracted_root)?.collect::<Result<_,_>>()?;
                let marker=extracted_root.join("projr-empty");
                anyhow::ensure!(entries.len()==1 && marker.is_file() && fs::metadata(&marker)?.len()==0,
                    "Empty ZIP variant must contain only an empty projr-empty sentinel");
                fs::remove_file(marker)?;
            }
            crate::local_remote::install_directory(root,extracted_root,destination,clear)?;
            Ok(())
        })();
        let cleanup=extracted.close();
        match (installed,cleanup) {
            (Ok(()),Ok(()))=>Ok(asset),
            (Ok(()),Err(cleanup))=>Err(cleanup.context("Directory installed, but extraction cleanup failed")),
            (Err(error),Ok(()))=>Err(error),
            (Err(error),Err(cleanup))=>Err(error.context(format!("Transfer failed; extraction cleanup also failed: {cleanup:#}"))),
        }
    })();
    let cleanup_path=transfer.path().to_owned();
    let cleanup=transfer.close().with_context(||format!("Transfer staging cleanup failed at {}",cleanup_path.display()));
    match (result,cleanup) {
        (Ok(asset),Ok(()))=>Ok(asset),
        (Ok(_),Err(cleanup))=>Err(cleanup.context("Directory installed, but transfer cleanup failed")),
        (Err(error),Ok(()))=>Err(error),
        (Err(error),Err(cleanup))=>Err(error.context(format!("Transfer failed; staging cleanup also failed: {cleanup:#}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_projr_flat_and_metadata_names() {
        assert_eq!(directory_asset_name(None,false,"output",GithubStructure::Latest,None,false).unwrap(),"output.zip");
        assert_eq!(directory_asset_name(Some("analysis"),true,"output",GithubStructure::Archive,Some("1.2.3-4"),true).unwrap(),"analysis-output-v1.2.3-4-empty.zip");
        assert_eq!(directory_asset_name(Some("analysis"),false,"output",GithubStructure::Archive,Some("v1.2.3"),false).unwrap(),"analysis-v1.2.3.zip");
        assert!(directory_asset_name(None,true,"../output",GithubStructure::Latest,None,false).is_err());
        assert_eq!(MetadataKind::Manifest.names(),("manifest.zip","manifest.csv"));
        assert_eq!(MetadataKind::Version.names(),("VERSION.zip","VERSION"));
        assert_eq!(MetadataKind::Changelog.names(),("CHANGELOG.zip","CHANGELOG.md"));
    }
}

