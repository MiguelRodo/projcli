use std::{fs,io::{Read,Write}};
use camino::{Utf8Path,Utf8PathBuf};
use proj::github_zip::{directory_zip,metadata_zip,extract_zip,MetadataKind};
fn path(temp:&tempfile::TempDir)->Utf8PathBuf {Utf8PathBuf::from_path_buf(temp.path().canonicalize().unwrap()).unwrap()}
fn members(file:&std::fs::File)->Vec<(String,Vec<u8>)> {
    let mut zip=zip::ZipArchive::new(file.try_clone().unwrap()).unwrap();let mut result=Vec::new();
    for index in 0..zip.len() {let mut member=zip.by_index(index).unwrap();let name=member.name().unwrap().into_owned();
        let mut bytes=Vec::new();member.read_to_end(&mut bytes).unwrap();result.push((name,bytes));}
    result.sort();result
}
fn archive(entries:&[(&str,&[u8])])->tempfile::NamedTempFile {
    let mut file=tempfile::NamedTempFile::new_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    {let mut zip=zip::ZipWriter::new(file.as_file_mut());for (name,bytes) in entries {
        zip.start_file(*name,zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored)).unwrap();zip.write_all(bytes).unwrap();
    }zip.finish().unwrap();}file
}
#[test]
fn source_relative_hidden_unicode_empty_and_exact_metadata_members() {
    let temp=tempfile::tempdir().unwrap();let root=path(&temp);
    fs::create_dir(root.join("nested")).unwrap();fs::write(root.join("nested/report-\u{e9}.txt"),"report").unwrap();
    fs::write(root.join(".hidden"),b"\0binary").unwrap();fs::create_dir(root.join("empty-dir")).unwrap();
    let (zip,empty)=directory_zip(&root).unwrap();assert!(!empty);
    assert_eq!(members(zip.as_file()),vec![(".hidden".into(),b"\0binary".to_vec()),("nested/".into(),vec![]),("nested/report-\u{e9}.txt".into(),b"report".to_vec())]);
    let extracted=extract_zip(Utf8Path::from_path(zip.path()).unwrap(),1024).unwrap();
    assert_eq!(fs::read(extracted.path().join("nested/report-\u{e9}.txt")).unwrap(),b"report");
    for (kind,name) in [(MetadataKind::Manifest,"manifest.csv"),(MetadataKind::Version,"VERSION"),(MetadataKind::Changelog,"CHANGELOG.md")] {
        fs::write(root.join(name),format!("exact-{name}\r\n")).unwrap();
        let zip=metadata_zip(&root,kind).unwrap();
        assert_eq!(members(zip.as_file()),vec![(name.into(),format!("exact-{name}\r\n").into_bytes())]);
    }
    let blank=tempfile::tempdir().unwrap();let (zip,empty)=directory_zip(&path(&blank)).unwrap();
    assert!(empty);assert_eq!(members(zip.as_file()),vec![("projr-empty".into(),vec![])]);
}
#[test]
fn malformed_paths_aliases_duplicates_and_expansion_limits_fail_privately() {
    for entries in [
        vec![("../outside",b"bad".as_slice())],
        vec![("/outside",b"bad".as_slice())],
        vec![("C:/outside",b"bad".as_slice())],
        vec![("folder\\outside",b"bad".as_slice())],
        vec![("CON.txt",b"bad".as_slice())],
        vec![("VERSION.",b"bad".as_slice())],
        vec![("same",b"one".as_slice()),("SAME",b"two".as_slice())],
        vec![("Folder/a",b"one".as_slice()),("folder/b",b"two".as_slice())],
        vec![("parent",b"one".as_slice()),("parent/child",b"two".as_slice())],
    ] {
        let zip=archive(&entries);
        assert!(extract_zip(Utf8Path::from_path(zip.path()).unwrap(),100).is_err(),"{entries:?}");
    }
    let zip=archive(&[("big",b"more than allowed")]);
    assert!(extract_zip(Utf8Path::from_path(zip.path()).unwrap(),3).is_err());
}
#[test]
fn crc_corruption_and_zip_symlink_are_rejected() {
    let mut zip=archive(&[("payload",b"unique-payload-12345")]);
    let bytes=fs::read(zip.path()).unwrap();let position=bytes.windows(b"unique-payload-12345".len()).position(|part|part==b"unique-payload-12345").unwrap();
    let mut corrupt=bytes;corrupt[position]^=1;fs::write(zip.path(),corrupt).unwrap();
    assert!(extract_zip(Utf8Path::from_path(zip.path()).unwrap(),100).is_err());
    {let mut writer=zip::ZipWriter::new(zip.as_file_mut());writer.add_symlink("link","../outside",zip::write::SimpleFileOptions::default()).unwrap();writer.finish().unwrap();}
    assert!(extract_zip(Utf8Path::from_path(zip.path()).unwrap(),100).is_err());
}
#[cfg(unix)]
#[test]
fn source_links_and_fifos_are_rejected() {
    use std::os::unix::fs::symlink;
    let source=tempfile::tempdir().unwrap();let root=path(&source);
    let outside=tempfile::NamedTempFile::new_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    symlink(outside.path(),root.join("linked")).unwrap();
    assert!(directory_zip(&root).is_err());fs::remove_file(root.join("linked")).unwrap();
    assert!(std::process::Command::new("mkfifo").arg(root.join("fifo")).status().unwrap().success());
    assert!(directory_zip(&root).is_err());
}

#[cfg(unix)]
#[test]
fn private_directory_modes_survive_packaging_and_extraction() {
    use std::os::unix::fs::PermissionsExt;
    let source=tempfile::tempdir().unwrap();let root=path(&source);
    fs::create_dir(root.join("private")).unwrap();
    fs::set_permissions(root.join("private"),fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(root.join("private/data"),"private").unwrap();
    let (zip,_)=directory_zip(&root).unwrap();
    let extracted=extract_zip(Utf8Path::from_path(zip.path()).unwrap(),100).unwrap();
    assert_eq!(fs::metadata(extracted.path().join("private")).unwrap().permissions().mode()&0o777,0o700);
}
#[test]
fn native_unicode_parent_aliases_are_rejected_when_filesystem_aliases_them() {
    let root=tempfile::tempdir().unwrap();let base=path(&root);
    let first="caf\u{e9}";let second="cafe\u{301}";
    fs::create_dir(base.join(first)).unwrap();
    if base.join(second).exists() {
        let first=format!("{first}/a");let second=format!("{second}/b");
        let zip=archive(&[(&first,b"a"),(&second,b"b")]);
        assert!(extract_zip(Utf8Path::from_path(zip.path()).unwrap(),100).is_err());
    }
}

#[test]
fn directory_payload_crc_and_understated_member_headers_are_rejected() {
    let zip=archive(&[("directory/",b"payload")]);
    assert!(extract_zip(Utf8Path::from_path(zip.path()).unwrap(),100).is_err());
    let zip=archive(&[("directory/",b"")]);
    let mut bytes=fs::read(zip.path()).unwrap();
    let headers:Vec<_>=bytes.windows(4).enumerate().filter_map(|(i,p)|match p {
        b"PK\x03\x04"=>Some((i,14)),b"PK\x01\x02"=>Some((i,16)),_=>None}).collect();
    for (offset,field) in headers {bytes[offset+field..offset+field+4].copy_from_slice(&1u32.to_le_bytes());}
    fs::write(zip.path(),bytes).unwrap();
    assert!(extract_zip(Utf8Path::from_path(zip.path()).unwrap(),100).is_err());
    let zip=archive(&[("first",b"oversized-first"),("second",b"oversized-second")]);
    let mut bytes=fs::read(zip.path()).unwrap();
    let headers:Vec<_>=bytes.windows(4).enumerate().filter_map(|(i,p)|match p {
        b"PK\x03\x04"=>Some((i,22)),b"PK\x01\x02"=>Some((i,24)),_=>None}).collect();
    for (offset,field) in headers {bytes[offset+field..offset+field+4].copy_from_slice(&1u32.to_le_bytes());}
    fs::write(zip.path(),bytes).unwrap();
    assert!(extract_zip(Utf8Path::from_path(zip.path()).unwrap(),2).is_err());
}

#[cfg(unix)]
#[test]
fn readonly_extraction_directories_cleanup_explicitly_and_on_drop() {
    use std::os::unix::fs::PermissionsExt;
    for mode in [0o555,0o000] {
        let zip=tempfile::NamedTempFile::new_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        {let mut writer=zip::ZipWriter::new(zip.as_file());
            writer.add_directory("restricted/",zip::write::SimpleFileOptions::default().unix_permissions(mode)).unwrap();
            writer.start_file("restricted/data",zip::write::SimpleFileOptions::default()).unwrap();writer.write_all(b"bytes").unwrap();writer.finish().unwrap();}
        let extracted=extract_zip(Utf8Path::from_path(zip.path()).unwrap(),100).unwrap();
        let path=extracted.path().to_owned();
        assert_eq!(fs::metadata(path.join("restricted")).unwrap().permissions().mode()&0o777,mode);
        extracted.close().unwrap();assert!(!path.exists());
        let extracted=extract_zip(Utf8Path::from_path(zip.path()).unwrap(),100).unwrap();let path=extracted.path().to_owned();
        drop(extracted);assert!(!path.exists());
    }
}
#[cfg(windows)]
#[test]
fn native_short_parent_alias_is_rejected_when_available() {
    let temp=tempfile::tempdir().unwrap();let root=path(&temp);
    fs::create_dir(root.join("LongDirectoryName")).unwrap();
    if root.join("LONGDI~1").exists() {
        let zip=archive(&[("LongDirectoryName/a",b"a"),("LONGDI~1/b",b"b")]);
        assert!(extract_zip(Utf8Path::from_path(zip.path()).unwrap(),100).is_err());
    }
}
