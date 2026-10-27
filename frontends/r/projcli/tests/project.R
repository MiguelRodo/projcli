library(projcli)
code <- function(expr, expected) {
  got <- tryCatch(force(expr), projcli_error = identity)
  stopifnot(inherits(got, "projcli_error"), identical(got$code, expected))
}
stopifnot(
  identical(
    projr_build_patch(backend = function(...) list(...), msg = "hello"),
    list(msg = "hello")
  ),
  identical(
    projr_build("minor", backend = function(...) list(...)),
    list(bump_component = "minor")
  ),
  identical(
    projr_build_dev(backend = function(old_dev_remove = TRUE) old_dev_remove),
    TRUE
  ),
  identical(
    projr_path_get("output", backend = function(label, create = TRUE) create),
    TRUE
  ),
  identical(projr_version_get(backend = function() "legacy"), "legacy"),
  identical(
    projr_version_set("2", backend = function(version, only_if_exists = FALSE) {
      only_if_exists
    }),
    FALSE
  )
)
code(projr_build(), "unsupported_implicit_bump")
code(
  projr_build_patch(args_engine = list(quiet = TRUE)),
  "unsupported_arguments"
)
code(projr_build_patch(archive_local = FALSE), "unsupported_arguments")
code(projr_build_dev(bump = TRUE), "unsupported_arguments")
code(projr_build_dev(old_dev_remove = TRUE), "unsupported_arguments")
code(projr_path_get("output", create = TRUE), "unsupported_creation")
code(projr_path_get("code"), "unsupported_legacy_label")
code(projr_path_get("data"), "unsupported_legacy_label")
code(projr_path_get("output", safe = NA), "invalid_arguments")
code(projr_build_patch(execution_dir = "unknown"), "invalid_arguments")
if (.Platform$OS.type == "windows") {
  r_path <- get(".proj_r_path", asNamespace("projcli"))
  stopifnot(
    identical(
      r_path("\\\\?\\C:\\project space\\child"),
      "C:/project space/child"
    ),
    identical(
      r_path("\\\\?\\UNC\\server\\share\\child"),
      "//server/share/child"
    )
  )
}
cat(
  paste0(
    "PASS explicit backend preservation and unsupported legacy ",
    "argument boundaries\n"
  )
)
binary <- Sys.getenv("PROJCLI_R_API_TEST_BINARY", unset = "")
if (!nzchar(binary)) {
  cat(
    paste0(
      "SKIP project integration: required candidate is supplied ",
      "separately by CI\n"
    )
  )
} else {
  stopifnot(file.exists(binary))
  run <- function() {
    root <- tempfile("R project wrappers ")
    dir.create(root)
    on.exit(unlink(root, recursive = TRUE), add = TRUE)
    writeLines("v1.2.3-7", file.path(root, "VERSION"))
    cfg <- list(
      directories = list(
        cache = list(path = "cache", safe = TRUE),
        output = list(path = "released")
      ),
      build = list(git = FALSE, scripts = list("analysis.R"))
    )
    write_cfg <- function() {
      writeLines(
        jsonlite::toJSON(cfg, auto_unbox = TRUE),
        file.path(root, "_proj.yml")
      )
    }
    write_cfg()
    args <- list(project = root, binary = binary)
    path <- do.call(
      projr_path_get,
      c(
        list("output", "sub folder", "r\u00e9sult.csv", format = "absolute"),
        args
      )
    )
    stopifnot(
      !dir.exists(file.path(root, "cache")),
      !file.exists(file.path(root, ".gitignore")),
      grepl("sub folder", path, fixed = TRUE),
      grepl("r\u00e9sult.csv", path, fixed = TRUE)
    )
    stopifnot(do.call(projr_version_get, args) == "1.2.3.7")
    quote_r <- function(value) encodeString(value, quote = '"')
    script <- c(
      paste0(
        paste0(
          "out <- projcli::projr_path_get('output','results.csv',",
          "format='absolute',binary="
        ),
        quote_r(binary),
        ")"
      ),
      "dir.create(dirname(out),recursive=TRUE,showWarnings=FALSE)",
      "writeLines(c('value','42'),out)",
      paste0("writeLines(getwd(),", quote_r(file.path(root, "cwd.txt")), ")")
    )
    writeLines(script, file.path(root, "analysis.R"), useBytes = TRUE)
    dir.create(file.path(root, "released"))
    writeLines("prior", file.path(root, "released", "prior.txt"))
    before <- readBin(file.path(root, "VERSION"), "raw", n = 1000)
    stopifnot(
      !withVisible(do.call(
        projr_build_dev,
        c(args, list(execution_dir = "project"))
      ))$visible
    )
    stopifnot(
      identical(readBin(file.path(root, "VERSION"), "raw", n = 1000), before),
      readLines(file.path(root, "released", "prior.txt")) == "prior",
      !file.exists(file.path(root, "released", "results.csv")),
      identical(
        normalizePath(readLines(file.path(root, "cwd.txt")), winslash = "/"),
        normalizePath(root, winslash = "/")
      )
    )
    for (step in c("patch", "minor", "major")) {
      fn <- get(paste0("projr_build_", step), asNamespace("projcli"))
      value <- do.call(fn, c(args, list(execution_dir = "project")))
      stopifnot(
        isTRUE(value),
        identical(
          readLines(file.path(root, "released", "results.csv")),
          c("value", "42")
        )
      )
    }
    stopifnot(do.call(projr_version_get, args) == "2.0.0.1")
    do.call(projr_version_set, c(list("3.4.5-8"), args))
    stopifnot(do.call(projr_version_get, args) == "3.4.5.8")
    stopifnot(grepl(
      "3.4.5.8",
      readLines(file.path(root, "VERSION")),
      fixed = TRUE
    ))
    old <- readBin(file.path(root, "VERSION"), "raw", n = 1000)
    code(do.call(projr_version_set, c(list("invalid"), args)), "command_failed")
    stopifnot(identical(
      old,
      readBin(file.path(root, "VERSION"), "raw", n = 1000)
    ))
    unlink(file.path(root, "VERSION"))
    do.call(projr_version_set, c(list("1.0.0"), args))
    stopifnot(!file.exists(file.path(root, "VERSION")))
    do.call(projr_version_set, c(list("1.0.0", only_if_exists = FALSE), args))
    stopifnot(file.exists(file.path(root, "VERSION")))
    writeLines("Package: existing", file.path(root, "DESCRIPTION"))
    stopifnot(do.call(projr_version_get, args) == "1.0.0")
    do.call(projr_build_patch, c(args, list(execution_dir = "project")))
    stopifnot(
      do.call(projr_version_get, args) == "1.0.1.1",
      any(readLines(file.path(root, "DESCRIPTION")) == "Version: 1.0.1.1")
    )
    unlink(file.path(root, "DESCRIPTION"))
    writeLines("book_filename: book", file.path(root, "_bookdown.yml"))
    code(do.call(projr_build_patch, args), "unsupported_bookdown_semantics")
    cat(
      paste0(
        "PASS candidate preview/build/path/version wrappers, exact ",
        "preview preservation and explicit legacy boundaries\n"
      )
    )
  }
  run()
}
