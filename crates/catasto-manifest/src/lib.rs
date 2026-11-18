//! Manifest syntax, schema models, and AST/CST editing for catasto.

pub mod edit;
pub mod lock;
pub mod manifest;
pub mod spec;

pub use edit::{AddOutcome, AlreadyDeclared, DeclaredKind, SourceText, UpstreamText};
pub use lock::{LOCK_FILE, Lock, LockMode, Locked, LockedRepo};
pub use manifest::{Managed, ManagedKind, Manifest, Repo, Source, Symlink};
pub use spec::{ManifestEntrySpec, UpstreamSpec, all_manifest_specs};
