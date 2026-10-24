//! Per-attempt local-release evidence, without a central mutable catalogue.
use std::{fs, io::{Read, Write}};
use anyhow::Context;
use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};

pub(crate) const LIMIT: u64 = 1_048_576;
pub(crate) const FINAL_CSV_LIMIT:u64=16*1024*1024;
pub(crate) const FINAL_ROW_LIMIT:usize=100_000;

/// Operation results describe returned status, never proof that a failed remote write had no effects.
#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Status { NotRequested, NotAttempted, Pending, Started, Completed, Failed }

/// Git component results are recorded after the operation, never recursively committed.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct GitOutcome {
    pub(crate) commit: Status,
    pub(crate) push: Status,
    pub(crate) head: Option<String>,
}

impl GitOutcome {
    /// Initial component states distinguish disabled operations from those not yet attempted.
    pub(crate) fn requested(commit: bool, push: bool) -> Self {
        Self { commit: if commit {Status::NotAttempted} else {Status::NotRequested},
            push: if push {Status::NotAttempted} else {Status::NotRequested}, head: None }
    }
}

/// One provider operation and its returned outcome.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Operation {
    pub(crate) provider: String,
    pub(crate) configured_targets: usize,
    pub(crate) status: Status,
    pub(crate) known_data_effects: Option<usize>,
}

/// Primary local-release evidence; immutable once offered to Git.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Record {
    pub(crate) schema_version: u32,
    pub(crate) attempt_id: String,
    pub(crate) started_at: String,
    pub(crate) recorded_at: String,
    pub(crate) profiles: Vec<String>,
    pub(crate) build_mode: String,
    pub(crate) version_after: Option<String>,
    pub(crate) version_before: String,
    pub(crate) release_version: String,
    pub(crate) local_build: String,
    pub(crate) stage: String,
    pub(crate) status: String,
    pub(crate) distribution_may_have_started: bool,
    pub(crate) operations: Vec<Operation>,
    pub(crate) git: GitOutcome,
    pub(crate) source_head: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) startup_provenance: Option<crate::provenance::Snapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) inventory: Option<InventoryEvidence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) final_outputs: Option<FinalOutputs>,
}

/// Later Git/finalization evidence bound to exact primary bytes.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Receipt {
    pub(crate) schema_version: u32,
    pub(crate) attempt_id: String,
    pub(crate) snapshot_hash: String,
    pub(crate) recorded_at: String,
    pub(crate) status: String,
    pub(crate) stage: String,
    pub(crate) version_after: Option<String>,
    pub(crate) git: GitOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) final_outputs: Option<FinalOutputs>,
}

/// Fingerprints a phase's canonical CSV rows without copying large inventories into JSON.
/// The current release rows remain in manifest.csv; MD5 retains its existing semantics.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct InventoryFingerprint {
    pub(crate) csv_version: String,
    pub(crate) rows: usize,
    pub(crate) files: usize,
    pub(crate) labels: Vec<String>,
    pub(crate) canonical_csv_blake3: String,
}

impl InventoryFingerprint {
    /// Hash the existing CSV codec's exact current rows, including paths and empty markers.
    pub(crate) fn capture(rows: &[crate::manifest::ManifestRow], version: &str) -> anyhow::Result<Self> {
        let mut labels:Vec<_>=rows.iter().map(|row|row.label.clone()).collect();
        labels.sort();labels.dedup();
        Ok(Self {csv_version:version.into(),rows:rows.len(),
            files:rows.iter().filter(|row|!row.r#fn.is_empty()).count(),labels,
            canonical_csv_blake3:blake3::hash(&crate::manifest_csv::encode(rows)?).to_hex().to_string()})
    }
}

/// Current-phase inventory witnesses, separate from the cumulative CSV history.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct InventoryEvidence {
    pub(crate) inputs: InventoryFingerprint,
    pub(crate) release_outputs: InventoryFingerprint,
}

/// Reference to one immutable four-column CSV of actually observed final rows.
#[derive(Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FinalRowsRef {
    pub(crate) file:String,
    pub(crate) bytes:u64,
    pub(crate) blake3:String,
}

/// Final observed outputs after finalization was attempted; failure is not an empty inventory.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FinalOutputs {
    pub(crate) status: Status,
    pub(crate) inventory: Option<InventoryFingerprint>,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub(crate) rows:Option<FinalRowsRef>,
    #[serde(skip)]
    captured_rows:Option<Vec<crate::manifest::ManifestRow>>,
}

impl FinalOutputs {
    /// Observed rows use the existing codec; they are persisted beside the terminal record.
    pub(crate) fn observed(rows:Vec<crate::manifest::ManifestRow>,version:&str)->anyhow::Result<Self> {
        let inventory=Some(InventoryFingerprint::capture(&rows,version)?);
        Ok(Self {status:Status::Completed,inventory,rows:None,captured_rows:Some(rows)})
    }
    /// Disabled observation is not an empty-directory observation.
    pub(crate) fn not_requested()->Self {
        Self {status:Status::NotRequested,inventory:None,rows:None,captured_rows:None}
    }
    /// Failed observation leaves earlier inventory evidence intact.
    pub(crate) fn failed()->Self {
        Self {status:Status::Failed,inventory:None,rows:None,captured_rows:None}
    }
}

/// Local-only failed attempt diagnostic: no raw error, configuration, host or command data.
#[derive(Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LocalFailure {
    pub(crate) schema_version:u32,
    pub(crate) attempt_id:String,
    pub(crate) started_at:String,
    pub(crate) recorded_at:String,
    pub(crate) stage:String,
    pub(crate) status:String,
    pub(crate) distribution_may_have_started:bool,
    pub(crate) version_before:Option<String>,
    pub(crate) version_after:Option<String>,
}

/// Best-effort numeric version evidence: reject links, special files and oversized input.
pub(crate) fn diagnostic_version(root:&Utf8Path)->Option<String> {
    let bytes=read_bounded(&root.join("VERSION"),4096).ok()?;
    let text=std::str::from_utf8(&bytes).ok()?;
    crate::version::ProjVersion::parse(text).map(|v|v.to_string(true))
}

/// Retain an owned production failure after ordinary recovery; never replace prior diagnostics.
/// Only this reserved directory is excluded from framework Git staging/status.
pub(crate) fn retain_local_failure(root:&Utf8Path,attempt:&crate::attempt::Attempt,stage:&str,
    version_before:Option<String>,version_after:Option<String>)->anyhow::Result<()> {
    let directory=root.join(".proj/log/local-failures");
    crate::path_safety::validate_native_names(root,".proj/log/local-failures",false)?;
    crate::path_safety::resolve_path(directory.as_std_path(),true)?;
    fs::create_dir_all(&directory).context("Cannot create local failure diagnostics directory")?;
    crate::path_safety::resolve_path(directory.as_std_path(),true)?;
    let ignore=directory.join(".gitignore");
    if crate::yml::configuration_entry_exists(&ignore)? {
        anyhow::ensure!(read(&ignore)?==b"*\n","Local diagnostic ignore policy changed; preserved");
    } else {create(&ignore,b"*\n")?;}
    let record=LocalFailure {schema_version:1,attempt_id:attempt.id().into(),
        started_at:attempt.started_at().into(),recorded_at:timestamp(),stage:stage.into(),
        status:"failed".into(),distribution_may_have_started:false,version_before,version_after};
    create(&directory.join(format!("{}.json",attempt.id())),&serde_json::to_vec_pretty(&record)?)
        .context("Cannot retain local failed-attempt diagnostic")
}

/// The snapshot is mutable until Git starts, then immutable; the outcome is a new file.
pub(crate) struct AttemptLog {
    path: Utf8PathBuf,
    record: Record,
    sealed_hash: Option<String>,
    created_directory: bool,
}

fn timestamp() -> String { chrono::Utc::now().to_rfc3339() }

/// Read one bounded guarded JSON evidence file.
pub(crate) fn read(path: &Utf8Path) -> anyhow::Result<Vec<u8>> {
    read_bounded(path,LIMIT)
}

/// Read regular evidence with an explicit byte budget and no linked path components.
pub(crate) fn read_bounded(path:&Utf8Path,limit:u64)->anyhow::Result<Vec<u8>> {
    crate::path_safety::resolve_path(path.as_std_path(), true)?;
    let metadata = fs::symlink_metadata(path)?;
    anyhow::ensure!(metadata.is_file() && metadata.len() <= limit, "Attempt evidence must be a bounded regular file");
    let mut bytes=Vec::new();
    fs::File::open(path).with_context(||format!("Cannot read attempt evidence at {path}"))?.take(limit+1).read_to_end(&mut bytes)?;
    anyhow::ensure!(bytes.len() as u64 <= limit, "Attempt evidence is too large");
    Ok(bytes)
}

fn create(path: &Utf8Path, bytes: &[u8]) -> anyhow::Result<()> {
    crate::path_safety::resolve_path(path.as_std_path(), true)?;
    let parent=path.parent().context("Attempt evidence has no directory")?;
    let mut file=tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    crate::path_safety::resolve_path(path.as_std_path(), true)?;
    file.persist_noclobber(path).map_err(|error|error.error)
        .context("Cannot create unique attempt evidence")?;
    Ok(())
}

impl AttemptLog {
    /// Prepare evidence without installing a record until start or completed_local succeeds.
    pub(crate) fn prepare(root:&Utf8Path, attempt:&crate::attempt::Attempt, profiles:Vec<String>,
        before:String, release:String, mode:&str, git:bool, push:bool, source_head:Option<String>, inventory:Option<InventoryEvidence>, startup:Option<crate::provenance::Snapshot>) -> anyhow::Result<Self> {
        let directory=root.join(".proj/log");
        crate::path_safety::validate_native_names(root,".proj/log",false)?;
        crate::path_safety::resolve_path(directory.as_std_path(),true)?;
        let created_directory=match fs::create_dir(&directory) {
            Ok(())=>true,
            Err(error) if error.kind()==std::io::ErrorKind::AlreadyExists=>{
                anyhow::ensure!(directory.is_dir(),"Attempt log path must be a directory");
                false
            },
            Err(error)=>return Err(error).context("Cannot create attempt log directory"),
        };
        let path=directory.join(format!("{}.json",attempt.id()));
        let record=Record {schema_version:1,attempt_id:attempt.id().to_owned(),
            started_at:attempt.started_at().to_owned(),recorded_at:timestamp(),profiles,build_mode:mode.into(),version_after:None,
            version_before:before,release_version:release,local_build:"completed".into(),
            stage:"local_release".into(),status:"in_progress".into(),distribution_may_have_started:false,
            operations:Vec::new(),git:GitOutcome::requested(git,push),source_head,startup_provenance:startup,inventory,final_outputs:None};
        Ok(Self {path,record,sealed_hash:None,created_directory})
    }

    /// Install the initial publication snapshot before any provider or Git entry.
    pub(crate) fn start(self) -> anyhow::Result<Self> {
        create(&self.path,&serde_json::to_vec_pretty(&self.record)?)?;
        Ok(self)
    }

    /// Remove only our intact unsealed snapshot when no distribution was entered.
    /// The caller then writes one local-only diagnostic after recovery/finalization.
    pub(crate) fn discard_undistributed(&self)->anyhow::Result<()> {
        anyhow::ensure!(self.sealed_hash.is_none()&&!self.record.distribution_may_have_started
            &&self.record.operations.is_empty(),"Cannot discard distribution evidence");
        anyhow::ensure!(read(&self.path)?==serde_json::to_vec_pretty(&self.record)?,
            "Undistributed attempt evidence changed; preserved");
        if let Some(outputs)=&self.record.final_outputs {
            if let Some(reference)=&outputs.rows {
                let root=self.path.parent().and_then(|p|p.parent()).and_then(|p|p.parent())
                    .context("Attempt evidence has no project root")?;
                final_rows(root,&self.record.attempt_id,outputs,&self.record.release_version)?;
                let csv=self.path.with_file_name(&reference.file);
                crate::path_safety::resolve_path(csv.as_std_path(),true)?;
                fs::remove_file(&csv)?;
            }
        }
        crate::path_safety::resolve_path(self.path.as_std_path(),true)?;
        fs::remove_file(&self.path).context("Cannot remove owned undistributed snapshot")
    }

    fn retain_rows(&self,outputs:&mut FinalOutputs)->anyhow::Result<Option<(Utf8PathBuf,String)>> {
        let Some(rows)=outputs.captured_rows.take() else{return Ok(None)};
        anyhow::ensure!(rows.len()<=FINAL_ROW_LIMIT,"Final inventory exceeds retained row limit");
        let bytes=crate::manifest_csv::encode(&rows)?;
        anyhow::ensure!(bytes.len() as u64<=FINAL_CSV_LIMIT,"Final inventory exceeds retained byte limit");
        let file=format!("{}.final.csv",self.record.attempt_id);
        let path=self.path.with_file_name(&file);
        let hash=blake3::hash(&bytes).to_hex().to_string();
        create(&path,&bytes)?;
        outputs.rows=Some(FinalRowsRef {file,bytes:bytes.len() as u64,blake3:hash.clone()});
        Ok(Some((path,hash)))
    }

    /// Install one completed plain-build record only after all recoverable work succeeds.
    /// Stage complete bytes before exclusive installation; do not truncate an existing record.
    pub(crate) fn completed_local(mut self,version_after:Option<String>,mut outputs:FinalOutputs)->anyhow::Result<()> {
        let retained=self.retain_rows(&mut outputs)?;
        self.record.status="completed".into();
        self.record.stage="end_hooks_and_next_dev".into();
        self.record.version_after=version_after;
        self.record.final_outputs=Some(outputs);
        let result=serde_json::to_vec_pretty(&self.record).map_err(anyhow::Error::from)
            .and_then(|bytes|create(&self.path,&bytes));
        if result.is_err() {
            if let Some((path,hash))=retained {
                let cleanup=(||->anyhow::Result<()> {
                    anyhow::ensure!(blake3::hash(&read_bounded(&path,FINAL_CSV_LIMIT)?).to_hex().as_str()==hash,
                        "Unpublished final CSV changed; preserved for inspection");
                    crate::path_safety::resolve_path(path.as_std_path(),true)?;
                    fs::remove_file(&path)?;
                    Ok(())
                })();
                return retain_result(result,cleanup.context("Cannot remove unpublished final CSV; preserved for inspection"));
            }
        }
        result
    }

    fn save(&mut self) -> anyhow::Result<()> {
        anyhow::ensure!(self.sealed_hash.is_none(),"Cannot rewrite an immutable pre-Git snapshot");
        self.record.recorded_at=timestamp();
        crate::path_safety::resolve_path(self.path.as_std_path(),true)?;
        // Replace via a fresh inode so hard-linked aliases are never overwritten.
        let parent=self.path.parent().context("Attempt evidence has no directory")?;
        let mut file=tempfile::NamedTempFile::new_in(parent)?;
        file.write_all(&serde_json::to_vec_pretty(&self.record)?)?;
        file.as_file().sync_all()?;
        crate::path_safety::resolve_path(self.path.as_std_path(),true)?;
        anyhow::ensure!(fs::symlink_metadata(&self.path)?.is_file(),"Attempt evidence is no longer regular");
        file.persist(&self.path).map_err(|error| error.error)?;
        Ok(())
    }

    /// Persist possible distribution activity before calling a provider; failures never imply rollback.
    pub(crate) fn operation<T>(&mut self,provider:&str,targets:usize,
        action:impl FnOnce()->anyhow::Result<T>, effects:impl FnOnce(&T)->Option<usize>) -> anyhow::Result<T> {
        if targets==0 {return action();}
        self.record.stage="distribution".into();
        self.record.distribution_may_have_started=true;
        self.record.operations.push(Operation {provider:provider.into(),configured_targets:targets,
            status:Status::Started,known_data_effects:None});
        self.save().context("Cannot retain evidence before distribution")?;
        let result=action();
        let operation=self.record.operations.last_mut().unwrap();
        operation.status=if result.is_ok(){Status::Completed}else{Status::Failed};
        operation.known_data_effects=result.as_ref().ok().and_then(effects);
        let evidence=self.save().context("Cannot retain distribution outcome; earlier attempt evidence remains");
        retain_result(result,evidence)
    }

    /// Freeze the snapshot before Git so the commit cannot claim its own later outcome.
    pub(crate) fn seal_git(&mut self) -> anyhow::Result<()> {
        self.record.distribution_may_have_started=true;
        self.record.stage="git".into();
        self.record.status="pre_git_snapshot".into();
        self.record.git.commit=Status::Pending;
        if self.record.git.push!=Status::NotRequested {self.record.git.push=Status::Pending;}
        self.save()?;
        self.sealed_hash=Some(blake3::hash(&read(&self.path)?).to_hex().to_string());
        Ok(())
    }

    /// Mark local finalization without rewriting a snapshot already offered to Git.
    pub(crate) fn finalization(&mut self) -> anyhow::Result<()> {
        if self.sealed_hash.is_none() {self.record.stage="end_hooks_and_next_dev".into();self.save()?;}
        Ok(())
    }

    /// Write a terminal local record or unique Git receipt, then register its local ownership witness.
    pub(crate) fn finish(&mut self,success:bool,git:GitOutcome,version_after:Option<String>,mut outputs:FinalOutputs,
        witness:impl FnOnce(&str,&str,&str)->anyhow::Result<()>) -> anyhow::Result<()> {
        let rows=self.retain_rows(&mut outputs).map(|_|());
        if rows.is_err(){outputs.status=Status::Failed;}
        let status=if success&&rows.is_ok() {"completed"}else{"failed"};
        let written=(||->anyhow::Result<()> {
        if let Some(hash)=&self.sealed_hash {
            let receipt=Receipt {schema_version:1,attempt_id:self.record.attempt_id.clone(),
                snapshot_hash:hash.clone(),recorded_at:timestamp(),
                status:status.into(),stage:"git_and_finalization".into(),version_after,git,final_outputs:Some(outputs)};
            let path=self.path.with_file_name(format!("{}.git.json",self.record.attempt_id));
            let bytes=serde_json::to_vec_pretty(&receipt)?;
            create(&path,&bytes)?;
            witness(&receipt.attempt_id,&receipt.snapshot_hash,&blake3::hash(&bytes).to_hex().to_string())?;
        }else{
            self.record.status=status.into();
            self.record.git=git;
            self.record.version_after=version_after;
            self.record.final_outputs=Some(outputs);
            self.save()?;
        }
        Ok(())
        })();
        retain_result(rows,written)
    }
}

impl Drop for AttemptLog {
    fn drop(&mut self) {
        // Remove only our newly created empty directory; never discard installed evidence.
        if self.created_directory {
            if let Some(parent)=self.path.parent() {let _=fs::remove_dir(parent);}
        }
    }
}

/// Recognise an exact pending receipt/CSV bundle tied to an identical snapshot in HEAD.
/// An edited/missing companion, tracked receipt or arbitrary neighbouring file is never exempted.
pub(crate) fn owned_receipt(root:&Utf8Path,relative:&str,committed:impl FnOnce(&str)->anyhow::Result<Vec<u8>>,
    witness:impl FnOnce(&str,&str,&str)->anyhow::Result<bool>) -> anyhow::Result<bool> {
    let Some(file)=relative.strip_prefix(".proj/log/") else{return Ok(false)};
    let final_csv=file.ends_with(".final.csv");
    let Some(id)=file.strip_suffix(".git.json").or_else(||file.strip_suffix(".final.csv")) else{return Ok(false)};
    if !id.starts_with("attempt-") || id.len()!=32 || !id.bytes().all(|b|b.is_ascii_alphanumeric()||b==b'-') {return Ok(false);}
    let receipt_path=root.join(format!(".proj/log/{id}.git.json"));
    if !crate::yml::configuration_entry_exists(&receipt_path)? {return Ok(false);}
    let receipt_bytes=read(&receipt_path)?;
    let receipt:Receipt=match serde_json::from_slice(&receipt_bytes) {Ok(value)=>value,Err(_)=>return Ok(false)};
    if receipt.schema_version!=1||receipt.attempt_id!=id
        ||!matches!(receipt.status.as_str(),"completed"|"failed")
        ||receipt.stage!="git_and_finalization" {return Ok(false);}
    let snapshot=format!(".proj/log/{id}.json");
    let bytes=match committed(&format!("HEAD:./{snapshot}")) {Ok(bytes)=>bytes,Err(_)=>return Ok(false)};
    if !crate::yml::configuration_entry_exists(&root.join(&snapshot))? {return Ok(false);}
    if read(&root.join(&snapshot))?!=bytes || receipt.snapshot_hash!=blake3::hash(&bytes).to_hex().as_str() {return Ok(false);}
    let record:Record=match serde_json::from_slice(&bytes) {Ok(value)=>value,Err(_)=>return Ok(false)};
    let reference=receipt.final_outputs.as_ref().and_then(|outputs|outputs.rows.as_ref());
    if final_csv&&reference.is_none(){return Ok(false);}
    if let Some(reference)=reference {
        let Some(outputs)=receipt.final_outputs.as_ref() else{return Ok(false)};
        if final_rows(root,id,outputs,&record.release_version).is_err(){return Ok(false);}
        if final_csv&&reference.file!=file{return Ok(false);}
    }
    Ok(record.schema_version==1&&record.attempt_id==id&&record.git.commit==Status::Pending&&record.status=="pre_git_snapshot"
        &&witness(id,&receipt.snapshot_hash,&blake3::hash(&receipt_bytes).to_hex().to_string())?)
}


/// Validate a retained final CSV against its exact bytes and canonical phase fingerprint.
pub(crate) fn final_rows(root:&Utf8Path,id:&str,outputs:&FinalOutputs,version:&str)
    ->anyhow::Result<Vec<crate::manifest::ManifestRow>> {
    let reference=outputs.rows.as_ref().context("No retained final CSV reference")?;
    anyhow::ensure!(reference.file==format!("{id}.final.csv"),"Invalid final CSV reference");
    let bytes=read_bounded(&root.join(".proj/log").join(&reference.file),FINAL_CSV_LIMIT)?;
    anyhow::ensure!(bytes.len() as u64==reference.bytes
        && blake3::hash(&bytes).to_hex().as_str()==reference.blake3,"Final CSV byte checksum mismatch");
    let rows=crate::manifest_csv::decode_bounded(&bytes,FINAL_ROW_LIMIT)?;
    let mut seen=std::collections::HashSet::new();
    anyhow::ensure!(rows.iter().all(|row|row.version==version&&seen.insert((&row.label,&row.r#fn))),
        "Final CSV contains duplicate or mismatched rows");
    let actual=InventoryFingerprint::capture(&rows,version)?;
    anyhow::ensure!(outputs.inventory.as_ref()==Some(&actual),"Final CSV inventory fingerprint mismatch");
    Ok(rows)
}

/// Retain both the operation failure and a subsequent evidence-writing failure.
pub(crate) fn retain_result<T>(result:anyhow::Result<T>,evidence:anyhow::Result<()>)->anyhow::Result<T> {
    match (result,evidence) {
        (Err(primary),Err(secondary))=>Err(primary.context(format!(
            "Attempt evidence update also failed: {secondary:#}; earlier evidence remains, distribution may be partial"))),
        (Err(primary),Ok(()))=>Err(primary),
        (Ok(_),Err(secondary))=>Err(secondary),
        (Ok(value),Ok(()))=>Ok(value),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn primary_failure_survives_an_evidence_failure() {
        let error=super::retain_result::<()>(Err(anyhow::anyhow!("primary publication failure")),
            Err(anyhow::anyhow!("secondary evidence failure"))).unwrap_err();
        let diagnostic=format!("{error:#}");
        assert!(diagnostic.contains("primary publication failure"));
        assert!(diagnostic.contains("secondary evidence failure"));
        assert!(diagnostic.contains("distribution may be partial"));
    }
}
