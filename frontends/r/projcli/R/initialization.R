# Thin argument adapters. Native metadata/licences/citations stay in the CLI.
.proj_optional_package <- function(
  package,
  install_missing = NULL,
  available = function(x) requireNamespace(x, quietly = TRUE),
  install = function(x) {
    repos <- getOption("repos")
    lib <- .libPaths()[1L]
    if (
      !length(repos) ||
        anyNA(repos) ||
        any(repos == "@CRAN@") ||
        file.access(lib, 2L) != 0L
    ) {
      .proj_error(
        "optional_package_install_failed",
        paste0(
          "Select a CRAN repository and a writable active R library ",
          "before requesting installation."
        )
      )
    }
    utils::install.packages(x, lib = lib, repos = repos, dependencies = NA)
  },
  can_prompt = interactive() &&
    !nzchar(Sys.getenv("CI")) &&
    !nzchar(Sys.getenv("R_TESTS")),
  ask = readline
) {
  if (!is.null(install_missing)) {
    .proj_flag(install_missing)
  }
  if (available(package)) {
    return(invisible(TRUE))
  }
  consent <- isTRUE(install_missing)
  if (is.null(install_missing) && can_prompt) {
    consent <- tolower(trimws(ask(paste0(
      "Install optional R package '",
      package,
      "' now? [y/N]: "
    )))) %in%
      c("y", "yes")
  }
  if (!consent) {
    .proj_error(
      "optional_package_missing",
      paste0(
        "Install the optional package with install.packages('",
        package,
        "'), or allow installation with install_missing=TRUE. ",
        "No installation was attempted."
      )
    )
  }
  tryCatch(install(package), error = function(e) {
    .proj_error(
      "optional_package_install_failed",
      paste0(
        "Optional package installation failed. Inspect the library ",
        "before retrying; raw diagnostics were withheld."
      )
    )
  })
  if (!available(package)) {
    .proj_error(
      "optional_package_install_failed",
      paste0(
        "The optional package is still unavailable after installation. ",
        "Check the selected R library."
      )
    )
  }
  invisible(TRUE)
}
.proj_init_text <- function(value, flag, vector = FALSE) {
  if (is.null(value) || (vector && !length(value))) {
    return(character())
  }
  if (
    !.proj_strings(value, scalar = !vector, empty = FALSE) ||
      any(grepl("[[:cntrl:]]", value))
  ) {
    .proj_error(
      "invalid_arguments",
      "Metadata must contain nonempty single-line character values."
    )
  }
  # Equals syntax preserves values starting with '-' without shell quoting.
  paste0("--", flag, "=", value)
}
.proj_init_prepare <- function(
  renv,
  install_missing,
  project,
  binary,
  env,
  timeout
) {
  .proj_flag(renv)
  if (!is.null(install_missing)) {
    .proj_flag(install_missing)
  }
  # Check project and CLI support before a possible package installation.
  projr_init_plan(project, binary, env, timeout)
  if (renv) .proj_optional_package("renv", install_missing)
}
#' Inspect initialization without changing the project
#' @param project Existing project directory; defaults to the current directory.
#' @param binary,env,timeout See proj_cli.
#' @return A named list describing existing metadata, Git and remote setup.
#' @export
projr_init_plan <- function(
  project = getwd(),
  binary = NULL,
  env = character(),
  timeout = Inf
) {
  value <- .proj_query_value(
    proj_cli(
      "init plan",
      project = project,
      binary = binary,
      env = env,
      timeout = timeout
    ),
    "initialization"
  )
  fields <- c(
    "git_available",
    "git_repository",
    "remote",
    "readme_md",
    "readme_rmd",
    "description",
    "citation"
  )
  if (
    !is.list(value) ||
      !setequal(names(value), fields) ||
      anyDuplicated(names(value)) ||
      !all(vapply(
        value,
        function(x) is.logical(x) && length(x) == 1L && !is.na(x),
        logical(1)
      ))
  ) {
    .proj_error(
      "invalid_protocol",
      "The CLI returned invalid initialization state."
    )
  }
  value
}
#' Create missing project files through the standalone command
#' @param description_file Create DESCRIPTION when TRUE; no package scaffold.
#' @param readme_rmd Choose README.Rmd when TRUE, otherwise plain Markdown.
#' @param renv Initialize bare renv only when TRUE; never restore dependencies.
#' @param citation Create CITATION.cff with explicit authors when TRUE.
#' @param title,purpose,contact_name,contact_email Optional single-line
#'   metadata.
#' @param code_license NULL, ccby, apache, cc0, proprietary or none.
#' @param copyright_holder Explicit holder required for proprietary licensing.
#' @param author_family,author_given,author_name Character vectors of person
#'   family/given names and team names. Contacts are not inferred as authors.
#' @param git,github Explicit opt-ins; existing repositories/remotes
#'   are preserved.
#' @param install_missing NULL asks only in an interactive non-CI R session when
#'   renv is missing; FALSE never installs, TRUE explicitly allows installation.
#' @param project,binary,env,timeout See projr_init_plan.
#' @return Invisible TRUE on success. Setup is incremental, not a transaction.
#' @export
projr_init <- function(
  description_file = FALSE,
  readme_rmd = FALSE,
  renv = FALSE,
  citation = FALSE,
  title = NULL,
  purpose = NULL,
  contact_name = NULL,
  contact_email = NULL,
  code_license = NULL,
  copyright_holder = NULL,
  author_family = character(),
  author_given = character(),
  author_name = character(),
  git = FALSE,
  github = FALSE,
  install_missing = NULL,
  project = getwd(),
  binary = NULL,
  env = character(),
  timeout = Inf
) {
  flags <- c(
    .proj_flag(description_file),
    .proj_flag(readme_rmd),
    .proj_flag(renv),
    .proj_flag(citation)
  )
  names(flags) <- c("description-file", "readme-rmd", "renv", "citation")
  args <- c(
    if (any(flags)) paste0("--", names(flags)[flags]),
    paste0("--git=", tolower(.proj_flag(git))),
    paste0("--github=", tolower(.proj_flag(github)))
  )
  if (
    !is.null(code_license) &&
      (!.proj_strings(code_license, scalar = TRUE) ||
        !code_license %in% c("ccby", "apache", "cc0", "proprietary", "none"))
  ) {
    .proj_error(
      "invalid_arguments",
      "Choose ccby, apache, cc0, proprietary or none."
    )
  }
  values <- list(
    title = title,
    purpose = purpose,
    "contact-name" = contact_name,
    "contact-email" = contact_email,
    "code-license" = code_license,
    "copyright-holder" = copyright_holder
  )
  for (key in names(values)) {
    args <- c(args, .proj_init_text(values[[key]], key))
  }
  for (key in c("family", "given", "name")) {
    value <- switch(
      key,
      family = author_family,
      given = author_given,
      name = author_name
    )
    args <- c(args, .proj_init_text(value, paste0("author-", key), TRUE))
  }
  if (length(author_given) && length(author_given) != length(author_family)) {
    .proj_error(
      "invalid_arguments",
      "Supply one given name per family name, or omit all given names."
    )
  }
  .proj_init_prepare(renv, install_missing, project, binary, env, timeout)
  .proj_complete(proj_cli("init", args, project, binary, env, timeout))
}
#' Initialize bare renv explicitly
#' @param install_missing See projr_init.
#' @param project,binary,env,timeout See projr_init_plan.
#' @return Invisible TRUE. Existing renv state is protected by the CLI.
#' @export
projr_init_renv <- function(
  install_missing = NULL,
  project = getwd(),
  binary = NULL,
  env = character(),
  timeout = Inf
) {
  .proj_init_prepare(TRUE, install_missing, project, binary, env, timeout)
  .proj_complete(proj_cli(
    "init renv",
    project = project,
    binary = binary,
    env = env,
    timeout = timeout
  ))
}
#' Create a missing README without rendering or replacing an existing README
#' @param readme_rmd Choose README.Rmd instead of README.md.
#' @param title,purpose Optional title and purpose paragraph.
#' @param project,binary,env,timeout See projr_init_plan.
#' @return Invisible TRUE on success. Creating Rmd does not require rmarkdown.
#' @export
projr_init_readme <- function(
  readme_rmd = FALSE,
  title = NULL,
  purpose = NULL,
  project = getwd(),
  binary = NULL,
  env = character(),
  timeout = Inf
) {
  args <- c(
    if (.proj_flag(readme_rmd)) "--rmd",
    .proj_init_text(title, "title"),
    .proj_init_text(purpose, "description")
  )
  .proj_complete(proj_cli("init readme", args, project, binary, env, timeout))
}
#' License project code or one explicitly declared project-contained directory
#' @param license One of ccby, apache, cc0 or proprietary.
#' @param label NULL selects root code; otherwise one declared directory label.
#' @param first_name,last_name Explicit proprietary copyright holder names.
#' @param project,binary,env,timeout See projr_init_plan.
#' @return Invisible TRUE. Existing licences are preserved.
#' @export
projr_init_license <- function(
  license,
  label = NULL,
  first_name = NULL,
  last_name = NULL,
  project = getwd(),
  binary = NULL,
  env = character(),
  timeout = Inf
) {
  if (
    missing(license) ||
      !.proj_strings(license, scalar = TRUE) ||
      !license %in% c("ccby", "apache", "cc0", "proprietary")
  ) {
    .proj_error(
      "invalid_arguments",
      "Select an explicit supported code licence."
    )
  }
  if (license == "proprietary" && (is.null(first_name) || is.null(last_name))) {
    .proj_error(
      "invalid_arguments",
      "Supply explicit first_name and last_name for proprietary licensing."
    )
  }
  args <- c(
    .proj_init_text(license, "license"),
    .proj_init_text(label, "label"),
    .proj_init_text(first_name, "first-name"),
    .proj_init_text(last_name, "last-name")
  )
  .proj_complete(proj_cli("init license", args, project, binary, env, timeout))
}
