# Compatibility boundaries belong to the R frontend; YAML merging stays in Rust.
.proj_cli_backend <- function(backend) {
  if (!identical(backend, "cli")) {
    .proj_error("invalid_backend", "Choose backend = 'cli' or an explicit existing R function.")
  }
}
.proj_callback <- function(backend, args, project, binary, env, timeout) {
  if (!is.null(project) || !is.null(binary) || !identical(env, character()) ||
      !identical(timeout, Inf)) {
    .proj_error("unsupported_callback_options",
      "An R callback runs in the current R session; CLI-only project/binary/env/timeout options do not apply.")
  }
  do.call(backend, args)
}
.proj_merged_only <- function(profile) {
  if (!is.null(profile)) {
    .proj_error("unsupported_profile_view",
      "The CLI getter returns merged active configuration. For a legacy isolated profile layer, pass that existing R getter as backend.")
  }
}
.proj_query_value <- function(reply, key) {
  payload <- reply[["result"]]
  if (!is.list(payload) || is.null(names(payload)) || anyDuplicated(names(payload)) ||
      !key %in% names(payload)) {
    .proj_error("invalid_protocol", "The CLI returned an invalid data-query result; raw output was withheld.")
  }
  payload[[key]]
}
.proj_r_value <- function(value) {
  if (!is.list(value) || !length(value)) return(value)
  out <- lapply(value, .proj_r_value)
  scalar <- vapply(out, function(x) is.atomic(x) && length(x) == 1L, logical(1))
  if (is.null(names(out)) && all(scalar)) {
    types <- vapply(out, typeof, character(1))
    if (length(unique(types)) == 1L || all(types %in% c("integer", "double"))) {
      return(unlist(out, use.names = FALSE))
    }
  }
  out
}

#' Read merged active configuration through the CLI
#'
#' Requires a CLI advertising yml get. Explicit legacy profile layers need an R
#' function backend; no silent fallback or YAML implementation lives in this package.
#' @param profile NULL for merged active configuration; legacy callbacks receive
#'   the original value unchanged.
#' @param check Whether to validate the CLI runtime schema; legacy check behavior
#'   is controlled by the explicitly supplied callback.
#' @param backend "cli", or an explicit R function such as projr::projr_yml_get.
#' @param project,binary,env,timeout CLI transport options; see proj_cli.
#' @return Configuration as an R named list, or the callback's unchanged result.
#' @export
projr_yml_get <- function(profile = NULL, check = FALSE, backend = "cli",
                          project = NULL, binary = NULL, env = character(), timeout = Inf) {
  if (is.function(backend)) return(.proj_callback(backend,
    list(profile = profile, check = check), project, binary, env, timeout))
  .proj_cli_backend(backend); .proj_merged_only(profile)
  if (!is.logical(check) || length(check) != 1L || is.na(check)) {
    .proj_error("invalid_arguments", "check must be TRUE or FALSE.")
  }
  reply <- proj_cli("yml get", if (check) "--check" else character(),
                    project, binary, env, timeout)
  value <- .proj_query_value(reply, "configuration")
  if (!is.list(value) || (length(value) && is.null(names(value)))) {
    .proj_error("invalid_protocol", "The CLI configuration result is not a mapping.")
  }
  .proj_r_value(value)
}

#' Validate merged active configuration
#' @param profile,backend,project,binary,env,timeout See projr_yml_get.
#' @return Invisible TRUE on CLI success, or the explicit callback's result.
#' @export
projr_yml_check <- function(profile = NULL, backend = "cli", project = NULL,
                            binary = NULL, env = character(), timeout = Inf) {
  if (is.function(backend)) return(.proj_callback(backend,
    list(profile = profile), project, binary, env, timeout))
  .proj_cli_backend(backend); .proj_merged_only(profile)
  projr_yml_get(check = TRUE, project = project, binary = binary, env = env, timeout = timeout)
  invisible(TRUE)
}

#' Read a parameter using literal mapping keys
#' @param ... Character scalar keys, never R expressions or dotted-path evaluation.
#' @param profile,backend,project,binary,env,timeout See projr_yml_get.
#' @return The selected R value, whole parameter value, NULL if absent, or callback result.
#' @export
projr_par_get <- function(..., profile = NULL, backend = "cli", project = NULL,
                          binary = NULL, env = character(), timeout = Inf) {
  keys <- list(...)
  if (is.function(backend)) return(.proj_callback(backend,
    c(keys, list(profile = profile)), project, binary, env, timeout))
  .proj_cli_backend(backend); .proj_merged_only(profile)
  if (!all(vapply(keys, .proj_strings, logical(1), scalar = TRUE, empty = FALSE))) {
    .proj_error("invalid_parameter_key", "Parameter selectors must be nonempty character scalar keys.")
  }
  args <- c("--", unlist(keys, use.names = FALSE))
  reply <- proj_cli("yml parameter", args, project, binary, env, timeout)
  .proj_r_value(.proj_query_value(reply, "value"))
}
#' @rdname projr_par_get
#' @export
projr_param_get <- projr_par_get
