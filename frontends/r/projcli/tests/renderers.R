library(projcli)
binary <- Sys.getenv("PROJCLI_R_API_TEST_BINARY",unset="")
required <- identical(Sys.getenv("PROJCLI_R_RENDER_TESTS_REQUIRED"),"1")
available <- nzchar(binary) && requireNamespace("rmarkdown",quietly=TRUE) &&
             nzchar(Sys.which("quarto")) && rmarkdown::pandoc_available()
if (required && !available) stop("Required native renderer prerequisites unavailable")
if (!available) {
 cat("SKIP native renderer/session test: required separately in pinned Linux CI\n")
} else {
 run <- function(extension) {
  root <- tempfile(paste0("R shared ",extension," "));dir.create(root)
  on.exit(unlink(root,recursive=TRUE),add=TRUE)
  writeLines("v1.2.3-7",file.path(root,"VERSION"))
  document <- paste0("report.",extension)
  cfg <- list(config=list(r=list(session="shared")),
    directories=list(cache=list(path="cache",safe=TRUE),docs=list(path="published")),
    build=list(git=FALSE,scripts=list("first.r",document,"last.R")))
  writeLines(jsonlite::toJSON(cfg,auto_unbox=TRUE),file.path(root,"_proj.yml"))
  writeLines(".analysis_shared_value <- 42L",file.path(root,"first.r"))
  quote_r <- function(x) encodeString(x,quote='"')
  writeLines(c("stopifnot(.analysis_shared_value==42L)",
    paste0("writeLines('shared state survived renderer',",quote_r(file.path(root,"last-ok")),")")),file.path(root,"last.R"))
  fence <- paste(rep(intToUtf8(96),3),collapse="")
  header <- if(extension=="rmd") c("---","title: Rmd separation","output: html_document","---")
            else c("---","title: Quarto separation","format: html","---")
  writeLines(c(header,"","Fresh document session","",
    paste0(fence,"{r}"),"stopifnot(!exists('.analysis_shared_value',envir=.GlobalEnv,inherits=FALSE))",
    "cat('Renderer has its own R workspace')",fence),file.path(root,document))
  before <- readBin(file.path(root,"VERSION"),"raw",n=1000)
  source <- readBin(file.path(root,document),"raw",n=100000)
  proj_cli("build-dev",c("--execution-dir","project"),project=root,binary=binary)
  stopifnot(readLines(file.path(root,"last-ok"))=="shared state survived renderer",
    identical(before,readBin(file.path(root,"VERSION"),"raw",n=1000)),
    identical(source,readBin(file.path(root,document),"raw",n=100000)),
    !dir.exists(file.path(root,"published")))
  html <- list.files(file.path(root,"cache"),pattern="report[.]html$",full.names=TRUE,recursive=TRUE)
  stopifnot(length(html)==1L,any(grepl("Renderer has its own R workspace",readLines(html,warn=FALSE),fixed=TRUE)))
  cat(paste0("PASS actual ",extension," render has no shared analysis globals; later R script retains its state; source/VERSION restored\n"))
 }
 run("rmd");run("qmd")
}
