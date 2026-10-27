# Exercise named R initialization and retain the generic transport escape hatch.
binary <- Sys.getenv("PROJCLI_R_API_TEST_BINARY")
stopifnot(nzchar(binary),file.exists(binary))
root <- tempfile("proj-init-R-")
dir.create(root)
state <- projcli::projr_init_plan(project=root,binary=binary)
stopifnot(!state$description,!state$citation,!file.exists(file.path(root,"VERSION")))
reply <- projcli::projr_init(description_file=TRUE,readme_rmd=TRUE,citation=TRUE,
 title="R transport project",purpose="Compare results.",
 contact_name="Contact Person",contact_email="contact@example.invalid",
 author_family="Author",author_given="Real",project=root,binary=binary)
stopifnot(isTRUE(reply),file.exists(file.path(root,"README.Rmd")),!file.exists(file.path(root,"README.md")),
 file.exists(file.path(root,"DESCRIPTION")),!dir.exists(file.path(root,"R")),
 !file.exists(file.path(root,"NAMESPACE")),!file.exists(file.path(root,"renv.lock")))
cff <- readLines(file.path(root,"CITATION.cff"))
stopifnot(any(grepl("Author",cff,fixed=TRUE)),!any(grepl("Contact Person",cff,fixed=TRUE)))
before <- readBin(file.path(root,"README.Rmd"),"raw",n=1e6)
projcli::proj_cli("init",c("--readme-rmd","--description-file","--citation",
 "--git=false","--github=false"),project=root,binary=binary)
stopifnot(identical(before,readBin(file.path(root,"README.Rmd"),"raw",n=1e6)))
# No R dependency is needed for README or native citation creation.
other <- tempfile("proj-readme-R-");dir.create(other)
projcli::projr_init_readme(title="Plain example",purpose="Plain purpose",project=other,binary=binary)
stopifnot(file.exists(file.path(other,"README.md")),!file.exists(file.path(other,"README.Rmd")))
projcli::projr_init_license("apache",project=other,binary=binary)
stopifnot(grepl("Apache License",paste(readLines(file.path(other,"LICENSE")),collapse=" "),fixed=TRUE))
# Required CI supplies pinned renv. Installation is explicitly forbidden here.
if(!requireNamespace("renv",quietly=TRUE)) stop("Required fixture renv is absent")
dependency <- tempfile("proj-renv-R-");dir.create(dependency)
projcli::projr_init(project=dependency,binary=binary)
projcli::projr_init_renv(install_missing=FALSE,project=dependency,binary=binary)
stopifnot(file.exists(file.path(dependency,"renv.lock")),file.exists(file.path(dependency,".Rprofile")))
unlink(c(root,other,dependency),recursive=TRUE)
cat("PASS named R initialization: metadata/read-only plan/idempotence, plain README/licence, explicit pinned renv and generic transport\n")
