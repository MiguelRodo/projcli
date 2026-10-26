# Exercise the existing public generic R transport; no new named init helper.
binary <- Sys.getenv("PROJCLI_R_API_TEST_BINARY")
stopifnot(nzchar(binary),file.exists(binary))
root <- tempfile("proj-init-R-")
dir.create(root)
state <- projcli::proj_cli("init plan",project=root,binary=binary)$result$initialization
stopifnot(!state$description,!state$citation,!file.exists(file.path(root,"VERSION")))
reply <- projcli::proj_cli("init",c("--description-file","--readme-rmd","--citation",
 "--title","R transport project","--purpose","Compare results.",
 "--contact-name","Contact Person","--contact-email","contact@example.invalid",
 "--author-family","Author","--author-given","Real",
 "--git=false","--github=false"),project=root,binary=binary)
stopifnot(reply$ok,file.exists(file.path(root,"README.Rmd")),!file.exists(file.path(root,"README.md")),
 file.exists(file.path(root,"DESCRIPTION")),!dir.exists(file.path(root,"R")),
 !file.exists(file.path(root,"NAMESPACE")),!file.exists(file.path(root,"renv.lock")))
cff <- readLines(file.path(root,"CITATION.cff"))
stopifnot(any(grepl("Author",cff,fixed=TRUE)),!any(grepl("Contact Person",cff,fixed=TRUE)))
before <- readBin(file.path(root,"README.Rmd"),"raw",n=1e6)
projcli::proj_cli("init",c("--readme-rmd","--description-file","--citation",
 "--git=false","--github=false"),project=root,binary=binary)
stopifnot(identical(before,readBin(file.path(root,"README.Rmd"),"raw",n=1e6)))
unlink(root,recursive=TRUE)
cat("PASS existing R transport: explicit metadata choices, read-only plan and idempotence\n")
