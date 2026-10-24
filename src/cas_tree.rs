//! Immutable v1 recursive CAS objects; no registry defaults or legacy migration.
//! Raw file objects are shared with the existing CAS. Metadata roots may differ.
use std::{collections::{HashMap,HashSet},fs};
use anyhow::{Context,Result};
use camino::{Utf8Path,Utf8PathBuf};
use crate::path_safety::{portable_paths_overlap as overlaps,portable_path_key};
use crate::snapshot_format::{DirectoryTree,TreeEntry,EntryKind,TreeSnapshot,FlatDirectory,FileRecord,
    FlatSnapshot,MAX_METADATA_BYTES,MAX_RECORDS,MAX_PATH_WORK};

const GRAPH_METADATA_BYTES:usize=64*1024*1024;
const MAX_DEPTH:usize=64;

struct Layout {objects:Utf8PathBuf,trees:Utf8PathBuf,snapshots:Utf8PathBuf}
fn physical(path:&Utf8Path)->Result<Utf8PathBuf> {
    Utf8PathBuf::try_from(crate::path_safety::resolve_path(path.as_std_path(),true)?)
        .context("CAS path is not UTF-8")
}
fn layout(content:&Utf8Path,metadata:&Utf8Path)->Result<Layout> {
    let content=physical(content)?;let metadata=physical(metadata)?;
    let value=Layout {objects:content.join("objects"),trees:metadata.join("trees/v1"),
        snapshots:metadata.join("tree-snapshots/v1")};
    for (base,relative) in [(&content,"objects"),(&metadata,"trees/v1"),(&metadata,"tree-snapshots/v1")] {
        let target=base.join(relative);crate::cas::checked_directory(&target)?;
        crate::path_safety::validate_native_names(base,relative,false)?;
    }
    anyhow::ensure!(!overlaps(&value.objects,&value.trees)&&!overlaps(&value.objects,&value.snapshots)
        &&!overlaps(&value.trees,&value.snapshots),"CAS content/metadata namespaces overlap");
    Ok(value)
}
fn control_name(name:&str)->bool {
    let name=portable_path_key(Utf8Path::new(name)).as_str().to_ascii_lowercase();
    [".git",".hg",".svn",".projr",".agents",".codex",".projects","version","description",
        "manifest.csv",".gitignore",".rbuildignore",".rprofile",".renviron","pyproject.toml",
        "project.toml","juliaproject.toml","renv.lock","_bookdown.yml","_output.yml",
        "readme.md","changelog.md"].contains(&name.as_str())
        ||((name.starts_with("_proj")||name.starts_with("_quarto"))
            &&[".yml",".yaml",".yml.local",".yaml.local"].iter().any(|suffix|name.ends_with(suffix)))
        ||name=="_environment"||name.starts_with("_environment.")||name.starts_with("_environment-")
}
fn project_target(root:&Utf8Path,target:&Utf8Path,allow_metadata:bool)->Result<()> {
    let root=physical(root)?;let root_key=portable_path_key(&root);
    anyhow::ensure!(root.is_dir(),"CAS project root must be an existing directory");
    let key=portable_path_key(target);
    anyhow::ensure!(!root_key.starts_with(&key),"CAS target contains the project root");
    if key.starts_with(&root_key) {
        let relative=target.strip_prefix(&root)
            .context("CAS target aliases the project root through a planned portable spelling")?;
        let first=relative.iter().next().context("CAS target is the project root")?;
        anyhow::ensure!(!control_name(first),"CAS target overlaps project authority/control path: {first}");
        if portable_path_key(Utf8Path::new(first)).as_str()==".PROJ" {
            anyhow::ensure!(allow_metadata&&first==".proj","CAS target overlaps project control state");
            for reserved in [".proj/log",".proj/build.lock"] {
                anyhow::ensure!(!overlaps(target,&root.join(reserved)),
                    "CAS target overlaps project attempt control state");
            }
        }
    }
    Ok(())
}
fn writer_paths(root:&Utf8Path,store:&Layout)->Result<()> {
    for namespace in [&store.objects,&store.trees,&store.snapshots] {
        project_target(root,namespace,true)?;
    }
    anyhow::ensure!(!overlaps(&store.objects,&physical(&root.join(".proj"))?),
        "CAS file storage overlaps project control state");
    Ok(())
}

fn native(base:&Utf8Path,relative:&str)->Result<Utf8PathBuf> {
    let path=base.join(relative);
    crate::path_safety::resolve_path(path.as_std_path(),true)?;
    crate::path_safety::validate_native_names(base,relative,false)?;
    Ok(path)
}
fn tree_path(store:&Layout,id:&str)->Result<Utf8PathBuf> {
    crate::cas::validate_digest(id)?;
    native(&store.trees,&format!("{}/{}.json",&id[..2],&id[2..]))
}
fn snapshot_path(store:&Layout,id:&str)->Result<Utf8PathBuf> {
    crate::cas::validate_digest(id)?;native(&store.snapshots,&format!("{}/{}.json",&id[..2],&id[2..]))
}
fn file_path(store:&Layout,id:&str)->Result<Utf8PathBuf> {
    crate::cas::validate_digest(id)?;
    native(&store.objects,&format!("{}/{}",&id[..2],&id[2..]))
}
fn metadata_bytes(path:&Utf8Path,kind:&str)->Result<Vec<u8>> {
    crate::path_safety::resolve_path(path.as_std_path(),true)?;
    match fs::symlink_metadata(path) {
        Err(error) if error.kind()==std::io::ErrorKind::NotFound=>anyhow::bail!("Missing CAS {kind}"),
        Err(error)=>return Err(error).with_context(||format!("Cannot inspect CAS {kind}")),
        Ok(metadata)=>anyhow::ensure!(metadata.is_file(),"CAS {kind} is not a regular file"),
    }
    crate::fs_utils::read_managed_bounded(path,MAX_METADATA_BYTES as u64)
        .with_context(||format!("Cannot read CAS {kind}"))
}
fn read_tree(store:&Layout,id:&str)->Result<(DirectoryTree,usize)> {
    let bytes=metadata_bytes(&tree_path(store,id)?,&format!("directory object {id}"))?;
    let tree=DirectoryTree::parse(&bytes).with_context(||format!("Malformed CAS directory object {id}"))?;
    let actual=tree.identity()?;
    anyhow::ensure!(actual==id,"Corrupt CAS directory-object identity: expected {id}, observed {actual}");
    Ok((tree,bytes.len()))
}
fn publish_metadata(path:&Utf8Path,bytes:&[u8],validate:impl Fn(&[u8])->Result<()>)->Result<()> {
    if crate::yml::configuration_entry_exists(path)? {
        return validate(&metadata_bytes(path,"metadata object")?)
            .context("Existing immutable CAS metadata is invalid; preserved");
    }
    match crate::cas::write_metadata(path,bytes,true) {
        Ok(())=>Ok(()),
        Err(primary)=>{
            // Another writer may have installed the same canonical identity with
            // different harmless JSON spacing. Validate and reuse; never overwrite.
            match crate::yml::configuration_entry_exists(path) {
                Ok(true)=>match metadata_bytes(path,"metadata object").and_then(|bytes|validate(&bytes)) {
                    Ok(())=>Ok(()),
                    Err(secondary)=>Err(primary.context(format!("Concurrent CAS metadata cannot be validated: {secondary:#}"))),
                },
                Ok(false)=>Err(primary),
                Err(secondary)=>Err(primary.context(format!("Cannot inspect concurrent CAS metadata: {secondary:#}"))),
            }
        }
    }
}
fn publish_tree(store:&Layout,tree:&DirectoryTree)->Result<String> {
    let id=tree.identity()?;let path=tree_path(store,&id)?;
    publish_metadata(&path,&tree.readable_json()?,|bytes| {
        let existing=DirectoryTree::parse(bytes)?;
        anyhow::ensure!(existing.identity()?==id,"CAS tree conflicts with its canonical identity");Ok(())
    })?;
    Ok(id)
}

struct Plan {entries:Vec<Planned>}
struct Planned {name:String,content:PlannedContent}
enum PlannedContent {File(Utf8PathBuf),Tree(Plan)}
#[derive(Default)]
struct Expansion {entries:usize,path_bytes:usize}
impl Expansion {
    fn charge(&mut self,path:&str)->Result<()> {
        anyhow::ensure!(path.len()<=4096&&path.split('/').count()<=MAX_DEPTH,"CAS path/depth limit exceeded");
        self.entries=self.entries.checked_add(1).context("CAS entry count overflow")?;
        self.path_bytes=self.path_bytes.checked_add(path.len()).context("CAS expanded path budget overflow")?;
        anyhow::ensure!(self.entries<=MAX_RECORDS&&self.path_bytes<=MAX_PATH_WORK,
            "CAS expanded entry/path budget exceeded");Ok(())
    }
}
fn scan(directory:&Utf8Path,relative:&str,work:&mut Expansion)->Result<Plan> {
    crate::path_safety::resolve_path(directory.as_std_path(),true)?;
    anyhow::ensure!(fs::symlink_metadata(directory)?.is_dir(),"CAS source directory changed type");
    let mut entries=Vec::new();
    for item in fs::read_dir(directory).context("Cannot enumerate CAS source directory")? {
        let item=item.context("Cannot read CAS source entry")?;
        let name=item.file_name().into_string().map_err(|_|anyhow::anyhow!("CAS source filename is not UTF-8"))?;
        let logical=if relative.is_empty(){name.clone()}else{format!("{relative}/{name}")};
        work.charge(&logical)?;
        let kind=item.file_type()?;
        let path=Utf8PathBuf::try_from(item.path())?;
        let content=if kind.is_symlink(){anyhow::bail!("Linked CAS source entries are unsupported")}
            else if kind.is_dir(){PlannedContent::Tree(scan(&path,&logical,work)?)}
            else if kind.is_file(){PlannedContent::File(path)}
            else{anyhow::bail!("Special CAS source entries are unsupported")};
        entries.push(Planned {name,content});
    }
    // Reuse the actual codec's sibling-name/type validation before any store write.
    DirectoryTree::new(entries.iter().map(|entry|TreeEntry {name:entry.name.clone(),
        kind:if matches!(&entry.content,PlannedContent::File(_)){EntryKind::File}else{EntryKind::Tree},
        hash:"0".repeat(64)}).collect())?;
    entries.sort_by(|a,b|a.name.as_bytes().cmp(b.name.as_bytes()));
    Ok(Plan {entries})
}
fn store_plan(store:&Layout,plan:&Plan)->Result<String> {
    let mut entries=Vec::new();
    for entry in &plan.entries {
        let (kind,hash)=match &entry.content {
            PlannedContent::Tree(child)=>(EntryKind::Tree,store_plan(store,child)?),
            PlannedContent::File(path)=>{
                crate::path_safety::resolve_path(path.as_std_path(),true)?;
                anyhow::ensure!(fs::symlink_metadata(path)?.is_file(),"CAS source file changed type");
                let mut file=fs::File::open(path).context("Cannot open current CAS source file")?;
                anyhow::ensure!(file.metadata()?.is_file(),"CAS source handle is not regular");
                (EntryKind::File,crate::cas::store_object(&mut file,&store.objects)?)
            }
        };
        entries.push(TreeEntry {name:entry.name.clone(),kind,hash});
    }
    publish_tree(store,&DirectoryTree::new(entries)?)
}

/// Copy/hash selected bytes and publish immutable directory objects; return the content root.
/// Content and metadata store bases may be equal (remote) or different (project metadata).
/// Scan links/special entries/aliases and source/storage overlap before writes. Failed I/O
/// may leave unreferenced objects; this does not publish a successful snapshot or run GC.
/// Inputs are observations, not an atomic consistent snapshot of a concurrently edited dataset.
pub fn ingest_directory(project_root:&Utf8Path,content:&Utf8Path,metadata:&Utf8Path,source:&Utf8Path)->Result<String> {
    let store=layout(content,metadata)?;writer_paths(project_root,&store)?;
    let source=source_path(project_root,&store,source)?;
    anyhow::ensure!(source.is_dir(),"CAS source must be an existing directory");
    let plan=scan(&source,"",&mut Expansion::default())?;
    store_plan(&store,&plan)
}

#[derive(Default)]
struct Graph {
    metadata_bytes:usize,
    trees:HashMap<String,DirectoryTree>,
    verified_files:HashSet<String>,
    active:HashSet<String>,
    expansion:Expansion,
}
impl Graph {
    fn tree(&mut self,store:&Layout,id:&str)->Result<DirectoryTree> {
        if let Some(value)=self.trees.get(id){return Ok(value.clone())}
        anyhow::ensure!(self.trees.len()<MAX_RECORDS,"CAS unique directory-object limit exceeded");
        let (tree,bytes)=read_tree(store,id)?;
        self.metadata_bytes=self.metadata_bytes.checked_add(bytes).context("CAS graph metadata budget overflow")?;
        anyhow::ensure!(self.metadata_bytes<=GRAPH_METADATA_BYTES,"CAS graph metadata byte limit exceeded");
        self.trees.insert(id.into(),tree.clone());Ok(tree)
    }
    fn file(&mut self,store:&Layout,id:&str)->Result<()> {
        if self.verified_files.contains(id){return Ok(())}
        let path=file_path(store,id)?;
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind()==std::io::ErrorKind::NotFound=>anyhow::bail!("Missing CAS file object {id}"),
            Err(error)=>return Err(error).context("Cannot inspect CAS file object"),
            Ok(metadata)=>anyhow::ensure!(metadata.is_file(),"CAS file object is not regular"),
        }
        let actual=crate::cas::hash_file(&path)?;
        anyhow::ensure!(actual==id,"Corrupt CAS file-object bytes: expected {id}, observed {actual}");
        self.verified_files.insert(id.into());Ok(())
    }
    fn walk(&mut self,store:&Layout,id:&str,prefix:&str,depth:usize,files:&mut Vec<FileRecord>,empty:&mut Vec<String>)->Result<()> {
        anyhow::ensure!(depth<=MAX_DEPTH,"CAS graph depth limit exceeded");
        if !prefix.is_empty(){self.expansion.charge(prefix)?;}
        anyhow::ensure!(self.active.insert(id.into()),"Cyclic CAS tree references");
        let tree=self.tree(store,id)?;
        if tree.entries.is_empty()&&!prefix.is_empty(){empty.push(prefix.into());}
        for entry in tree.entries {
            let length=prefix.len()+usize::from(!prefix.is_empty())+entry.name.len();
            anyhow::ensure!(length<=4096,"CAS expanded path length limit exceeded");
            let name=if prefix.is_empty(){entry.name}else{format!("{prefix}/{}",entry.name)};
            match entry.kind {
                EntryKind::File=>{
                    anyhow::ensure!(depth<MAX_DEPTH,"CAS graph path depth limit exceeded");
                    self.expansion.charge(&name)?;self.file(store,&entry.hash)?;
                    files.push(FileRecord {path:name,hash:entry.hash});
                },
                EntryKind::Tree=>self.walk(store,&entry.hash,&name,depth+1,files,empty)?,
            }
        }
        self.active.remove(id);Ok(())
    }
}

/// Fully verified graph observation, not a guarantee against later out-of-band mutation.
#[derive(Clone,Debug)]
pub struct VerifiedSnapshot {
    /// Canonical v1 snapshot identity.
    pub snapshot_id:String,
    /// Version/date and logical directory roots.
    pub snapshot:TreeSnapshot,
    /// Complete observed names/hashes and empty-directory leaves, per label.
    pub directories:Vec<FlatDirectory>,
}
impl VerifiedSnapshot {
    /// Export one complete readable flat view if it fits the flat codec's explicit limits.
    /// No truncation or cross-version reconstruction is substituted on a limit error.
    pub fn readable_flat_json(&self)->Result<Vec<u8>> {
        FlatSnapshot::new(self.snapshot.version.clone(),self.snapshot.created_at.clone(),self.directories.clone())?.readable_json()
    }
}
fn verified_graph(store:&Layout,snapshot:&TreeSnapshot)->Result<(Graph,Vec<FlatDirectory>)> {
    // Validate supplied metadata as well as loaded references before graph traversal.
    let snapshot=TreeSnapshot::parse(&snapshot.readable_json()?)?;
    let mut graph=Graph::default();let mut directories=Vec::new();
    for root in snapshot.directories {
        let mut files=Vec::new();let mut empty=Vec::new();
        graph.walk(store,&root.tree,"",0,&mut files,&mut empty)?;
        files.sort_by(|a,b|a.path.as_bytes().cmp(b.path.as_bytes()));empty.sort();
        directories.push(FlatDirectory {label:root.label,files,empty_directories:empty});
    }
    Ok((graph,directories))
}

fn verify_roots(store:&Layout,snapshot:&TreeSnapshot)->Result<Vec<FlatDirectory>> {
    verified_graph(store,snapshot).map(|(_,directories)|directories)
}

/// Publish one immutable remote/project snapshot reference only after its complete graph verifies.
/// Identical supplied metadata is idempotent; no current time, mutable latest index or ledger
/// row is injected. A failed metadata write may leave objects, never a successful partial graph.
pub fn publish_snapshot(project_root:&Utf8Path,content:&Utf8Path,metadata:&Utf8Path,snapshot:&TreeSnapshot)->Result<String> {
    let store=layout(content,metadata)?;writer_paths(project_root,&store)?;
    anyhow::ensure!(!snapshot.directories.is_empty(),"Snapshot publication requires selected directory roots");
    verify_roots(&store,snapshot)?;
    let id=snapshot.identity()?;let path=snapshot_path(&store,&id)?;
    publish_metadata(&path,&snapshot.readable_json()?,|bytes| {
        let existing=TreeSnapshot::parse(bytes)?;
        anyhow::ensure!(existing.identity()?==id,"CAS snapshot conflicts with its canonical identity");Ok(())
    })?;
    Ok(id)
}

/// Read an explicit v1 reference and validate its canonical ID; does not infer a legacy format.
/// Missing/corrupt metadata fails clearly. This function alone does not verify referenced data.
pub fn read_snapshot(content:&Utf8Path,metadata:&Utf8Path,id:&str)->Result<TreeSnapshot> {
    let store=layout(content,metadata)?;
    let bytes=metadata_bytes(&snapshot_path(&store,id)?,&format!("snapshot reference {id}"))?;
    let snapshot=TreeSnapshot::parse(&bytes).with_context(||format!("Malformed CAS snapshot reference {id}"))?;
    let actual=snapshot.identity()?;
    anyhow::ensure!(actual==id,"Corrupt CAS snapshot-reference identity: expected {id}, observed {actual}");Ok(snapshot)
}

/// Verify an explicitly selected v1 snapshot's tree identities, names and actual file bytes.
/// No local project index is required and no files/claims are written. Bounds cover unique
/// metadata (64 MiB), depth (64), expanded entries (100,000) and path text (16 MiB).
/// A root-reference file alone is not treated as a complete archive.
pub fn verify_snapshot(content:&Utf8Path,metadata:&Utf8Path,id:&str)->Result<VerifiedSnapshot> {
    let store=layout(content,metadata)?;
    let snapshot=read_snapshot(content,metadata,id)?;
    let directories=verify_roots(&store,&snapshot)?;
    Ok(VerifiedSnapshot {snapshot_id:id.into(),snapshot,directories})
}


/// Outcome of verified single-label materialisation, not a remote mutation.
#[derive(Clone,Debug,serde::Serialize)]
pub struct TreeRestore {
    /// Explicit v1 snapshot reference selected by the caller.
    pub snapshot_id:String,
    /// Exact logical label restored from that snapshot.
    pub label:String,
    /// Snapshot version metadata, separate from the content root.
    pub version:String,
    /// Installed destination directory.
    pub destination:Utf8PathBuf,
    /// Whether destination-only entries were explicitly discarded.
    pub clear:bool,
    /// Number of selected file membership rows, including repeated content.
    pub files:usize,
}
fn restore_destination(root:&Utf8Path,store:&Layout,destination:&Utf8Path)->Result<Utf8PathBuf> {
    anyhow::ensure!(destination.is_absolute(),"CAS restore destination must be an absolute anchored path");
    let anchor=destination.ancestors().last().context("CAS destination has no anchor")?;
    crate::path_safety::validate_native_names(anchor,destination.strip_prefix(anchor)?.as_str(),false)?;
    let destination=physical(destination)?;project_target(root,&destination,false)?;
    let legacy=store.objects.parent().context("CAS objects have no store base")?.join("manifests");
    for namespace in [&store.objects,store.trees.parent().context("CAS trees have no namespace parent")?,
        store.snapshots.parent().context("CAS snapshots have no namespace parent")?,&legacy] {
        anyhow::ensure!(!overlaps(&destination,namespace),"CAS restore destination overlaps storage");
    }
    let parent=destination.parent().context("CAS restore destination has no parent")?;
    anyhow::ensure!(fs::symlink_metadata(parent)?.is_dir(),"CAS restore destination parent must already exist");
    if crate::yml::configuration_entry_exists(&destination)? {
        anyhow::ensure!(fs::symlink_metadata(&destination)?.is_dir(),"CAS restore destination is not a directory");
    }
    Ok(destination)
}
fn copy_object(store:&Layout,id:&str,target:&Utf8Path)->Result<()> {
    use std::io::{Read,Write};
    let source=file_path(store,id)?;
    anyhow::ensure!(fs::symlink_metadata(&source)?.is_file(),"CAS source object is not regular");
    let mut input=fs::File::open(source).context("Cannot open CAS restore object")?;
    anyhow::ensure!(input.metadata()?.is_file(),"CAS source object handle is not regular");
    let parent=target.parent().context("Materialised CAS file has no parent")?;
    fs::create_dir_all(parent)?;
    let mut output=fs::OpenOptions::new().write(true).create_new(true).open(target)?;
    let mut digest=blake3::Hasher::new();let mut buffer=[0u8;65536];
    loop {
        let count=input.read(&mut buffer)?;if count==0 {break;}
        digest.update(&buffer[..count]);output.write_all(&buffer[..count])?;
    }
    output.flush()?;output.sync_all()?;
    anyhow::ensure!(digest.finalize().to_hex().as_str()==id,"CAS object changed while materialising expected digest {id}");
    Ok(())
}
fn observed(plan:&Plan,prefix:&str,files:&mut Vec<FileRecord>,empty:&mut Vec<String>,directories:&mut HashSet<String>)->Result<()> {
    if !prefix.is_empty(){directories.insert(prefix.into());}
    if plan.entries.is_empty()&&!prefix.is_empty(){empty.push(prefix.into());}
    for entry in &plan.entries {
        let name=if prefix.is_empty(){entry.name.clone()}else{format!("{prefix}/{}",entry.name)};
        match &entry.content {
            PlannedContent::File(path)=>files.push(FileRecord {path:name,hash:crate::cas::hash_file(path)?}),
            PlannedContent::Tree(child)=>observed(child,&name,files,empty,directories)?,
        }
    }
    Ok(())
}
fn verify_materialised(stage:&Utf8Path,expected:&FlatDirectory,clear:bool)->Result<()> {
    let plan=scan(stage,"",&mut Expansion::default())?;
    let mut files=Vec::new();let mut empty=Vec::new();let mut directories=HashSet::new();
    observed(&plan,"",&mut files,&mut empty,&mut directories)?;
    // scan validates each actual directory's names/types under the graph's
    // expanded limits. Do not impose the optional flat-export prefix-work bound.
    files.sort_by(|left,right|left.path.as_bytes().cmp(right.path.as_bytes()));empty.sort();
    let actual=FlatDirectory {label:expected.label.clone(),files,empty_directories:empty};
    if clear {anyhow::ensure!(&actual==expected,"Materialised CAS membership or bytes differ from selected inventory");}
    else {
        let files:HashMap<_,_>=actual.files.iter().map(|file|(&file.path,&file.hash)).collect();
        for file in &expected.files {
            anyhow::ensure!(files.get(&file.path).copied()==Some(&file.hash),"Materialised CAS file is missing or corrupt: {}",file.path);
        }
        for path in &expected.empty_directories {
            anyhow::ensure!(directories.contains(path),"Materialised CAS directory is missing or has a native name alias: {path}");
        }
    }
    Ok(())
}

/// Recover one explicit logical label through verified private staging and the existing installer.
/// The complete selected snapshot is deeply verified before destination work. No project ledger
/// or working source copy is needed. The destination is absolute, its parent must already exist,
/// and it cannot overlap project controls or the CAS namespaces. No source/store bytes change.
///
/// With clear=false, destination-only entries survive; selected files still receive exact bytes.
/// With clear=true, final membership matches the selected inventory, including empty directories.
/// Actual private-stage bytes are checked again before any existing destination is renamed.
/// Recovery uses the ordinary installer's bounded rollback/cleanup behavior, not crash atomicity,
/// distributed locking, permission restoration or protection against arbitrary concurrent writers.
pub fn restore_label(project_root:&Utf8Path,content:&Utf8Path,metadata:&Utf8Path,
    snapshot_id:&str,label:&str,destination:&Utf8Path,clear:bool)->Result<TreeRestore> {
    crate::path_safety::validate_directory_label(label)?;
    let store=layout(content,metadata)?;
    let destination=restore_destination(project_root,&store,destination)?;
    let verified=verify_snapshot(content,metadata,snapshot_id)?;
    let selected=verified.directories.iter().find(|directory|directory.label==label)
        .context("Selected CAS snapshot has no requested logical label")?;
    let parent=destination.parent().context("CAS destination has no parent")?;
    let staging=tempfile::Builder::new().prefix(".proj-cas-restore-").tempdir_in(parent)?;
    let source=Utf8PathBuf::try_from(staging.path().join("selected"))?;
    let execution=(||->Result<TreeRestore> {
    fs::create_dir(&source)?;
    for directory in &selected.empty_directories {fs::create_dir_all(source.join(directory))?;}
    for file in &selected.files {copy_object(&store,&file.hash,&source.join(&file.path))?;}
    verify_materialised(&source,selected,true)?;
    crate::local_remote::install_directory_checked(project_root,&source,&destination,clear,|stage| {
        restore_destination(project_root,&store,&destination)?;
        verify_materialised(stage,selected,clear)
    })?;
    Ok(TreeRestore {snapshot_id:snapshot_id.into(),label:label.into(),version:verified.snapshot.version,
        destination:destination.clone(),clear,files:selected.files.len()})
    })();
    let cleanup=crate::fs_utils::close_owned_staging(staging);
    match (execution,cleanup) {
        (Err(error),Err(cleanup))=>Err(error.context(format!("CAS source staging cleanup also failed: {cleanup:#}"))),
        (Err(error),Ok(()))=>Err(error),
        (Ok(_),Err(cleanup))=>Err(cleanup.context(format!("CAS destination installed at {destination}, but source staging cleanup failed"))),
        (Ok(result),Ok(()))=>Ok(result),
    }
}


fn source_path(root:&Utf8Path,store:&Layout,source:&Utf8Path)->Result<Utf8PathBuf> {
    let source=physical(source)?;
    for target in [&store.objects,&store.trees,&store.snapshots,
        &physical(&root.join(".proj"))?,&physical(&root.join(".projr"))?] {
        anyhow::ensure!(!overlaps(&source,target),"CAS source overlaps storage or project metadata");
    }
    Ok(source)
}
/// Read-only routing validation allows source directories produced later by analysis.
pub(crate) fn preflight_storage(root:&Utf8Path,content:&Utf8Path,metadata:&Utf8Path,sources:&[Utf8PathBuf])->Result<()> {
    let store=layout(content,metadata)?;writer_paths(root,&store)?;
    for source in sources {source_path(root,&store,source)?;}Ok(())
}
/// Validate all selected source names/types and the aggregate graph expansion budget
/// before entering this target's distribution. No source bytes or store paths are written.
pub(crate) fn validate_sources(root:&Utf8Path,content:&Utf8Path,metadata:&Utf8Path,sources:&[Utf8PathBuf])->Result<()> {
    let store=layout(content,metadata)?;writer_paths(root,&store)?;let mut work=Expansion::default();
    for source in sources {
        let source=source_path(root,&store,source)?;
        anyhow::ensure!(source.is_dir(),"CAS source must be an existing directory");
        scan(&source,"",&mut work)?;
    }
    Ok(())
}
/// Copy verified immutable tree metadata only; raw blobs stay in their common content base.
/// Snapshot references are not published here, so callers can finish all required copies first.
pub(crate) fn copy_graph(root:&Utf8Path,content:&Utf8Path,from:&Utf8Path,to:&Utf8Path,snapshot:&TreeSnapshot)->Result<()> {
    let source=layout(content,from)?;let target=layout(content,to)?;writer_paths(root,&target)?;
    let (graph,_)=verified_graph(&source,snapshot)?;
    let mut trees:Vec<_>=graph.trees.into_iter().collect();trees.sort_by(|a,b|a.0.cmp(&b.0));
    for (id,tree) in trees {
        anyhow::ensure!(publish_tree(&target,&tree)?==id,"Copied CAS directory identity changed");
    }
    Ok(())
}
fn project_reference_namespace(root:&Utf8Path,metadata:&Utf8Path)->Result<Utf8PathBuf> {
    let root=physical(root)?;
    let relative=metadata.strip_prefix(&root).context("Project CAS references must be inside the selected project")?;
    anyhow::ensure!(relative.starts_with(".proj/remotes"),"Project CAS references require the managed remotes metadata namespace");
    crate::path_safety::validate_native_names(&root,relative.as_str(),false)?;
    let namespace=physical(metadata)?.join("references/v1");
    project_target(&root,&namespace,true)?;crate::cas::checked_directory(&namespace)?;
    crate::path_safety::validate_native_names(metadata,"references/v1",false)?;
    Ok(namespace)
}
/// Check the project reference namespace without creating directories or claiming an archive.
pub(crate) fn preflight_project_reference(root:&Utf8Path,metadata:&Utf8Path)->Result<()> {
    project_reference_namespace(root,metadata)?;Ok(())
}
/// Persist a lightweight Git-eligible root reference after required publication succeeds.
/// This reference alone is not a complete archive and does not copy the remote tree graph.
pub(crate) fn write_project_reference(root:&Utf8Path,metadata:&Utf8Path,snapshot:&TreeSnapshot)->Result<String> {
    anyhow::ensure!(!snapshot.directories.is_empty(),"CAS root reference requires selected directory roots");
    let namespace=project_reference_namespace(root,metadata)?;let id=snapshot.identity()?;
    let path=native(&namespace,&format!("{}/{}.json",&id[..2],&id[2..]))?;
    publish_metadata(&path,&snapshot.readable_json()?,|bytes| {
        anyhow::ensure!(TreeSnapshot::parse(bytes)?.identity()?==id,"Project CAS reference conflicts with its canonical identity");Ok(())
    })?;Ok(id)
}
