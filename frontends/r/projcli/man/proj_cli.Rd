\name{proj_cli}
\alias{proj_cli}
\title{Call a Separately Installed proj Command}
\usage{
proj_cli(command = "capabilities", args = character(), project = NULL,
  binary = NULL, env = character(), timeout = Inf)
}
\arguments{
\item{command}{One advertised machine command, for example \code{"path get"}.}
\item{args}{Command-specific character arguments, without shell quoting.}
\item{project}{An existing project directory; \code{NULL} preserves the CLI inherited root or nearest-project discovery.}
\item{binary}{Executable path; \code{NULL} checks \code{options(projcli.binary)},
then \code{PROJCLI_BIN}, then \code{PATH}.}
\item{env}{Explicit additional named child variables, including any credentials.}
\item{timeout}{Positive subprocess timeout in seconds, or \code{Inf}.}
}
\value{The validated machine envelope as an R list. Failures raise a
\code{projcli_error} with a safe \code{code} and optional exit \code{status}.}
\description{
Calls the standalone CLI without embedding Rust or changing the existing projr
package. Protocol and capability version 1 are required. Arguments are passed
directly, not through a shell. No binary or package download is attempted.
}
\details{
The baseline environment forwards PATH, HOME, LANG and LC_CTYPE when set,
and active \code{.libPaths()} through R_LIBS. The two managed context variables
PROJ_PROJECT_ROOT and PROJ_RUN_CONTEXT also pass through; the CLI validates them
and retains captured root/profile/version when an R script changes folders.
Explicit project selection retains its normal override semantics.
Other extra variables must be explicit.
processx also supplies its control variables and required Windows variables.
This is not a sandbox and does not disable project or user startup files.
Raw command output, stderr and machine error text are withheld from conditions:
arbitrary child logs cannot be reliably secret-redacted. Use the CLI directly
when private detailed logs are needed. Result data remains the requested data.
The response read is bounded to 8 MiB; subprocess output file size and execution
side effects are not a resource or transaction guarantee.
On timeout or interruption effects may already have occurred; never blindly
retry publication. Global transport/project/help flags cannot override the
adapter before a literal argument separator.
}
\examples{
# Does not download a CLI when unavailable:
if (nzchar(Sys.which("proj"))) {
  try(proj_cli("capabilities"))
}
}
