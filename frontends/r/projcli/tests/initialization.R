library(projcli)
inside <- function(x) get(x, asNamespace("projcli"), inherits = FALSE)
code <- function(expr, expected) {
  value <- tryCatch(force(expr), projcli_error = identity)
  stopifnot(inherits(value, "projcli_error"), identical(value$code, expected))
}
optional <- inside(".proj_optional_package")
# Dependency-install branches use no network or global mutations.
state <- new.env(parent = emptyenv())
state$n <- 0L
install <- function(x) {
  stopifnot(x == "renv")
  state$n <- state$n + 1L
}
optional(
  "renv",
  available = function(x) TRUE,
  install = install,
  can_prompt = FALSE
)
stopifnot(state$n == 0L)
for (policy in list(NULL, FALSE)) {
  code(
    optional(
      "renv",
      policy,
      available = function(x) FALSE,
      install = install,
      can_prompt = FALSE,
      ask = function(...) stop("unexpected prompt")
    ),
    "optional_package_missing"
  )
}
code(
  optional(
    "renv",
    available = function(x) FALSE,
    install = install,
    can_prompt = TRUE,
    ask = function(...) "n"
  ),
  "optional_package_missing"
)
stopifnot(state$n == 0L)
# Explicit consent must succeed without asking even in noninteractive sessions.
state$present <- FALSE
optional(
  "renv",
  TRUE,
  available = function(x) state$present,
  install = function(x) {
    state$present <- TRUE
  },
  can_prompt = FALSE,
  ask = function(...) stop("unexpected prompt")
)
stopifnot(state$present)
for (policy in list(TRUE, NULL)) {
  state$present <- FALSE
  optional(
    "renv",
    policy,
    available = function(x) state$present,
    install = function(x) {
      state$n <- state$n + 1L
      state$present <- TRUE
    },
    can_prompt = TRUE,
    ask = function(...) "yes"
  )
}
stopifnot(state$n == 2L)
code(
  optional(
    "renv",
    TRUE,
    available = function(x) FALSE,
    install = function(x) stop("private-canary"),
    can_prompt = FALSE
  ),
  "optional_package_install_failed"
)
code(
  optional(
    "renv",
    TRUE,
    available = function(x) FALSE,
    install = function(x) NULL,
    can_prompt = FALSE
  ),
  "optional_package_install_failed"
)
code(optional("renv", NA, available = function(x) TRUE), "invalid_arguments")
code(projr_init(readme_rmd = NA), "invalid_arguments")
code(
  projr_init(author_family = "Example", author_given = c("A", "B")),
  "invalid_arguments"
)
code(projr_init(title = "a\nb"), "invalid_arguments")
code(projr_init_license(), "invalid_arguments")
code(projr_init_license("proprietary"), "invalid_arguments")
# Test argument construction independently of installed CLI availability.
state$calls <- list()
scope <- new.env(parent = asNamespace("projcli"))
scope$.proj_init_prepare <- function(...) NULL
scope$proj_cli <- function(command, args = character(), ...) {
  state$calls[[length(state$calls) + 1L]] <- list(command = command, args = args)
  list(result = list(completed = TRUE))
}
adapt <- function(name) {
  f <- inside(name)
  environment(f) <- scope
  f
}
adapt("projr_init")()
stopifnot(identical(state$calls[[1]]$args, c("--git=false", "--github=false")))
adapt("projr_init")(
  title = "-Title with spaces",
  citation = TRUE,
  author_family = "Example",
  author_given = "Ada"
)
stopifnot(
  "--title=-Title with spaces" %in% state$calls[[2]]$args,
  "--author-family=Example" %in% state$calls[[2]]$args,
  !any(c("--", "--author-name=") %in% state$calls[[2]]$args)
)
adapt("projr_init_readme")(readme_rmd = TRUE, purpose = "Some purpose")
stopifnot(identical(state$calls[[3]]$args, c("--rmd", "--description=Some purpose")))
adapt("projr_init_license")("ccby", label = "raw-data")
stopifnot(identical(state$calls[[4]]$args, c("--license=ccby", "--label=raw-data")))
cat(
  "PASS named init adapters and optional dependency consent without network\n"
)
adapt("projr_init")(
  description_file = c(named = TRUE),
  readme_rmd = c(named = TRUE)
)
stopifnot(all(c("--description-file", "--readme-rmd") %in% state$calls[[5]]$args))
