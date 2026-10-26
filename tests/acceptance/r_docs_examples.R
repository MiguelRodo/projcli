binary <- Sys.getenv("PROJCLI_R_API_TEST_BINARY")
stopifnot(nzchar(binary), file.exists(binary))
Sys.setenv(PATH = paste(dirname(normalizePath(binary, winslash = "/")),
                       Sys.getenv("PATH"), sep = .Platform$path.sep))
Sys.unsetenv(c("PROJCLI_BIN", "PROJ_PROJECT_ROOT", "PROJ_RUN_CONTEXT",
              "PROJR_PROFILE", "QUARTO_PROFILE"))
lines <- readLines("docs/r-frontend.qmd", warn = FALSE)
start <- which(lines == "<!-- R_QUICKSTART_BEGIN -->")
end <- which(lines == "<!-- R_QUICKSTART_END -->")
stopifnot(length(start) == 1L, length(end) == 1L, end > start,
          lines[start + 1L] == "~~~r", lines[end - 1L] == "~~~")
code <- lines[seq.int(start + 2L, end - 2L)]
eval(parse(text = code), envir = new.env(parent = globalenv()))
cat("PASS documented R quick start against the candidate CLI\n")
