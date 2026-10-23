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
    /// Inspect ordinary local remote storage without publishing or restoring.
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

/// Read-only remote inspection; does not change the configured remote registry.
#[derive(Subcommand)]
pub enum RemoteCommands {
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
            Some(InitCommands::Readme { title, description }) => {
                if let Err(e) = proj::init_readme(title.clone(), description.clone()) {
                    fail(format_args!("Initialization failed: {:#}", e));
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
    /// Initialize README
    Readme {
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
