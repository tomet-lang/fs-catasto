//! Manifest syntax, schema models, and AST/CST editing for catasto.

pub mod edit;
pub mod lock;
pub mod manifest;

pub use edit::{AddOutcome, AlreadyDeclared, SourceText, UpstreamText};
pub use lock::{LOCK_FILE, Lock, Locked};
pub use manifest::{Managed, Manifest, Source, Symlink};
