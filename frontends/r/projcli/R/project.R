# Public argument adaptation only; build/version/path behavior stays in Rust.
.proj_flag <- function(value) {
  if (!is.logical(value) || length(value) != 1L || is.na(value)) {
    .proj_error("invalid_arguments", "Provide TRUE or FALSE for boolean options.")
  }
  value
}
.proj_complete <- function(reply) {
  if (!isTRUE(.proj_query_value(reply, "completed"))) {
    .proj_error("invalid_protocol", "The CLI did not confirm completion. Inspect possible effects before retrying.")
  }
  invisible(TRUE)
}
.proj_r_path <- function(path) {
  if (.Platform$OS.type != "windows") return(path)
  # Native Rust absolute paths may use the verbatim namespace. Windows forbids
  # mixed '/' separators there; R file.path adds '/', so return ordinary syntax.
  if (startsWith(path,"\\\\?\\UNC\\")) {
    path <- paste0("//",substring(path,9L))
  } else if (startsWith(path,"\\\\?\\")) {
    path <- substring(path,5L)
  }
  gsub("\\","/",path,fixed=TRUE)
}
.proj_version_root <- function(project, binary, env, timeout) {
  reply <- proj_cli("path get", c("--label","project","--format","absolute"),
                    project,binary,env,timeout)
  root <- .proj_query_value(reply,"path")
  if (!.proj_strings(root,scalar=TRUE,empty=FALSE)) {
    .proj_error("invalid_protocol","The CLI returned an invalid project path.")
  }
  root <- .proj_r_path(root)

  root
}
.proj_build_root <- function(project,binary,env,timeout) {
  root <- .proj_version_root(project,binary,env,timeout)
  if (file.exists(file.path(root,"_bookdown.yml"))) {
    .proj_error("unsupported_bookdown_semantics",
      "The legacy Bookdown build uses render_book; the standalone CLI site driver is different. Select an explicit existing R build function for books.")
  }
  root
}
.proj_execution_arg <- function(value) {
  if (is.null(value)) return(character())
  if (!.proj_strings(value,scalar=TRUE) || !value %in% c("native","project")) {
    .proj_error("invalid_arguments","execution_dir must be NULL, native or project.")
  }
  paste0("--execution-dir=",value)
}
.proj_build_options <- function(msg=NULL,args_engine=list(),profile=NULL,
  archive_github=FALSE,archive_local=FALSE,always_archive=TRUE,clear_output=NULL,execution_dir=NULL) {
  list(msg=msg,args_engine=args_engine,profile=profile,clear_output=clear_output,execution_dir=execution_dir,
       provided=names(match.call())[-1L])
}
.proj_build_call <- function(component,args,backend,project,binary,env,timeout) {
  if (is.function(backend)) return(.proj_callback(backend,args,project,binary,env,timeout))
  .proj_cli_backend(backend)
  if (!.proj_strings(component,scalar=TRUE) || !component %in% c("major","minor","patch")) {
    .proj_error("unsupported_bump","Select an explicit major, minor or patch bump, or an existing R function backend.")
  }
  options <- tryCatch(do.call(.proj_build_options,args),
    error=function(e) .proj_error("unsupported_arguments","Unsupported build arguments; select an explicit R function for the legacy API."))
  if (length(options$args_engine) || (!is.null(options$args_engine) && !is.list(options$args_engine)) ||
      any(c("archive_github","archive_local","always_archive") %in% options$provided)) {
    .proj_error("unsupported_arguments",
      "Renderer objects and legacy archive switches are not CLI overrides. Use configured destinations or an explicit existing R function.")
  }
  if (!is.null(options$msg) && !.proj_strings(options$msg,scalar=TRUE)) {
    .proj_error("invalid_arguments","msg must be NULL or one character value.")
  }
  if (!is.null(options$profile) && !.proj_strings(options$profile,scalar=TRUE,empty=FALSE)) {
    .proj_error("invalid_arguments","profile must be NULL or one profile-list string.")
  }
  stage <- options$clear_output
  if (!is.null(stage) && (!.proj_strings(stage,scalar=TRUE) ||
      !stage %in% c("none","never","pre","post"))) {
    .proj_error("invalid_arguments","clear_output must be NULL, none, never, pre or post.")
  }
  execution <- .proj_execution_arg(options$execution_dir)
  .proj_build_root(project,binary,env,timeout)
  args <- c(paste0("--",component),execution,
    if (!is.null(options$msg)) paste0("--description=",options$msg),
    if (!is.null(options$profile)) paste0("--profile=",options$profile),
    if (!is.null(stage)) paste0("--output-clear-stage=",if(stage=="never") "none" else stage))
  .proj_complete(proj_cli("build",args,project,binary,env,timeout))
}

#' Build a configured VERSION project
#' @param bump_component Explicit major, minor or patch.
#' @param ... Common build options: msg, empty args_engine, profile, clear_output, execution_dir.
#'   Explicit legacy archive switches require an existing R callback.
#' @param backend,project,binary,env,timeout See projr_yml_get.
#' @return Invisible TRUE on CLI success; callback results stay unchanged.
#' @export
projr_build <- function(bump_component,...,backend="cli",project=NULL,
                        binary=NULL,env=character(),timeout=Inf) {
  args <- list(...)
  if (is.function(backend)) {
    if (!missing(bump_component)) args <- c(list(bump_component=bump_component),args)
    return(.proj_callback(backend,args,project,binary,env,timeout))
  }
  if (missing(bump_component)) {
    .proj_error("unsupported_implicit_bump","Choose an explicit bump component; no legacy configured bump is inferred.")
  }
  .proj_build_call(bump_component,args,backend,project,binary,env,timeout)
}
#' @rdname projr_build
#' @export
projr_build_major <- function(...,backend="cli",project=NULL,binary=NULL,env=character(),timeout=Inf)
  .proj_build_call("major",list(...),backend,project,binary,env,timeout)
#' @rdname projr_build
#' @export
projr_build_minor <- function(...,backend="cli",project=NULL,binary=NULL,env=character(),timeout=Inf)
  .proj_build_call("minor",list(...),backend,project,binary,env,timeout)
#' @rdname projr_build
#' @export
projr_build_patch <- function(...,backend="cli",project=NULL,binary=NULL,env=character(),timeout=Inf)
  .proj_build_call("patch",list(...),backend,project,binary,env,timeout)

#' Preview configured R steps without version or publication changes
#' @param file,bump,old_dev_remove,args_engine Legacy controls; CLI supports only
#'   NULL file, FALSE bump, FALSE removal and empty renderer arguments.
#' @param profile,clear_output Explicit CLI profile/clear policy.
#' @param backend,project,binary,env,timeout See projr_yml_get.
#' @return Invisible TRUE on CLI success; callback results stay unchanged.
#' @export
projr_build_dev <- function(file=NULL,bump=FALSE,old_dev_remove=FALSE,args_engine=list(),
 profile=NULL,clear_output="never",backend="cli",project=NULL,binary=NULL,env=character(),timeout=Inf,execution_dir=NULL) {
  supplied <- names(match.call())[-1L]
  values <- list(file=file,bump=bump,old_dev_remove=old_dev_remove,args_engine=args_engine,
                 profile=profile,clear_output=clear_output,execution_dir=execution_dir)
  if (is.function(backend)) return(.proj_callback(backend,
    values[intersect(names(values),supplied)],project,binary,env,timeout))
  .proj_cli_backend(backend)
  if (!is.null(file) || !identical(bump,FALSE) || !identical(old_dev_remove,FALSE) ||
      length(args_engine) || (!is.null(args_engine) && !is.list(args_engine))) {
    .proj_error("unsupported_arguments",
      "CLI preview uses configured scripts without dev bumps, old-cache removal or renderer objects. Select an explicit existing R function for those controls.")
  }
  if (!is.null(profile) && !.proj_strings(profile,scalar=TRUE,empty=FALSE)) {
    .proj_error("invalid_arguments","profile must be NULL or one profile-list string.")
  }
  if (!.proj_strings(clear_output,scalar=TRUE) || !clear_output %in% c("none","never","pre","post")) {
    .proj_error("invalid_arguments","Select a supported clear policy.")
  }
  execution <- .proj_execution_arg(execution_dir)
  .proj_build_root(project,binary,env,timeout)
  args <- c(execution,if(!is.null(profile)) paste0("--profile=",profile),
    if("clear_output" %in% supplied) paste0("--output-clear-stage=",if(clear_output=="never") "none" else clear_output))
  .proj_complete(proj_cli("build-dev",args,project,binary,env,timeout))
}

#' Resolve one logical project path without creating directories
#' @param label One declared logical label or supported builtin.
#' @param ... Unnamed character scalar fragments.
#' @param create FALSE for CLI lookup; TRUE requires an existing R callback.
#' @param format auto, relative or absolute. Omission preserves core configured policy.
#' @param safe Use the captured versioned safe target or configured final target.
#' @param backend,project,binary,env,timeout See projr_yml_get.
#' @return One character path, or the existing callback result.
#' @export
projr_path_get <- function(label,...,create=FALSE,format=c("auto","relative","absolute"),
 safe=TRUE,backend="cli",project=NULL,binary=NULL,env=character(),timeout=Inf) {
  parts <- list(...);supplied <- names(match.call())[-1L]
  if (is.function(backend)) {
    flags <- list(create=create,format=format,safe=safe)
    return(.proj_callback(backend,c(list(label=label),parts,
      flags[intersect(names(flags),supplied)]),project,binary,env,timeout))
  }
  .proj_cli_backend(backend)
  if (!.proj_strings(label,scalar=TRUE,empty=FALSE) ||
      !all(vapply(parts,.proj_strings,logical(1),scalar=TRUE)) ||
      (!is.null(names(parts)) && any(nzchar(names(parts))))) {
    .proj_error("invalid_arguments","Use one label and unnamed character scalar fragments; use format for path presentation.")
  }
  if (tolower(label) %in% c("code","data")) {
    .proj_error("unsupported_legacy_label","Legacy code/data lookups have special behavior. Use explicit supported CLI roles or an existing R function backend.")
  }
  if (.proj_flag(create)) .proj_error("unsupported_creation",
    "CLI lookup is read-only. Create the analysis folder explicitly or choose an existing R function backend.")
  safe <- .proj_flag(safe)
  selected <- tryCatch(match.arg(format,c("auto","relative","absolute")),
    error=function(e) .proj_error("invalid_arguments","format must be auto, relative or absolute."))
  args <- c("--label",label,paste0("--target=",if(safe) "safe" else "final"),
    if("format" %in% supplied) paste0("--format=",selected),"--",unlist(parts,use.names=FALSE))
  reply <- proj_cli("path get",args,project,binary,env,timeout)
  path <- .proj_query_value(reply,"path")
  if (!.proj_strings(path,scalar=TRUE,empty=FALSE)) .proj_error("invalid_protocol","The CLI returned an invalid path.")
  .proj_r_path(path)
}

#' Read the captured VERSION using the CLI canonical spelling
#' @param backend,project,binary,env,timeout See projr_yml_get.
#' @return Character version without v, or unchanged callback result.
#' @export
projr_version_get <- function(backend="cli",project=NULL,binary=NULL,env=character(),timeout=Inf) {
  if(is.function(backend)) return(.proj_callback(backend,list(),project,binary,env,timeout))
  .proj_cli_backend(backend);.proj_version_root(project,binary,env,timeout)
  reply <- proj_cli("version get","--no-v",project,binary,env,timeout)
  text <- .proj_query_value(reply,"text")
  if(!.proj_strings(text,scalar=TRUE,empty=FALSE)) .proj_error("invalid_protocol","The CLI returned invalid version text.")
  text
}
#' Set VERSION and synchronize existing DESCRIPTION unless disabled in settings
#' @param version One CLI-supported version string.
#' @param only_if_exists Leave an absent VERSION absent when TRUE.
#' @param backend,project,binary,env,timeout See projr_yml_get.
#' @return Invisible TRUE on CLI success/no-op; callback result otherwise.
#' @export
projr_version_set <- function(version,only_if_exists=TRUE,backend="cli",project=NULL,
 binary=NULL,env=character(),timeout=Inf) {
  if(missing(version)) .proj_error("invalid_arguments","Supply a version string.")
  if(is.function(backend)) {
    args <- list(version=version)
    if(!missing(only_if_exists)) args$only_if_exists <- only_if_exists
    return(.proj_callback(backend,args,project,binary,env,timeout))
  }
  .proj_cli_backend(backend)
  if(!.proj_strings(version,scalar=TRUE,empty=FALSE)) .proj_error("unsupported_version_value","Use a version string or an existing R function for R version objects.")
  only_if_exists <- .proj_flag(only_if_exists)
  root <- .proj_version_root(project,binary,env,timeout)
  if(only_if_exists && !file.exists(file.path(root,"VERSION"))) return(invisible(TRUE))
  .proj_complete(proj_cli("version set",c("--",version),project,binary,env,timeout))
}
