//! Experiment adapter only: use the unmodified upstream local Xet pipeline.
use std::{fs, path::PathBuf, sync::Arc, time::Instant};
use anyhow::{Context, Result, ensure};
use serde_json::json;
use xet_data::processing::{FileUploadSession, FileDownloadSession, XetFileInfo, Sha256Policy};
use xet_data::processing::configurations::TranslatorConfig;
use xet_runtime::core::XetContext;

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
