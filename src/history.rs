//! Read-only views over per-attempt evidence; never edits a catalogue or changelog.
use anyhow::Context;
use camino::Utf8Path;
use serde::Serialize;
use crate::attempt_log::{self,Record,Receipt,Status,FinalOutputs,InventoryFingerprint};

/// A bounded newest-first view, serializable for the machine protocol.
#[derive(Serialize)]
pub struct History { entries:Vec<Entry> }

/// Safe summary fields only: arbitrary configuration, URLs and vendor output are not forwarded.
#[derive(Serialize)]
struct Entry {
    attempt_id:String, started_at:String, release_version:String, status:String,
    git:crate::attempt_log::GitOutcome, distribution_may_have_started:bool,
    final_files:Option<Files>,
}
/// A verified CSV snapshot is an observation, not proof that files still exist.
#[derive(Serialize)]
struct Files { source:&'static str, rows:Vec<crate::manifest::ManifestRow> }

fn valid_id(id:&str)->bool {
    id.starts_with("attempt-")&&id.len()==32&&id.bytes().all(|b|b.is_ascii_alphanumeric()||b==b'-')
}
fn terminal(status:&str)->bool {matches!(status,"completed"|"failed")}

/// Read summaries or one attempt's exact final rows without configuration, Git, locks or writes.
/// IDs are validated before joining paths. Missing/malformed/linked evidence fails closed;
/// a missing post-Git receipt is unresolved. Limits: 10,000 directory entries, 64 MiB
/// total JSON, 1 MiB per JSON, 16 MiB/100,000 rows per selected CSV; limit is 1..=100.
pub fn read(root:&Utf8Path,attempt:Option<&str>,limit:usize)->anyhow::Result<History> {
    anyhow::ensure!((1..=100).contains(&limit),"History limit must be between 1 and 100");
    if let Some(id)=attempt {anyhow::ensure!(valid_id(id),"Invalid attempt ID");}
    let directory=root.join(".proj/log");
    if !crate::yml::configuration_entry_exists(&directory)? {
        anyhow::ensure!(attempt.is_none(),"Requested attempt does not exist");
        return Ok(History {entries:Vec::new()});
    }
    crate::path_safety::resolve_path(directory.as_std_path(),true)?;
    anyhow::ensure!(std::fs::symlink_metadata(&directory)?.is_dir(),"History is not a directory");
    let mut ids=Vec::new();
    for (count,item) in std::fs::read_dir(&directory)?.enumerate() {
        anyhow::ensure!(count<10_000,"History directory exceeds 10,000 entries");
        let name=item?.file_name();
        let Some(name)=name.to_str() else {continue};
        let Some(id)=name.strip_suffix(".json") else {continue};
        if valid_id(id)&&attempt.is_none_or(|wanted|wanted==id){ids.push(id.to_owned());}
    }
    if attempt.is_some(){anyhow::ensure!(ids.len()==1,"Requested attempt does not exist");}
    let mut budget=64*1024*1024usize;
    let mut entries=Vec::new();
    for id in ids {
        let bytes=attempt_log::read(&directory.join(format!("{id}.json")))?;
        budget=budget.checked_sub(bytes.len()).context("History exceeds total JSON read budget")?;
        let record:Record=serde_json::from_slice(&bytes).context("Invalid attempt record")?;
        anyhow::ensure!(record.schema_version==1&&record.attempt_id==id&&record.local_build=="completed",
            "Attempt identity/schema mismatch");
        anyhow::ensure!(terminal(&record.status)||matches!(record.status.as_str(),"in_progress"|"pre_git_snapshot"),
            "Unknown attempt outcome");
        let started=chrono::DateTime::parse_from_rfc3339(&record.started_at).context("Invalid attempt time")?;
        let version=crate::version::ProjVersion::parse(&record.release_version).context("Invalid release version")?;
        anyhow::ensure!(version.dev==0&&version.to_string(true)==record.release_version,"Invalid release version");
        let receipt_path=directory.join(format!("{id}.git.json"));
        let receipt=if crate::yml::configuration_entry_exists(&receipt_path)? {
            let receipt_bytes=attempt_log::read(&receipt_path)?;
            budget=budget.checked_sub(receipt_bytes.len()).context("History exceeds total JSON read budget")?;
            let value:Receipt=serde_json::from_slice(&receipt_bytes).context("Invalid attempt receipt")?;
            anyhow::ensure!(value.schema_version==1&&value.attempt_id==id&&value.stage=="git_and_finalization"
                &&terminal(&value.status)&&record.status=="pre_git_snapshot"
                &&value.snapshot_hash==blake3::hash(&bytes).to_hex().as_str(),"Receipt does not match snapshot");
            Some(value)
        }else{None};
        let status=receipt.as_ref().map(|r|r.status.as_str()).unwrap_or(&record.status);
        let status=if terminal(status){status.to_owned()}else{"unresolved".into()};
        let outputs=receipt.as_ref().and_then(|r|r.final_outputs.as_ref()).or(record.final_outputs.as_ref());
        let files=if attempt.is_some(){files(root,&id,outputs,&record.release_version)?}else{None};
        let git=receipt.map(|r|r.git).unwrap_or(record.git);
        anyhow::ensure!(git.head.as_ref().is_none_or(|head|matches!(head.len(),40|64)
            &&head.bytes().all(|b|b.is_ascii_hexdigit())),"Invalid recorded Git object ID");
        entries.push((started,Entry {attempt_id:id,started_at:record.started_at,
            release_version:record.release_version,status,git,
            distribution_may_have_started:record.distribution_may_have_started,final_files:files}));
    }
    entries.sort_by(|a,b|b.0.cmp(&a.0).then_with(||b.1.attempt_id.cmp(&a.1.attempt_id)));
    Ok(History {entries:entries.into_iter().take(limit).map(|(_,entry)|entry).collect()})
}
fn files(root:&Utf8Path,id:&str,outputs:Option<&FinalOutputs>,version:&str)->anyhow::Result<Option<Files>> {
    let Some(outputs)=outputs else{return Ok(None)};
    if outputs.rows.is_some() {
        anyhow::ensure!(outputs.status==Status::Completed,"Final observation did not complete");
        return Ok(Some(Files {source:"retained_csv",rows:attempt_log::final_rows(root,id,outputs,version)?}));
    }
    // Old records only have fingerprints: use historical rows only on an exact match.
    let Some(expected)=outputs.inventory.as_ref().filter(|_|outputs.status==Status::Completed) else{return Ok(None)};
    let path=root.join("manifest.csv");
    if !crate::yml::configuration_entry_exists(&path)?{return Ok(None)}
    let bytes=attempt_log::read_bounded(&path,attempt_log::FINAL_CSV_LIMIT)?;
    let rows:Vec<_>=crate::manifest_csv::decode_bounded(&bytes,attempt_log::FINAL_ROW_LIMIT)?
        .into_iter().filter(|row|row.version==version&&expected.labels.contains(&row.label)).collect();
    if InventoryFingerprint::capture(&rows,version)?!=*expected{return Ok(None)}
    Ok(Some(Files {source:"project_manifest",rows}))
}

// Escape untrusted filenames as literal table text, retaining Unicode without raw markup/control bytes.
fn literal(text:&str)->String {
    let mut result=String::new();
    for c in text.chars() {
        if c.is_control(){result.push_str(&format!("\\u{{{:x}}}",c as u32));}
        else if c.is_ascii_punctuation(){result.push_str(&format!("&#{};",c as u32));}
        else{result.push(c)}
    }
    result
}
impl History {
    /// Generate human-readable Markdown from the same safe projection as machine JSON.
    /// No file is written; unavailable final rows and unresolved attempts remain explicit.
    pub fn markdown(&self)->String {
        let mut text=String::from("# Build history\n\n| Attempt | Started | Release | Outcome |\n| --- | --- | --- | --- |\n");
        for entry in &self.entries {
            text.push_str(&format!("| {} | {} | {} | {} |\n",literal(&entry.attempt_id),
                literal(&entry.started_at),literal(&entry.release_version),literal(&entry.status)));
        }
        for entry in &self.entries {
            text.push_str(&format!("\n## {}\n\n",literal(&entry.attempt_id)));
            if let Some(files)=&entry.final_files {
                text.push_str(&format!("Final observation ({}):\n\n| Label | File | MD5 |\n| --- | --- | --- |\n",literal(files.source)));
                for row in &files.rows {text.push_str(&format!("| {} | {} | {} |\n",
                    literal(&row.label),literal(&row.r#fn),literal(&row.hash)));}
                text.push('\n');
            }else{text.push_str("Final rows not loaded or unavailable; use --attempt for details.\n");}
        }
        text
    }
}
