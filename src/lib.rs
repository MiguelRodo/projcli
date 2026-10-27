pub mod build;
pub mod build_pre;
pub mod cas;
pub mod cas_tree;
pub mod clear;
pub mod env;
pub mod fs_utils;
pub mod git;
pub mod ignore;
pub mod init;
pub mod profile;
mod r_housekeeping;
pub mod snapshot_format;
pub mod version;
pub mod yml;

pub use build::*;
pub use build_pre::*;
pub use cas::*;
pub use clear::*;
pub use env::*;
pub use fs_utils::*;
pub use git::*;
pub use ignore::*;
pub use init::*;
pub use profile::*;
pub use version::*;
pub use yml::*;

pub mod project;
pub use project::*;

pub(crate) mod path_safety;

pub(crate) mod build_state;

pub(crate) mod run_environment;

pub mod run_context;
pub use run_context::{RUN_CONTEXT_ENV, configured_path_from, run_version_from};

pub mod paths;
pub use paths::resource_path_from;

pub use paths::{PathFormat, RelativeBase, present_resource_path};

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

/// Optional pinned local Xet storage with independently verified reconstruction.
pub mod xet;
mod xet_engine;

mod xet_remote;

/// Closed-writer portable Xet store exports and verified private imports.
pub mod xet_archive;

/// Immutable complete-store Xet ZIP transfer over the existing GitHub client.
pub mod xet_github;

/// Configured experimental GitHub Xet build and explicit snapshot recovery.
pub mod configured_xet_github;

/// Portable verified complete CAS store exports/imports.
pub mod cas_archive;
mod store_archive;

/// Immutable complete CAS-store ZIPs over the existing GitHub transport.
pub mod cas_github;
mod store_github;

/// Configured complete-CAS-store GitHub publication and explicit recovery.
pub mod configured_cas_github;
