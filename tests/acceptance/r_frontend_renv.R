# Explicit remote integration only; never run dependency installation in R CMD check.
source("frontends/r/projcli/tests/dependencies.R",local=TRUE)
required <- identical(Sys.getenv("PROJCLI_R_RENV_TESTS_REQUIRED"),"1")
if(!required) {
 cat("SKIP explicit dependency integration: gate not selected\n")
} else {
 stopifnot(requireNamespace("renv",quietly=TRUE))
 run <- function() {
  root <- tempfile("renv restore project ");dir.create(root)
  on.exit(unlink(root,recursive=TRUE),add=TRUE)
  dir.create(file.path(root,"library"));dir.create(file.path(root,"source"))
  package <- file.path(root,"source","projclifixture");dir.create(package);dir.create(file.path(package,"R"))
  writeLines(c("Package: projclifixture","Version: 0.0.1","Title: Owned Dependency Test",
   "Description: Small fixture for explicit dependency restoration.",
   'Authors@R: person("Test","Fixture",email="test@example.invalid",role=c("aut","cre"))',
   "License: MIT","Encoding: UTF-8"),file.path(package,"DESCRIPTION"))
  writeLines("export(answer)",file.path(package,"NAMESPACE"))
  writeLines("answer <- function() 42L",file.path(package,"R","answer.R"))
  repo <- file.path(root,"repository");dir.create(repo);dir.create(file.path(repo,"src"));dir.create(file.path(repo,"src","contrib"))
  result <- processx::run(file.path(R.home("bin"),if(.Platform$OS.type=="windows")"R.exe"else"R"),
    c("CMD","build","--no-build-vignettes","--no-manual",package),wd=file.path(repo,"src","contrib"),error_on_status=FALSE)
  stopifnot(result$status==0L)
  tools::write_PACKAGES(file.path(repo,"src","contrib"),type="source")
  repo_path<-normalizePath(repo,winslash="/")
  repo_url<-utils::URLencode(paste0(if(.Platform$OS.type=="windows")"file:///"else"file://",repo_path))
  lock <- list(R=list(Version=as.character(getRversion()),Repositories=list(list(Name="fixture",URL=repo_url))),
    Packages=list(projclifixture=list(Package="projclifixture",Version="0.0.1",Source="Repository",Repository="fixture")))
  lockfile <- file.path(root,"renv.lock")
  writeLines(jsonlite::toJSON(lock,auto_unbox=TRUE,pretty=TRUE),lockfile)
  before <- unname(tools::md5sum(lockfile))
  environment <- c(RENV_CONFIG_CACHE_ENABLED="FALSE",RENV_CONFIG_SANDBOX_ENABLED="FALSE",
   RENV_CONFIG_SYSREQS_CHECK="FALSE",RENV_CONFIG_AUTO_SNAPSHOT="FALSE",
   RENV_PATHS_ROOT=file.path(root,"renv-state"),RENV_PATHS_CACHE=file.path(root,"empty-cache"))
  old_libraries <- .libPaths()
  stopifnot(!length(list.files(file.path(root,"library"))))
  receipt <- proj_renv_restore(root,"library",env=environment,timeout=180)
  stopifnot(receipt$lockfile_md5==before,receipt$packages$projclifixture=="0.0.1",
    unname(tools::md5sum(lockfile))==before,identical(old_libraries,.libPaths()),
    !file.exists(file.path(root,".Rprofile")))
  ns <- loadNamespace("projclifixture",lib.loc=file.path(root,"library"))
  stopifnot(get("answer",ns)()==42L)
  unloadNamespace("projclifixture")
  # Repeat is explicit and succeeds without treating a cache hit as fresh install.
  repeated <- proj_renv_restore(root,"library",env=environment,timeout=180)
  stopifnot(identical(receipt$packages,repeated$packages))
  # Unavailable locked dependency is a failure, not a successful reanalysis.
  lock$Packages$projclifixture$Version <- "9.9.9"
  writeLines(jsonlite::toJSON(lock,auto_unbox=TRUE),lockfile)
  code(proj_renv_restore(root,"library",env=environment,timeout=180),"renv_restore_failed")
  stopifnot(utils::packageVersion("projclifixture",lib.loc=file.path(root,"library"))=="0.0.1")
  outside <- tempfile("outside library ");dir.create(outside);on.exit(unlink(outside,recursive=TRUE),add=TRUE)
  code(proj_renv_restore(root,outside),"invalid_arguments")
  code(proj_renv_restore(root,"library",rscript="missing private-restore-canary"),"rscript_missing")
  cat("PASS explicit renv: empty library/cache, local source fixture, pinned package version, unchanged lock/caller libraries, repeat and failed version\n")
 }
 run()
}
