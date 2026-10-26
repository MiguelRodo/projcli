//! Build-scoped plain-R worker; never a daemon or the caller's live R process.
use anyhow::Context;
use camino::Utf8Path;
use std::{io::Write, process::{Child, ChildStdin, Stdio}};
use crate::run_environment::RunEnvironment;

// Requests contain raw UTF-8 path bytes, never generated R expressions. Status has
// its own private file so arbitrary analysis stdout cannot impersonate completion.
const DRIVER: &str = r#"local({
  args <- commandArgs(trailingOnly=TRUE)
  request <- file.path(args[[1]], "request")
  result <- file.path(args[[1]], "result")
  input <- file("stdin", open="r")
  on.exit(close(input))
  repeat {
    line <- readLines(input, n=1L, warn=FALSE)
    if (!length(line)) break
    if (!identical(line,"run")) quit(status=2L)
    ok <- tryCatch({
      path <- rawToChar(readBin(request,"raw",n=file.info(request)$size))
      source(path,local=.GlobalEnv,chdir=FALSE)
      TRUE
    },error=function(e) {message(conditionMessage(e));FALSE},
      interrupt=function(e) FALSE)
    pending <- paste0(result,".tmp")
    writeBin(charToRaw(if(ok) "ok\n" else "error\n"),pending)
    if (!file.rename(pending,result)) quit(status=2L)
    if (!ok) quit(status=1L)
  }
})"#;

/// One lazily created child per build pipeline. Hooks and renderers do not join it.
pub(crate) struct Worker {
    child: Child,
    input: Option<ChildStdin>,
    directory: tempfile::TempDir,
}
impl Worker {
    pub(crate) fn start(root: &Utf8Path, profile: Option<&str>, environment: &RunEnvironment) -> anyhow::Result<Self> {
        let directory = tempfile::Builder::new().prefix("projcli-r-session-").tempdir()?;
        // Rscript -e truncates multiline expressions on the supported Windows
        // launch path. A private script file preserves the same fixed driver on
        // both platforms without shell quoting or generated R expressions.
        let driver = directory.path().join("driver.R");
        std::fs::write(&driver, DRIVER).context("Cannot prepare shared R driver")?;
        let mut command = environment.command("Rscript");
        command.arg(&driver).arg(directory.path()).stdin(Stdio::piped());
        command.env(crate::project::PROJECT_ROOT_ENV,root.as_str());
        if let Some(profile)=profile {command.env("PROJR_PROFILE",profile);}
        let mut child = command.spawn().context("Cannot start shared R analysis session")?;
        let input=child.stdin.take().context("Shared R input pipe is unavailable")?;
        Ok(Self {child,input:Some(input),directory})
    }
    /// Close analysis input and require successful R shutdown before finalization.
    pub(crate) fn finish(&mut self) -> anyhow::Result<()> {
        self.input.take();
        let status=self.child.wait().context("Cannot wait for shared R shutdown")?;
        anyhow::ensure!(status.success(),"Shared R shutdown failed: {status}");
        Ok(())
    }
    pub(crate) fn execute(&mut self, path: &Utf8Path) -> anyhow::Result<()> {
        let result=self.directory.path().join("result");
        match std::fs::remove_file(&result) {
            Ok(())=>{},
            Err(error) if error.kind()==std::io::ErrorKind::NotFound=>{},
            Err(error)=>return Err(error).context("Cannot reset shared R completion"),
        }
        std::fs::write(self.directory.path().join("request"),path.as_str().as_bytes())?;
        let input=self.input.as_mut().context("Shared R input is closed")?;
        input.write_all(b"run\n").context("Cannot request shared R script")?;
        input.flush()?;
        loop {
            match std::fs::read_to_string(&result) {
                Ok(status)=>{
                    anyhow::ensure!(status=="ok\n","Shared R script failed: {path}");
                    return Ok(());
                },
                Err(error) if error.kind()==std::io::ErrorKind::NotFound=>{},
                Err(error)=>return Err(error).context("Cannot read shared R completion"),
            }
            if let Some(status)=self.child.try_wait()? {
                anyhow::bail!("Shared R session exited before script completion: {path} ({status})");
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        // Close the owned control pipe: the worker exits at EOF. No daemon, global
        // process lookup or forced termination. Existing script hangs stay hangs.
        self.input.take();
        let _ = self.child.wait();
    }
}
