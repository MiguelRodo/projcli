library(projcli)
inside <- function(name) get(name, envir = asNamespace("projcli"), inherits = FALSE)
error_code <- function(expr, code) {
  value <- tryCatch(force(expr), projcli_error = identity)
  stopifnot(inherits(value, "projcli_error"), identical(value$code, code),
            is.null(value$call),
            !grepl("secret-canary", paste(capture.output(str(value)), collapse = "")))
  invisible(value)
}
envelope <- function(ok = TRUE, result = list(path = "a b"), error = NULL,
                     protocol = 1L) {
  jsonlite::toJSON(list(protocol_version = protocol, cli_version = "0.4.0-preview.4",
    ok = ok, result = result, error = error), auto_unbox = TRUE, null = "null")
}
decode <- inside(".proj_decode")
stopifnot(identical(decode(envelope(), 0L)$result$path, "a b"))
error_code(decode("secret-canary not JSON", 1L), "invalid_protocol")
error_code(decode(envelope(protocol = 2L), 0L), "incompatible_protocol")
error_code(decode(envelope(), 17L), "invalid_protocol")
error_code(decode(envelope(FALSE, NULL, list(code = "command_failed",
  message = "secret-canary")), 17L), "command_failed")
for (bad in list("secret-canary", 42, list("secret-canary"))) {
  error_code(decode(envelope(FALSE, NULL, bad), 1L), "invalid_protocol")
}
error_code(inside(".proj_status")(list(timeout = TRUE, status = NA_integer_)), "timeout")
error_code(inside(".proj_status")(list(timeout = FALSE, status = NA_integer_)), "transport_failed")
# Native R is available even when the optional proj executable is absent.
# These owned subprocesses test malformed-output/timeout cleanup on Windows too.
responses <- function() sort(list.files(tempdir(), pattern = "^projcli-response-", full.names = TRUE))
before_responses <- responses()
rscript <- file.path(R.home("bin"), if (.Platform$OS.type == "windows") "Rscript.exe" else "Rscript")
error_code(inside(".proj_process")(rscript,
  c("--vanilla", "-e", 'cat("secret-canary malformed JSON")'),
  inside(".proj_environment")(character()), 30), "invalid_protocol")
stopifnot(identical(responses(), before_responses))
error_code(inside(".proj_process")(rscript, c("--vanilla", "-e", "Sys.sleep(5)"),
  inside(".proj_environment")(character()), 0.05), "timeout")
stopifnot(identical(responses(), before_responses))
error_code(decode('{"protocol_version":1,"protocol_version":1}', 0L), "invalid_protocol")
error_code(decode(envelope(FALSE, NULL, list(code_extra = "command_failed",
  message = "secret-canary")), 1L), "invalid_protocol")
error_code(decode(envelope(FALSE, NULL, list(code = "command_failed",
  message_extra = "secret-canary")), 1L), "invalid_protocol")
valid_cap <- list(capability_version = 1L, non_interactive = TRUE,
                  machine_commands = list("capabilities"))
stopifnot(identical(inside(".proj_capabilities")(list(result = valid_cap)), "capabilities"))
for (name in names(valid_cap)) {
  bad <- valid_cap
  names(bad)[names(bad) == name] <- paste0(name, "_extra")
  error_code(inside(".proj_capabilities")(list(result = bad)), "incompatible_capabilities")
}
error_code(decode(paste0(envelope(), envelope()), 0L), "invalid_protocol")
error_code(inside(".proj_capabilities")(list(result = list(capability_version = 2L))),
           "incompatible_capabilities")
error_code(proj_cli(binary = file.path(tempdir(), "missing-proj-command")), "binary_missing")
error_code(proj_cli(args = NA_character_), "invalid_arguments")
env <- inside(".proj_environment")(c(PROJCLI_TEST_TOKEN = "secret-canary"),
  libraries = c("one library", "another"))
stopifnot(env[["PROJCLI_TEST_TOKEN"]] == "secret-canary",
          env[["R_LIBS"]] == paste("one library", "another", sep = .Platform$path.sep),
          !any(c("R_PROFILE_USER", "R_ENVIRON_USER", "PROJCLI_SESSION_CANARY") %in% names(env)))
error_code(inside(".proj_environment")(c(R_LIBS = "bad")), "invalid_environment")
error_code(inside(".proj_environment")(c(A = "one", a = "two")), "invalid_environment")
error_code(inside(".proj_environment")(setNames("bad", "")), "invalid_environment")
argv <- inside(".proj_argv")("path get", c("--label", "output", "a;$(echo literal)&.csv"), "project space")
stopifnot(identical(tail(argv, 1L), "a;$(echo literal)&.csv"),
          identical(argv[4L], "project space"))
error_code(inside(".proj_argv")("build", "--project=other", "p"), "invalid_arguments")
stopifnot(identical(tail(inside(".proj_argv")("path get", c("--", "--project"), "p"), 1L), "--project"))
cat("PASS unconditional R validation, malformed protocol, environment and argv checks\n")

binary <- Sys.getenv("PROJCLI_R_TEST_BINARY", unset = "")
required <- nzchar(binary)
if (!required) binary <- unname(Sys.which("proj"))
if (!nzchar(binary)) {
  cat("SKIP executable integration: no separately installed proj CLI\n")
} else {
  # A required integration binary must never become a silent skip.
  stopifnot(file.exists(binary))
  run_integration <- function(binary) {
    root <- tempfile("projcli R space ");dir.create(root)
    on.exit(unlink(root, recursive = TRUE), add = TRUE)
    suffix <- if (.Platform$OS.type == "windows") ".exe" else ""
    spaced <- file.path(root, paste0("proj command", suffix))
    stopifnot(file.copy(binary, spaced, copy.mode = TRUE))
    cap <- proj_cli(binary = spaced)
    stopifnot(cap$protocol_version == 1L, cap$result$non_interactive)
    project <- file.path(root, "project \u00e9");dir.create(project)
    dir.create(file.path(project, "released"))
    writeLines("v1.2.3-7", file.path(project, "VERSION"), useBytes = TRUE)
    cfg <- list(directories = list(output = list(path = "released"),
      cache = list(path = "cache", safe = TRUE)),
      build = list(git = FALSE, scripts = list("analysis.R")))
    writeLines(jsonlite::toJSON(cfg, auto_unbox = TRUE),
               file.path(project, "_proj.yml"), useBytes = TRUE)
    fragment <- "data \u00e9;$(echo literal)&.csv"
    answer <- proj_cli("path get", c("--label", "output", fragment),
                       project, binary = spaced)
    stopifnot(endsWith(answer$result$path, fragment))
    error_code(proj_cli("not advertised", binary = spaced), "unsupported_command")
    error_code(proj_cli("path get", c("--label", "raw-missing"),
                        project, binary = spaced), "query_failed")
    custom <- file.path(root, "active library");dir.create(custom)
    old_libs <- .libPaths();on.exit(.libPaths(old_libs), add = TRUE)
    .libPaths(c(custom, old_libs))
    original <- Sys.getenv("PROJCLI_SESSION_CANARY", unset = NA_character_)
    on.exit(if (is.na(original)) Sys.unsetenv("PROJCLI_SESSION_CANARY") else
      Sys.setenv(PROJCLI_SESSION_CANARY = original), add = TRUE)
    Sys.setenv(PROJCLI_SESSION_CANARY = "not forwarded")
    script <- c('saveRDS(list(libs = .libPaths(),',
      ' token = Sys.getenv("PROJCLI_TEST_TOKEN"),',
      ' pollution = Sys.getenv("PROJCLI_SESSION_CANARY", unset = "absent")), "observed.rds")',
      'cat("secret-canary private child diagnostic\\n", file = stderr())')
    writeLines(script, file.path(project, "analysis.R"), useBytes = TRUE)
    invisible(proj_cli("build-dev", c("--execution-dir", "project"),
      project, spaced, env = c(PROJCLI_TEST_TOKEN = "secret-canary")))
    observed <- readRDS(file.path(project, "observed.rds"))
    stopifnot(normalizePath(custom, winslash = "/") %in%
      normalizePath(observed$libs, winslash = "/"),
      observed$token == "secret-canary", observed$pollution == "absent")
    writeLines('stop("secret-canary private failure")', file.path(project, "analysis.R"))
    error_code(proj_cli("build-dev", c("--execution-dir", "project"),
      project, spaced, env = c(PROJCLI_TEST_TOKEN = "secret-canary")), "command_failed")
    stopifnot(readLines(file.path(project, "VERSION")) == "v1.2.3-7")
    cat("PASS real CLI: native binary and Unicode/spaced paths, literal argv, R_LIBS, explicit dummy credential, absent session pollution, safe errors\n")
  }
  if (required) {
    run_integration(binary)
  } else {
    compatible <- tryCatch(proj_cli(binary = binary), projcli_error = identity)
    if (inherits(compatible, "projcli_error") &&
        compatible$code %in% c("incompatible_protocol", "incompatible_capabilities")) {
      cat("SKIP executable integration: installed CLI protocol is incompatible\n")
    } else {
      if (inherits(compatible, "projcli_error")) stop(compatible)
      run_integration(binary)
    }
  }
}
