\name{projr_init}
\alias{projr_init}
\alias{projr_init_plan}
\alias{projr_init_readme}
\alias{projr_init_license}
\alias{projr_init_renv}
\title{Create missing project files through the standalone command}
\usage{
projr_init(description_file = FALSE, readme_rmd = FALSE, renv = FALSE,
  citation = FALSE, title = NULL, purpose = NULL, contact_name = NULL,
  contact_email = NULL, code_license = NULL, copyright_holder = NULL,
  author_family = character(), author_given = character(),
  author_name = character(), git = FALSE, github = FALSE,
  install_missing = NULL, project = getwd(), binary = NULL,
  env = character(), timeout = Inf)
projr_init_plan(project = getwd(), binary = NULL, env = character(), timeout = Inf)
projr_init_readme(readme_rmd = FALSE, title = NULL, purpose = NULL,
  project = getwd(), binary = NULL, env = character(), timeout = Inf)
projr_init_license(license, label = NULL, first_name = NULL, last_name = NULL,
  project = getwd(), binary = NULL, env = character(), timeout = Inf)
projr_init_renv(install_missing = NULL, project = getwd(), binary = NULL,
  env = character(), timeout = Inf)
}
\arguments{
\item{description_file}{Create DESCRIPTION only when TRUE; no R package scaffold.}
\item{readme_rmd}{Choose README.Rmd instead of the default README.md. No rendering.}
\item{renv}{Explicitly initialize bare renv; never restore dependencies.}
\item{citation}{Create native CITATION.cff with explicit authors; no cffr dependency.}
\item{title,purpose,contact_name,contact_email}{Optional single-line metadata.
Contacts are not inferred as citation authors.}
\item{code_license}{NULL to skip, or ccby, apache, cc0, proprietary or none.}
\item{copyright_holder}{Explicit holder for proprietary root code licensing.}
\item{author_family,author_given,author_name}{Person family/given names and team
names as character vectors; given names must correspond to family names or be omitted.}
\item{git,github}{Explicit opt-ins, default FALSE. Any existing remote is preserved.}
\item{install_missing}{NULL asks only in interactive non-CI sessions if the selected
renv feature needs a missing package. FALSE never installs; TRUE explicitly allows
installation, including noninteractive sessions. A configured CRAN repository and
first active R library, which must be writable are required.}
\item{project}{Existing project folder; defaults to getwd().}
\item{binary,env,timeout}{Executable, explicit child environment and time limit;
see \code{\link{proj_cli}}.}
\item{license}{Explicit ccby, apache, cc0 or proprietary.}
\item{label}{NULL for root code, otherwise one declared project-contained folder.}
\item{first_name,last_name}{Explicit proprietary copyright holder names.}
}
\description{Thin adapters to the shared CLI. Existing metadata is preserved,
except configured synchronization of an existing DESCRIPTION Version. General
setup is incremental; optional action failures can leave completed earlier steps.}
\details{
Metadata values must be supplied explicitly in R: machine transport never asks
project/Git/licence questions. Use the terminal CLI for interactive setup.
Only opted-in renv initialization checks the optional package. Native README and
citation creation need no extra R packages. No package is installed on package
load, in a plan, or by ordinary initialization. The existing explicit restoration
and build interfaces retain their no-bootstrap policy.
The CLI uses its configured Rscript (otherwise PATH); active R libraries are
forwarded. Installing renv in this R library does not guarantee compatibility with
a different configured R installation. Select matching installations or prepare
that installation's library explicitly. Installation and package/cache effects
are outside project rollback. A missing package fails without a network attempt
in noninteractive/CI use unless install_missing=TRUE was supplied.
}
\value{projr_init_plan returns a named logical state list; other functions return
invisible TRUE on CLI success. Failures signal projcli_error.}
\examples{
if (FALSE) {
  projr_init(project = ".", title = "Example analysis", purpose = "Compare models.")
  projr_init_renv(project = ".", install_missing = FALSE)
}
}
