//! `catasto-digest` -- SHA-256 content hashing for catasto.

use std::fs::File;
use std::io::Read;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

/// Kind of entry in a deterministic directory tree digest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeEntryKind {
    File { executable: bool },
    Link,
}

/// One leaf entry included in a deterministic directory tree digest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeDigestEntry {
    /// Relative path inside the directory, using `/` separators (e.g. `"a/b.txt"`).
    pub rel_path: String,
    pub kind: TreeEntryKind,
    /// Hex SHA-256 of the file bytes or symlink target bytes.
    pub sha256: String,
}

/// Computes a deterministic SHA-256 digest over a set of tree entries.
pub fn sha256_tree(entries: &[TreeDigestEntry]) -> String {
    let mut sorted: Vec<&TreeDigestEntry> = entries.iter().collect();
    sorted.sort_by(|a, b| a.rel_path.as_bytes().cmp(b.rel_path.as_bytes()));

    let mut hasher = Sha256::new();
    for entry in sorted {
        let tag = match entry.kind {
            TreeEntryKind::File { executable: true } => "exec",
            TreeEntryKind::File { executable: false } => "file",
            TreeEntryKind::Link => "link",
        };
        hasher.update(format!("{tag} {} {}\n", entry.sha256, entry.rel_path).as_bytes());
    }

    to_hex(hasher.finalize().as_slice())
}

/// Computes the deterministic SHA-256 tree digest of the directory at `path`.
pub fn sha256_dir(path: &Path) -> Result<String> {
    let mut entries = Vec::new();
    collect_dir_entries(path, "", &mut entries)?;
    Ok(sha256_tree(&entries))
}

fn collect_dir_entries(
    dir: &Path,
    prefix: &str,
    entries: &mut Vec<TreeDigestEntry>,
) -> Result<()> {
    let read_dir =
        std::fs::read_dir(dir).with_context(|| format!("cannot read directory {}", dir.display()))?;
    for item in read_dir {
        let item = item.with_context(|| format!("cannot read entry in {}", dir.display()))?;
        let entry_path = item.path();
        let file_name = item.file_name();
        let name = file_name
            .to_str()
            .with_context(|| format!("path is not valid UTF-8: {}", entry_path.display()))?;
        let rel_path = if prefix.is_empty() {
            name.to_owned()
        } else {
            format!("{prefix}/{name}")
        };

        let meta = std::fs::symlink_metadata(&entry_path)
            .with_context(|| format!("cannot stat {}", entry_path.display()))?;
        if meta.is_symlink() {
            entries.push(TreeDigestEntry {
                rel_path,
                kind: TreeEntryKind::Link,
                sha256: sha256_link(&entry_path)?,
            });
        } else if meta.is_dir() {
            collect_dir_entries(&entry_path, &rel_path, entries)?;
        } else if meta.is_file() {
            #[cfg(unix)]
            let executable = meta.permissions().mode() & 0o111 != 0;
            #[cfg(not(unix))]
            let executable = false;
            entries.push(TreeDigestEntry {
                rel_path,
                kind: TreeEntryKind::File { executable },
                sha256: sha256_file(&entry_path)?,
            });
        } else {
            bail!("unsupported file type at {}", entry_path.display());
        }
    }
    Ok(())
}

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

const HEX_CHARS: &[u8; 16] = b"0123456789abcdef";

fn to_hex(bytes: &[u8]) -> String {
    let mut hex = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        hex.push(char::from(HEX_CHARS[usize::from(byte >> 4)]));
        hex.push(char::from(HEX_CHARS[usize::from(byte & 0x0f)]));
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

    #[test]
    #[cfg(unix)]
    fn sha256_dir_matches_sha256_tree_for_nested_exec_and_symlink() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("sub/deep")).unwrap();
        std::fs::create_dir_all(dir.path().join("empty_dir")).unwrap();

        let regular = dir.path().join("a.txt");
        std::fs::write(&regular, "hello").unwrap();

        let script = dir.path().join("sub/deep/run.sh");
        std::fs::write(&script, "#!/bin/sh\necho hi\n").unwrap();
        let mut perms = std::fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script, perms).unwrap();

        let link = dir.path().join("sub/link");
        std::os::unix::fs::symlink("../a.txt", &link).unwrap();

        let disk_digest = sha256_dir(dir.path()).unwrap();

        // Supply entries out of order to verify deterministic sorting.
        let mem_entries = vec![
            TreeDigestEntry {
                rel_path: "sub/link".into(),
                kind: TreeEntryKind::Link,
                sha256: sha256_hex(b"../a.txt"),
            },
            TreeDigestEntry {
                rel_path: "sub/deep/run.sh".into(),
                kind: TreeEntryKind::File { executable: true },
                sha256: sha256_hex(b"#!/bin/sh\necho hi\n"),
            },
            TreeDigestEntry {
                rel_path: "a.txt".into(),
                kind: TreeEntryKind::File { executable: false },
                sha256: sha256_hex(b"hello"),
            },
        ];

        assert_eq!(disk_digest, sha256_tree(&mem_entries));

        // Flipping executable bit must change the digest.
        let non_exec = vec![
            mem_entries[0].clone(),
            TreeDigestEntry {
                rel_path: "sub/deep/run.sh".into(),
                kind: TreeEntryKind::File { executable: false },
                sha256: sha256_hex(b"#!/bin/sh\necho hi\n"),
            },
            mem_entries[2].clone(),
        ];
        assert_ne!(disk_digest, sha256_tree(&non_exec));
    }

    #[test]
    fn sha256_dir_missing_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(sha256_dir(&dir.path().join("missing")).is_err());
    }
}
