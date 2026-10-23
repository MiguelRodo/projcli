//! Per-attempt local-release evidence, without a central mutable catalogue.
use std::{fs, io::{Read, Write}};
use anyhow::Context;
use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};

const LIMIT: u64 = 1_048_576;

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

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Operation {
    provider: String,
    configured_targets: usize,
    status: Status,
    known_data_effects: Option<usize>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    schema_version: u32,
    attempt_id: String,
    started_at: String,
    recorded_at: String,
    profiles: Vec<String>,
    build_mode: String,
    version_after: Option<String>,
    version_before: String,
    release_version: String,
    local_build: String,
    stage: String,
    status: String,
    distribution_may_have_started: bool,
    operations: Vec<Operation>,
    git: GitOutcome,
    source_head: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    schema_version: u32,
    attempt_id: String,
    snapshot_hash: String,
    recorded_at: String,
    status: String,
    stage: String,
    version_after: Option<String>,
    git: GitOutcome,
}

/// The snapshot is mutable until Git starts, then immutable; the outcome is a new file.
pub(crate) struct AttemptLog {
    path: Utf8PathBuf,
    record: Record,
    sealed_hash: Option<String>,
}

fn timestamp() -> String { chrono::Utc::now().to_rfc3339() }

fn read(path: &Utf8Path) -> anyhow::Result<Vec<u8>> {
    crate::path_safety::resolve_path(path.as_std_path(), true)?;
    let metadata = fs::symlink_metadata(path)?;
    anyhow::ensure!(metadata.is_file() && metadata.len() <= LIMIT, "Attempt evidence must be a bounded regular file");
    let mut bytes=Vec::new();
    fs::File::open(path).with_context(||format!("Cannot read attempt evidence at {path}"))?.take(LIMIT+1).read_to_end(&mut bytes)?;
    anyhow::ensure!(bytes.len() as u64 <= LIMIT, "Attempt evidence is too large");
    Ok(bytes)
}

fn create(path: &Utf8Path, bytes: &[u8]) -> anyhow::Result<()> {
    crate::path_safety::resolve_path(path.as_std_path(), true)?;
    let mut file=fs::OpenOptions::new().write(true).create_new(true).open(path)
        .context("Cannot create unique attempt evidence")?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

impl AttemptLog {
    /// Start durable evidence only after the successful local-release boundary.
    pub(crate) fn start(root:&Utf8Path, attempt:&crate::attempt::Attempt, profiles:Vec<String>,
        before:String, release:String, mode:&str, git:bool, push:bool, source_head:Option<String>) -> anyhow::Result<Self> {
        let directory=root.join(".proj/log");
        crate::path_safety::validate_native_names(root,".proj/log",false)?;
        crate::path_safety::resolve_path(directory.as_std_path(),true)?;
        fs::create_dir_all(&directory)?;
        let path=directory.join(format!("{}.json",attempt.id()));
        let record=Record {schema_version:1,attempt_id:attempt.id().to_owned(),
            started_at:attempt.started_at().to_owned(),recorded_at:timestamp(),profiles,build_mode:mode.into(),version_after:None,
            version_before:before,release_version:release,local_build:"completed".into(),
            stage:"local_release".into(),status:"in_progress".into(),distribution_may_have_started:false,
            operations:Vec::new(),git:GitOutcome::requested(git,push),source_head};
        create(&path,&serde_json::to_vec_pretty(&record)?)?;
        Ok(Self {path,record,sealed_hash:None})
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
    pub(crate) fn finish(&mut self,success:bool,git:GitOutcome,version_after:Option<String>,
        witness:impl FnOnce(&str,&str,&str)->anyhow::Result<()>) -> anyhow::Result<()> {
        let status=if success {"completed"}else{"failed"};
        if let Some(hash)=&self.sealed_hash {
            let receipt=Receipt {schema_version:1,attempt_id:self.record.attempt_id.clone(),
                snapshot_hash:hash.clone(),recorded_at:timestamp(),
                status:status.into(),stage:"git_and_finalization".into(),version_after,git};
            let path=self.path.with_file_name(format!("{}.git.json",self.record.attempt_id));
            let bytes=serde_json::to_vec_pretty(&receipt)?;
            create(&path,&bytes)?;
            witness(&receipt.attempt_id,&receipt.snapshot_hash,&blake3::hash(&bytes).to_hex().to_string())?;
        }else{
            self.record.status=status.into();
            self.record.git=git;
            self.record.version_after=version_after;
            self.save()?;
        }
        Ok(())
    }
}

/// Recognise only a new outcome receipt tied to an identical snapshot in HEAD.
/// A changed/tracked receipt or arbitrary neighbouring file is never exempted.
pub(crate) fn owned_receipt(root:&Utf8Path,relative:&str,committed:impl FnOnce(&str)->anyhow::Result<Vec<u8>>,
    witness:impl FnOnce(&str,&str,&str)->anyhow::Result<bool>) -> anyhow::Result<bool> {
    let Some(id)=relative.strip_prefix(".proj/log/").and_then(|s|s.strip_suffix(".git.json")) else{return Ok(false)};
    if !id.starts_with("attempt-") || id.len()!=32 || !id.bytes().all(|b|b.is_ascii_alphanumeric()||b==b'-') {return Ok(false);}
    let receipt_bytes=read(&root.join(relative))?;
    let receipt:Receipt=match serde_json::from_slice(&receipt_bytes) {Ok(value)=>value,Err(_)=>return Ok(false)};
    if receipt.schema_version!=1||receipt.attempt_id!=id
        ||!matches!(receipt.status.as_str(),"completed"|"failed")
        ||receipt.stage!="git_and_finalization" {return Ok(false);}
    let snapshot=format!(".proj/log/{id}.json");
    let bytes=match committed(&format!("HEAD:./{snapshot}")) {Ok(bytes)=>bytes,Err(_)=>return Ok(false)};
    if read(&root.join(&snapshot))?!=bytes || receipt.snapshot_hash!=blake3::hash(&bytes).to_hex().as_str() {return Ok(false);}
    let record:Record=match serde_json::from_slice(&bytes) {Ok(value)=>value,Err(_)=>return Ok(false)};
    Ok(record.schema_version==1&&record.attempt_id==id&&record.git.commit==Status::Pending&&record.status=="pre_git_snapshot"
        &&witness(id,&receipt.snapshot_hash,&blake3::hash(&receipt_bytes).to_hex().to_string())?)
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
