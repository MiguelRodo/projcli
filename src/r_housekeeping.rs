//! Explicit R housekeeping, separate from analysis sessions.
//! Batches production post-actions in one short-lived R child; never a daemon.
use anyhow::Context;
use camino::Utf8Path;
use std::{fs, io::Write, path::PathBuf};
use crate::{run_environment::RunEnvironment, yml::RBuildConfig};

fn enabled(actions: RBuildConfig) -> bool {
    actions.document || actions.readme || actions.renv_snapshot
}

/// Exact project-local output roots captured by the existing build recovery layer.
pub(crate) fn managed_paths(root: &Utf8Path, actions: RBuildConfig) -> Vec<PathBuf> {
    let mut names = Vec::new();
    if actions.document { names.extend(["man", "NAMESPACE", "DESCRIPTION"]); }
    if actions.readme { names.extend(["README.md", "README_files"]); }
    if actions.renv_snapshot { names.push("renv.lock"); }
    names.into_iter().map(|name| root.join(name).into_std_path_buf()).collect()
}

// Native R tools write their own outputs. Check their fixed inputs/outputs before
// launch, not merely while capturing recovery. This is not a sandbox for R code.
fn check_tree(path: &Utf8Path) -> anyhow::Result<()> {
    crate::path_safety::resolve_path(path.as_std_path(), true)?;
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
        Ok(metadata) if metadata.is_dir() => {
            for entry in fs::read_dir(path)? {
                let path = camino::Utf8PathBuf::try_from(entry?.path())?;
                check_tree(&path)?;
            }
        },
        Ok(metadata) => {
            anyhow::ensure!(metadata.is_file(), "R housekeeping requires regular files/directories: {path}");
            #[cfg(unix)] {
                use std::os::unix::fs::MetadataExt;
                anyhow::ensure!(metadata.nlink() == 1, "R housekeeping rejects hard-linked files: {path}");
            }
        }
    }
    Ok(())
}

fn validate(root: &Utf8Path, actions: RBuildConfig) -> anyhow::Result<()> {
    for path in managed_paths(root, actions) {
        check_tree(Utf8Path::from_path(&path).context("Non-UTF-8 R housekeeping path")?)?;
    }
    if actions.document {
        check_tree(&root.join("R"))?;
        anyhow::ensure!(root.join("DESCRIPTION").is_file() && root.join("R").is_dir(),
            "build.r.document requires DESCRIPTION and an R directory");
    }
    if actions.readme {
        check_tree(&root.join("README.Rmd"))?;
        anyhow::ensure!(root.join("README.Rmd").is_file(),
            "build.r.readme requires README.Rmd; create it explicitly with proj init readme --rmd");
    }
    if actions.renv_snapshot {
        anyhow::ensure!(root.join("renv.lock").is_file(),
            "build.r.renv-snapshot requires an initialized renv project with renv.lock");
    }
    Ok(())
}

/// Check fixed paths and already installed packages before hooks or managed writes.
/// Missing dependencies are errors, never implicit package installation.
pub(crate) fn preflight(root: &Utf8Path, actions: RBuildConfig, env: &RunEnvironment) -> anyhow::Result<()> {
    if !enabled(actions) { return Ok(()); }
    validate(root, actions)?;
    invoke(root, actions, env, "check")
}

/// Snapshot before analysis; snapshot/document/readme after successful promotion.
/// The post batch shares one child to avoid repeatedly activating the R project.
pub(crate) fn run(root: &Utf8Path, actions: RBuildConfig, env: &RunEnvironment, before: bool) -> anyhow::Result<()> {
    if !enabled(actions) || (before && !actions.renv_snapshot) { return Ok(()); }
    validate(root, actions)?;
    invoke(root, actions, env, if before { "pre" } else { "post" })
}

/// Explicit initialization is separate from snapshots and restoration.
pub(crate) fn initialize_renv(root: &Utf8Path, env: &RunEnvironment) -> anyhow::Result<()> {
    for name in ["renv", "renv.lock", ".Rprofile"] {
        let path = root.join(name);
        crate::path_safety::resolve_path(path.as_std_path(), true)?;
        anyhow::ensure!(!path.try_exists()?, "renv initialization preserves existing {name}; use renv explicitly for an existing project");
    }
    for name in [".gitignore", ".Rbuildignore"] { check_tree(&root.join(name))?; }
    let backup = crate::build_state::BuildState::capture(&[], &[
        root.join("renv").into_std_path_buf(), root.join("renv.lock").into_std_path_buf(),
        root.join(".Rprofile").into_std_path_buf(),
        root.join(".gitignore").into_std_path_buf(), root.join(".Rbuildignore").into_std_path_buf(),
    ])?;
    let result = invoke(root, RBuildConfig::default(), env, "init");
    match result { Ok(()) => Ok(()), Err(error) => Err(backup.restore(error)) }
}

fn invoke(root: &Utf8Path, actions: RBuildConfig, env: &RunEnvironment, stage: &str) -> anyhow::Result<()> {
    let directory = tempfile::Builder::new().prefix("proj-r-housekeeping-").tempdir()?;
    let driver = directory.path().join("driver.R");
    let receipt = directory.path().join("complete");
    let mut file = fs::OpenOptions::new().write(true).create_new(true).open(&driver)?;
    file.write_all(DRIVER.as_bytes())?;
    drop(file);
    let status = env.command("Rscript").args(["--no-save", "--no-restore"])
        .arg(&driver).arg(stage).arg(root).arg(if actions.document {"1"} else {"0"})
        .arg(if actions.readme {"1"} else {"0"})
        .arg(if actions.renv_snapshot {"1"} else {"0"}).arg(&receipt)
        .current_dir(root).status().context("Cannot launch R housekeeping; install R or configure config.r.rscript")?;
    anyhow::ensure!(status.success(), "R housekeeping {stage} failed; no distribution attempted by this action");
    anyhow::ensure!(fs::read(receipt).ok().as_deref() == Some(b"complete\n"),
        "R housekeeping {stage} ended without confirming all requested actions");
    Ok(())
}

const DRIVER: &str = r#"local({
args <- commandArgs(trailingOnly = TRUE)
stopifnot(length(args) == 6L)
stage <- args[[1L]]
r_path <- function(path) {
    if (.Platform$OS.type == "windows") {
        path <- chartr(intToUtf8(92L), "/", path)
        if (startsWith(path, "//?/UNC/")) path <- paste0("//", substring(path, 9L))
        else if (startsWith(path, "//?/")) path <- substring(path, 5L)
    }
    path
}
root <- r_path(args[[2L]])
document <- identical(args[[3L]], "1")
readme <- identical(args[[4L]], "1")
snapshot <- identical(args[[5L]], "1")
setwd(root)
need <- function(package) {
    if (!requireNamespace(package, quietly = TRUE))
        stop(paste("Install the requested R housekeeping dependency first:", package))
}
if (identical(stage, "init")) {
    need("renv")
    renv::init(project = root, bare = TRUE, restart = FALSE)
    # bare init does not create a lockfile. Record only its installed project
    # library, without discovering/installing missing analysis dependencies.
    renv::snapshot(project = root, type = "all", prompt = FALSE)
} else {
    if (snapshot) need("renv")
    if (document) need("devtools")
    if (readme) need("rmarkdown")
    if (identical(stage, "pre") || identical(stage, "post")) {
        if (snapshot) renv::snapshot(project = root, prompt = FALSE)
        if (identical(stage, "post")) {
            if (document) devtools::document(pkg = root)
            if (readme) rmarkdown::render(
                input = file.path(root, "README.Rmd"), output_format = "md_document",
                output_file = "README.md", output_dir = root, knit_root_dir = root,
                envir = new.env(parent = globalenv()), quiet = TRUE)
        }
    } else if (!identical(stage, "check")) stop("Invalid housekeeping stage")
}
connection <- file(r_path(args[[6L]]), open = "wb")
writeBin(charToRaw("complete\n"), connection)
close(connection)
})
"#;
