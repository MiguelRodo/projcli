//! Directory metadata for ordinary payloads; never resolves file hashes through CAS objects.
use std::{collections::{BTreeMap,HashMap,HashSet}};
use anyhow::{Context,Result};
use camino::{Utf8Path,Utf8PathBuf};
use crate::snapshot_format::{FlatSnapshot,FlatDirectory,FileRecord,DirectoryTree,TreeEntry,EntryKind,
    MAX_METADATA_BYTES,MAX_RECORDS,MAX_PATH_WORK};
use crate::cas_tree::{GRAPH_METADATA_BYTES,MAX_DEPTH};
/// Temporary trie of validated portable membership; no physical data is held here.
#[derive(Default)]
struct Node {files:BTreeMap<String,String>,dirs:BTreeMap<String,Node>}
/// Aggregate inferred-entry/prefix budget, charged before derived allocations.
#[derive(Default)]
struct Budget {records:usize,path_bytes:usize}
impl Budget {
    fn charge(&mut self,length:usize)->Result<()> {
        self.records=self.records.checked_add(1).context("Tree record count overflow")?;
        self.path_bytes=self.path_bytes.checked_add(length).context("Tree path count overflow")?;
        anyhow::ensure!(self.records<=MAX_RECORDS,"Ordinary tree expanded record limit exceeded");
        anyhow::ensure!(self.path_bytes<=MAX_PATH_WORK,"Ordinary tree expanded path budget exceeded");Ok(())
    }
}
fn directory<'a>(node:&'a mut Node,parts:&[&str],budget:&mut Budget)->Result<&'a mut Node> {
    anyhow::ensure!(parts.len()<=MAX_DEPTH,"Ordinary tree depth limit exceeded");
    let mut current=node;let mut length=0;
    for part in parts {
        length+=part.len()+usize::from(length>0);
        anyhow::ensure!(length<=4096,"Ordinary tree path byte limit exceeded");
        if !current.dirs.contains_key(*part) {
            budget.charge(length)?;
            current.dirs.insert((*part).into(),Node::default());
        }
        current=current.dirs.get_mut(*part).unwrap();
    }Ok(current)
}
fn node_id(node:Node,nodes:&mut Vec<(String,Vec<u8>)>,seen:&mut HashSet<String>,bytes:&mut usize)->Result<String> {
    let mut entries=Vec::new();
    for (name,child) in node.dirs {entries.push(TreeEntry {name,kind:EntryKind::Tree,hash:node_id(child,nodes,seen,bytes)?});}
    for (name,hash) in node.files {entries.push(TreeEntry {name,kind:EntryKind::File,hash});}
    let tree=DirectoryTree::new(entries)?;let id=tree.identity()?;
    if seen.insert(id.clone()) {
        let encoded=tree.readable_json()?;
        *bytes=bytes.checked_add(encoded.len()).context("Tree metadata size overflow")?;
        anyhow::ensure!(*bytes<=GRAPH_METADATA_BYTES,"Ordinary tree metadata budget exceeded");
        nodes.push((id.clone(),encoded));
    }Ok(id)
}
/// Pure directory objects plus per-label roots and their readable inventory checksums.
pub(crate) struct Forest {
    /// Logical labels bind outside pure directory identities.
    pub(crate) roots:BTreeMap<String,(String,String)>,
    nodes:Vec<(String,Vec<u8>)>,
}
impl Forest {
    /// Build existing v1 directory objects from validated complete payload observations.
    /// Inferred parent records are charged globally before allocation; no file data is copied.
    pub(crate) fn build(snapshot:&FlatSnapshot)->Result<Self> {
        let snapshot=FlatSnapshot::parse(&snapshot.readable_json()?)?;
        let projection=crate::catalogue::Catalogue::default().with_snapshot(&snapshot)?;
        let hashes:HashMap<_,_>=projection.records.iter().map(|record|(record.label.as_str(),record.inventory_hash.as_str())).collect();
        let mut budget=Budget::default();let mut seen=HashSet::new();let mut bytes=0;
        let mut nodes=Vec::new();let mut roots=BTreeMap::new();
        for inventory in &snapshot.directories {
            budget.charge(0)?;let mut root=Node::default();
            for file in &inventory.files {
                let parts:Vec<_>=file.path.split('/').collect();
                anyhow::ensure!(parts.len()<=MAX_DEPTH,"Ordinary tree depth limit exceeded");
                let parent=directory(&mut root,&parts[..parts.len()-1],&mut budget)?;
                budget.charge(file.path.len())?;
                anyhow::ensure!(parent.files.insert(parts.last().unwrap().to_string(),file.hash.clone()).is_none(),"Duplicate ordinary tree file");
            }
            for empty in &inventory.empty_directories {directory(&mut root,&empty.split('/').collect::<Vec<_>>(),&mut budget)?;}
            let id=node_id(root,&mut nodes,&mut seen,&mut bytes)?;
            roots.insert(inventory.label.clone(),(id,hashes[inventory.label.as_str()].to_string()));
        }
        let forest=Self {roots,nodes};
        let index:HashMap<_,_>=forest.nodes.iter().map(|(id,bytes)|(id.as_str(),bytes.as_slice())).collect();
        let mut verification=Verification::default();
        for (tree,expected) in forest.roots.values() {
            let actual=verification.inventory(tree,|id|Ok(index.get(id).context("Prepared directory object missing")?.to_vec()))?;
            anyhow::ensure!(&actual==expected,"Prepared tree disagrees with readable inventory");
        }
        Ok(forest)
    }
    /// Publish metadata nodes only, in dependency order, then check the complete stored graph.
    /// File hashes refer to the ordinary physical payload; no objects/ lookup or data copy.
    pub(crate) fn publish(&self,root:&Utf8Path,base:&Utf8Path)->Result<()> {
        let namespace=crate::cas_tree::native(base,"trees/v1")?;
        crate::cas_tree::project_target(root,&namespace,true)?;
        crate::cas::checked_directory(&namespace)?;
        for (id,bytes) in &self.nodes {
            crate::cas_tree::publish_metadata(&path(base,id)?,bytes,|existing|{
                anyhow::ensure!(DirectoryTree::parse(existing)?.identity()?==*id,"Existing ordinary directory object is corrupt");Ok(())
            })?;
        }
        let mut verification=Verification::default();
        for (tree,expected) in self.roots.values() {
            anyhow::ensure!(verification.inventory(tree,|id|read(base,id))?==*expected,"Stored tree disagrees with readable inventory");
        }Ok(())
    }
}
/// Sharded v1 directory-object path. Validate the full digest before slicing or following it.
pub(crate) fn path(base:&Utf8Path,id:&str)->Result<Utf8PathBuf> {
    crate::cas::validate_digest(id)?;
    crate::cas_tree::native(base,&format!("trees/v1/{}/{}.json",&id[..2],&id[2..]))
}
/// Bounded regular metadata read; ordinary file-content bytes are not addressed here.
pub(crate) fn read(base:&Utf8Path,id:&str)->Result<Vec<u8>> {
    crate::fs_utils::read_managed_bounded(&path(base,id)?,MAX_METADATA_BYTES as u64)
}
/// Per-controlled-read graph integrity cache; this does not prove historical payload availability.
#[derive(Default)]
pub(crate) struct Verification {roots:HashMap<String,String>,path_bytes:usize}
impl Verification {
    /// Verify directory IDs/membership and return the companion inventory checksum.
    /// Cached roots are safe only while the input object set is immutable.
    pub(crate) fn inventory(&mut self,id:&str,mut load:impl FnMut(&str)->Result<Vec<u8>>)->Result<String> {
        crate::cas::validate_digest(id)?;
        if let Some(hash)=self.roots.get(id){return Ok(hash.clone())}
        let mut budget=Budget::default();let mut bytes=0;let mut ancestors=HashSet::new();
        let mut files=Vec::new();let mut empty=Vec::new();
        walk(id,"",0,&mut load,&mut ancestors,&mut budget,&mut self.path_bytes,&mut bytes,&mut files,&mut empty)?;
        let snapshot=FlatSnapshot::new("inventory".into(),None,vec![FlatDirectory {label:"inventory".into(),files,empty_directories:empty}])?;
        let projection=crate::catalogue::Catalogue::default().with_snapshot(&snapshot)?;
        let hash=projection.records[0].inventory_hash.clone();self.roots.insert(id.into(),hash.clone());Ok(hash)
    }
}
#[allow(clippy::too_many_arguments)]
fn walk(id:&str,prefix:&str,depth:usize,load:&mut impl FnMut(&str)->Result<Vec<u8>>,ancestors:&mut HashSet<String>,
    budget:&mut Budget,path_work:&mut usize,bytes:&mut usize,files:&mut Vec<FileRecord>,empty:&mut Vec<String>)->Result<()> {
    crate::cas::validate_digest(id)?;
    anyhow::ensure!(depth<=MAX_DEPTH&&ancestors.insert(id.into()),"Ordinary tree cycle/depth violation");
    let encoded=load(id)?;*bytes=bytes.checked_add(encoded.len()).context("Tree read budget overflow")?;
    anyhow::ensure!(*bytes<=GRAPH_METADATA_BYTES,"Ordinary tree metadata read budget exceeded");
    let tree=DirectoryTree::parse(&encoded)?;anyhow::ensure!(tree.identity()?==id,"Corrupt ordinary directory-object identity");
    if depth==0{budget.charge(0)?;}
    if tree.entries.is_empty()&&!prefix.is_empty(){empty.push(prefix.into());}
    for entry in tree.entries {
        let length=prefix.len()+entry.name.len()+usize::from(!prefix.is_empty());
        anyhow::ensure!(depth<MAX_DEPTH&&length<=4096,"Ordinary tree path/depth limit exceeded");
        budget.charge(length)?;
        *path_work=path_work.checked_add(length).context("Tree path work overflow")?;
        anyhow::ensure!(*path_work<=MAX_PATH_WORK,"Ordinary tree cumulative path budget exceeded");
        let name=if prefix.is_empty(){entry.name}else{format!("{prefix}/{}",entry.name)};
        match entry.kind {
            EntryKind::File=>files.push(FileRecord {path:name,hash:entry.hash}),
            EntryKind::Tree=>walk(&entry.hash,&name,depth+1,load,ancestors,budget,path_work,bytes,files,empty)?,
        }
    }
    ancestors.remove(id);Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn snapshot(label:&str,version:&str)->FlatSnapshot {
        FlatSnapshot::new(version.into(),None,vec![FlatDirectory {label:label.into(),
            files:vec![FileRecord {path:"nested/caf\u{e9},file".into(),hash:blake3::hash(b"bytes").to_hex().to_string()}],
            empty_directories:vec!["empty".into()]}]).unwrap()
    }
    #[test]
    fn roots_are_existing_directory_identities_independent_of_label_version_and_projection() {
        let first=Forest::build(&snapshot("raw","v1")).unwrap();
        let mut later=snapshot("output","v2");later.created_at=Some("2026-10-24T00:00:00Z".into());
        let second=Forest::build(&later).unwrap();
        assert_eq!(first.roots["raw"].0,second.roots["output"].0);
        let child=DirectoryTree::new(vec![TreeEntry {name:"caf\u{e9},file".into(),kind:EntryKind::File,hash:blake3::hash(b"bytes").to_hex().to_string()}]).unwrap();
        let empty=DirectoryTree::new(vec![]).unwrap();
        let root=DirectoryTree::new(vec![TreeEntry {name:"nested".into(),kind:EntryKind::Tree,hash:child.identity().unwrap()},
            TreeEntry {name:"empty".into(),kind:EntryKind::Tree,hash:empty.identity().unwrap()}]).unwrap();
        assert_eq!(first.roots["raw"].0,root.identity().unwrap());assert_eq!(first.nodes,second.nodes);
    }
    #[test]
    fn inferred_parent_records_are_bounded_even_when_flat_file_count_is_valid() {
        let snapshot=FlatSnapshot::new("v1".into(),None,vec![FlatDirectory {label:"raw".into(),
            files:(0..30_000).map(|index|FileRecord {path:format!("d{index}/a/b/file"),hash:"0".repeat(64)}).collect(),
            empty_directories:vec![]}]).unwrap();
        assert!(Forest::build(&snapshot).err().unwrap().to_string().contains("expanded record limit"));
    }
    #[test]
    fn invalid_and_corrupt_node_ids_are_rejected_before_following_children() {
        let mut calls=0;let mut verification=Verification::default();
        assert!(verification.inventory("../escape",|_|{calls+=1;Ok(vec![])}).is_err());assert_eq!(calls,0);
        let child=DirectoryTree::new(vec![]).unwrap();let id=child.identity().unwrap();
        let wrong=DirectoryTree::new(vec![TreeEntry {name:"file".into(),kind:EntryKind::File,hash:"0".repeat(64)}]).unwrap().readable_json().unwrap();
        assert!(Verification::default().inventory(&id,|_|{calls+=1;Ok(wrong.clone())}).is_err());assert_eq!(calls,1);
        // Exercise excessive metadata depth without a deep filesystem fixture.
        let mut node=DirectoryTree::new(vec![]).unwrap();let mut nodes=HashMap::new();
        for _ in 0..65 {
            let child=node.identity().unwrap();nodes.insert(child.clone(),node.readable_json().unwrap());
            node=DirectoryTree::new(vec![TreeEntry {name:"a".into(),kind:EntryKind::Tree,hash:child}]).unwrap();
        }
        let root=node.identity().unwrap();nodes.insert(root.clone(),node.readable_json().unwrap());
        let error=Verification::default().inventory(&root,|id|Ok(nodes.get(id).context("Missing depth witness node")?.clone())).unwrap_err();
        assert!(error.to_string().contains("depth"));
    }
}
