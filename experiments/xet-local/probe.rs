//! Experiment adapter only: use the unmodified upstream local Xet pipeline.
use std::{fs, path::PathBuf, sync::Arc, time::Instant};
use anyhow::{Context, Result, ensure};
use serde_json::json;
use xet_data::processing::{FileUploadSession, FileDownloadSession, XetFileInfo, Sha256Policy};
use xet_data::processing::configurations::TranslatorConfig;
use xet_runtime::core::XetContext;

// Harness-owned reference/barrier commits. This is not an upstream Xet transaction API.
fn atomic_json(path: &std::path::Path, value: &serde_json::Value) -> Result<()> {
    use std::io::Write;
    let temp=path.with_extension("pending");
    let mut file=fs::OpenOptions::new().create_new(true).write(true).open(&temp)?;
    file.write_all(&serde_json::to_vec_pretty(value)?)?;file.sync_all()?;
    fs::rename(&temp,path)?;
    fs::File::open(path.parent().context("reference has no parent")?)?.sync_all()?;
    Ok(())
}
async fn stop_at(path:&std::path::Path,value:serde_json::Value)->Result<()> {
    atomic_json(path,&value)?;
    // Controller kills this one disposable OS process after observing the atomic barrier.
    tokio::time::sleep(std::time::Duration::from_secs(120)).await;
    anyhow::bail!("controller did not interrupt the synthetic writer")
}
async fn fault_upload(mode:&str,config:Arc<TranslatorConfig>,input:&std::path::Path,
    pointer:&std::path::Path,barrier:&std::path::Path)->Result<()> {
    let data=fs::read(input)?;
    let upload=FileUploadSession::new(config).await?;
    let held_open=upload.clone();
    let (_,mut cleaner)=upload.start_clean(Some("synthetic-fault-input".into()),
        Some(data.len() as u64),Sha256Policy::Compute)?;
    if mode=="fault-partial" {
        cleaner.add_data(&data[..data.len()/2]).await?;
        cleaner.checkpoint().await?;upload.checkpoint().await?;
        return stop_at(barrier,json!({"stage":mode,"reference_committed":false,
            "partial_input_bytes":data.len()/2,"checkpoint_returned":true})).await;
    }
    cleaner.add_data(&data).await?;
    let (file,metrics)=cleaner.finish().await?;
    if mode=="fault-finished" {
        return stop_at(barrier,json!({"stage":mode,"reference_committed":false,
            "candidate":file,"file_metrics":metrics})).await;
    }
    ensure!(mode=="fault-finalized"||mode=="fault-committed","unknown fault stage");
    let finalized=upload.finalize().await?;
    if mode=="fault-committed" {atomic_json(pointer,&serde_json::to_value(&file)?)?;}
    let result=stop_at(barrier,json!({"stage":mode,"reference_committed":mode=="fault-committed",
        "candidate":file,"finalize_returned":true,"metrics":finalized,
        "client_kept_open":Arc::strong_count(&held_open)>=1})).await;
    drop(held_open);result
}

async fn run(a: Vec<String>) -> Result<()> {
    ensure!(a.len()==6,"expected upload/download store input output report");
    let store=PathBuf::from(&a[2]);
    fs::create_dir_all(&store)?;
    let ctx=XetContext::default()?;
    let config=Arc::new(TranslatorConfig::local_config(&ctx,&store)?);
    ensure!(config.session.auth.is_none() && config.session.is_local(&ctx),"local no-auth endpoint required");
    let cache=config.shard_cache_directory.strip_prefix(&store)?.to_string_lossy().to_string();
    let staging=config.shard_session_directory.strip_prefix(&store)?.to_string_lossy().to_string();
    let start=Instant::now();
    let report=match a[1].as_str() {
        mode if mode.starts_with("fault-") => {
            fault_upload(mode,config,&PathBuf::from(&a[3]),&PathBuf::from(&a[4]),&PathBuf::from(&a[5])).await?;
            anyhow::bail!("fault writer unexpectedly returned")
        },
        "upload" => {
            let upload=FileUploadSession::new(config).await?;
            let files=upload.upload_files([(PathBuf::from(&a[3]),Sha256Policy::Compute)]).await?;
            ensure!(files.len()==1,"expected one uploaded file");
            let metrics=upload.finalize().await?;
            fs::write(&a[4],serde_json::to_vec_pretty(&files[0])?)?;
            json!({"operation":"upload","file":files[0],"metrics":metrics})
        },
        "download" => {
            let pointer:XetFileInfo=serde_json::from_slice(&fs::read(&a[3])?)?;
            let download=FileDownloadSession::new(config,None).await?;
            let (_,bytes)=download.download_file(&pointer,&PathBuf::from(&a[4])).await?;
            download.finalize().await?;
            json!({"operation":"download","file":pointer,"reconstructed_bytes":bytes})
        },
        _ => anyhow::bail!("unknown operation"),
    };
    fs::write(&a[5],serde_json::to_vec_pretty(&json!({
        "result":report,"pipeline_ms":start.elapsed().as_millis(),
        "auth":false,"local":true,"download_chunk_cache":false,
        "client_cache_relative":[cache,staging]
    }))?)?;
    Ok(())
}
fn main() -> Result<()> {
    let runtime=tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build()?;
    runtime.block_on(run(std::env::args().collect())).context("actual upstream Xet pipeline failed")
}
