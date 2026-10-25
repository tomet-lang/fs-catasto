//! `catasto-digest` -- SHA-256 content hashing for catasto.

use std::fmt::Write as _;
use std::fs::File;
use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

/// Returns the lowercase hex SHA-256 of the file at `path`.
pub fn sha256_file(path: &Path) -> Result<String> {
    let mut file = File::open(path).with_context(|| format!("cannot open {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file
            .read(&mut buf)
            .with_context(|| format!("cannot read {}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }

    Ok(to_hex(hasher.finalize().as_slice()))
}

/// Returns the lowercase hex SHA-256 of the symlink target text at `path`.
pub fn sha256_link(path: &Path) -> Result<String> {
    let target =
        std::fs::read_link(path).with_context(|| format!("cannot read link {}", path.display()))?;
    let text = target
        .to_str()
        .with_context(|| format!("symlink target is not valid UTF-8: {}", target.display()))?;
    Ok(sha256_hex(text.as_bytes()))
}

/// Returns the lowercase hex SHA-256 of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    to_hex(Sha256::digest(bytes).as_slice())
}

fn to_hex(bytes: &[u8]) -> String {
    let mut hex = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(hex, "{byte:02x}").expect("writing to a String cannot fail");
    }
    hex
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_known_content() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("abc");
        std::fs::write(&file, "abc").unwrap();

        assert_eq!(
            sha256_file(&file).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn hashes_bytes_like_files() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn missing_file_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(sha256_file(&dir.path().join("nope")).is_err());
    }

    #[test]
    #[cfg(unix)]
    fn hashes_symlink_target_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink("abc", &link).unwrap();

        assert_eq!(
            sha256_link(&link).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn not_a_symlink_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("file");
        std::fs::write(&file, "abc").unwrap();
        assert!(sha256_link(&file).is_err());
    }
}
