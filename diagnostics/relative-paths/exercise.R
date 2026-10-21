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
    record <- tryCatch({
      data_path <- get_path("raw-data", "data.csv")
      helper_path <- get_path("raw-data", "helper.R")
      image_path <- get_path("raw-data", "images", "sentinel.png")
      output_path <- get_path("output", paste0(Sys.getenv("DIAG_CASE"), suffix, "-", resolver, ".txt"))
      record$data_path <- data_path
      record$helper_path <- helper_path
      record$image_path <- image_path
      record$output_path <- output_path
      record$output_absolute <- as.character(fs::path_norm(fs::path_abs(output_path)))
      values <- utils::read.csv(data_path)
      helper_environment <- new.env(parent = baseenv())
      source(helper_path, local = helper_environment)
      stopifnot(values$value[[1]] == expected, helper_environment$helper_marker() == "helper-sentinel", file.exists(image_path))
      dir.create(dirname(output_path), recursive = TRUE, showWarnings = FALSE)
      writeLines("cache output sentinel", output_path)
      record$data_path <- data_path
      record$helper_path <- helper_path
      record$image_path <- image_path
      record$output_path <- output_path
      record$output_absolute <- as.character(fs::path_norm(fs::path_abs(output_path)))
      record$relative <- !fs::is_absolute_path(data_path)
      cache <- diag_config(root)$directories$cache$path
      if (!fs::is_absolute_path(cache)) cache <- file.path(root, cache)
      wanted <- fs::path_norm(file.path(cache, "projr", Sys.getenv("DIAG_VERSION"), "output"))
      record$output_in_expected_cache <- fs::path_has_parent(fs::path_abs(output_path), wanted)
      record$ok <- TRUE
      record
    }, error = function(error) { record$ok <- FALSE; record$error <- conditionMessage(error); record })
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
if (Sys.getenv("DIAG_AUTO_EXERCISE") == "yes") diag_exercise()
