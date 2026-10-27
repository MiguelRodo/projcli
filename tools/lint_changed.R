paths <- commandArgs(trailingOnly = TRUE)
stopifnot(as.character(utils::packageVersion("lintr")) == "3.4.0")
lint_count <- 0L
for (path in paths) {
  findings <- lintr::lint(path, cache = FALSE)
  if (length(findings) > 0L) {
    print(findings)
    lint_count <- lint_count + length(findings)
  }
}
if (lint_count > 0L) {
  stop("Changed R files contain lint findings.", call. = FALSE)
}
