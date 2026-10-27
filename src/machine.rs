//! Opt-in CLI transport; domain work continues through existing anyhow-based APIs.
use serde::Serialize;
use serde_json::{Value, json};
use std::{io::{self, Write}, process::{Command, Stdio}};
use crate::{Cli, Commands, PathCommands, RemoteCommands, VersionCommands, YmlCommands};

const PROTOCOL_VERSION: u32 = 1;
const CAPABILITY_VERSION: u32 = 1;

#[derive(Serialize)]
struct Envelope {
    protocol_version: u32,
    cli_version: &'static str,
    ok: bool,
    result: Option<Value>,
    error: Option<MachineError>,
}

#[derive(Serialize)]
struct MachineError {
    code: ErrorCode,
    message: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum ErrorCode { InvalidArguments, ProjectSelectionFailed, QueryFailed, CommandFailed, WorkerUnavailable }

fn emit(result: Result<Value, MachineError>) -> i32 {
    let (value, error) = match result { Ok(value) => (Some(value), None), Err(error) => (None, Some(error)) };
    let ok = error.is_none();
    let envelope = Envelope { protocol_version: PROTOCOL_VERSION, cli_version: env!("CARGO_PKG_VERSION"), ok, result: value, error };
    let mut stdout = io::stdout().lock();
    // Never panic on a closed protocol pipe, and never include raw errors/config/argv.
    if serde_json::to_writer(&mut stdout, &envelope).is_err() || stdout.write_all(b"\n").is_err() || stdout.flush().is_err() { return 1; }
    if ok { 0 } else { 1 }
}

fn failure(code: ErrorCode, message: &'static str) -> MachineError { MachineError { code, message } }

pub(crate) fn emit_argument_error() -> i32 {
    emit(Err(failure(ErrorCode::InvalidArguments, "Invalid command arguments; consult proj --help.")))
}

pub(crate) fn emit_capabilities() -> i32 {
    emit(Ok(json!({
        "capability_version": CAPABILITY_VERSION,
        "machine_commands": ["capabilities", "init plan", "init readme", "init license", "init renv", "history", "remote restore", "remote verify", "remote versions", "remote download", "remote store", "remote github assets", "remote github upload", "remote github download", "remote github upload-directory", "remote github upload-metadata", "remote github download-directory", "path get", "version get", "version set", "yml read", "yml get", "yml parameter", "init", "ignore add", "ignore remove", "build", "build-dev"],
        "non_interactive": true,
        "path_targets": ["safe", "final"],
        "path_fragments": true,
        "execution_directories": ["native", "project"],
        "path_formats": ["auto", "absolute", "relative"],
        "default_path_target": "safe",
        "default_path_format": "auto",
        "logs": "stderr",
        "attempt_history": true,
        "queries": {"init plan":"initialization","remote verify": "snapshot","history": "history","remote github assets": "assets", "remote store --dry-run": "plan", "remote versions": "versions", "path get": "path", "version get": "version", "yml read": "validated", "yml get": "configuration", "yml parameter": "value"},
        "preview_output_isolation": false,
        "attempt_release_records": false,
        "direct_r_ide_parity": false
    })))
}

pub(crate) fn run(cli: &Cli) -> i32 {
    if matches!(&cli.command, Commands::Capabilities) { return emit_capabilities(); }
    let root = match proj::resolve_project_root(cli.project.as_deref()) {
        Ok(root) => root,
        Err(_) => return emit(Err(failure(ErrorCode::ProjectSelectionFailed, "Cannot select the project directory."))),
    };
    let result = match &cli.command {
        Commands::Init {command:Some(crate::InitCommands::Plan),..} =>
            proj::init_setup::inspect(&root).map(|state|json!({"initialization":state}))
                .map_err(|_|failure(ErrorCode::QueryFailed,"Cannot inspect existing project setup.")),
        Commands::History {attempt,limit} => proj::history::read(&root,attempt.as_deref(),*limit)
            .map(|history|json!({"history":history}))
            .map_err(|_|failure(ErrorCode::QueryFailed,"Cannot inspect the requested build history.")),
        Commands::Remote {command:RemoteCommands::Verify {title,snapshot,profile}} =>
            proj::configured_cas::verify(&root,title,snapshot,profile.as_deref())
                .map(|result|json!({"snapshot":result}))
                .map_err(|error|{
                    eprintln!("CAS verification failed: {error:#}");
                    failure(ErrorCode::QueryFailed,"Cannot verify the requested CAS snapshot; inspect stderr.")
                }),
        Commands::Remote {command:command @ RemoteCommands::Restore {..}} => {
            crate::configured_restore(&root,command)
                .map(|result|serde_json::to_value(result).expect("Restore result is serializable"))
                .map_err(|error| {
                    eprintln!("Configured restore failed: {error:#}");
                    failure(ErrorCode::CommandFailed,"Configured restore failed; inspect stderr for installation/recovery details.")
                })
        }

        Commands::Remote { command: RemoteCommands::Github { command: command @ crate::GithubCommands::Assets { .. } } } => {
            crate::github_assets(&root, command)
                .map_err(|_| failure(ErrorCode::QueryFailed, "Cannot list the requested GitHub Release assets."))
        }
        Commands::Remote { command: RemoteCommands::Store { source, destination, strategy, inspect: _, dry_run: true } } => {
            crate::local_store_files(&root, source, destination, strategy, true)
                .map(|plan| json!({"plan": plan}))
                .map_err(|_| failure(ErrorCode::QueryFailed, "Cannot plan the requested local file transfer."))
        }
        Commands::Remote { command: RemoteCommands::Versions { path, label, structure, path_append_label } } => {
            crate::query_local_versions(&root, path, label, structure, *path_append_label)
                .map(|versions| json!({"versions": versions}))
                .map_err(|_| failure(ErrorCode::QueryFailed, "Cannot inspect the requested local remote versions."))
        }
        Commands::Path { command: PathCommands::Get { label, target, parts, format, relative_to } } => {
            crate::query_path(&root, label, target, parts, format.as_deref(), relative_to.as_deref())
                .map(|path| json!({"path": path}))
                .map_err(|_| failure(ErrorCode::QueryFailed, "Cannot resolve the requested configured path."))
        }
        Commands::Version { command: VersionCommands::Get { no_v, format } } => {
            if !format.eq_ignore_ascii_case("text") && !format.eq_ignore_ascii_case("json") { return emit_argument_error(); }
            proj::run_version_from(&root).map(|version| json!({"version": version, "text": version.to_string(!no_v)}))
                .map_err(|_| failure(ErrorCode::QueryFailed, "Cannot read a valid project version."))
        }
        Commands::Yml { command: YmlCommands::Read } => {
            proj::yml_read_from(&root, false).map(|_| json!({"validated": true}))
                .map_err(|_| failure(ErrorCode::QueryFailed, "Cannot read valid active project configuration."))
        }
        Commands::Yml { command: YmlCommands::Get { profile, check } } =>
            proj::yml::merged_configuration_from(&root, profile.as_deref(), *check)
                .map(|configuration| json!({"configuration": configuration}))
                .map_err(|_| failure(ErrorCode::QueryFailed, "Cannot read selected project configuration.")),
        Commands::Yml { command: YmlCommands::Parameter { profile, keys } } =>
            proj::yml::parameter_value_from(&root, profile.as_deref(), keys)
                .map(|value| json!({"value": value}))
                .map_err(|_| failure(ErrorCode::QueryFailed, "Cannot read the selected project parameter.")),
        _ => run_worker(&root),
    };
    emit(result)
}

fn run_worker(root: &camino::Utf8Path) -> Result<Value, MachineError> {
    let executable = std::env::current_exe().map_err(|_| failure(ErrorCode::WorkerUnavailable, "Cannot locate the CLI command worker."))?;
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    let boundary = arguments.iter().position(|arg| arg == "--").unwrap_or(arguments.len());
    // Only controls before the option terminator are flags. Literal positional
    // values after it (including --machine) must reach the existing CLI unchanged.
    let non_interactive = arguments[..boundary].iter().any(|arg| arg == "--non-interactive");
    let mut worker_arguments = Vec::new();
    if !non_interactive { worker_arguments.push("--non-interactive".into()); }
    worker_arguments.extend(arguments.into_iter().enumerate().filter_map(|(index, arg)| {
        if index < boundary && arg == "--machine" { None } else { Some(arg) }
    }));
    let status = Command::new(executable).args(worker_arguments)
        .env(proj::PROJECT_ROOT_ENV, root.as_str())
        .stdin(Stdio::null()).stdout(Stdio::from(io::stderr())).stderr(Stdio::inherit())
        .status().map_err(|_| failure(ErrorCode::WorkerUnavailable, "Cannot start the CLI command worker."))?;
    if status.success() { Ok(json!({"completed": true})) }
    else { Err(failure(ErrorCode::CommandFailed, "Command failed; inspect stderr diagnostics. Local publication state may need inspection.")) }
}
