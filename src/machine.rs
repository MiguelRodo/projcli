//! Opt-in CLI transport; domain work continues through existing anyhow-based APIs.
use serde::Serialize;
use serde_json::{Value, json};
use std::{io::{self, Write}, process::{Command, Stdio}};
use crate::{Cli, Commands, PathCommands, VersionCommands, YmlCommands};

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
        "machine_commands": ["capabilities", "path get", "version get", "version set", "yml read", "init", "ignore add", "ignore remove", "build", "build-dev"],
        "non_interactive": true,
        "logs": "stderr",
        "queries": {"path get": "path", "version get": "version", "yml read": "validated"},
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
        Commands::Path { command: PathCommands::Get { label } } => {
            proj::yml_read_from(&root, false).and_then(|config| config.get_path(&root, label))
                .map(|path| json!({"path": path}))
                .map_err(|_| failure(ErrorCode::QueryFailed, "Cannot resolve the requested configured path."))
        }
        Commands::Version { command: VersionCommands::Get { no_v, format } } => {
            if !format.eq_ignore_ascii_case("text") && !format.eq_ignore_ascii_case("json") { return emit_argument_error(); }
            proj::version_get_from(&root).map(|version| json!({"version": version, "text": version.to_string(!no_v)}))
                .ok_or_else(|| failure(ErrorCode::QueryFailed, "Cannot read a valid project version."))
        }
        Commands::Yml { command: YmlCommands::Read } => {
            proj::yml_read_from(&root, false).map(|_| json!({"validated": true}))
                .map_err(|_| failure(ErrorCode::QueryFailed, "Cannot read valid active project configuration."))
        }
        _ => run_worker(&root),
    };
    emit(result)
}

fn run_worker(root: &camino::Utf8Path) -> Result<Value, MachineError> {
    let executable = std::env::current_exe().map_err(|_| failure(ErrorCode::WorkerUnavailable, "Cannot locate the CLI command worker."))?;
    let mut arguments: Vec<_> = std::env::args_os().skip(1).filter(|arg| arg != "--machine").collect();
    if !arguments.iter().any(|arg| arg == "--non-interactive") { arguments.push("--non-interactive".into()); }
    let status = Command::new(executable).args(arguments)
        .env(proj::PROJECT_ROOT_ENV, root.as_str())
        .stdin(Stdio::null()).stdout(Stdio::from(io::stderr())).stderr(Stdio::inherit())
        .status().map_err(|_| failure(ErrorCode::WorkerUnavailable, "Cannot start the CLI command worker."))?;
    if status.success() { Ok(json!({"completed": true})) }
    else { Err(failure(ErrorCode::CommandFailed, "Command failed; inspect stderr diagnostics. Local publication state may need inspection.")) }
}
