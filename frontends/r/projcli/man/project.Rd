\name{projr_build}
\alias{projr_build}
\alias{projr_build_major}
\alias{projr_build_minor}
\alias{projr_build_patch}
\alias{projr_build_dev}
\alias{projr_path_get}
\alias{projr_version_get}
\alias{projr_version_set}
\title{Build, Preview and Query a Supported VERSION Project}
\usage{
projr_build(bump_component, ..., backend = "cli", project = NULL,
  binary = NULL, env = character(), timeout = Inf)
projr_build_major(..., backend = "cli", project = NULL, binary = NULL,
  env = character(), timeout = Inf)
projr_build_minor(..., backend = "cli", project = NULL, binary = NULL,
  env = character(), timeout = Inf)
projr_build_patch(..., backend = "cli", project = NULL, binary = NULL,
  env = character(), timeout = Inf)
projr_build_dev(file = NULL, bump = FALSE, old_dev_remove = FALSE,
  args_engine = list(), profile = NULL, clear_output = "never",
  backend = "cli", project = NULL, binary = NULL, env = character(),
  timeout = Inf, execution_dir = NULL)
projr_path_get(label, ..., create = FALSE,
  format = c("auto", "relative", "absolute"), safe = TRUE,
  backend = "cli", project = NULL, binary = NULL, env = character(), timeout = Inf)
projr_version_get(backend = "cli", project = NULL, binary = NULL,
  env = character(), timeout = Inf)
projr_version_set(version, only_if_exists = TRUE, backend = "cli",
  project = NULL, binary = NULL, env = character(), timeout = Inf)
}
\arguments{
\item{bump_component}{Explicit major, minor or patch; no legacy implicit bump is inferred.}
\item{...}{For builds: msg, empty args_engine, profile, clear_output and execution_dir.
Explicit archive_github/archive_local/always_archive switches require an R callback.
For paths: unnamed character scalar fragments.}
\item{file,bump,old_dev_remove,args_engine}{CLI preview supports only NULL file,
FALSE bump/removal and empty renderer arguments. Analysis comes from configuration.}
\item{profile}{NULL or an explicit CLI profile-list string for builds.}
\item{clear_output}{NULL or none/never/pre/post. Never is the explicit none alias.}
\item{execution_dir}{NULL to inherit policy, or explicit native/project execution.}
\item{label}{One supported logical label. Legacy code/data special lookups require callbacks.}
\item{create}{FALSE for CLI lookup. TRUE needs a supplied existing R function.}
\item{format}{auto, relative or absolute; omission retains configured core presentation.}
\item{safe}{Select safe versioned output or configured final target.}
\item{version}{One CLI-supported version string, not an arbitrary R version object.}
\item{only_if_exists}{Do not create absent VERSION when TRUE.}
\item{backend,project,binary,env,timeout}{See \code{\link{projr_yml_get}}.}
}
\value{Build/dev/set return invisible TRUE on CLI success. Path and version getters
return character values (version without v, using the CLI canonical spelling).
Callbacks retain their original results.}
\description{Thin calls to the standalone CLI for a documented subset. No renderer,
YAML, version parser, Git workflow or dependency installer is duplicated in R.}
\details{
CLI build/version wrappers reject DESCRIPTION projects because legacy authority
may be DESCRIPTION while Rust owns VERSION. Bookdown builds with _bookdown.yml
require explicit existing R callbacks: legacy render_book and the CLI site driver
are different. Direct proj_cli still exposes its documented core capabilities.
Arbitrary renderer objects, ad-hoc dev files, dev bump and prior-cache removal are
unsupported rather than ignored. CLI preview never publishes or bumps VERSION.
Configured destinations/cues control production publication; old archive switches
are not equivalent overrides. Unspecified clear policy follows the core; it does
not silently adopt the legacy production pre-clear default.
Path lookup never creates directories, changes ignores or rewrites renderer YAML.
Create defaults FALSE in this frontend; missing callback options retain the existing
R function's own defaults. Unlike clean3ac legacy auto presentation, CLI auto follows
the core configured/captured presentation policy, not YAML spelling. Explicit relative
is caller-relative; absolute is native absolute. No relative=/absolute= flag API from
a different local mirror is advertised. Construct output parents explicitly when needed.
R callbacks run in the calling R session, without CLI-only transport options; they
can retain legacy globals, prompts, side effects and diagnostics. No silent fallback
occurs. This subset is not full R/IDE or arbitrary-hook-state compatibility.
}
\examples{
# An explicit callback needs neither a CLI nor automatic legacy package loading:
projr_build_patch(backend = function(...) invisible(TRUE))
}
