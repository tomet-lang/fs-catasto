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
mod manage;
mod sync;

pub use catasto_manifest::edit;
pub use catasto_manifest::lock;
pub use catasto_manifest::manifest;

pub use check::{CheckReport, Violation, ViolationKind, check};
pub use edit::{SourceText, UpstreamText};
pub use lockfile::{LOCK_FILE, Lock, Locked, lock};
pub use manage::{Row, add, init, list, remove, upstream_add, upstream_list, upstream_remove};
pub use manifest::{MANIFEST_FILE, Managed, Manifest, Source, UpstreamInfo, upstreams_of};
pub use sync::{SyncMode, sync, sync_only, sync_with};
