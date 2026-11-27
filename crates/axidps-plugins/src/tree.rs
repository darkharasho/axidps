//! Tree hash of a plugin folder: SHA-256 over its files sorted by `/`-separated relative path,
//! each fed as `path NUL size NUL bytes` (`size` in decimal ASCII). Same bytes on every OS.


use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Tree hash of in-memory files. Paths are already `/`-separated and relative; `\` is an
/// ordinary byte here (unpacking refuses such names before they get this far).
pub fn hash_files(files: &[(String, Vec<u8>)]) -> [u8; 32] {
    let mut sorted: Vec<&(String, Vec<u8>)> = files.iter().collect();
    sorted.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    let mut h = Sha256::new();
    for (path, bytes) in sorted {
        feed(&mut h, path, bytes);
    }
    h.finalize().into()
}

/// Tree hash of a folder on disk, walked recursively. Refuses symlinks inside it, anything
/// that isn't a plain file or folder, and names that aren't UTF-8. Empty folders don't count.
pub fn hash_dir(dir: &Path) -> Result<[u8; 32], String> {
    let mut files = Vec::new();
    collect(dir, "", &mut files)?;
    files.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    let mut h = Sha256::new();
    for (rel, path) in files {
        let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        feed(&mut h, &rel, &bytes);
    }
    Ok(h.finalize().into())
}

/// Lowercase hex of a hash.
pub fn hex(h: &[u8; 32]) -> String {
    h.iter().map(|b| format!("{b:02x}")).collect()
}

fn feed(h: &mut Sha256, path: &str, bytes: &[u8]) {
    h.update(path.as_bytes());
    h.update([0]);
    h.update(bytes.len().to_string().as_bytes());
    h.update([0]);
    h.update(bytes);
}

fn collect(dir: &Path, prefix: &str, out: &mut Vec<(String, PathBuf)>) -> Result<(), String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("{}: {e}", dir.display()))?;
        let path = entry.path();
        let name = entry
            .file_name()
            .into_string()
            .map_err(|n| format!("{}: name is not UTF-8: {n:?}", dir.display()))?;
        let rel = format!("{prefix}{name}");
        // `file_type` doesn't follow symlinks.
        let kind = entry.file_type().map_err(|e| format!("{}: {e}", path.display()))?;
        if kind.is_symlink() {
            return Err(format!("{rel}: symlinks aren't allowed"));
        } else if kind.is_dir() {
            collect(&path, &format!("{rel}/"), out)?;
        } else if kind.is_file() {
            out.push((rel, path));
        } else {
            return Err(format!("{rel}: not a plain file"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    use serde_json::Value;

    fn vectors() -> Value {
        serde_json::from_str(include_str!("../tests/vectors.json")).unwrap()
    }

    #[test]
    fn tree_vectors() {
        let v = vectors();
        let cases = v["tree"].as_array().unwrap();
        assert!(cases.len() >= 4);
        for case in cases {
            let files: Vec<(String, Vec<u8>)> = case["files"]
                .as_array()
                .unwrap()
                .iter()
                .map(|f| {
                    let path = f[0].as_str().unwrap().to_string();
                    (path, STANDARD.decode(f[1].as_str().unwrap()).unwrap())
                })
                .collect();
            assert_eq!(hex(&hash_files(&files)), case["hash"].as_str().unwrap(), "{files:?}");
        }
    }

    #[test]
    fn order_is_by_path_bytes_not_input_order() {
        let a = vec![("a.js".to_string(), b"1".to_vec()), ("a/b.js".to_string(), b"2".to_vec())];
        let b = vec![a[1].clone(), a[0].clone()];
        assert_eq!(hash_files(&a), hash_files(&b));
        // Content and path both count.
        let c = vec![("a.js".to_string(), b"2".to_vec()), ("a/b.js".to_string(), b"1".to_vec())];
        assert_ne!(hash_files(&a), hash_files(&c));
    }

    #[test]
    fn hex_is_lowercase_64_chars() {
        let h = hash_files(&[]);
        assert_eq!(hex(&h), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    }

    #[test]
    fn hash_dir_equals_hash_files() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("a/b")).unwrap();
        std::fs::create_dir_all(d.path().join("emptydir")).unwrap();
        std::fs::write(d.path().join("a.js"), b"a();\n").unwrap();
        std::fs::write(d.path().join("a/b.js"), b"b\r\n").unwrap();
        std::fs::write(d.path().join("a/b/c.css"), b"").unwrap();
        let files = vec![
            ("a/b/c.css".to_string(), b"".to_vec()),
            ("a.js".to_string(), b"a();\n".to_vec()),
            ("a/b.js".to_string(), b"b\r\n".to_vec()),
        ];
        assert_eq!(hash_dir(d.path()).unwrap(), hash_files(&files));
    }

    #[test]
    fn hash_dir_refuses_a_missing_dir() {
        let d = tempfile::tempdir().unwrap();
        assert!(hash_dir(&d.path().join("nope")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn hash_dir_refuses_symlinks() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("real.js"), b"x").unwrap();
        std::os::unix::fs::symlink(d.path().join("real.js"), d.path().join("link.js")).unwrap();
        let e = hash_dir(d.path()).unwrap_err();
        assert!(e.contains("link.js"), "{e}");

        let d2 = tempfile::tempdir().unwrap();
        std::fs::create_dir(d2.path().join("sub")).unwrap();
        std::os::unix::fs::symlink(d.path(), d2.path().join("sub/dir")).unwrap();
        assert!(hash_dir(d2.path()).is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn hash_dir_refuses_non_utf8_names() {
        use std::os::unix::ffi::OsStrExt;
        let d = tempfile::tempdir().unwrap();
        let name = std::ffi::OsStr::from_bytes(b"bad\xff.js");
        std::fs::write(d.path().join(name), b"x").unwrap();
        assert!(hash_dir(d.path()).is_err());
    }
}
