# Diagnostic prototype only: never replaces or modifies projr functions.
diag_config <- function(root) {
  config <- yaml::read_yaml(file.path(root, "_projr.yml"))
  profiles <- strsplit(Sys.getenv("DIAG_PROFILES"), "[,;]")[[1]]
  for (profile in rev(profiles[nzchar(profiles)])) {
    path <- file.path(root, paste0("_projr-", profile, ".yml"))
    if (file.exists(path)) config <- utils::modifyList(config, yaml::read_yaml(path))
  }
  config
}
diag_prototype <- function(label, ...) {
  root <- Sys.getenv("DIAG_ROOT")
  config <- diag_config(root)
  configured <- config$directories[[label]]$path
  if (!fs::is_absolute_path(configured)) configured <- file.path(root, configured)
  if (grepl("^output", label)) {
    cache <- config$directories$cache$path
    if (!fs::is_absolute_path(cache)) cache <- file.path(root, cache)
    configured <- file.path(cache, "projr", Sys.getenv("DIAG_VERSION"), label)
  }
  as.character(fs::path_rel(fs::path_norm(file.path(configured, ...)), start = getwd()))
}
diag_exercise <- function(suffix = "") {
  root <- Sys.getenv("DIAG_ROOT")
  expected <- as.numeric(Sys.getenv("DIAG_SENTINEL"))
  records <- list()
  for (resolver in c("unchanged_projr_relative", "unchanged_projr_auto", "test_only_prototype")) {
    get_path <- switch(resolver,
      unchanged_projr_relative = function(label, ...) projr::projr_path_get(label, ..., create = label == "output", format = "relative"),
      unchanged_projr_auto = function(label, ...) projr::projr_path_get(label, ..., create = label == "output", format = "auto"),
      test_only_prototype = diag_prototype)
    record <- list(resolver = resolver, cwd = getwd(), captured_root = root,
                   expected_version = Sys.getenv("DIAG_VERSION"), profiles = Sys.getenv("DIAG_PROFILES"))
    record$stages <- list()
    for (stage in c("data", "helper", "image", "output")) {
      phase <- tryCatch({
        path <- switch(stage,
          data = get_path("raw-data", "data.csv"),
          helper = get_path("raw-data", "helper.R"),
          image = get_path("raw-data", "images", "sentinel.png"),
          output = get_path("output", paste0(Sys.getenv("DIAG_CASE"), suffix, "-", resolver, ".txt")))
        absolute <- as.character(fs::path_norm(fs::path_abs(path)))
        record[[paste0(stage, "_path")]] <- path
        phase <- list(path = path, absolute = absolute, relative = !fs::is_absolute_path(path))
        if (stage == "data") stopifnot(utils::read.csv(path)$value[[1]] == expected)
        if (stage == "helper") {
          helper_environment <- new.env(parent = baseenv())
          source(path, local = helper_environment)
          stopifnot(helper_environment$helper_marker() == "helper-sentinel")
        }
        if (stage == "image") stopifnot(file.exists(path))
        if (stage == "output") {
          dir.create(dirname(path), recursive = TRUE, showWarnings = FALSE)
          writeLines("cache output sentinel", path)
          cache <- diag_config(root)$directories$cache$path
          if (!fs::is_absolute_path(cache)) cache <- file.path(root, cache)
          wanted <- fs::path_norm(file.path(cache, "projr", Sys.getenv("DIAG_VERSION"), "output"))
          phase$in_expected_cache <- fs::path_has_parent(absolute, wanted)
        }
        phase$ok <- TRUE
        phase
      }, error = function(error) list(ok = FALSE, error = conditionMessage(error)))
      record$stages[[stage]] <- phase
    }
    record$ok <- all(vapply(record$stages, function(stage) isTRUE(stage$ok), logical(1)))
    records[[length(records) + 1L]] <- record
  }
  # Current released Rust CLI is contrasted, not treated as the proposed resolver.
  rust <- tryCatch({
    output <- system2(Sys.getenv("DIAG_PROJ_BIN"), c("--machine", "path", "get", "--label", "output"), stdout = TRUE, stderr = FALSE)
    value <- jsonlite::fromJSON(paste(output, collapse = "\n"))
    list(resolver = "released_rust_cli", cwd = getwd(), ok = value$ok, path = value$result$path)
  }, error = function(error) list(resolver = "released_rust_cli", ok = FALSE, error = conditionMessage(error)))
  records[[length(records) + 1L]] <- rust
  target <- file.path(Sys.getenv("DIAG_EVIDENCE"), paste0(Sys.getenv("DIAG_CASE"), suffix, ".json"))
  jsonlite::write_json(records, target, pretty = TRUE, auto_unbox = TRUE, null = "null")
  invisible(records)
}
diag_image_path <- function(resolver) {
  result <- tryCatch({
    path <- if (resolver == "unchanged_projr_relative") {
      projr::projr_path_get("raw-data", "images", "sentinel.png", create = FALSE, format = "relative")
    } else diag_prototype("raw-data", "images", "sentinel.png")
    if (!file.exists(path)) stop("Returned image does not exist from effective execution cwd")
    list(ok = TRUE, resolver = resolver, cwd = getwd(), path = path,
         absolute = as.character(fs::path_norm(fs::path_abs(path))))
  }, error = function(error) list(ok = FALSE, resolver = resolver, cwd = getwd(), error = conditionMessage(error)))
  jsonlite::write_json(result,
    file.path(Sys.getenv("DIAG_IMAGE_EVIDENCE"), paste0(Sys.getenv("DIAG_CASE"), "-", resolver, ".json")),
    pretty = TRUE, auto_unbox = TRUE)
  if (isTRUE(result$ok)) result$path else NULL
}
if (Sys.getenv("DIAG_AUTO_EXERCISE") == "yes") diag_exercise()
