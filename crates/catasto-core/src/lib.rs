//! `catasto-core` -- core engine and models for filesystem schemas, manifests, and lockfiles.
//!
//! Just as a cadastre (*catasto*) records the boundaries and parcels of land,
//! `catasto` defines and verifies directory rules, naming conventions, media
//! manifests, and content lockfiles across a workspace.
//!
//! A workspace declares the files it manages in `catasto.ctmt`. [`lock`] records
//! their SHA-256 in `catasto.lock`, [`check`] reports drift, and [`sync`]
//! re-fetches the files from their sources: local paths, or files in git
//! repositories pinned to a commit.

mod check;
mod lockfile;
mod manifest;
mod sync;

pub use check::{CheckReport, Violation, ViolationKind, check};
pub use lockfile::{LOCK_FILE, Lock, Locked, lock};
pub use manifest::{MANIFEST_FILE, Managed, Manifest, Source};
pub use sync::{SyncMode, sync, sync_with};
