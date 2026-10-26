# The transport deliberately has no workflow, renderer or dependency installer.
.proj_error <- function(code, message, status = NULL) {
  stop(structure(list(message = message, call = NULL, code = code, status = status),
                 class = c("projcli_error", "error", "condition")))
}

.proj_strings <- function(x, scalar = FALSE, empty = TRUE) {
  is.character(x) && !anyNA(x) && (!scalar || length(x) == 1L) &&
    (empty || (length(x) > 0L && all(nzchar(x))))
}

.proj_binary <- function(binary) {
  if (is.null(binary)) binary <- getOption("projcli.binary", NULL)
  if (is.null(binary)) binary <- Sys.getenv("PROJCLI_BIN", unset = "")
  if (!.proj_strings(binary, scalar = TRUE)) {
    .proj_error("invalid_binary", "Choose one executable path, not a command line.")
  }
  if (!nzchar(binary)) binary <- unname(Sys.which("proj"))
  if (!nzchar(binary) || !file.exists(binary) || dir.exists(binary) ||
      (.Platform$OS.type != "windows" && file.access(binary, 1L) != 0L)) {
    .proj_error("binary_missing",
      "Install the separate proj CLI, then set binary or options(projcli.binary = ...). No download was attempted.")
  }
  if (.Platform$OS.type == "windows" &&
      !grepl("\\.exe$", binary, ignore.case = TRUE)) {
    .proj_error("invalid_binary", "On Windows choose the native proj.exe, not a batch or shell launcher.")
  }
  normalizePath(binary, winslash = "/", mustWork = TRUE)
}

.proj_environment <- function(env, libraries = .libPaths()) {
  if (!.proj_strings(env) || (length(env) &&
      (is.null(names(env)) || anyNA(names(env)) ||
       any(!grepl("^[A-Za-z_][A-Za-z0-9_]*$", names(env))) ||
       anyDuplicated(toupper(names(env)))))) {
    .proj_error("invalid_environment", "env must be a uniquely named character vector.")
  }
  baseline <- c("PATH", "HOME", "LANG", "LC_CTYPE",
                "PROJ_PROJECT_ROOT", "PROJ_RUN_CONTEXT")
  if (length(env) && any(toupper(names(env)) %in% c(baseline, "R_LIBS"))) {
    .proj_error("invalid_environment", "env cannot replace the baseline variables or active R_LIBS.")
  }
  if (!.proj_strings(libraries, empty = FALSE) ||
      any(grepl(.Platform$path.sep, libraries, fixed = TRUE))) {
    .proj_error("invalid_libraries", "An active R library path cannot contain the library-list separator.")
  }
  values <- Sys.getenv(baseline, unset = NA_character_)
  values <- values[!is.na(values)]
  c(values, R_LIBS = paste(libraries, collapse = .Platform$path.sep), env)
}

.proj_decode <- function(text, status) {
  bad <- function() .proj_error("invalid_protocol",
    "proj returned an invalid machine response. Install a compatible CLI; raw output was withheld.")
  if (!.proj_strings(text, scalar = TRUE) || nchar(text, type = "bytes") > 8 * 1024^2 ||
      length(status) != 1L || !is.numeric(status) || is.na(status)) bad()
  value <- tryCatch(suppressWarnings(jsonlite::fromJSON(text, simplifyVector = FALSE)),
                    error = function(e) NULL)
  if (!is.list(value) || is.null(names(value)) || anyDuplicated(names(value)) ||
      !all(c("protocol_version", "cli_version", "ok", "result", "error") %in% names(value))) bad()
  if (!identical(value[["protocol_version"]], 1L)) {
    .proj_error("incompatible_protocol",
      "This R adapter requires machine protocol 1. Install a compatible proj CLI.")
  }
  if (!.proj_strings(value[["cli_version"]], scalar = TRUE, empty = FALSE) ||
      !grepl("^[0-9]+\\.[0-9]+\\.[0-9]+(-[A-Za-z0-9.-]+)?(\\+[A-Za-z0-9.-]+)?$", value[["cli_version"]]) ||
      !is.logical(value[["ok"]]) || length(value[["ok"]]) != 1L || is.na(value[["ok"]]) ||
      !identical(status == 0L, value[["ok"]])) bad()
  if (value[["ok"]]) {
    if (!is.null(value[["error"]])) bad()
    return(value)
  }
  messages <- c(
    invalid_arguments = "proj rejected the command arguments.",
    project_selection_failed = "proj could not select the project directory.",
    query_failed = "The requested proj query failed.",
    command_failed = "The proj command failed. Changes or partial distribution may already have occurred.",
    worker_unavailable = "proj could not start its command worker."
  )
  if (!is.list(value[["error"]]) || is.null(names(value[["error"]])) ||
      anyDuplicated(names(value[["error"]])) ||
      !all(c("code", "message") %in% names(value[["error"]])) ||
      !.proj_strings(value[["error"]][["message"]], scalar = TRUE, empty = FALSE)) bad()
  code <- value[["error"]][["code"]]
  if (!.proj_strings(code, scalar = TRUE) ||
      !code %in% names(messages) || !is.null(value[["result"]])) bad()
  .proj_error(code, unname(messages[[code]]), status)
}

.proj_status <- function(result) {
  if (isTRUE(result[["timeout"]])) {
    .proj_error("timeout",
      "The proj subprocess timed out. Changes or partial distribution may already have occurred; inspect before retrying.")
  }
  if (length(result[["status"]]) != 1L || !is.numeric(result[["status"]]) || is.na(result[["status"]])) {
    .proj_error("transport_failed",
      "The proj subprocess stopped without an exit status. Effects may already exist; inspect before retrying.")
  }
}

.proj_process <- function(binary, args, env, timeout) {
  output <- tempfile("projcli-response-", fileext = ".json")
  if (!file.create(output)) .proj_error("transport_failed", "Cannot prepare the CLI response file.")
  on.exit(unlink(output), add = TRUE)
  Sys.chmod(output, mode = "0600")
  result <- tryCatch(suppressWarnings(processx::run(
    binary, args, env = env, timeout = timeout, stdin = NULL,
    stdout = output, stderr = NULL, error_on_status = FALSE,
    echo = FALSE, echo_cmd = FALSE, windows_hide_window = TRUE,
    windows_verbatim_args = FALSE, cleanup_tree = TRUE)),
    error = function(e) .proj_error("transport_failed",
      "Could not complete the proj subprocess. Effects may already exist; inspect before retrying. Raw diagnostics were withheld."),
    interrupt = function(e) .proj_error("interrupted",
      "The proj subprocess was interrupted. Changes or partial distribution may already have occurred; inspect before retrying."))
  .proj_status(result)
  size <- file.info(output)$size
  if (is.na(size) || size > 8 * 1024^2) {
    .proj_error("invalid_protocol", "The CLI response exceeds the supported size.")
  }
  bytes <- readBin(output, "raw", n = as.integer(size))
  text <- tryCatch(rawToChar(bytes), error = function(e) "")
  Encoding(text) <- "UTF-8"
  .proj_decode(text, result[["status"]])
}

.proj_capabilities <- function(reply) {
  cap <- reply[["result"]]
  if (!is.list(cap) || is.null(names(cap)) || anyDuplicated(names(cap)) ||
      !all(c("capability_version", "non_interactive", "machine_commands") %in% names(cap)) ||
      !identical(cap[["capability_version"]], 1L) ||
      !isTRUE(cap[["non_interactive"]]) || !is.list(cap[["machine_commands"]]) ||
      !is.null(names(cap[["machine_commands"]])) ||
      !length(cap[["machine_commands"]]) ||
      !all(vapply(cap[["machine_commands"]], .proj_strings, logical(1), scalar = TRUE, empty = FALSE))) {
    .proj_error("incompatible_capabilities",
      "This R adapter requires capability version 1 and noninteractive machine commands.")
  }
  unlist(cap[["machine_commands"]], use.names = FALSE)
}

.proj_argv <- function(command, args, project) {
  before_literal <- if ("--" %in% args) args[seq_len(match("--", args) - 1L)] else args
  if (any(grepl("^(--(machine|non-interactive|project|help|version)(=|$)|-h$|-V$)", before_literal))) {
    .proj_error("invalid_arguments", "Use the adapter's project argument; transport and help flags cannot be overridden.")
  }
  c("--machine", "--non-interactive",
    if (!is.null(project)) c("--project", project),
    strsplit(command, " ", fixed = TRUE)[[1L]], args)
}

#' Call a separately installed proj command
#'
#' This low-level interface returns the validated protocol envelope as an R list.
#' It does not replace the existing projr API, source hooks in this R session,
#' install packages, restore dependencies or choose a legacy backend implicitly.
#'
#' @param command One advertised machine command, such as "path get".
#' @param args Character vector of command-specific arguments, without shell quoting.
#' @param project Existing project directory. NULL preserves CLI inherited root/discovery.
#' @param binary Executable path, or NULL for the option, PROJCLI_BIN, then PATH.
#' @param env Explicit additional child variables, including any credentials needed.
#' @param timeout Subprocess time limit in seconds; Inf allows long builds.
#' @return A protocol-v1 envelope. Failures signal a projcli_error condition with
#'   a safe code and optional exit status, without raw arguments, environment or logs.
#' @export
proj_cli <- function(command = "capabilities", args = character(), project = NULL,
                     binary = NULL, env = character(), timeout = Inf) {
  if (!.proj_strings(command, scalar = TRUE, empty = FALSE) ||
      !.proj_strings(args) || length(timeout) != 1L || !is.numeric(timeout) ||
      is.na(timeout) || timeout <= 0) {
    .proj_error("invalid_arguments", "Supply one command, character arguments and a positive timeout.")
  }
  binary <- .proj_binary(binary)
  child_env <- .proj_environment(env)
  capabilities <- .proj_process(binary, c("--machine", "--non-interactive", "capabilities"),
                                child_env, min(timeout, 15))
  commands <- .proj_capabilities(capabilities)
  if (!command %in% commands) {
    .proj_error("unsupported_command", "This CLI does not advertise the requested machine command.")
  }
  if (identical(command, "capabilities")) {
    if (length(args)) .proj_error("invalid_arguments", "capabilities takes no command arguments.")
    return(capabilities)
  }
  if (!is.null(project)) {
    if (!.proj_strings(project, scalar = TRUE, empty = FALSE) || !dir.exists(project)) {
      .proj_error("invalid_project", "Choose one existing project directory.")
    }
    project <- normalizePath(project, winslash = "/", mustWork = TRUE)
  }
  reply <- .proj_process(binary, .proj_argv(command, args, project), child_env, timeout)
  if (!identical(reply[["cli_version"]], capabilities[["cli_version"]])) {
    .proj_error("cli_changed", "The CLI version changed during the call. Check any effects before retrying.")
  }
  reply
}
