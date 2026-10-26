# Remote-only native acceptance; dependencies must be provisioned explicitly.
stopifnot(identical(Sys.getenv("PROJCLI_R_HOUSEKEEPING_REQUIRED"), "1"))
binary <- Sys.getenv("PROJCLI_R_API_TEST_BINARY")
stopifnot(nzchar(binary), file.exists(binary))
for (package in c("processx", "devtools", "rmarkdown", "renv", "jsonlite"))
    stopifnot(requireNamespace(package, quietly = TRUE))
root <- tempfile("R housekeeping with spaces ")
dir.create(root)
root <- normalizePath(root, winslash = "/", mustWork = TRUE)
# The fixture owns only its temporary root; retained on failure.
run <- function(args, success = TRUE) {
    environment <- Sys.getenv()
    environment <- environment[!names(environment) %in%
        c("PROJ_PROJECT_ROOT", "PROJ_RUN_CONTEXT", "PROJR_PROFILE", "QUARTO_PROFILE")]
    result <- processx::run(binary, c("--non-interactive", "--project", root, args),
        wd = root, error_on_status = FALSE, env = environment)
    stopifnot(identical(result$status == 0L, success))
    result
}
writeLines("Preserve plain README", file.path(root, "README.md"))
run(c("init", "readme", "--rmd", "--title", "A: quoted project"))
stopifnot(identical(readLines(file.path(root, "README.md")), "Preserve plain README"))
fence <- strrep(intToUtf8(96L), 3L)
writeLines(c("---", "output: md_document", "---", "",
    paste0(fence, "{r}"), "cat('Housekeeping result: ', 6 * 7)", fence),
    file.path(root, "README.Rmd"))
# Explicit bare setup, not restoration; no broad installs in this fixture.
run(c("init", "renv"))
stopifnot(file.exists(file.path(root, "renv.lock")),
    file.exists(file.path(root, "renv/activate.R")), file.exists(file.path(root, ".Rprofile")))
# Keep the provisioned runner library for tasks, not a bootstrapped project library.
unlink(file.path(root, ".Rprofile"))
dir.create(file.path(root, "R"))
writeLines(c("Package: housekeepingfixture", "Version: 1.2.3",
    "Title: Housekeeping Fixture", "Description: A minimal documentation fixture.",
    "Authors@R: person('Test', 'Author', email='test@example.invalid', role=c('aut','cre'))",
    "License: MIT", "Encoding: UTF-8"), file.path(root, "DESCRIPTION"))
writeLines(c("#' Return a value", "#' @return Forty two.", "#' @export",
    "answer <- function() 42"), file.path(root, "R/answer.R"))
writeLines("v1.2.3.7", file.path(root, "VERSION"))
# Explicit native executable also exercises Windows R installation paths.
executable <- unname(Sys.which("Rscript"))
stopifnot(nzchar(executable))
config <- c("config:", "  r:",
    paste0("    rscript: ", jsonlite::toJSON(executable, auto_unbox=TRUE)),
    "build:", "  scripts: []", "  git: false",
    "  r: {document: true, readme: true, renv-snapshot: true}")
writeLines(config, file.path(root, "_proj.yml"))
prior <- readBin(file.path(root, "README.md"), "raw", n=1e6)
run("build-dev")
stopifnot(identical(readBin(file.path(root, "README.md"), "raw", n=1e6), prior),
    !dir.exists(file.path(root, "man")))
run(c("build", "--patch"))
stopifnot(file.exists(file.path(root, "man/answer.Rd")),
    any(grepl("export(answer)", readLines(file.path(root, "NAMESPACE")), fixed=TRUE)),
    any(grepl("Housekeeping result:.*42", readLines(file.path(root, "README.md")))))
lock <- jsonlite::fromJSON(file.path(root, "renv.lock"), simplifyVector=FALSE)
stopifnot(is.list(lock$R), !is.null(lock$R$Version))
paths <- c("VERSION", "renv.lock", "README.md", "NAMESPACE", "DESCRIPTION", "man/answer.Rd")
before <- lapply(file.path(root, paths), function(path) readBin(path, "raw", n=1e7))
writeLines(c("---", "output: md_document", "---", paste0(fence, "{r}"),
    "stop('deliberate README failure')", fence), file.path(root, "README.Rmd"))
failure <- run(c("build", "--patch"), success=FALSE)
stopifnot(grepl("deliberate README failure", paste(failure$stdout, failure$stderr), fixed=TRUE))
after <- lapply(file.path(root, paths), function(path) readBin(path, "raw", n=1e7))
stopifnot(identical(before, after))
cat("R housekeeping native acceptance passed\n")
unlink(root, recursive=TRUE)
