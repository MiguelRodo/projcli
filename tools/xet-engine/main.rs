//! Optional pinned upstream local-disk engine. No remote Xet service or credentials.
use anyhow::{Context, Result, ensure};
use std::{fs, path::PathBuf, sync::Arc};
use xet_data::processing::configurations::TranslatorConfig;
use xet_data::processing::{FileDownloadSession, FileUploadSession, Sha256Policy, XetFileInfo};
use xet_runtime::core::XetContext;
const COMMIT: &str = "629562cd9fc37432317eb57811faf13a6935c601";
async fn run(a: Vec<String>) -> Result<()> {
    if a.len() == 2 && a[1] == "info" {
        println!(
            "{}",
            serde_json::json!({"protocol":1,"upstream":COMMIT,"backend":"xet-local-v1"})
        );
        return Ok(());
    }
    ensure!(
        a.len() == 6,
        "expected upload/download store input output receipt"
    );
    let store = PathBuf::from(&a[2]);
    fs::create_dir_all(&store)?;
    let context = XetContext::default()?;
    let config = Arc::new(TranslatorConfig::local_config(&context, &store)?);
    ensure!(
        config.session.auth.is_none() && config.session.is_local(&context),
        "local no-auth engine required"
    );
    let result = match a[1].as_str() {
        "upload" => {
            let session = FileUploadSession::new(config).await?;
            let files = session
                .upload_files([(PathBuf::from(&a[3]), Sha256Policy::Compute)])
                .await?;
            ensure!(files.len() == 1, "expected one input");
            session.finalize().await?;
            fs::write(&a[4], serde_json::to_vec(&files[0])?)?;
            serde_json::json!({"finalized":true})
        }
        "download" => {
            let file: XetFileInfo = serde_json::from_slice(&fs::read(&a[3])?)?;
            let session = FileDownloadSession::new(config, None).await?;
            let (_, bytes) = session.download_file(&file, &PathBuf::from(&a[4])).await?;
            session.finalize().await?;
            serde_json::json!({"finalized":true,"bytes":bytes})
        }
        _ => anyhow::bail!("unknown engine command"),
    };
    fs::write(&a[5], serde_json::to_vec(&result)?)?;
    Ok(())
}
fn main() -> Result<()> {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?
        .block_on(run(std::env::args().collect()))
        .context("local Xet operation failed")
}
