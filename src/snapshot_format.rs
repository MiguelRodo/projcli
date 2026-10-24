//! Explicit versioned metadata codecs; legacy CSV and flat CAS encodings are unchanged.
//! Content-tree identity excludes release/label/date metadata. Snapshot identity does not.
//!
//! ```
//! use proj::snapshot_format::{DirectoryTree, TreeSnapshot, DirectoryRoot};
//! let tree = DirectoryTree::new(vec![])?;
//! let snapshot = TreeSnapshot::new("v1.2.3".into(), None,
//!     vec![DirectoryRoot { label: "output".into(), tree: tree.identity()? }])?;
//! assert_eq!(TreeSnapshot::parse(&snapshot.readable_json()?)?, snapshot);
//! # Ok::<(), anyhow::Error>(())
//! ```
use anyhow::{Context,Result};
use serde::{Deserialize,Serialize,de::DeserializeOwned};
use std::collections::{BTreeMap,HashSet};

/// Maximum accepted or emitted JSON metadata bytes per document.
pub const MAX_METADATA_BYTES:usize=16*1024*1024;
const MAX_TEXT_BYTES:usize=8*1024*1024;
const MAX_RECORDS:usize=100_000;
const MAX_PATH_WORK:usize=16*1024*1024;
const MAX_LABELS:usize=4096;
const FLAT_SCHEMA:&str="projcli.flat-snapshot.v1";
const TREE_SCHEMA:&str="projcli.directory-tree.v1";
const SNAPSHOT_SCHEMA:&str="projcli.tree-snapshot.v1";
const ALGORITHM:&str="blake3";

/// One complete flat inventory file, using BLAKE3 over actual file bytes.
#[derive(Clone,Debug,Serialize,Deserialize,PartialEq,Eq)]
#[serde(deny_unknown_fields)]
pub struct FileRecord {
    /// Relative portable path; never absolute or traversing.
    pub path:String,
    /// Lowercase 64-character BLAKE3 content digest.
    pub hash:String,
}

/// Every file and empty-directory leaf for one logical directory.
/// An empty files/empty_directories pair represents an empty logical root.
#[derive(Clone,Debug,Serialize,Deserialize,PartialEq,Eq)]
#[serde(deny_unknown_fields)]
pub struct FlatDirectory {
    /// Logical directory label, separate from the physical root path.
    pub label:String,
    /// Complete file list, without cross-version deltas or hidden references.
    pub files:Vec<FileRecord>,
    /// Empty directory leaves; other parents are evident from the listed paths.
    pub empty_directories:Vec<String>,
}

/// A complete readable snapshot; grouping is not recursive tree storage.
#[derive(Clone,Debug,Serialize,Deserialize,PartialEq,Eq)]
#[serde(deny_unknown_fields)]
pub struct FlatSnapshot {
    /// Explicit wire schema; unknown versions fail rather than being guessed.
    pub schema:String,
    /// File-content algorithm; legacy CSV's MD5 remains a different format.
    pub hash_algorithm:String,
    /// Supplied release/version metadata, outside content-tree identity.
    pub version:String,
    /// Optional supplied instant, normalized to UTC. No clock is queried.
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub created_at:Option<String>,
    /// All selected logical directories, including empty roots.
    pub directories:Vec<FlatDirectory>,
}

/// Direct-child kind; links, devices and submodules are deliberately unsupported.
#[derive(Clone,Copy,Debug,Serialize,Deserialize,PartialEq,Eq)]
#[serde(rename_all="lowercase")]
pub enum EntryKind {
    /// Hash addresses file-content bytes.
    File,
    /// Hash addresses another v1 directory object, not a legacy flat manifest.
    Tree,
}

/// One named direct child of a content-addressed directory.
#[derive(Clone,Debug,Serialize,Deserialize,PartialEq,Eq)]
#[serde(deny_unknown_fields)]
pub struct TreeEntry {
    /// Exactly one portable path component.
    pub name:String,
    /// File or subtree; the type is part of canonical identity.
    pub kind:EntryKind,
    /// Lowercase BLAKE3 child identity.
    pub hash:String,
}

/// One directory's children; no label, release version, date or physical path.
/// Names/types/child hashes define identity; permissions and ownership do not.
#[derive(Clone,Debug,Serialize,Deserialize,PartialEq,Eq)]
#[serde(deny_unknown_fields)]
pub struct DirectoryTree {
    /// Explicit directory-object schema.
    pub schema:String,
    /// BLAKE3 for file bytes and domain-separated canonical tree metadata.
    pub hash_algorithm:String,
    /// Direct children only; an empty list is an empty directory.
    pub entries:Vec<TreeEntry>,
}

/// Logical directory association, outside the content-root identity.
#[derive(Clone,Debug,Serialize,Deserialize,PartialEq,Eq)]
#[serde(deny_unknown_fields)]
pub struct DirectoryRoot {
    /// Logical label, independent of the directory object.
    pub label:String,
    /// V1 directory-tree identity; never silently a legacy flat CAS identifier.
    pub tree:String,
}

/// Small immutable snapshot metadata, suitable for a Git root reference.
/// The referenced tree graph must still be retained and verified separately.
#[derive(Clone,Debug,Serialize,Deserialize,PartialEq,Eq)]
#[serde(deny_unknown_fields)]
pub struct TreeSnapshot {
    /// Explicit snapshot-reference schema.
    pub schema:String,
    /// Algorithm for the referenced graph.
    pub hash_algorithm:String,
    /// Supplied version metadata; changing it does not change directory roots.
    pub version:String,
    /// Optional explicit instant; omitted means unknown, never current time.
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub created_at:Option<String>,
    /// Complete selected label/root associations, without full file rows.
    pub directories:Vec<DirectoryRoot>,
}

struct Budget(usize);
impl Budget {
    fn add(&mut self,value:&str)->Result<()> {
        self.0=self.0.checked_add(value.len()).context("Metadata text budget overflow")?;
        anyhow::ensure!(self.0<=MAX_TEXT_BYTES,"Metadata exceeds text budget");
        Ok(())
    }
}
fn key(value:&str)->String {
    icu_normalizer::ComposingNormalizer::new_nfc().normalize(&value.to_uppercase()).into_owned()
}
fn labels<'a>(values:impl Iterator<Item=&'a str>,budget:&mut Budget)->Result<()> {
    let mut seen=HashSet::new();
    for (count,label) in values.enumerate() {
        anyhow::ensure!(count<MAX_LABELS,"Snapshot exceeds logical-directory limit");
        budget.add(label)?;
        anyhow::ensure!(label.len()<=1024,"Logical label exceeds byte limit");
        crate::path_safety::validate_directory_label(label)?;
        anyhow::ensure!(seen.insert(key(label)),"Duplicate or portable-alias logical labels");
    }
    Ok(())
}
fn path(value:&str,budget:&mut Budget)->Result<()> {
    budget.add(value)?;
    anyhow::ensure!(value.len()<=4096&&!value.ends_with('/')&&value.split('/').count()<=64,"Invalid or over-deep stored path");
    crate::path_safety::portable_relative(value)?;
    Ok(())
}
fn digest(value:&str,budget:&mut Budget)->Result<()> {
    budget.add(value)?;
    crate::cas::validate_digest(value)
}
fn version(value:&str,budget:&mut Budget)->Result<()> {
    budget.add(value)?;
    anyhow::ensure!(!value.is_empty()&&value.len()<=256&&!value.chars().any(char::is_control),
        "Snapshot version must be nonempty bounded printable metadata");
    Ok(())
}
fn date(value:&Option<String>,budget:&mut Budget)->Result<Option<String>> {
    value.as_ref().map(|text| {
        budget.add(text)?;
        anyhow::ensure!(text.len()<=128,"Snapshot date is too long");
        Ok(chrono::DateTime::parse_from_rfc3339(text).context("Invalid snapshot date")?
            .with_timezone(&chrono::Utc).to_rfc3339_opts(chrono::SecondsFormat::AutoSi,true))
    }).transpose()
}
fn parse<T:DeserializeOwned>(bytes:&[u8])->Result<T> {
    anyhow::ensure!(bytes.len()<=MAX_METADATA_BYTES,"Snapshot metadata exceeds byte limit");
    serde_json::from_slice(bytes).context("Invalid versioned snapshot metadata")
}
fn bounded(bytes:Vec<u8>)->Result<Vec<u8>> {
    anyhow::ensure!(bytes.len()<=MAX_METADATA_BYTES,"Snapshot metadata exceeds byte limit");
    Ok(bytes)
}
fn identity(context:&str,value:&impl Serialize)->Result<String> {
    let bytes=bounded(serde_json::to_vec(value)?)?;
    let mut hasher=blake3::Hasher::new_derive_key(context);hasher.update(&bytes);
    Ok(hasher.finalize().to_hex().to_string())
}
fn pretty(value:&impl Serialize)->Result<Vec<u8>> {
    let mut bytes=serde_json::to_vec_pretty(value)?;bytes.push(b'\n');bounded(bytes)
}

// Validate both portable parent aliases and file/empty-directory shape conflicts.
fn flat_paths(directory:&FlatDirectory,budget:&mut Budget,work:&mut usize)->Result<()> {
    let mut seen=BTreeMap::<String,(String,u8)>::new();
    for (value,kind) in directory.files.iter().map(|f|(f.path.as_str(),1u8))
        .chain(directory.empty_directories.iter().map(|p|(p.as_str(),2u8))) {
        path(value,budget)?;
        let parts:Vec<_>=value.split('/').collect();
        let mut length=0usize;
        for end in 1..=parts.len() {
            length+=parts[end-1].len()+usize::from(end>1);
            *work=work.checked_add(length).context("Expanded path-work budget overflow")?;
            anyhow::ensure!(*work<=MAX_PATH_WORK,"Flat snapshot exceeds expanded path-work budget");
            let prefix=parts[..end].join("/");
            let last=end==parts.len();
            if let Some((original,previous))=seen.get(&key(&prefix)) {
                anyhow::ensure!(original==&prefix,"Portable parent/path alias in flat inventory");
                anyhow::ensure!(!last&&*previous==0,"Duplicate or conflicting file/directory paths");
            }else{seen.insert(key(&prefix),(prefix,if last{kind}else{0}));}
        }
    }
    Ok(())
}

impl FlatSnapshot {
    /// Build complete metadata; input order is canonicalized and no time is inferred.
    pub fn new(version:String,created_at:Option<String>,directories:Vec<FlatDirectory>)->Result<Self> {
        Self {schema:FLAT_SCHEMA.into(),hash_algorithm:ALGORITHM.into(),version,created_at,directories}.canonical()
    }
    fn canonical(&self)->Result<Self> {
        anyhow::ensure!(self.schema==FLAT_SCHEMA&&self.hash_algorithm==ALGORITHM,
            "Unsupported flat snapshot schema/hash algorithm");
        let mut budget=Budget(0);version(&self.version,&mut budget)?;
        let created_at=date(&self.created_at,&mut budget)?;
        labels(self.directories.iter().map(|d|d.label.as_str()),&mut budget)?;
        let mut count=0usize;let mut work=0usize;
        for directory in &self.directories {
            count=count.checked_add(directory.files.len()).and_then(|c|c.checked_add(directory.empty_directories.len()))
                .context("Snapshot entry count overflow")?;
            anyhow::ensure!(count<=MAX_RECORDS,"Flat snapshot exceeds entry limit");
            flat_paths(directory,&mut budget,&mut work)?;
            for file in &directory.files {digest(&file.hash,&mut budget)?;}
        }
        let mut result=self.clone();result.created_at=created_at;
        result.directories.sort_by(|a,b|a.label.as_bytes().cmp(b.label.as_bytes()));
        for directory in &mut result.directories {
            directory.files.sort_by(|a,b|a.path.as_bytes().cmp(b.path.as_bytes()));
            directory.empty_directories.sort();
        }
        Ok(result)
    }
    /// Parse the explicit v1 JSON schema; legacy CSV/flat-CAS readers remain separate.
    pub fn parse(bytes:&[u8])->Result<Self> {parse::<Self>(bytes)?.canonical()}
    /// BLAKE3 derive-key mode with the schema context over compact canonical UTF-8 JSON.
    pub fn identity(&self)->Result<String> {identity("projcli.flat-snapshot.v1",&self.canonical()?)}
    /// A complete document with one file entry per line; no delta reconstruction.
    pub fn readable_json(&self)->Result<Vec<u8>> {
        let value=self.canonical()?;
        let mut text=format!("{{\n  \"schema\": {},\n  \"hash_algorithm\": {},\n  \"version\": {},\n",
            serde_json::to_string(&value.schema)?,serde_json::to_string(&value.hash_algorithm)?,serde_json::to_string(&value.version)?);
        if let Some(date)=&value.created_at {text.push_str(&format!("  \"created_at\": {},\n",serde_json::to_string(date)?));}
        text.push_str("  \"directories\": [\n");
        for (i,directory) in value.directories.iter().enumerate() {
            text.push_str(&format!("    {{\n      \"label\": {},\n      \"files\": [\n",serde_json::to_string(&directory.label)?));
            for (index,file) in directory.files.iter().enumerate() {
                text.push_str(&format!("        {}{}\n",serde_json::to_string(file)?,
                    if index+1<directory.files.len(){","}else{""}));
            }
            text.push_str("      ],\n      \"empty_directories\": [\n");
            for (index,path) in directory.empty_directories.iter().enumerate() {
                text.push_str(&format!("        {}{}\n",serde_json::to_string(path)?,
                    if index+1<directory.empty_directories.len(){","}else{""}));
            }
            text.push_str(&format!("      ]\n    }}{}\n",if i+1<value.directories.len(){","}else{""}));
        }
        text.push_str("  ]\n}\n");bounded(text.into_bytes())
    }
}
impl DirectoryTree {
    /// Canonical direct children; changing label/version outside this object cannot change its ID.
    pub fn new(entries:Vec<TreeEntry>)->Result<Self> {
        Self {schema:TREE_SCHEMA.into(),hash_algorithm:ALGORITHM.into(),entries}.canonical()
    }
    fn canonical(&self)->Result<Self> {
        anyhow::ensure!(self.schema==TREE_SCHEMA&&self.hash_algorithm==ALGORITHM,
            "Unsupported directory-tree schema/hash algorithm");
        anyhow::ensure!(self.entries.len()<=MAX_RECORDS,"Tree exceeds child limit");
        let mut budget=Budget(0);let mut seen=HashSet::new();
        for entry in &self.entries {
            anyhow::ensure!(entry.name.len()<=1024,"Tree child name exceeds byte limit");
            crate::path_safety::validate_directory_label(&entry.name)?;
            path(&entry.name,&mut budget)?;digest(&entry.hash,&mut budget)?;
            anyhow::ensure!(seen.insert(key(&entry.name)),"Duplicate or portable-alias tree children");
        }
        let mut result=self.clone();result.entries.sort_by(|a,b|a.name.as_bytes().cmp(b.name.as_bytes()));Ok(result)
    }
    /// Parse a directory object without inferring its schema from a bare legacy ID.
    pub fn parse(bytes:&[u8])->Result<Self> {parse::<Self>(bytes)?.canonical()}
    /// Content ID binds names, types and child IDs using a versioned domain.
    pub fn identity(&self)->Result<String> {identity("projcli.directory-tree.v1",&self.canonical()?)}
    /// Deterministic readable bytes; readers verify the canonical semantic ID separately.
    pub fn readable_json(&self)->Result<Vec<u8>> {pretty(&self.canonical()?)}
}
impl TreeSnapshot {
    /// Bind roots to explicit release metadata; identical supplied inputs are idempotent.
    pub fn new(version:String,created_at:Option<String>,directories:Vec<DirectoryRoot>)->Result<Self> {
        Self {schema:SNAPSHOT_SCHEMA.into(),hash_algorithm:ALGORITHM.into(),version,created_at,directories}.canonical()
    }
    fn canonical(&self)->Result<Self> {
        anyhow::ensure!(self.schema==SNAPSHOT_SCHEMA&&self.hash_algorithm==ALGORITHM,
            "Unsupported tree-snapshot schema/hash algorithm");
        let mut budget=Budget(0);version(&self.version,&mut budget)?;
        let created_at=date(&self.created_at,&mut budget)?;
        labels(self.directories.iter().map(|d|d.label.as_str()),&mut budget)?;
        for directory in &self.directories {digest(&directory.tree,&mut budget)?;}
        let mut result=self.clone();result.created_at=created_at;
        result.directories.sort_by(|a,b|a.label.as_bytes().cmp(b.label.as_bytes()));Ok(result)
    }
    /// Parse only versioned root references, not old label/version-bearing flat manifests.
    pub fn parse(bytes:&[u8])->Result<Self> {parse::<Self>(bytes)?.canonical()}
    /// Snapshot metadata has a separate identity from its content roots.
    pub fn identity(&self)->Result<String> {identity("projcli.tree-snapshot.v1",&self.canonical()?)}
    /// Small complete root-reference document; does not contain the referenced tree graph.
    pub fn readable_json(&self)->Result<Vec<u8>> {pretty(&self.canonical()?)}
}

#[cfg(test)]
mod tests {
    use super::*;
    fn hash()->String {"a".repeat(64)}
    #[test]
    fn canonical_trees_ignore_order_and_separate_snapshot_metadata_and_domains() {
        let entries=vec![TreeEntry {name:"z".into(),kind:EntryKind::File,hash:hash()},
            TreeEntry {name:"a".into(),kind:EntryKind::Tree,hash:"b".repeat(64)}];
        let tree=DirectoryTree::new(entries.clone()).unwrap();
        let reversed=DirectoryTree::new(entries.into_iter().rev().collect()).unwrap();
        assert_eq!(tree.identity().unwrap(),reversed.identity().unwrap());
        let compact=serde_json::to_vec(&tree).unwrap();
        assert_eq!(DirectoryTree::parse(&compact).unwrap().identity().unwrap(),tree.identity().unwrap());
        assert_ne!(tree.identity().unwrap(),blake3::hash(&compact).to_hex().as_str());
        let expected=format!(r#"{{"schema":"projcli.directory-tree.v1","hash_algorithm":"blake3","entries":[{{"name":"a","kind":"tree","hash":"{}"}},{{"name":"z","kind":"file","hash":"{}"}}]}}"#,"b".repeat(64),hash());
        assert_eq!(compact,expected.as_bytes());
        let mut identity=blake3::Hasher::new_derive_key("projcli.directory-tree.v1");
        identity.update(expected.as_bytes());
        assert_eq!(tree.identity().unwrap(),identity.finalize().to_hex().as_str());
        let root=DirectoryRoot {label:"output".into(),tree:tree.identity().unwrap()};
        let one=TreeSnapshot::new("v1".into(),None,vec![root.clone()]).unwrap();
        let two=TreeSnapshot::new("v2".into(),None,vec![DirectoryRoot {label:"raw".into(),..root}]).unwrap();
        assert_ne!(one.identity().unwrap(),two.identity().unwrap());
        assert_eq!(one.directories[0].tree,two.directories[0].tree);
        assert_eq!(TreeSnapshot::parse(&one.readable_json().unwrap()).unwrap(),one);
    }
    #[test]
    fn flat_document_is_complete_readable_and_preserves_empty_shapes_and_escaping() {
        let snapshot=FlatSnapshot::new("v1".into(),Some("2026-10-24T01:00:00+01:00".into()),
            vec![FlatDirectory {label:"output".into(),files:vec![
                FileRecord {path:"sub/a,\"quoted\".csv".into(),hash:hash()},
                FileRecord {path:"\u{e9}.csv".into(),hash:"b".repeat(64)}],
                empty_directories:vec!["empty".into()]},
                FlatDirectory {label:"docs".into(),files:vec![],empty_directories:vec![]}]).unwrap();
        let bytes=snapshot.readable_json().unwrap();let text=std::str::from_utf8(&bytes).unwrap();
        assert_eq!(text.lines().filter(|l|l.trim_start().starts_with("{\"path\":")).count(),2);
        assert!(text.contains("\\\"quoted\\\"")&&text.contains("\u{e9}.csv"));
        assert_eq!(snapshot.created_at.as_deref(),Some("2026-10-24T00:00:00Z"));
        assert_eq!(FlatSnapshot::parse(&bytes).unwrap(),snapshot);
        let compact=serde_json::to_vec(&snapshot).unwrap();
        assert_eq!(FlatSnapshot::parse(&compact).unwrap().identity().unwrap(),snapshot.identity().unwrap());
        let mut duplicate=snapshot.clone();let repeated=duplicate.directories[1].files[0].clone();
        duplicate.directories[1].files.push(repeated);
        assert!(duplicate.identity().is_err());
    }
    #[test]
    fn malformed_versions_hashes_aliases_and_shapes_fail_closed() {
        for name in ["..","a/b","C:drive","NUL","trail.","line\nbreak"] {
            assert!(DirectoryTree::new(vec![TreeEntry {name:name.into(),kind:EntryKind::File,hash:hash()}]).is_err());
        }
        assert!(DirectoryTree::new(vec![TreeEntry {name:"a".into(),kind:EntryKind::File,hash:"bad".into()}]).is_err());
        for pair in [("A","a"),("\u{e9}","e\u{301}")] {
            assert!(DirectoryTree::new(vec![TreeEntry {name:pair.0.into(),kind:EntryKind::File,hash:hash()},
                TreeEntry {name:pair.1.into(),kind:EntryKind::Tree,hash:hash()}]).is_err());
        }
        for (file,empty) in [("a/b","a"),("A/b","a/empty"),("a","a/b")] {
            assert!(FlatSnapshot::new("v1".into(),None,vec![FlatDirectory {label:"output".into(),
                files:vec![FileRecord {path:file.into(),hash:hash()}],empty_directories:vec![empty.into()]}]).is_err());
        }
        assert!(DirectoryTree::parse(br#"{"schema":"unknown","hash_algorithm":"blake3","entries":[]}"#).is_err());
        let bytes=DirectoryTree::new(vec![]).unwrap().readable_json().unwrap();
        let mut json:serde_json::Value=serde_json::from_slice(&bytes).unwrap();json["extra"]=serde_json::json!(true);
        assert!(DirectoryTree::parse(&serde_json::to_vec(&json).unwrap()).is_err());
        assert!(TreeSnapshot::new("v1".into(),Some("not a date".into()),vec![]).is_err());
        assert!(FlatSnapshot::new("".into(),None,vec![]).is_err());
        assert!(DirectoryTree::new(vec![TreeEntry {name:"x".repeat(1025),kind:EntryKind::File,hash:hash()}]).is_err());
        assert!(DirectoryTree::parse(&vec![b' ';MAX_METADATA_BYTES+1]).is_err());
    }
    #[test]
    fn derived_prefix_work_is_bounded_before_expansion() {
        let prefix=format!("{}/{}","x".repeat(900),vec!["d".repeat(20);60].join("/"));
        let files=(0..256).map(|i|FileRecord {path:format!("{prefix}/f{i}"),hash:hash()}).collect();
        let error=FlatSnapshot::new("v1".into(),None,vec![FlatDirectory {label:"output".into(),
            files,empty_directories:vec![]}]).unwrap_err();
        assert!(error.to_string().contains("expanded path-work budget"));
        let very_long=FileRecord {path:"x".repeat(MAX_TEXT_BYTES),hash:hash()};
        assert!(FlatSnapshot::new("v1".into(),None,vec![FlatDirectory {label:"output".into(),
            files:vec![very_long],empty_directories:vec![]}]).is_err());
    }

}
