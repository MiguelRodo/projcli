\name{projr_yml_get}
\alias{projr_yml_get}
\alias{projr_yml_check}
\alias{projr_par_get}
\alias{projr_param_get}
\title{Read Active Configuration and Parameters}
\usage{
projr_yml_get(profile = NULL, check = FALSE, backend = "cli",
  project = NULL, binary = NULL, env = character(), timeout = Inf)
projr_yml_check(profile = NULL, backend = "cli", project = NULL,
  binary = NULL, env = character(), timeout = Inf)
projr_par_get(..., profile = NULL, backend = "cli", project = NULL,
  binary = NULL, env = character(), timeout = Inf)
projr_param_get(..., profile = NULL, backend = "cli", project = NULL,
  binary = NULL, env = character(), timeout = Inf)
}
\arguments{
\item{profile}{NULL for merged active CLI configuration. Explicit legacy layer
selection is supported only by a supplied existing R function backend.}
\item{check}{Validate the existing CLI runtime schema as well as parsing/merging.}
\item{backend}{\code{"cli"}, or an explicit R function, such as
\code{projr::projr_yml_get}. No legacy package is loaded or installed automatically.}
\item{project,binary,env,timeout}{CLI-only options documented by \code{\link{proj_cli}}.}
\item{...}{Literal nonempty character scalar mapping keys.}
}
\value{A named configuration list or selected parameter value. Homogeneous scalar arrays
become R vectors; mixed or null-bearing arrays and records stay lists, without
automatic date or data-frame conversion. Missing parameters return
NULL. Validation returns invisible TRUE. A callback's original result is retained.}
\description{Thin helpers for explicit merged configuration and literal parameter
queries. A CLI advertising yml get and yml parameter is required; preview.4's
validation-only yml read is not silently treated as a configuration getter.}
\details{
Configuration contents are read at query time. The selected build profile order
is retained from managed context unless an explicit CLI profile overrides it.
Parameter queries validate the supported runtime schema and use its existing four
aliases: parameters, parameter, params and param. Legacy par/pars aliases and
R-only configuration are not silently translated. Getters normalize shared aliases.
Only the CLI's supported data/configuration subset is promised.
The old R getter ignores check and interprets an explicit profile as a single
layer; the CLI check flag actually validates and explicit CLI profiles select merged
overlays. The frontend rejects the incompatible explicit-profile form instead of
changing its meaning. Use an existing R getter as backend for that behavior.
A callback executes in the caller's R session/current folder with original
arguments, results and errors. Its effects, prompts, installers and diagnostics
remain those of the original function, outside CLI transport redaction. CLI-only
options cannot be mixed with callback mode. No automatic backend fallback occurs.
Explicit configuration/parameter results may include secrets requested by the
caller. Old validation-only queries and error conditions still withhold them.
}
\examples{
# Deliberate R callback selection requires no CLI or automatic package loading:
projr_yml_get(backend = function(profile, check) list(example = TRUE))
}
