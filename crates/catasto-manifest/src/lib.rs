//! Manifest syntax, schema models, and AST/CST editing for catasto.

pub mod edit;
pub mod lock;
pub mod manifest;
pub mod spec;

pub use edit::{AddOutcome, AlreadyDeclared, SourceText, UpstreamText};
pub use lock::{LOCK_FILE, Lock, Locked};
pub use manifest::{Managed, Manifest, Source, Symlink};
pub use spec::{ManifestEntrySpec, UpstreamSpec, all_manifest_specs};
