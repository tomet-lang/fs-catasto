//! Manifest syntax, schema models, and AST/CST editing for catasto.

pub mod edit;
pub mod lock;
pub mod manifest;
pub mod spec;

pub use edit::{AddOutcome, AlreadyDeclared, DeclaredKind, SourceText, UpstreamText};
pub use lock::{LOCK_FILE, Lock, LockMode, Locked, LockedSubmodule};
pub use manifest::{
    EntryRules, Managed, ManagedKind, Manifest, Source, Submodule, Symlink, UpstreamRef, Zone,
    ZonePattern,
};
pub use spec::{ManifestEntrySpec, UpstreamSpec, ZoneSpec, all_manifest_specs, blueprint_spec};
