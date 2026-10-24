pub mod check;
pub mod diff;
pub mod lock;
pub mod manage;
pub mod spec;
pub mod sync;

use std::path::PathBuf;

use anyhow::{Result, bail};

/// The workspace root: the trailing path of `check`, `lock`, `diff` and `sync`, or
/// `-C`, or the current directory. Saying it twice, differently, is an error.
pub fn root(global: &Option<PathBuf>, trailing: Option<PathBuf>) -> Result<PathBuf> {
    match (global, trailing) {
        (Some(a), Some(b)) if *a != b => {
            bail!(
                "the root is given twice, as -C {} and {}",
                a.display(),
                b.display()
            )
        }
        (_, Some(path)) => Ok(path),
        (Some(path), None) => Ok(path.clone()),
        (None, None) => Ok(PathBuf::from(".")),
    }
}
