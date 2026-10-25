library(projcli)
binary <- Sys.getenv("PROJCLI_R_API_TEST_BINARY",unset="")
if (!nzchar(binary)) {
 cat("SKIP native R session integration: candidate supplied separately by CI\n")
} else {
 run <- function() {
  root <- tempfile("R sessions project ");dir.create(root)
  on.exit(unlink(root,recursive=TRUE),add=TRUE)
  dir.create(file.path(root,"changed cwd"))
  quote_r <- function(value) encodeString(value,quote='"')
  at <- function(name) quote_r(file.path(root,name))
  writeLines("v1.2.3-7",file.path(root,"VERSION"))
  cfg <- list(config=list(r=list(session="shared")),
    build=list(git=FALSE,scripts=list("first.r","middle.py","second.R"),
               hooks=list(pre=list("pre.R"),post=list("post.R"))))
  write_cfg <- function() writeLines(jsonlite::toJSON(cfg,auto_unbox=TRUE),file.path(root,"_proj.yml"))
  writeLines(c("stopifnot(!exists('.analysis_shared_value',envir=.GlobalEnv,inherits=FALSE))",
    paste0("writeLines(as.character(Sys.getpid()),",at("pre-pid"),")")),file.path(root,"pre.R"))
  writeLines(c("stopifnot(!exists('.analysis_shared_value',envir=.GlobalEnv,inherits=FALSE))",
    paste0("writeLines(as.character(Sys.getpid()),",at("post-pid"),")")),file.path(root,"post.R"))
  writeLines(c(".analysis_shared_value <- 42L","options(projcli_shared_option=42L)",
    paste0("writeLines(as.character(Sys.getpid()),",at("first-pid"),")"),
    paste0("setwd(",at("changed cwd"),")")),file.path(root,"first.r"))
  writeLines(paste0("from pathlib import Path\nPath(",
    jsonlite::toJSON(file.path(root,"middle-ok"),auto_unbox=TRUE),").write_text('middle')"),file.path(root,"middle.py"))
  for (mode in c("shared","isolated")) {
   cfg$config$r$session <- mode;write_cfg()
   writeLines(c(
    paste0("stopifnot(identical(exists('.analysis_shared_value',envir=.GlobalEnv,inherits=FALSE),",if(mode=="shared")"TRUE" else "FALSE","))"),
    if(mode=="shared") c("stopifnot(.analysis_shared_value==42L,getOption('projcli_shared_option')==42L)",
       paste0("stopifnot(normalizePath(getwd(),winslash='/')==normalizePath(",at("changed cwd"),",winslash='/'))"))
    else c("stopifnot(is.null(getOption('projcli_shared_option')))",
       paste0("stopifnot(normalizePath(getwd(),winslash='/')==normalizePath(",quote_r(root),",winslash='/'))")),
    paste0("stopifnot(readLines(",at("middle-ok"),")=='middle')"),
    paste0("writeLines(as.character(Sys.getpid()),",at("second-pid"),")")),file.path(root,"second.R"))
   for (name in c("first-pid","second-pid","pre-pid","post-pid","middle-ok")) unlink(file.path(root,name))
   before <- readBin(file.path(root,"VERSION"),"raw",n=1000)
   proj_cli("build-dev",c("--execution-dir","project"),project=root,binary=binary)
   read <- function(name) readLines(file.path(root,name))
   stopifnot(identical(read("first-pid"),read("second-pid"))==(mode=="shared"),
     read("pre-pid")!=read("first-pid"),read("post-pid")!=read("first-pid"),
     identical(before,readBin(file.path(root,"VERSION"),"raw",n=1000)))
  }
  # Configured executable in a path containing spaces, relative to the project.
  executable <- unname(Sys.which("Rscript"))
  stopifnot(nzchar(executable))
  link <- file.path(root,"R executable space")
  if(.Platform$OS.type=="windows") {
   stopifnot(Sys.junction(dirname(executable),link))
   cfg$config$r$rscript <- paste0("R executable space/",basename(executable))
  } else {
   dir.create(link)
   stopifnot(file.symlink(executable,file.path(link,"Rscript")))
   cfg$config$r$rscript <- "R executable space/Rscript"
  }
  cfg$config$r$session <- "isolated";write_cfg()
  for (name in c("first-pid","second-pid","pre-pid","post-pid","middle-ok")) unlink(file.path(root,name))
  proj_cli("build-dev",c("--execution-dir","project"),project=root,binary=binary)
  stopifnot(file.exists(file.path(root,"second-pid")))
  unlink(link,recursive=TRUE)
  # Missing executable and invalid session fail before hooks or script execution.
  for (name in c("pre-pid","second-pid")) unlink(file.path(root,name))
  cfg$config$r$rscript <- "missing executable";write_cfg()
  failed <- tryCatch(proj_cli("build-dev",project=root,binary=binary),projcli_error=identity)
  stopifnot(inherits(failed,"projcli_error"),!file.exists(file.path(root,"pre-pid")))
  cfg$config$r$rscript <- NULL;cfg$config$r$session <- "misspelled";write_cfg()
  failed <- tryCatch(proj_cli("build-dev",project=root,binary=binary),projcli_error=identity)
  stopifnot(inherits(failed,"projcli_error"),!file.exists(file.path(root,"pre-pid")))
  # Shared script failure stops later steps and end hooks, retains exact VERSION.
  cfg$config$r$session <- "shared";cfg$build$scripts <- list("first.r","fail.R","never.R");write_cfg()
  writeLines("stop('deliberate shared-session failure')",file.path(root,"fail.R"))
  writeLines(paste0("writeLines('unexpected',",at("never-marker"),")"),file.path(root,"never.R"))
  unlink(file.path(root,"post-pid"))
  before <- readBin(file.path(root,"VERSION"),"raw",n=1000)
  failed <- tryCatch(proj_cli("build-dev",c("--execution-dir","project"),project=root,binary=binary),projcli_error=identity)
  stopifnot(inherits(failed,"projcli_error"),!file.exists(file.path(root,"never-marker")),
    !file.exists(file.path(root,"post-pid")),identical(before,readBin(file.path(root,"VERSION"),"raw",n=1000)))
  cat("PASS native shared/isolated R state, mixed-language order, hook separation, executable paths and fail-fast recovery\n")
 }
 run()
}
