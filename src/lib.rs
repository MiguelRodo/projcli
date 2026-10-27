pub mod fs_utils;
pub mod init;
mod r_housekeeping;
pub mod version;
pub mod yml;
pub mod profile;
pub mod ignore;
pub mod build_pre;
pub mod build;
pub mod git;
pub mod clear;
pub mod cas;
pub mod snapshot_format;
pub mod cas_tree;
pub mod env;

pub use version::*;
pub use yml::*;
pub use profile::*;
pub use ignore::*;
pub use build_pre::*;
pub use build::*;
pub use git::*;
pub use clear::*;
pub use cas::*;
pub use init::*;
pub use env::*;
pub use fs_utils::*;

pub mod project;
pub use project::*;

pub(crate) mod path_safety;

pub(crate) mod build_state;

pub(crate) mod run_environment;

pub mod run_context;
pub use run_context::{configured_path_from, run_version_from, RUN_CONTEXT_ENV};

pub mod paths;
pub use paths::resource_path_from;

pub use paths::{present_resource_path, PathFormat, RelativeBase};

mod embedding;

pub mod manifest;
pub mod manifest_csv;

pub mod local_remote;

pub mod local_store;

pub mod github_release;

pub mod github_zip;

pub mod restore;

/// Configured ordinary publication, separate from CAS storage.
pub mod publication;

/// Configured ordinary GitHub Release publication through guarded ZIP staging.
pub mod github_publication;

mod attempt;

mod attempt_log;

mod provenance;

/// Read-only attempt history and exact final CSV observations.
pub mod history;

mod config_evidence;

pub mod catalogue;

pub mod remote_registry;

mod cas_remote;

/// Explicit named local CAS verification and guarded snapshot recovery.
pub mod configured_cas;

/// Flat CAS catalogue publication and explicit verified recovery.
pub mod cas_flat;

pub(crate) mod ordinary_manifest;

pub(crate) mod github_manifest;

pub(crate) mod ordinary_tree;

mod r_session;

mod xet_engine;
/// Optional pinned local Xet storage with independently verified reconstruction.
pub mod xet;
