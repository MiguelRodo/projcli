# Explicit bounded migration gate; never run by package checks.
args <- commandArgs(trailingOnly=TRUE)
spec <- jsonlite::fromJSON(args[[1]],simplifyVector=FALSE)
action <- "start"
write_result <- function(value) writeLines(jsonlite::toJSON(value,auto_unbox=TRUE,null="null",pretty=TRUE),
                                           spec$result,useBytes=TRUE)
main <- function() {
  stopifnot(identical(Sys.getenv("PROJCLI_R_REANALYSIS_REQUIRED"),"1"),
            as.character(getRversion())=="4.6.1")
  environment <- unlist(spec$environment,use.names=TRUE)
  do.call(Sys.setenv,as.list(environment))
  options(repos=c(CRAN="https://cloud.r-project.org"),renv.config.auto.snapshot=FALSE,
          renv.config.sysreqs.check=FALSE,renv.config.cache.enabled=TRUE)
  if(spec$phase=="prepare") {
    action <<- "prepare-public-analysis-packages"
    dir.create(spec$bootstrap,recursive=TRUE,showWarnings=FALSE)
    .libPaths(c(spec$bootstrap,.libPaths()))
    required <- c("tidyverse","lubridate","readxl","here","rmarkdown")
    ip <- utils::installed.packages()
    missing <- setdiff(required,ip[,"Package"])
    if(length(missing)) utils::install.packages(missing,lib=spec$bootstrap,Ncpus=2L)
    ip <- utils::installed.packages()
    ip <- ip[!duplicated(ip[,"Package"]),,drop=FALSE]
    stopifnot(all(required %in% ip[,"Package"]))
    action <<- "capture-matched-library"
    closure <- sort(unique(c(required,unlist(tools::package_dependencies(required,db=ip,
                         which=c("Depends","Imports","LinkingTo"),recursive=TRUE),use.names=FALSE))))
    closure <- intersect(closure,rownames(ip))
    priority <- ip[closure,"Priority"]
    closure <- closure[is.na(priority)|!priority %in% c("base","recommended")]
    dir.create(spec$seed,recursive=TRUE,showWarnings=FALSE)
    seed_library <- file.path(spec$seed,"library");dir.create(seed_library)
    renv::hydrate(packages=closure,library=seed_library,sources=.libPaths(),
                  project=spec$seed,prompt=FALSE,report=FALSE)
    renv::snapshot(project=spec$seed,library=seed_library,lockfile=file.path(spec$seed,"renv.lock"),
                   packages=closure,prompt=FALSE,repos=getOption("repos"))
    action <<- "install-clean-legacy-comparator"
    dir.create(spec$legacy_library,recursive=TRUE,showWarnings=FALSE)
    child_env <- get(".proj_environment",asNamespace("projcli"))(environment)
    installed <- processx::run(file.path(R.home("bin"),"R"),
      c("CMD","INSTALL",paste0("--library=",spec$legacy_library),spec$legacy_source),
      env=child_env,stdout=spec$install_stdout,stderr=spec$install_stderr,error_on_status=FALSE)
    stopifnot(installed$status==0L)
    return(list(ok=TRUE,phase=spec$phase,R=as.character(getRversion()),
       packages=as.list(setNames(ip[closure,"Version"],closure)),
       lockfile_md5=unname(tools::md5sum(file.path(spec$seed,"renv.lock"))),
       dependency_cache="owned cache primed from matched runtime; not a cold full-analysis restore"))
  }
  library <- file.path(spec$project,"library")
  if(spec$phase=="restore-packages") {
    action <<- "explicit-selected-dependency-restore"
    stopifnot(!length(list.files(library,all.files=TRUE,no..=TRUE)))
    receipt <- projcli::proj_renv_restore(spec$project,library,env=environment,timeout=1200)
    return(list(ok=TRUE,phase=spec$phase,receipt=receipt,empty_target_before_restore=TRUE))
  }
  .libPaths(c(library,spec$legacy_library,.libPaths()))
  setwd(spec$project)
  do.call(Sys.setenv,as.list(c(environment,PROJCLI_M4_BACKEND=spec$backend,
    PROJCLI_M4_CLI=spec$binary,PROJCLI_M4_CASE_ROOT=spec$project,
    PROJR_AUTO_INSTALL="FALSE")))
  if(spec$phase=="restore-data") {
    action <<- "configured-data-restore-through-R-frontend"
    labels <- c("raw-data","output","docs")
    for(label in labels) {
      reply <- projcli::proj_cli("remote restore",
        c("--provider","local","--title","backup","--remote-version","v1.2.4",
          "--label",label,"--max-bytes",as.character(64*1024^2),
          "--max-expanded-bytes",as.character(128*1024^2)),
        project=spec$project,binary=spec$binary,env=environment,timeout=180)
      stopifnot(isTRUE(reply$ok))
    }
    return(list(ok=TRUE,phase=spec$phase,labels=as.list(labels)))
  }
  if(spec$phase=="preview") {
    action <<- paste0(spec$backend,"-preview")
    if(spec$backend=="legacy") {
      stopifnot(projcli::projr_build_dev(backend=projr::projr_build_dev,clear_output="never"))
    } else {
      stopifnot(projcli::projr_build_dev(project=spec$project,binary=spec$binary,
        execution_dir="project",clear_output="never",
        env=c(environment,PROJCLI_M4_BACKEND="cli",PROJCLI_M4_CLI=spec$binary,
              PROJCLI_M4_CASE_ROOT=spec$project)))
    }
  } else if(spec$phase=="build") {
    action <<- paste0(spec$backend,"-production")
    if(spec$backend=="legacy") {
      stopifnot(projcli::projr_build_patch(backend=projr::projr_build_patch,
        msg="M4 bounded comparison",clear_output="never",
        archive_local=FALSE,archive_github=FALSE,always_archive=FALSE))
    } else {
      stopifnot(projcli::projr_build("patch",project=spec$project,binary=spec$binary,
        execution_dir="project",clear_output="none",msg="M4 bounded comparison",
        env=c(environment,PROJCLI_M4_BACKEND="cli",PROJCLI_M4_CLI=spec$binary,
              PROJCLI_M4_CASE_ROOT=spec$project)))
    }
  } else if(spec$phase=="failed-build") {
    action <<- "failure-preserves-completed-build"
    cfg <- file.path(spec$project,"_proj.yml");before<-readBin(cfg,"raw",n=file.info(cfg)$size)
    on.exit(writeBin(before,cfg),add=TRUE)
    yaml <- jsonlite::fromJSON(rawToChar(before),simplifyVector=FALSE)
    yaml$build$scripts <- list("deliberate.R")
    writeLines(jsonlite::toJSON(yaml,auto_unbox=TRUE,null="null"),cfg)
    witness <- file.path(dirname(spec$project),"deliberate-analysis-witness")
    stopifnot(!file.exists(witness))
    writeLines(c(paste0("writeLines('analysis-entered',",encodeString(witness,quote='"'),")"),
      "stop('deliberate M4 analysis failure')"),file.path(spec$project,"deliberate.R"))
    result <- tryCatch(projcli::projr_build_patch(project=spec$project,binary=spec$binary,
        execution_dir="project",env=environment),projcli_error=identity)
    stopifnot(inherits(result,"projcli_error"),identical(result$code,"command_failed"),
      identical(readLines(witness,warn=FALSE),"analysis-entered"))
    return(list(ok=TRUE,phase=spec$phase,error_code=result$code,analysis_marker_confirmed=TRUE))
  } else stop("Unsupported gate phase")
  list(ok=TRUE,phase=spec$phase,backend=spec$backend)
}
tryCatch(write_result(main()),error=function(e) {
  package <- if(inherits(e,"packageNotFoundError")&&is.character(e$package))e$package else NULL
  if(!is.null(package)&&!grepl("^[A-Za-z][A-Za-z0-9.]*$",package))package<-NULL
  # Only this fixed public-source fixture retains a bounded diagnosis. Product
  # frontend conditions remain static; never upload raw child transcripts.
  diagnostic <- conditionMessage(e)
  diagnostic <- gsub("https?://[^[:space:]\"']+","[url]",diagnostic)
  diagnostic <- gsub("(/[^[:space:]\"'()]+|[A-Za-z]:[\\\\/][^[:space:]\"'()]+)","[path]",diagnostic)
  if(grepl("token|password|secret|credential|cookie|authorization|hostname|userprofile",diagnostic,ignore.case=TRUE)) {
    diagnostic <- "[withheld]"
  }
  write_result(list(ok=FALSE,phase=spec$phase,action=action,condition=class(e)[[1]],
                    package=package,diagnostic=substr(diagnostic,1L,350L)))
  quit(status=1L)
})
