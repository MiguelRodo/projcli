mod machine;
use clap::{Parser, Subcommand};

/// Cross-platform CLI for version-linked project builds.
///
/// This application manages environment variables, validates configuration files (`_proj.yml`),
/// and maintains project versioning across Rust, Python, and R environments.
///
/// # Panics
///
/// Commands might panic if standard streams (`stdout`/`stderr`) fail or if
/// configuration parsing encounters entirely corrupted filesystem states.
/// ```rust
/// use clap::Parser;
/// // let args = vec!["proj", "version", "get"];
/// // let cli = Cli::parse_from(args);
/// ```
#[derive(Parser)]
#[command(name = "proj", version)]
pub struct Cli {
    /// Emit one protocol-v1 JSON result on stdout; diagnostics and child logs use stderr.
    #[arg(long, global = true)]
    machine: bool,
    /// Disable CLI prompts and automatic package installation, and close runner stdin.
    #[arg(long, global = true)]
    non_interactive: bool,
    /// Select an exact existing project directory without requiring root markers.
    #[arg(long, global = true, value_name = "PATH")]
    project: Option<camino::Utf8PathBuf>,
    #[command(subcommand)]
    command: Commands,
}

/// The top-level commands available in the `proj` CLI.
///
/// This enum routes execution to specific domain logic such as YAML validation,
/// version string manipulation, or path resolution.
///
/// # Errors
///
/// Dispatching subcommands might fail if underlying methods return an `Err`
/// (e.g. unreadable configuration file).
/// ```rust
/// // use proj::Commands;
/// ```
#[derive(Subcommand)]
pub enum Commands {
    /// Report the CLI and machine protocol capabilities without loading a project.
    Capabilities,
    /// Inspect recorded build outcomes and final files without changing project state.
    History {
        #[arg(long)]
        attempt: Option<String>,
        #[arg(long, default_value_t=20)]
        limit: usize,
    },
    /// Inspect, publish or restore remote data.
    Remote {
        #[command(subcommand)]
        command: RemoteCommands,
    },
    /// Init related operations
    Init {
        #[command(subcommand)]
        command: Option<InitCommands>,
    },
    /// YML related operations
    Yml {
        #[command(subcommand)]
        command: YmlCommands,
    },
    /// Version related operations
    Version {
        #[command(subcommand)]
        command: VersionCommands,
    },
    /// Path related operations
    Path {
        #[command(subcommand)]
        command: PathCommands,
    },
    /// Ignore related operations
    Ignore {
        #[command(subcommand)]
        command: IgnoreCommands,
    },
    /// Build operations
    Build {
        /// Build a major version
        #[arg(long, group = "bump")]
        major: bool,

        /// Build a minor version
        #[arg(long, group = "bump")]
        minor: bool,

        /// Build a patch version
        #[arg(long, group = "bump")]
        patch: bool,

        /// Optional profile
        #[arg(long)]
        profile: Option<String>,

        /// Optional description for the build commit
        #[arg(long)]
        description: Option<String>,
        /// Native runner execution directories, or explicit project-root override.
        #[arg(long, value_parser = ["native", "project"])]
        execution_dir: Option<String>,
        /// Clear final output immediately before/after analysis; preview never clears it.
        #[arg(long, value_parser = ["none", "pre", "post"])]
        output_clear_stage: Option<String>,
    },
    /// Dev Build operations
    BuildDev {
        /// Optional profile
        #[arg(long)]
        profile: Option<String>,
        /// Native runner execution directories, or explicit project-root override.
        #[arg(long, value_parser = ["native", "project"])]
        execution_dir: Option<String>,
        /// Clear final output immediately before/after analysis; preview never clears it.
        #[arg(long, value_parser = ["none", "pre", "post"])]
        output_clear_stage: Option<String>,
    },
}

/// Remote operations reuse registered definitions or explicit ordinary/local arguments.
#[derive(Subcommand)]
pub enum RemoteCommands {
    /// Restore one declared directory using registered or inline sources.
    Restore {
        #[arg(long)]
        label:String,
        #[arg(long,value_parser=["source","dest"])]
        position:Option<String>,
        #[arg(long,value_parser=["local","github"])]
        provider:Option<String>,
        /// Select a configured remote by name.
        #[arg(long)]
        title:Option<String>,
        #[arg(long)]
        remote_version:Option<String>,
        /// Restore an exact local CAS or experimental Xet snapshot; requires its configured remote title.
        #[arg(long,requires="title")]
        snapshot:Option<String>,
        /// Explicit owner/repository; otherwise resolve the selected project's Git origin.
        #[arg(long)]
        repository:Option<String>,
        #[arg(long)]
        profile:Option<String>,
        #[arg(long)]
        clear:bool,
        #[arg(long,required_unless_present="snapshot")]
        max_bytes:Option<u64>,
        #[arg(long,required_unless_present="snapshot")]
        max_expanded_bytes:Option<u64>,
    },
    /// Check an exact local or GitHub snapshot; GitHub Xet requires archive byte budgets.
    Verify {
        #[arg(long)]
        title:String,
        #[arg(long)]
        snapshot:String,
        #[arg(long)]
        profile:Option<String>,
        /// Required for GitHub Xet; maximum complete-store ZIP bytes.
        #[arg(long)]
        max_bytes:Option<u64>,
        /// Required for GitHub Xet; maximum expanded store bytes.
        #[arg(long)]
        max_expanded_bytes:Option<u64>,
    },
    /// Explicit GitHub Release assets; does not change configured destinations.
    Github {
        #[command(subcommand)]
        command: GithubCommands,
    },
    /// Apply a file-inspected policy to a concrete local target; no metadata publication.
    Store {
        /// Explicit physical source directory.
        #[arg(long)]
        source: camino::Utf8PathBuf,
        /// Concrete target directory whose parent exists.
        #[arg(long)]
        destination: camino::Utf8PathBuf,
        /// Existing projr file transfer strategy.
        #[arg(long, default_value = "sync-diff", value_parser = ["upload-all", "upload-missing", "sync-diff", "sync-purge"])]
        strategy: String,
        /// This bounded command requires explicit file inspection.
        #[arg(long, value_parser = ["file"])]
        inspect: String,
        /// Report actions without creating or modifying files.
        #[arg(long)]
        dry_run: bool,
    },
    /// Download an ordinary local version using staged destination replacement.
    Download {
        /// Storage base, relative to the selected project root unless absolute.
        #[arg(long)]
        path: camino::Utf8PathBuf,
        /// Existing remote directory label.
        #[arg(long)]
        label: String,
        /// Ordinary remote layout.
        #[arg(long, default_value = "archive", value_parser = ["latest", "archive"])]
        structure: String,
        /// Exact archive version directory spelling; omit for latest.
        #[arg(long)]
        remote_version: Option<String>,
        /// Explicit local destination whose parent already exists.
        #[arg(long)]
        destination: camino::Utf8PathBuf,
        /// Whether the storage layout appends the label to its base.
        #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
        path_append_label: bool,
        /// Discard destination-only files after a successful staged download.
        #[arg(long)]
        clear: bool,
    },
    /// List versions in a local latest/archive layout; does not verify file bytes.
    Versions {
        /// Storage base, relative to the selected project root unless absolute.
        #[arg(long)]
        path: camino::Utf8PathBuf,
        /// Existing remote directory label.
        #[arg(long)]
        label: String,
        /// Ordinary remote layout (CAS is separate).
        #[arg(long, default_value = "archive", value_parser = ["latest", "archive"])]
        structure: String,
        /// Whether the storage layout appends the label to its base.
        #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
        path_append_label: bool,
    },
}

/// Native GitHub Release API operations. Tokens are read from the existing environment.
#[derive(Subcommand)]
pub enum GithubCommands {
    /// Package a directory with projr naming and upload a new asset.
    UploadDirectory {
        #[arg(long)]
        repository: String,
        #[arg(long)]
        tag: String,
        #[arg(long)]
        source: camino::Utf8PathBuf,
        #[arg(long)]
        label: String,
        #[arg(long, default_value = "archive", value_parser = ["latest", "archive"])]
        structure: String,
        #[arg(long)]
        remote_version: Option<String>,
        #[arg(long)]
        prefix: Option<String>,
        #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
        path_append_label: bool,
        #[arg(long)]
        create_release: bool,
        /// Explicitly delete an existing same-name asset before uploading.
        #[arg(long)]
        replace: bool,
    },
    /// Upload a new canonical manifest/VERSION/CHANGELOG ZIP asset.
    UploadMetadata {
        #[arg(long)]
        repository: String,
        #[arg(long)]
        tag: String,
        /// Directory containing the canonical metadata file.
        #[arg(long)]
        source: camino::Utf8PathBuf,
        #[arg(long, value_parser = ["manifest", "version", "changelog"])]
        kind: String,
        #[arg(long)]
        create_release: bool,
        /// Explicitly delete an existing same-name asset before uploading.
        #[arg(long)]
        replace: bool,
    },
    /// Safely extract a selected directory ZIP and merge/clear its destination.
    DownloadDirectory {
        #[arg(long)]
        repository: String,
        #[arg(long)]
        tag: String,
        #[arg(long)]
        name: String,
        #[arg(long)]
        destination: camino::Utf8PathBuf,
        #[arg(long)]
        max_bytes: u64,
        #[arg(long)]
        max_expanded_bytes: u64,
        #[arg(long)]
        clear: bool,
        /// Require and consume the projr empty-variant sentinel.
        #[arg(long)]
        empty: bool,
    },
    /// List all assets on an existing exact-tag Release.
    Assets {
        #[arg(long)]
        repository: String,
        #[arg(long)]
        tag: String,
    },
    /// Upload an asset; existing names require explicit --replace.
    Upload {
        #[arg(long)]
        repository: String,
        #[arg(long)]
        tag: String,
        #[arg(long)]
        name: String,
        #[arg(long)]
        source: camino::Utf8PathBuf,
        /// Explicitly allow creating the Release if its tag is absent.
        #[arg(long)]
        create_release: bool,
        /// Explicitly delete an existing same-name asset before uploading.
        #[arg(long)]
        replace: bool,
    },
    /// Download one asset with staged regular-file replacement.
    Download {
        #[arg(long)]
        repository: String,
        #[arg(long)]
        tag: String,
        #[arg(long)]
        name: String,
        #[arg(long)]
        destination: camino::Utf8PathBuf,
        /// Maximum advertised and downloaded size in bytes.
        #[arg(long)]
        max_bytes: u64,
    },
}

/// Operations specific to the `_proj.yml` configuration file.
///
/// Allows interacting with the YAML definition of the project, including
/// deep validation and parsing of directory labels.
///
/// # Errors
///
/// May fail if the YAML file contains structurally invalid labels or
/// malformed content.
/// ```rust
/// // use proj::YmlCommands;
/// ```
#[derive(Subcommand)]
pub enum YmlCommands {
    /// Read the _proj.yml content and trigger validation.
    Read,
    /// Return explicitly requested merged configuration as JSON (may contain secrets).
    Get {
        /// Select merged profile overlays; default retains an inherited run's profiles.
        #[arg(long)]
        profile: Option<String>,
        /// Also validate the existing runtime schema and paths.
        #[arg(long)]
        check: bool,
    },
    /// Return a parameter selected by exact mapping keys, without evaluating code.
    Parameter {
        /// Select merged profile overlays.
        #[arg(long)]
        profile: Option<String>,
        /// Literal keys; use -- before a key that begins with a dash.
        #[arg(value_name = "KEY")]
        keys: Vec<String>,
    },
}

/// Path resolution commands.
///
/// Maps logical directory labels to concrete absolute paths within the current project.
///
/// # Errors
///
/// Might return an error if the project root cannot be found or the specified
/// label does not follow the required prefixed naming convention.
/// ```rust
/// // use proj::PathCommands;
/// ```
#[derive(Subcommand)]
pub enum PathCommands {
    /// Get the resolved path for a given label
    Get {
        /// The label of the directory to get
        #[arg(long)]
        label: String,
        /// Safe versioned cache target or configured final mapping.
        #[arg(long, default_value = "safe", value_parser = ["safe", "final"])]
        target: String,
        /// Relative file/subdirectory fragments appended to the selected role.
        #[arg(value_name = "PART")]
        parts: Vec<camino::Utf8PathBuf>,
        /// Path representation, independent of target and runner cwd.
        #[arg(long, value_parser = ["auto", "absolute", "relative"])]
        format: Option<String>,
        /// cwd, project, document, or an absolute directory; requires relative format.
        #[arg(long, value_name = "BASE")]
        relative_to: Option<String>,
    },
}

/// Commands for reading and updating the project version.
///
/// This manages the `VERSION` file, ensuring consistent formatting.
///
/// # Errors
///
/// Can fail if the file is missing, permissions are denied, or an invalid
/// version format is supplied during a `set` operation.
/// ```rust
/// // use proj::VersionCommands;
/// ```
#[derive(Subcommand)]
pub enum VersionCommands {
    /// Get the project version
    Get {
        /// Omit the 'v' prefix
        #[arg(long, default_value_t = false)]
        no_v: bool,

        /// Output format: "text" or "json"
        #[arg(long, default_value = "text")]
        format: String,
    },
    /// Set the project version
    Set {
        /// The new version string (e.g. v1.2.3.4, 1.2.3, or JSON)
        version: String,
    },
}

/// Commands for interacting with tracking file exclusions.
///
/// Handles operations to manually or automatically modify `.gitignore`
/// and `.Rbuildignore` files.
///
/// # Errors
///
/// May fail if an ignore file cannot be created, written to, or modified
/// due to lack of filesystem permissions.
/// ```rust
/// // use proj::IgnoreCommands;
/// ```
#[derive(Subcommand)]
pub enum IgnoreCommands {
    /// Manually append paths to project ignore files
    Add {
        /// One or more raw file or directory paths
        #[arg(required = true)]
        paths: Vec<String>,

        /// Flag to force create ignore files if they do not exist
        #[arg(long, action = clap::ArgAction::Set, default_value_t = true)]
        force_create: bool,

        /// Target tracking surfaces (all, git, rbuild)
        #[arg(long, value_enum, default_value_t = proj::IgnoreType::All)]
        r#type: proj::IgnoreType,
    },
    /// Manually force paths to be tracked by removing them from ignores via negation
    Remove {
        /// One or more raw file or directory paths
        #[arg(required = true)]
        paths: Vec<String>,

        /// Target tracking surfaces (all, git, rbuild)
        #[arg(long, value_enum, default_value_t = proj::IgnoreType::All)]
        r#type: proj::IgnoreType,
    },
}

fn fail(message: std::fmt::Arguments<'_>) -> ! {
    eprintln!("{}", message);
    std::process::exit(1);
}

fn main() {
    let machine_requested = std::env::args_os().skip(1).take_while(|argument| argument != "--").any(|argument| argument == "--machine");
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) if machine_requested => {
            let informational = matches!(error.kind(), clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion);
            let code = if informational { machine::emit_capabilities() } else { machine::emit_argument_error() };
            std::process::exit(code);
        }
        Err(error) => error.exit(),
    };
    if cli.machine {
        std::process::exit(machine::run(&cli));
    }
    if matches!(&cli.command, Commands::Capabilities) {
        std::process::exit(machine::emit_capabilities());
    }
    let mut interaction_guard = proj::EnvGuard::new();
    if cli.non_interactive {
        interaction_guard.track_and_set(proj::NON_INTERACTIVE_ENV, "true")
            .unwrap_or_else(|error| fail(format_args!("Invalid interaction environment: {error}")));
    }
    let root = match proj::resolve_project_root(cli.project.as_deref()) {
        Ok(root) => root,
        Err(error) => {
            fail(format_args!("Project selection failed: {:#}", error));
        }
    };
    let mut root_guard = proj::EnvGuard::new();
    root_guard.track_and_set(proj::PROJECT_ROOT_ENV, root.as_str())
        .unwrap_or_else(|error| fail(format_args!("Invalid project environment: {error}")));

    match &cli.command {
        Commands::Capabilities => unreachable!("Capabilities handled before project selection"),
        Commands::History {attempt,limit} => match proj::history::read(&root,attempt.as_deref(),*limit) {
            Ok(history)=>print!("{}",history.markdown()),
            Err(error)=>fail(format_args!("Cannot inspect build history: {error:#}")),
        },
        Commands::Remote {command:RemoteCommands::Verify {title,snapshot,profile,max_bytes,max_expanded_bytes}} => {
            match configured_verify(&root,title,snapshot,profile.as_deref(),*max_bytes,*max_expanded_bytes) {
                Ok(result)=>println!("{}",serde_json::to_string_pretty(&result).expect("CAS observation is serializable")),
                Err(error)=>fail(format_args!("CAS verification failed: {error:#}")),
            }
        },
        Commands::Remote {command:command @ RemoteCommands::Restore {..}} => {
            match configured_restore(&root,command) {
                Ok(result)=>println!("{}",serde_json::to_string_pretty(&result).expect("Restore result is serializable")),
                Err(error)=>fail(format_args!("Configured restore failed: {error:#}")),
            }
        },
        Commands::Remote { command: RemoteCommands::Github { command } } => {
            match github_assets(&root, command) {
                Ok(result) => println!("{}", serde_json::to_string_pretty(&result).expect("Asset metadata is serializable")),
                Err(error) => fail(format_args!("GitHub Release operation failed: {error:#}")),
            }
        },
        Commands::Remote { command: RemoteCommands::Store { source, destination, strategy, inspect: _, dry_run } } => {
            match local_store_files(&root, source, destination, strategy, *dry_run) {
                Ok(plan) => println!("{}", serde_json::to_string_pretty(&plan).expect("Store plan is serializable")),
                Err(error) => fail(format_args!("Cannot transfer local store files: {error:#}")),
            }
        },
        Commands::Remote { command: RemoteCommands::Download { path, label, structure, remote_version, destination, path_append_label, clear } } => {
            match download_local_version(&root, path, label, structure, remote_version.as_deref(), destination, *path_append_label, *clear) {
                Ok(result) => println!("Downloaded {} to {}", result.version, result.destination),
                Err(error) => fail(format_args!("Cannot download local remote version: {error:#}")),
            }
        },
        Commands::Remote { command: RemoteCommands::Versions { path, label, structure, path_append_label } } => {
            match query_local_versions(&root, path, label, structure, *path_append_label) {
                Ok(versions) => {
                    for entry in versions {
                        println!("{} {}{}{}{}", entry.version, entry.path,
                            if entry.empty { " [empty marker]" } else { "" },
                            if !entry.present { " [missing target]" } else { "" },
                            if entry.metadata_untrusted == Some(true) { " [metadata untrusted]" } else { "" });
                    }
                }
                Err(error) => fail(format_args!("Cannot list local remote versions: {error:#}")),
            }
        },
        Commands::Yml { command } => match command {
            YmlCommands::Read => {
                // Pass false for is_dev as this is just a read check
                match proj::yml_read(false) {
                    Ok(_) => println!("Successfully read and validated _proj.yml"),
                    Err(e) => fail(format_args!("Error reading _proj.yml: {}", e)),
                }
            }
            YmlCommands::Get { profile, check } => {
                match proj::yml::merged_configuration_from(&root, profile.as_deref(), *check) {
                    Ok(value) => println!("{}", serde_json::to_string_pretty(&value).expect("JSON value is serializable")),
                    Err(_) => fail(format_args!("Cannot read selected configuration. Check the selected files/profile; private details were withheld.")),
                }
            }
            YmlCommands::Parameter { profile, keys } => {
                match proj::yml::parameter_value_from(&root, profile.as_deref(), keys) {
                    Ok(value) => println!("{}", serde_json::to_string_pretty(&value).expect("JSON value is serializable")),
                    Err(_) => fail(format_args!("Cannot read selected parameter. Check the selected files/profile and literal keys; private details were withheld.")),
                }
            }
        },
        Commands::Path { command } => match command {
            PathCommands::Get { label, target, parts, format, relative_to } => {
                match query_path(&root, label, target, parts, format.as_deref(), relative_to.as_deref()) {
                    Ok(path) => println!("{}", path),
                    Err(error) => fail(format_args!("Error getting path: {error:#}")),
                }
            }
        },
        Commands::Build { major, minor, patch: _, profile, description, execution_dir, output_clear_stage } => {
            if let Some(root) = proj::root() {
                let mode = if *major {
                    proj::BuildMode::ProdMajor
                } else if *minor {
                    proj::BuildMode::ProdMinor
                } else {
                    proj::BuildMode::ProdPatch
                };

                let mut desc = description.clone();
                if desc.as_ref().map(|s| s.trim().is_empty()).unwrap_or(true) {
                    use std::io::{self, BufRead};
                    let is_terminal = std::io::IsTerminal::is_terminal(&io::stdin());
                    if is_terminal && !proj::is_non_interactive() {
                        let mut input = String::new();
                        let stdin = io::stdin();
                        loop {
                            println!("Enter a one-line description of the build: ");
                            input.clear();
                            if stdin.lock().read_line(&mut input).is_ok() {
                                let trimmed = input.trim();
                                if !trimmed.is_empty() {
                                    desc = Some(trimmed.to_string());
                                    break;
                                }
                            }
                        }
                    }
                }

                if let Err(e) = proj::build_project_with_policies(&camino::Utf8PathBuf::try_from(root).unwrap(), mode, profile.as_deref(), desc.as_deref(), execution_mode(execution_dir.as_deref()), output_clear_stage.as_deref().map(proj::OutputClearStage::parse).transpose().expect("clap validates clear stage")) {
                    fail(format_args!("Build failed: {:#}", e));
                } else {
                    println!("Build completed successfully.");
                }
            } else {
                fail(format_args!("Error finding project root"));
            }
        },
        Commands::BuildDev { profile, execution_dir, output_clear_stage } => {
            if let Some(root) = proj::root() {
                if let Err(e) = proj::build_project_with_policies(&camino::Utf8PathBuf::try_from(root).unwrap(), proj::BuildMode::Dev, profile.as_deref(), None, execution_mode(execution_dir.as_deref()), output_clear_stage.as_deref().map(proj::OutputClearStage::parse).transpose().expect("clap validates clear stage")) {
                    fail(format_args!("Build failed: {:#}", e));
                } else {
                    println!("Build completed successfully.");
                }
            } else {
                fail(format_args!("Error finding project root"));
            }
        },
        Commands::Ignore { command } => match command {
            IgnoreCommands::Add { paths, force_create, r#type } => {
                if let Some(root) = proj::root() {
                    if let Err(e) = proj::add_manual_ignores(&root, paths, *force_create, r#type.clone()) {
                        fail(format_args!("Error adding ignores: {}", e));
                    } else {
                        println!("Successfully added paths to ignores");
                    }
                } else {
                    fail(format_args!("Error finding project root"));
                }
            }
            IgnoreCommands::Remove { paths, r#type } => {
                if let Some(root) = proj::root() {
                    if let Err(e) = proj::remove_manual_ignores(&root, paths, r#type.clone()) {
                        fail(format_args!("Error removing ignores: {}", e));
                    } else {
                        println!("Successfully removed paths from ignores");
                    }
                } else {
                    fail(format_args!("Error finding project root"));
                }
            }
        },
        Commands::Init { command } => match command {
            Some(InitCommands::Version) => {
                if let Err(e) = proj::init_version() {
                    fail(format_args!("Initialization failed: {:#}", e));
                }
            }
            Some(InitCommands::Directories) => {
                if let Err(e) = proj::init_directories() {
                    fail(format_args!("Initialization failed: {:#}", e));
                }
            }
            Some(InitCommands::Readme { title, description, rmd }) => {
                let result = if *rmd { proj::init_readme_rmd(title.clone(), description.clone()) }
                    else { proj::init_readme(title.clone(), description.clone()) };
                if let Err(e) = result {
                    fail(format_args!("Initialization failed: {:#}", e));
                }
            }
            Some(InitCommands::Renv) => {
                if let Err(e) = proj::init_renv() {
                    fail(format_args!("R dependency initialization failed: {:#}", e));
                }
            }
            Some(InitCommands::License { license, first_name, last_name }) => {
                if let Err(e) = proj::init_license(license.clone(), first_name.clone(), last_name.clone()) {
                    fail(format_args!("Initialization failed: {:#}", e));
                }
            }
            Some(InitCommands::Git { commit }) => {
                if let Err(e) = proj::init_git(*commit) {
                    fail(format_args!("Initialization failed: {:#}", e));
                }
            }
            Some(InitCommands::Github { public }) => {
                if let Err(e) = proj::init_github(*public) {
                    fail(format_args!("Initialization failed: {:#}", e));
                }
            }
            None => {
                if let Err(e) = proj::init_full() {
                    fail(format_args!("Initialization failed: {:#}", e));
                }
            }
        },
        Commands::Version { command } => match command {
            VersionCommands::Get { no_v, format } => {
                if let Ok(version) = proj::run_version_from(&root) {
                    if format.to_lowercase() == "json" {
                        if let Ok(json) = serde_json::to_string(&version) {
                            println!("{}", json);
                        } else {
                            fail(format_args!("Failed to format version as JSON"));
                        }
                    } else {
                        println!("{}", version.to_string(!no_v));
                    }
                } else {
                    fail(format_args!("Failed to get version from VERSION file"));
                }
            }
            VersionCommands::Set { version } => {
                if let Err(e) = proj::version_set(version) {
                    fail(format_args!("Error setting version: {}", e));
                }
            }
        },
    }
}

/// Operations for initializing a project.
///
/// Sets up the project structure, versions, git, github, etc.
///
/// # Errors
///
/// May fail if an initialization step encounters an error (e.g., IO, permission).
/// ```rust
/// // use proj::InitCommands;
/// ```
#[derive(Subcommand)]
pub enum InitCommands {
    /// Initialize version
    Version,
    /// Initialize directories
    Directories,
    /// Initialize an empty renv project using already installed renv; never implicit.
    Renv,
    /// Initialize README (plain Markdown by default).
    Readme {
        /// Create README.Rmd instead, without replacing or rendering README.md.
        #[arg(long)]
        rmd: bool,
        /// Project Title
        #[arg(long)]
        title: Option<String>,
        /// Project Description
        #[arg(long)]
        description: Option<String>,
    },
    /// Initialize License
    License {
        /// License type (ccby, apache, cc0, proprietary)
        #[arg(long)]
        license: Option<String>,
        /// First name (for proprietary)
        #[arg(long)]
        first_name: Option<String>,
        /// Last name (for proprietary)
        #[arg(long)]
        last_name: Option<String>,
    },
    /// Initialize Git
    Git {
        /// Whether to commit initial changes
        #[arg(long)]
        commit: Option<bool>,
    },
    /// Initialize GitHub
    Github {
        /// Make repository public (default is private)
        #[arg(long)]
        public: Option<bool>,
    },
}


fn query_path(root: &camino::Utf8Path, label: &str, target: &str, parts: &[camino::Utf8PathBuf], format: Option<&str>, relative_to: Option<&str>) -> anyhow::Result<camino::Utf8PathBuf> {
    let format = match format {
        None => proj::run_context::default_path_format_from(root)?,
        Some("auto") => proj::PathFormat::Auto,
        Some("relative") => proj::PathFormat::Relative,
        Some("absolute") => proj::PathFormat::Absolute,
        _ => anyhow::bail!("Unsupported path representation"),
    };
    let base = relative_to.map(|value| match value {
        "cwd" => proj::RelativeBase::Caller,
        "project" => proj::RelativeBase::Project,
        "document" => proj::RelativeBase::Document,
        path => proj::RelativeBase::Directory(path.into()),
    });
    let path = proj::resource_path_from(root, label, parts, target == "safe")?;
    proj::present_resource_path(root, &path, format, base.as_ref())
}


fn execution_mode(value: Option<&str>) -> Option<proj::ExecutionDirectory> {
    value.map(|value| if value == "project" { proj::ExecutionDirectory::Project } else { proj::ExecutionDirectory::Native })
}

fn query_local_versions(root: &camino::Utf8Path, path: &camino::Utf8Path, label: &str, structure: &str, append_label: bool) -> anyhow::Result<Vec<proj::local_remote::LocalVersion>> {
    let base = if path.is_absolute() { path.to_owned() } else { root.join(path) };
    let structure = match structure {
        "latest" => proj::local_remote::LocalStructure::Latest,
        "archive" => proj::local_remote::LocalStructure::Archive,
        _ => anyhow::bail!("Unsupported ordinary local remote structure"),
    };
    proj::local_remote::list_versions(&base, label, structure, append_label)
}

fn download_local_version(root: &camino::Utf8Path, path: &camino::Utf8Path, label: &str, structure: &str, remote_version: Option<&str>, destination: &camino::Utf8Path, append_label: bool, clear: bool) -> anyhow::Result<proj::local_remote::LocalDownload> {
    let base = if path.is_absolute() { path.to_owned() } else { root.join(path) };
    let destination = if destination.is_absolute() { destination.to_owned() } else { root.join(destination) };
    let structure = match structure {
        "latest" => proj::local_remote::LocalStructure::Latest,
        "archive" => proj::local_remote::LocalStructure::Archive,
        _ => anyhow::bail!("Unsupported ordinary local remote structure"),
    };
    let selected = proj::local_remote::select_version(&base, label, structure, remote_version, append_label)?;
    proj::local_remote::download_version(root, &selected, &destination, clear)
}

fn local_store_files(root: &camino::Utf8Path, source: &camino::Utf8Path, destination: &camino::Utf8Path, strategy: &str, dry_run: bool) -> anyhow::Result<proj::local_store::FileStorePlan> {
    let source = if source.is_absolute() { source.to_owned() } else { root.join(source) };
    let destination = if destination.is_absolute() { destination.to_owned() } else { root.join(destination) };
    let strategy = match strategy {
        "upload-all" => proj::local_store::StoreStrategy::UploadAll,
        "upload-missing" => proj::local_store::StoreStrategy::UploadMissing,
        "sync-diff" => proj::local_store::StoreStrategy::SyncDiff,
        "sync-purge" => proj::local_store::StoreStrategy::SyncPurge,
        _ => anyhow::bail!("Unsupported file transfer strategy"),
    };
    proj::local_store::store_files(root, &source, &destination, strategy, dry_run)
}


fn github_assets(root: &camino::Utf8Path, command: &GithubCommands) -> anyhow::Result<serde_json::Value> {
    let repository = match command {
        GithubCommands::Assets { repository, .. } | GithubCommands::Upload { repository, .. }
            | GithubCommands::Download { repository, .. } | GithubCommands::UploadDirectory { repository, .. }
            | GithubCommands::UploadMetadata { repository, .. } | GithubCommands::DownloadDirectory { repository, .. } => repository,
    };
    let token = ["GITHUB_PAT", "GH_TOKEN", "GITHUB_TOKEN"].iter()
        .find_map(|name| std::env::var(name).ok().filter(|value| !value.trim().is_empty()));
    let api = std::env::var("GITHUB_API_URL").unwrap_or_else(|_| "https://api.github.com".into());
    let client = proj::github_release::GithubReleaseClient::new(repository, &api, token)?;
    let result = match command {
        GithubCommands::UploadDirectory { tag, source, label, structure, remote_version, prefix, path_append_label, create_release, replace, .. } => {
            let source=if source.is_absolute() {source.clone()} else {root.join(source)};
            let (zip,empty)=proj::github_zip::directory_zip(&source)?;
            let structure=if structure=="latest" {proj::github_zip::GithubStructure::Latest} else {proj::github_zip::GithubStructure::Archive};
            let name=proj::github_zip::directory_asset_name(prefix.as_deref(),*path_append_label,label,structure,remote_version.as_deref(),empty)?;
            let file=camino::Utf8Path::from_path(zip.path()).ok_or_else(||anyhow::anyhow!("ZIP path must be UTF-8"))?;
            serde_json::json!({"asset":upload_github_asset(&client,tag,&name,file,*create_release,*replace)?})
        },
        GithubCommands::UploadMetadata { tag,source,kind,create_release,replace,.. } => {
            let source=if source.is_absolute() {source.clone()} else {root.join(source)};
            let kind=match kind.as_str() {
                "manifest"=>proj::github_zip::MetadataKind::Manifest,
                "version"=>proj::github_zip::MetadataKind::Version,
                "changelog"=>proj::github_zip::MetadataKind::Changelog,
                _=>anyhow::bail!("Unsupported metadata kind"),
            };
            let zip=proj::github_zip::metadata_zip(&source,kind)?;
            let file=camino::Utf8Path::from_path(zip.path()).ok_or_else(||anyhow::anyhow!("ZIP path must be UTF-8"))?;
            serde_json::json!({"asset":upload_github_asset(&client,tag,kind.names().0,file,*create_release,*replace)?})
        },
        GithubCommands::DownloadDirectory { tag,name,destination,max_bytes,max_expanded_bytes,clear,empty,.. } => {
            let destination=if destination.is_absolute() {destination.clone()} else {root.join(destination)};
            serde_json::json!({"asset":proj::github_zip::download_directory(&client,root,tag,name,&destination,*max_bytes,*max_expanded_bytes,*clear,*empty)?})
        },
        GithubCommands::Assets { tag, .. } => serde_json::json!({"assets": client.assets(tag)?}),
        GithubCommands::Upload { tag, name, source, create_release, replace, .. } => {
            let source = if source.is_absolute() { source.clone() } else { root.join(source) };
            serde_json::json!({"asset": upload_github_asset(&client,tag,name,&source,*create_release,*replace)?})
        },
        GithubCommands::Download { tag, name, destination, max_bytes, .. } => {
            let destination = if destination.is_absolute() { destination.clone() } else { root.join(destination) };
            serde_json::json!({"asset": client.download_asset(root, tag, name, &destination, *max_bytes)?})
        },
    };
    Ok(result)
}

fn upload_github_asset(client:&proj::github_release::GithubReleaseClient,tag:&str,name:&str,source:&camino::Utf8Path,create_release:bool,replace:bool)->anyhow::Result<proj::github_release::ReleaseAsset> {
    if replace {client.replace_asset(tag,name,source,create_release)}
    else {client.upload_asset(tag,name,source,create_release)}
}

fn configured_restore(root:&camino::Utf8Path,command:&RemoteCommands)->anyhow::Result<proj::restore::RestoreResult> {
    let RemoteCommands::Restore {label,position,provider,title,remote_version,snapshot,repository,profile,clear,max_bytes,max_expanded_bytes}=command
        else {anyhow::bail!("Expected configured restore command");};
    if let Some(snapshot)=snapshot {
        let named=title.as_deref().ok_or_else(||anyhow::anyhow!("Snapshot restore requires --title"))?;
        if proj::configured_xet_github::selected(root,named,profile.as_deref())? {
            anyhow::ensure!(position.is_none()&&repository.is_none(),"GitHub Xet uses its trusted project locator, not position/repository overrides");
            anyhow::ensure!(provider.as_deref().is_none_or(|value|value=="github"),"Selected Xet snapshot uses GitHub transport");
            return proj::configured_xet_github::restore(root,named,snapshot,label,profile.as_deref(),remote_version.as_deref(),*clear,
                max_bytes.ok_or_else(||anyhow::anyhow!("GitHub Xet requires --max-bytes"))?,
                max_expanded_bytes.ok_or_else(||anyhow::anyhow!("GitHub Xet requires --max-expanded-bytes"))?);
        }
        anyhow::ensure!(position.is_none()&&repository.is_none(),"Explicit CAS snapshot restore does not use position or GitHub repository filters");
        anyhow::ensure!(provider.as_deref().is_none_or(|value|value=="local"),"Explicit CAS snapshots require a local source");
        anyhow::ensure!(max_bytes.is_none()&&max_expanded_bytes.is_none(),"CAS snapshots use bounded tree readers, not ordinary archive byte-limit flags");
        return proj::configured_cas::restore(root,title.as_deref().ok_or_else(||anyhow::anyhow!("CAS snapshot restore requires --title"))?,
            snapshot,label,profile.as_deref(),remote_version.as_deref(),*clear);
    }
    let request=proj::restore::RestoreRequest {label:label.clone(),position:position.clone(),provider:provider.clone(),
        title:title.clone(),remote_version:remote_version.clone(),repository:repository.clone(),profile:profile.clone(),
        clear:*clear,max_bytes:max_bytes.ok_or_else(||anyhow::anyhow!("Ordinary restore requires --max-bytes"))?,
        max_expanded_bytes:max_expanded_bytes.ok_or_else(||anyhow::anyhow!("Ordinary restore requires --max-expanded-bytes"))?};
    proj::restore::restore_label(root,&request)
}


fn configured_verify(root:&camino::Utf8Path,title:&str,snapshot:&str,profile:Option<&str>,
    max_bytes:Option<u64>,max_expanded:Option<u64>)->anyhow::Result<proj::configured_cas::SnapshotVerification> {
    if proj::configured_xet_github::selected(root,title,profile)? {
        let observation=proj::configured_xet_github::verify(root,title,snapshot,profile,
            max_bytes.ok_or_else(||anyhow::anyhow!("GitHub Xet requires --max-bytes"))?,
            max_expanded.ok_or_else(||anyhow::anyhow!("GitHub Xet requires --max-expanded-bytes"))?)?;
        Ok(proj::configured_cas::SnapshotVerification::Xet {snapshot_id:snapshot.into(),observation})
    }else{
        anyhow::ensure!(max_bytes.is_none()&&max_expanded.is_none(),"Local snapshots use bounded readers, not archive budgets");
        proj::configured_cas::verify(root,title,snapshot,profile)
    }
}
