\name{proj_renv_restore}
\alias{proj_renv_restore}
\title{Explicit Optional renv Restoration}
\usage{
proj_renv_restore(project, library, lockfile = "renv.lock", rscript = NULL,
  env = character(), timeout = Inf)
}
\arguments{
\item{project}{Existing trusted or disposable project directory.}
\item{library}{Existing library directory strictly inside project. Relative paths use project.}
\item{lockfile}{Existing lockfile inside project; relative paths use project.}
\item{rscript}{Installed native Rscript executable. NULL uses the calling R installation.}
\item{env}{Explicit child variables as in \code{\link{proj_cli}}.}
\item{timeout}{Positive subprocess time limit in seconds.}
}
\value{A list with lockfile_md5, renv_version and observed locked package versions.
No source URLs, tokens or personal paths are retained in this result.}
\description{Run already installed renv in a fresh Rscript process, only when explicitly requested.
Ordinary builds and package loading do not call this function or install renv.}
\details{
Pass prompt=FALSE, clean=FALSE and transactional=TRUE to renv::restore. This restores
locked versions without deleting extra installed packages or silently choosing an
update operation. Check locked package versions in the selected library and unchanged
lockfile bytes after success. MD5 identifies the lockfile bytes, not a security claim.
Only existing project-contained paths are supported by this bounded helper.

The child uses --vanilla, deliberate environment and active R libraries. It does not
activate renv in the calling session. Choose an existing Rscript explicitly to use
another R installation; config.r.rscript controls the Rust build drivers separately.
Existing renv repository/cache settings and package install scripts retain their
normal effects. A failure or timeout can leave cache/download/install effects even
with renv transactional restore; inspect before retrying. Raw child logs are withheld.
This is not the legacy github/non_github/update/BiocManager-selection API; call the
existing R function explicitly for those semantics. No dependency restore is implicit
in build, preview, data restore or language-frontend setup.
}
\examples{
# Explicit operation on a trusted project; never runs during examples:
if (FALSE) {
  dir.create("renv-library", showWarnings = FALSE)
  proj_renv_restore(".", "renv-library")
}
}
