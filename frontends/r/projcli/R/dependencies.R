# Explicit optional dependency operation; ordinary builds never call this.
.proj_renv_driver <- paste(
 "local({a<-commandArgs(trailingOnly=TRUE);",
 "encode<-jsonlite::toJSON; output<-a[[4]];",
 "sink(stderr());",
 "reply<-tryCatch({",
 "if(!requireNamespace('renv',quietly=TRUE)) {list(ok=FALSE,code='renv_missing')} else {",
 "before<-unname(tools::md5sum(a[[2]]));",
 "renv::restore(project=a[[1]],lockfile=a[[2]],library=a[[3]],prompt=FALSE,clean=FALSE,transactional=TRUE);",
 "after<-unname(tools::md5sum(a[[2]])); if(!identical(before,after))stop('lock changed');",
 "lock<-jsonlite::fromJSON(a[[2]],simplifyVector=FALSE);",
 "installed<-utils::installed.packages(lib.loc=a[[3]]);",
 "names<-sort(names(lock$Packages)); versions<-list();",
 "for(name in names) {",
 "if(!name %in% rownames(installed)||!identical(unname(installed[name,'Version']),lock$Packages[[name]]$Version))stop('version mismatch');",
 "versions[[name]]<-unname(installed[name,'Version']);};",
 "list(ok=TRUE,result=list(lockfile_md5=before,renv_version=as.character(utils::packageVersion('renv')),packages=versions))",
 "}},error=function(e)list(ok=FALSE,code='renv_restore_failed'));",
 "sink();writeBin(charToRaw(encode(reply,auto_unbox=TRUE,null='null')),output);",
 "if(!isTRUE(reply$ok))quit(status=1L)})",
 sep="\n")

.proj_existing_path <- function(path,project,directory) {
 if(!.proj_strings(path,scalar=TRUE,empty=FALSE)) .proj_error("invalid_arguments","Supply one existing project path.")
 if(!grepl("^(/|[A-Za-z]:[/\\\\]|\\\\\\\\)",path)) path<-file.path(project,path)
 value<-tryCatch(normalizePath(path,winslash="/",mustWork=TRUE),error=function(e)NULL)
 if(is.null(value)||!identical(dir.exists(value),directory)||!file.exists(value)) {
  .proj_error("invalid_arguments","The selected dependency path must exist and have the required type.")
 }
 value
}

#' Explicitly restore an existing renv lockfile into a selected project library
#' @param project Existing disposable or trusted project directory.
#' @param library Existing library directory strictly inside that project.
#' @param lockfile Existing lockfile, relative to project unless absolute.
#' @param rscript Native Rscript executable; NULL uses the calling R installation.
#' @param env Explicit additional child environment variables; see proj_cli.
#' @param timeout Positive subprocess time limit in seconds.
#' @return A list containing lockfile_md5, renv_version and observed locked package versions.
#' @export
proj_renv_restore <- function(project,library,lockfile="renv.lock",rscript=NULL,
                              env=character(),timeout=Inf) {
 if(missing(project)||missing(library)||!.proj_strings(project,scalar=TRUE,empty=FALSE)||
    !dir.exists(project)||!is.numeric(timeout)||length(timeout)!=1L||is.na(timeout)||timeout<=0) {
  .proj_error("invalid_arguments","Select an existing project and library and a positive timeout.")
 }
 project<-normalizePath(project,winslash="/",mustWork=TRUE)
 library<-.proj_existing_path(library,project,TRUE)
 lockfile<-.proj_existing_path(lockfile,project,FALSE)
 compare<-function(x) if(.Platform$OS.type=="windows")tolower(x)else x
 prefix<-paste0(sub("/$","",compare(project)),"/")
 if(!startsWith(compare(library),prefix)||!startsWith(compare(lockfile),prefix)) {
  .proj_error("invalid_arguments","Choose a library and lockfile inside the selected project.")
 }
 if(is.null(rscript)) rscript<-file.path(R.home("bin"),if(.Platform$OS.type=="windows")"Rscript.exe"else"Rscript")
 if(!.proj_strings(rscript,scalar=TRUE,empty=FALSE)||!file.exists(rscript)||dir.exists(rscript)||
    (.Platform$OS.type!="windows"&&file.access(rscript,1L)!=0L)||
    (.Platform$OS.type=="windows"&&!grepl("[.]exe$",rscript,ignore.case=TRUE))) {
  .proj_error("rscript_missing","Select an installed native Rscript executable. No R installation was attempted.")
 }
 rscript<-normalizePath(rscript,winslash="/",mustWork=TRUE)
 receipt<-tempfile("projcli-renv-",fileext=".json")
 if(!file.create(receipt)) .proj_error("transport_failed","Cannot prepare the dependency result.")
 on.exit(unlink(receipt),add=TRUE);Sys.chmod(receipt,"0600")
 driver<-tempfile("projcli-renv-driver-",fileext=".R")
 if(!file.create(driver)) .proj_error("transport_failed","Cannot prepare the dependency driver.")
 on.exit(unlink(driver),add=TRUE);Sys.chmod(driver,"0600")
 writeBin(charToRaw(.proj_renv_driver),driver)
 result<-.proj_capture(rscript,c("--vanilla",driver,project,lockfile,library,receipt),
                        .proj_environment(env),timeout)
 size<-file.info(receipt)$size
 if(is.na(size)||size>8*1024^2) .proj_error("renv_restore_failed","Dependency result unavailable; inspect the library and cache before retrying.")
 bytes<-tryCatch(readBin(receipt,"raw",n=as.integer(size)),
   error=function(e).proj_error("renv_restore_failed","Dependency result unreadable; inspect effects before retrying."))
 .proj_renv_decode(bytes,result$status)
}

.proj_renv_decode <- function(bytes,status) {
 bad <- function() .proj_error("renv_restore_failed","Dependency result invalid; inspect the library and cache before retrying.")
 if(!is.raw(bytes)||length(bytes)>8*1024^2||!is.numeric(status)||length(status)!=1L||is.na(status)) bad()
 text<-tryCatch(rawToChar(bytes),error=function(e)NULL)
 if(is.null(text)) bad()
 Encoding(text)<-"UTF-8"
 value<-tryCatch(suppressWarnings(jsonlite::fromJSON(text,simplifyVector=FALSE)),error=function(e)NULL)
 if(!is.list(value)||is.null(names(value))||anyDuplicated(names(value))||
    !is.logical(value[["ok"]])||length(value[["ok"]])!=1L||is.na(value[["ok"]])) bad()
 if(!isTRUE(value[["ok"]])||status!=0L) {
  if(identical(value[["ok"]],FALSE)&&identical(value[["code"]],"renv_missing")) {
   .proj_error("renv_missing","Install renv explicitly in the selected R installation first. No bootstrap was attempted.")
  }
  .proj_error("renv_restore_failed","Dependency restoration failed. Library, cache or package installation effects may remain; inspect before retrying.")
 }
 if(!setequal(names(value),c("ok","result"))) bad()
 observed<-value[["result"]]
 version <- function(value) .proj_strings(value,scalar=TRUE,empty=FALSE)&&
   grepl("^[0-9]+([.-][0-9]+)*$",value)
 if(!is.list(observed)||anyDuplicated(names(observed))||
    !setequal(names(observed),c("lockfile_md5","renv_version","packages"))||
    !.proj_strings(observed[["lockfile_md5"]],scalar=TRUE)||
    !grepl("^[0-9a-f]{32}$",observed[["lockfile_md5"]])||
    !version(observed[["renv_version"]])||!is.list(observed[["packages"]])) bad()
 packages<-observed[["packages"]]
 if(length(packages)&&(is.null(names(packages))||anyNA(names(packages))||
    anyDuplicated(names(packages))||any(!grepl("^[A-Za-z][A-Za-z0-9.]*$",names(packages)))||
    !all(vapply(packages,version,logical(1))))) bad()
 observed
}
