library(projcli)
inside <- function(name) get(name, envir = asNamespace("projcli"), inherits = FALSE)
error_code <- function(expr, code) {
  value <- tryCatch(force(expr), projcli_error = identity)
  stopifnot(inherits(value, "projcli_error"), identical(value$code, code),
            !grepl("private-query-canary", paste(capture.output(str(value)), collapse = "")))
  invisible(value)
}
error_code(inside(".proj_query_value")(list(result=list(configuration_extra=list())),
           "configuration"),"invalid_protocol")
error_code(inside(".proj_query_value")(list(result="private-query-canary"),
           "value"),"invalid_protocol")
stopifnot(is.null(inside(".proj_query_value")(list(result=list(value=NULL)), "value")))
convert <- inside(".proj_r_value")
stopifnot(identical(convert(list(1L, 2L)), c(1L, 2L)),
          identical(convert(list(1L, TRUE, NULL)), list(1L, TRUE, NULL)),
          identical(convert(list("$date" = 12L)), list("$date" = 12L)),
          identical(convert(list(list(x = 1L), list(x = 2L))), list(list(x = 1L), list(x = 2L))))
legacy <- function(profile, check) list(profile = profile, check = check, object = globalenv())
observed <- projr_yml_get(profile = "local", check = TRUE, backend = legacy)
stopifnot(observed$profile == "local", observed$check, identical(observed$object, globalenv()))
stopifnot(identical(projr_par_get("a", "b", backend = function(..., profile) list(...)),
                    list("a", "b")))
error_code(projr_yml_get(backend = "legacy"), "invalid_backend")
error_code(projr_yml_get(profile = "local"), "unsupported_profile_view")
error_code(projr_yml_get(check = NA), "invalid_arguments")
error_code(projr_par_get(1), "invalid_parameter_key")
error_code(projr_yml_get(backend = legacy, env = c(TOKEN = "private-query-canary")),
           "unsupported_callback_options")
stopifnot(identical(projr_par_get, projr_param_get))
cat("PASS R-only configuration conversion, literal selector validation and explicit callback routing\n")
binary <- Sys.getenv("PROJCLI_R_API_TEST_BINARY", unset = "")
if (!nzchar(binary)) {
  candidate <- unname(Sys.which("proj"))
  if (nzchar(candidate)) {
    cap <- tryCatch(proj_cli(binary=candidate), projcli_error=identity)
    if (inherits(cap,"projcli_error")) {
      if (!cap$code %in% c("incompatible_protocol","incompatible_capabilities")) stop(cap)
    } else if (all(c("yml get","yml parameter") %in% unlist(cap$result$machine_commands))) {
      binary <- candidate
    }
  }
}
if (!nzchar(binary)) {
  cat("SKIP typed-query integration: no compatible data-query CLI available\n")
} else {
  stopifnot(file.exists(binary))
  run <- function() {
    root <- tempfile("R config project ");dir.create(root)
    on.exit(unlink(root, recursive = TRUE), add = TRUE)
    writeLines("v1.2.3-7", file.path(root, "VERSION"))
    cfg <- list(directories = list(cache = list(path = "cache", safe = TRUE),
      output = list(path = "released")),
      build = list(git = FALSE, profile = "team", scripts = list("analysis.R")),
      parameters = list(base = 1L, numeric = list(1L, 2L), mixed = list(1L, TRUE, NULL),
        "$date" = list("$date" = 12L), "--project" = "literal flag key",
        "x);system('unexpected')" = "literal expression key",
        private = "private-query-canary"))
    text <- jsonlite::toJSON(cfg, auto_unbox = TRUE, null = "null")
    writeLines(text, file.path(root, "_proj.yml"), useBytes = TRUE)
    writeLines('{"params":{"selected":"team","precedence":"profile"}}',
               file.path(root, "_projr-team.yml"))
    writeLines('{"param":{"precedence":"local"}}', file.path(root, "_projr-local.yml"))
    writeLines('{"parameters":{"selected":"other"}}', file.path(root, "_projr-other.yml"))
    config <- projr_yml_get(project = root, binary = binary)
    stopifnot(config$parameters$selected == "team", config$parameters$precedence == "local")
    stopifnot(identical(projr_par_get("numeric", project = root, binary = binary), c(1L,2L)),
              identical(projr_par_get("mixed", project = root, binary = binary), list(1L,TRUE,NULL)),
              identical(projr_par_get("$date", project = root, binary = binary), list("$date"=12L)),
              identical(projr_param_get("--project", project = root, binary = binary), "literal flag key"),
              identical(projr_par_get("x);system('unexpected')", project = root, binary = binary), "literal expression key"),
              is.null(projr_par_get("absent", project = root, binary = binary)),
              projr_yml_check(project = root, binary = binary))
    before <- readBin(file.path(root, "_proj.yml"), "raw", n = 1000000)
    for (i in 1:2) invisible(projr_yml_get(project = root, check = TRUE, binary = binary))
    stopifnot(identical(before, readBin(file.path(root, "_proj.yml"), "raw", n = 1000000)),
              !file.exists(file.path(root, ".gitignore")), !dir.exists(file.path(root,"cache")))
    # A controlled R analysis changes cwd/profile hints/configuration but must keep
    # the selected managed profile. Parameter values are deliberately read now.
    decoy <- file.path(root,"other project");dir.create(decoy)
    writeLines("v9.9.9",file.path(decoy,"VERSION"))
    quote_r <- function(x) encodeString(x,quote='"')
    changed <- cfg;changed$build$profile <- "other";changed$parameters$base <- 99L
    script <- c(
      paste0('writeLines(', quote_r(jsonlite::toJSON(changed,auto_unbox=TRUE,null="null")), ',',
             quote_r(file.path(root,"_proj.yml")), ',useBytes=TRUE)'),
      paste0('setwd(',quote_r(decoy),')'),
      'Sys.setenv(PROJR_PROFILE="other")',
      paste0('value <- projcli::projr_yml_get(binary=',quote_r(binary),')'),
      paste0('parameter <- projcli::projr_par_get("base",binary=',quote_r(binary),')'),
      paste0('saveRDS(list(config=value,parameter=parameter),',quote_r(file.path(root,"observed.rds")),')'))
    writeLines(script,file.path(root,"analysis.R"),useBytes=TRUE)
    invisible(proj_cli("build-dev", c("--execution-dir","project"),
                      root,binary=binary))
    result <- readRDS(file.path(root,"observed.rds"))
    stopifnot(result$config$parameters$selected=="team",result$config$parameters$base==99L,
              result$parameter==99L,identical(before,readBin(file.path(root,"_proj.yml"),"raw",n=1000000)))
    writeLines('{"parameters":{},"param":{"private":"private-query-canary"}}',file.path(root,"_proj.yml"))
    error_code(projr_yml_get(project=root,binary=binary),"query_failed")
    cat("PASS candidate CLI: merged aliases/types, literal keys, read-only queries and captured profile with current values\n")
  }
  run()
}
