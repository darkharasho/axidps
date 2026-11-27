//! Store installs: check a downloaded package against the index, unpack it beside the plugins
//! folder, and swap it in whole. Only folders the store installed (`installed.json`) are ever
//! replaced or removed; a player's own folder with the same id is left alone.

use crate::MANIFEST_FILE;
use crate::index::Version;
use crate::manifest::{self, Manifest, valid_id};
use crate::tree;
use crate::trust::Trust;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};

/// The most bytes a store package may unpack to.
pub const MAX_UNPACKED: u64 = 5 * 1024 * 1024;
/// The most files a store package may hold.
pub const MAX_FILES: usize = 500;
/// The record's file name inside the store folder.
pub const RECORD_FILE: &str = "installed.json";
/// Why an install refuses to touch a folder the player made.
pub const IN_THE_WAY: &str = "a plugin you added yourself is in the way";

/// What the store installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    pub version: String,
    pub tree: String,
}

/// `<store>/installed.json`: `{ "<id>": {"version", "tree"} }`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Record {
    pub plugins: BTreeMap<String, Installed>,
}

impl Record {
    /// The record in `store_dir`; missing or unreadable means nothing is a store install
    /// (fails safe: nothing gets replaced or removed).
    pub fn load(store_dir: &Path) -> Record {
        let mut r = Record::default();
        let Ok(bytes) = fs::read(store_dir.join(RECORD_FILE)) else { return r };
        let Ok(Value::Object(o)) = serde_json::from_slice::<Value>(&bytes) else { return r };
        for (id, v) in o {
            let s = |f: &str| v.get(f).and_then(Value::as_str).map(str::to_owned);
            if let (Some(version), Some(tree)) = (s("version"), s("tree")) {
                r.plugins.insert(id, Installed { version, tree });
            }
        }
        r
    }

    /// Writes the record atomically (a temp file renamed over the old one).
    pub fn save(&self, store_dir: &Path) -> Result<(), String> {
        let err = |e: std::io::Error| format!("{}: {e}", store_dir.display());
        fs::create_dir_all(store_dir).map_err(err)?;
        let mut o = Map::new();
        for (id, i) in &self.plugins {
            o.insert(id.clone(), json!({"version": i.version, "tree": i.tree}));
        }
        let bytes = serde_json::to_vec_pretty(&Value::Object(o)).map_err(|e| e.to_string())?;
        let tmp = store_dir.join(format!(".{RECORD_FILE}.tmp-{}", std::process::id()));
        let write = || -> std::io::Result<()> {
            let mut f = fs::File::create(&tmp)?;
            f.write_all(&bytes)?;
            f.sync_all()?;
            fs::rename(&tmp, store_dir.join(RECORD_FILE))
        };
        write().map_err(|e| {
            let _ = fs::remove_file(&tmp);
            err(e)
        })
    }

    pub fn contains(&self, id: &str) -> bool {
        self.plugins.contains_key(id)
    }
}

/// Lowercase hex SHA-256 of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    tree::hex(&Sha256::digest(bytes).into())
}

/// Installs package `zip` as version `v` of plugin `id` into `<plugins_dir>/<id>`, replacing
/// an earlier store install. Checks the zip's SHA-256 and the unpacked tree hash against the
/// index, unpacks into a fresh scratch folder beside it, and swaps it in. On any failure
/// nothing new is left in `plugins_dir`.
pub fn install(
    plugins_dir: &Path,
    store_dir: &Path,
    id: &str,
    v: &Version,
    zip: &[u8],
) -> Result<(), String> {
    if !valid_id(id) || id.starts_with("axidps.") {
        return Err(format!("\"{id}\" is not a store plugin id"));
    }
    if sha256_hex(zip) != v.sha256 {
        return Err("the download doesn't match the index (SHA-256 differs)".into());
    }
    let mut record = Record::load(store_dir);
    let target = plugins_dir.join(id);
    let in_the_way = |r: &Record| target.symlink_metadata().is_ok() && !r.contains(id);
    if in_the_way(&record) {
        return Err(IN_THE_WAY.into());
    }
    fs::create_dir_all(plugins_dir).map_err(|e| format!("{}: {e}", plugins_dir.display()))?;

    let pid = std::process::id();
    let scratch = plugins_dir.join(format!(".{id}.new-{pid}"));
    remove(&scratch);
    fs::create_dir(&scratch).map_err(|e| format!("{}: {e}", scratch.display()))?;
    if let Err(e) = unpack(zip, &scratch).and_then(|()| check(&scratch, id, v)) {
        remove(&scratch);
        return Err(e);
    }

    // Read the record again just before the swap: the folder may have appeared meanwhile.
    record = Record::load(store_dir);
    if in_the_way(&record) {
        remove(&scratch);
        return Err(IN_THE_WAY.into());
    }
    let old = plugins_dir.join(format!(".{id}.old-{pid}"));
    remove(&old);
    let had_old = target.symlink_metadata().is_ok();
    if had_old && let Err(e) = fs::rename(&target, &old) {
        remove(&scratch);
        return Err(format!("{}: {e}", target.display()));
    }
    if let Err(e) = fs::rename(&scratch, &target) {
        if had_old {
            let _ = fs::rename(&old, &target);
        }
        remove(&scratch);
        return Err(format!("{}: {e}", target.display()));
    }
    remove(&old);
    record.plugins.insert(id.into(), Installed { version: v.version.clone(), tree: v.tree.clone() });
    record.save(store_dir)
}

/// Removes store install `id`. A folder the store didn't install is never touched.
pub fn uninstall(plugins_dir: &Path, store_dir: &Path, id: &str) -> Result<(), String> {
    let mut record = Record::load(store_dir);
    if !valid_id(id) || !record.contains(id) {
        return Err(format!("{id} wasn't installed by the store"));
    }
    let target = plugins_dir.join(id);
    if target.symlink_metadata().is_ok() {
        // Out of the way first, so a half-deleted folder never loads as the plugin.
        let old = plugins_dir.join(format!(".{id}.old-{}", std::process::id()));
        remove(&old);
        fs::rename(&target, &old).map_err(|e| format!("{}: {e}", target.display()))?;
        remove(&old);
    }
    record.plugins.remove(id);
    record.save(store_dir)
}

/// An update may install without asking only if it asks for nothing new and keeps the same
/// trust level.
pub fn auto_update_ok(installed: &Manifest, old_level: Trust, new: &Version, new_level: Trust) -> bool {
    old_level == new_level
        && new.permissions.iter().all(|p| installed.permissions.contains(p))
        && new.network.iter().all(|n| installed.network.contains(n))
}

/// Removes the `.<id>.new-<pid>` and `.<id>.old-<pid>` folders an interrupted install or
/// uninstall left in `plugins_dir` (links are removed, never followed). Nothing else is
/// touched. Returns one log line per leftover.
pub fn sweep(plugins_dir: &Path) -> Vec<String> {
    let mut log = Vec::new();
    let Ok(rd) = fs::read_dir(plugins_dir) else { return log };
    for e in rd.filter_map(Result::ok) {
        let Ok(name) = e.file_name().into_string() else { continue };
        if !is_leftover(&name) {
            continue;
        }
        let path = e.path();
        remove(&path);
        log.push(match path.symlink_metadata() {
            Err(_) => crate::logline::clean(&format!("removed leftover install folder {name}")),
            Ok(_) => crate::logline::clean(&format!("couldn't remove leftover install folder {name}")),
        });
    }
    log
}

/// `.<id>.new-<digits>` or `.<id>.old-<digits>` with a valid id.
fn is_leftover(name: &str) -> bool {
    let Some(rest) = name.strip_prefix('.') else { return false };
    [".new-", ".old-"].iter().any(|tag| {
        rest.rsplit_once(tag).is_some_and(|(id, pid)| {
            valid_id(id) && !pid.is_empty() && pid.bytes().all(|b| b.is_ascii_digit())
        })
    })
}

/// Deletes a file, link or folder (a link itself, never what it points at); missing is fine.
fn remove(p: &Path) {
    match p.symlink_metadata() {
        Ok(m) if m.is_dir() => {
            let _ = fs::remove_dir_all(p);
        }
        Ok(_) => {
            let _ = fs::remove_file(p);
        }
        Err(_) => {}
    }
}

/// The manifest at the root names this id and version, and the files hash to the index's tree.
fn check(dir: &Path, id: &str, v: &Version) -> Result<(), String> {
    let bytes = fs::read(dir.join(MANIFEST_FILE))
        .map_err(|_| format!("the package has no {MANIFEST_FILE} at its root"))?;
    let m = manifest::parse(&bytes, dir)?;
    if m.id != id || m.version != v.version {
        return Err(format!(
            "the package is {} {}, the index says {id} {}",
            m.id, m.version, v.version
        ));
    }
    // The listing's claims are what auto-update decides on, so they must be the manifest's.
    let same = |a: &[String], b: &[String]| {
        a.iter().collect::<BTreeSet<_>>() == b.iter().collect::<BTreeSet<_>>()
    };
    if !same(&m.permissions, &v.permissions) || !same(&m.network, &v.network) || m.api != v.api {
        return Err(
            "the package's manifest asks for different permissions, network hosts or api \
             than the index lists"
                .into(),
        );
    }
    let tree = tree::hex(&tree::hash_dir(dir)?);
    if tree != v.tree {
        return Err("the unpacked files don't match the index (tree hash differs)".into());
    }
    Ok(())
}

/// An entry name as `/`-separated components, or why it's refused. Names are taken raw (no
/// decoding, no cleanup): anything that could mean a different place on some OS is refused.
fn components(raw: &[u8]) -> Result<(Vec<&str>, bool), String> {
    let name = std::str::from_utf8(raw).map_err(|_| "an entry name isn't UTF-8".to_string())?;
    let shown = crate::logline::clean(name);
    let bad = |why: &str| Err(format!("entry \"{shown}\": {why}"));
    if name.chars().any(char::is_control) {
        return bad("control characters in the name");
    }
    if name.contains('\\') {
        return bad("\\ in the name");
    }
    if name.contains(':') {
        return bad("a drive letter or : in the name");
    }
    if name.starts_with('/') {
        return bad("an absolute path");
    }
    let (body, is_dir) = match name.strip_suffix('/') {
        Some(b) => (b, true),
        None => (name, false),
    };
    let parts: Vec<&str> = body.split('/').collect();
    for p in &parts {
        match *p {
            "" => return bad("an empty path component"),
            "." | ".." => return bad("a . or .. component"),
            _ => {}
        }
    }
    Ok((parts, is_dir))
}

/// Unpacks `zip` into the empty folder `dest`, refusing hostile entries before writing them.
fn unpack(zip: &[u8], dest: &Path) -> Result<(), String> {
    let mut ar = zip::ZipArchive::new(Cursor::new(zip)).map_err(|e| format!("not a zip: {e}"))?;
    if ar.len() > MAX_FILES * 2 {
        return Err(format!("more than {MAX_FILES} files"));
    }
    // Case-folded path -> (exact path, is a folder): catches duplicates that would land on
    // the same file on a case-insensitive disk, and a file clashing with a folder.
    let mut seen: HashMap<String, (String, bool)> = HashMap::new();
    let mut claim = |path: &str, dir: bool| -> Result<(), String> {
        let key = path.to_lowercase();
        match seen.get(&key) {
            Some((p, d)) if dir && *d && p == path => Ok(()),
            Some(_) => Err(format!("entry \"{}\": a duplicate path", crate::logline::clean(path))),
            None => {
                seen.insert(key, (path.to_owned(), dir));
                Ok(())
            }
        }
    };
    let mut files = 0usize;
    let mut total = 0u64;
    for i in 0..ar.len() {
        let mut f = ar.by_index(i).map_err(|e| format!("entry {i}: {e}"))?;
        let (parts, is_dir) = components(f.name_raw())?;
        let path = parts.join("/");
        // Only plain files and folders: no links, devices, pipes or sockets.
        if let Some(mode) = f.unix_mode() {
            let kind = mode & 0o170000;
            let ok = kind == 0 || kind == if is_dir { 0o040000 } else { 0o100000 };
            if !ok || f.is_symlink() {
                return Err(format!("entry \"{}\": not a plain file", crate::logline::clean(&path)));
            }
        }
        for n in 1..parts.len() {
            claim(&parts[..n].join("/"), true)?;
        }
        claim(&path, is_dir)?;
        let out: PathBuf = parts.iter().fold(dest.to_path_buf(), |p, c| p.join(c));
        let io = |e: std::io::Error| format!("{}: {e}", out.display());
        if is_dir {
            fs::create_dir_all(&out).map_err(io)?;
            continue;
        }
        files += 1;
        if files > MAX_FILES {
            return Err(format!("more than {MAX_FILES} files"));
        }
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent).map_err(io)?;
        }
        let mut w = fs::OpenOptions::new().write(true).create_new(true).open(&out).map_err(io)?;
        // Count what actually comes out, not what the header claims.
        let left = MAX_UNPACKED - total;
        let n = std::io::copy(&mut (&mut f).take(left + 1), &mut w).map_err(io)?;
        total += n;
        if total > MAX_UNPACKED {
            return Err(format!("unpacks to more than {MAX_UNPACKED} bytes"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use zip::write::SimpleFileOptions;

    const MANIFEST: &str = r#"{"id": "someone.card", "name": "Card", "version": "VER",
        "api": 1, "permissions": ["fight"], "network": ["api.example.com"],
        "windows": [{"id": "card", "title": "Card", "entry": "card.html"}]}"#;

    fn manifest(version: &str) -> Vec<u8> {
        MANIFEST.replace("VER", version).into_bytes()
    }

    fn good_files(version: &str) -> Vec<(String, Vec<u8>)> {
        vec![
            (MANIFEST_FILE.into(), manifest(version)),
            ("card.html".into(), format!("<p>{version}</p>").into_bytes()),
            ("js/app.js".into(), b"console.log(1)\r\n".to_vec()),
        ]
    }

    fn zip_of(files: &[(String, Vec<u8>)]) -> Vec<u8> {
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (name, bytes) in files {
            if let Some(d) = name.strip_suffix('/') {
                w.add_directory(d, opts).unwrap();
            } else {
                w.start_file(name.as_str(), opts).unwrap();
                w.write_all(bytes).unwrap();
            }
        }
        w.finish().unwrap().into_inner()
    }

    fn version_of(version: &str, zip: &[u8], files: &[(String, Vec<u8>)]) -> Version {
        Version {
            version: version.into(),
            url: "https://example.com/x.zip".into(),
            sha256: sha256_hex(zip),
            tree: tree::hex(&tree::hash_files(files)),
            permissions: vec!["fight".into()],
            network: vec!["api.example.com".into()],
            api: 1,
        }
    }

    struct Dirs {
        _t: tempfile::TempDir,
        plugins: PathBuf,
        store: PathBuf,
    }

    fn dirs() -> Dirs {
        let t = tempfile::tempdir().unwrap();
        let plugins = t.path().join("plugins");
        let store = t.path().join("store");
        fs::create_dir_all(plugins.join("mine")).unwrap();
        fs::write(plugins.join("mine/a.txt"), "mine").unwrap();
        Dirs { plugins, store, _t: t }
    }

    fn listing(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> =
            fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        v.sort();
        v
    }

    #[test]
    fn install_round_trip() {
        let d = dirs();
        let files = good_files("1.0.0");
        let zip = zip_of(&files);
        let v = version_of("1.0.0", &zip, &files);
        install(&d.plugins, &d.store, "someone.card", &v, &zip).unwrap();
        let dir = d.plugins.join("someone.card");
        assert_eq!(tree::hex(&tree::hash_dir(&dir).unwrap()), v.tree);
        assert_eq!(listing(&d.plugins), ["mine", "someone.card"]);
        let r = Record::load(&d.store);
        assert_eq!(r.plugins["someone.card"], Installed { version: "1.0.0".into(), tree: v.tree.clone() });
        let on_disk: Value = serde_json::from_slice(&fs::read(d.store.join(RECORD_FILE)).unwrap()).unwrap();
        assert_eq!(on_disk["someone.card"]["version"], "1.0.0");

        // A newer version replaces it whole (the old version's extra file goes too).
        fs::write(dir.join("stale.txt"), "x").unwrap();
        let mut files2 = good_files("1.1.0");
        files2.retain(|(n, _)| n != "js/app.js");
        let zip2 = zip_of(&files2);
        let v2 = version_of("1.1.0", &zip2, &files2);
        install(&d.plugins, &d.store, "someone.card", &v2, &zip2).unwrap();
        assert_eq!(tree::hex(&tree::hash_dir(&dir).unwrap()), v2.tree);
        assert!(!dir.join("js").exists() && !dir.join("stale.txt").exists());
        assert_eq!(listing(&d.plugins), ["mine", "someone.card"]);
        assert_eq!(Record::load(&d.store).plugins["someone.card"].version, "1.1.0");
    }

    /// A zip with the good files plus `extra` entries, written raw (names unchecked).
    fn hostile(extra: &[(&str, &[u8])]) -> Vec<u8> {
        let mut files = good_files("1.0.0");
        files.extend(extra.iter().map(|(n, b)| (n.to_string(), b.to_vec())));
        zip_of(&files)
    }

    fn with_symlink() -> Vec<u8> {
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts = SimpleFileOptions::default();
        for (name, bytes) in good_files("1.0.0") {
            w.start_file(name, opts).unwrap();
            w.write_all(&bytes).unwrap();
        }
        w.add_symlink("link", "/etc/passwd", opts).unwrap();
        w.finish().unwrap().into_inner()
    }

    /// A deflated run of zeros bigger than MAX_UNPACKED, its headers claiming `claimed` bytes.
    fn bomb(claimed: Option<u32>) -> Vec<u8> {
        let mut zip = hostile(&[("zeros.bin", &vec![0u8; MAX_UNPACKED as usize + 1024])]);
        if let Some(size) = claimed {
            let name = b"zeros.bin";
            // Local header: sig 50 4b 03 04, uncompressed size at +22, name at +30.
            // Central header: sig 50 4b 01 02, uncompressed size at +24, name at +46.
            for (sig, size_at, name_at) in [([0x50, 0x4b, 3, 4], 22, 30), ([0x50, 0x4b, 1, 2], 24, 46)] {
                let at = (0..zip.len() - name_at - name.len())
                    .find(|&i| zip[i..i + 4] == sig && &zip[i + name_at..i + name_at + name.len()] == name)
                    .unwrap();
                zip[at + size_at..at + size_at + 4].copy_from_slice(&size.to_le_bytes());
            }
        }
        zip
    }

    #[test]
    fn hostile_zips_leave_nothing() {
        let many: Vec<(String, Vec<u8>)> =
            (0..MAX_FILES).map(|i| (format!("f/{i}.txt"), vec![b'x'])).collect();
        let mut too_many = good_files("1.0.0");
        too_many.extend(many);
        let cases: Vec<(&str, Vec<u8>)> = vec![
            ("dotdot", hostile(&[("../x", b"x")])),
            ("inner dotdot", hostile(&[("js/../../x", b"x")])),
            ("absolute", hostile(&[("/tmp/x", b"x")])),
            ("drive letter", hostile(&[("C:/x", b"x")])),
            ("backslash", hostile(&[("js\\x.js", b"x")])),
            ("nul", hostile(&[("a\0b", b"x")])),
            ("empty component", hostile(&[("js//x.js", b"x")])),
            ("dot component", hostile(&[("./x.js", b"x")])),
            ("case duplicate", hostile(&[("Card.html", b"x")])),
            ("case duplicate folder", hostile(&[("JS/b.js", b"x")])),
            ("file over folder", hostile(&[("js", b"x")])),
            ("symlink", with_symlink()),
            ("bomb", bomb(None)),
            ("lying bomb", bomb(Some(10))),
            ("too many files", zip_of(&too_many)),
            ("not a zip", b"PK nope".to_vec()),
            ("no manifest", zip_of(&good_files("1.0.0")[1..])),
        ];
        for (what, zip) in cases {
            for preinstalled in [false, true] {
                let d = dirs();
                let mut before_tree = None;
                if preinstalled {
                    let files = good_files("0.9.0");
                    let z = zip_of(&files);
                    install(&d.plugins, &d.store, "someone.card", &version_of("0.9.0", &z, &files), &z).unwrap();
                    before_tree = Some(tree::hash_dir(&d.plugins.join("someone.card")).unwrap());
                }
                let before = listing(&d.plugins);
                let mut v = version_of("1.0.0", &zip, &good_files("1.0.0"));
                v.sha256 = sha256_hex(&zip);
                let r = install(&d.plugins, &d.store, "someone.card", &v, &zip);
                assert!(r.is_err(), "{what}: installed");
                assert_eq!(listing(&d.plugins), before, "{what}: left something");
                if let Some(t) = before_tree {
                    assert_eq!(tree::hash_dir(&d.plugins.join("someone.card")).unwrap(), t, "{what}");
                    assert_eq!(Record::load(&d.store).plugins["someone.card"].version, "0.9.0");
                }
                assert!(!d._t.path().join("x").exists(), "{what}: wrote outside");
            }
        }
    }

    #[test]
    fn wrong_sha_or_tree_refused() {
        let d = dirs();
        let files = good_files("1.0.0");
        let zip = zip_of(&files);
        let good = version_of("1.0.0", &zip, &files);

        let mut v = good.clone();
        v.sha256 = "0".repeat(64);
        let e = install(&d.plugins, &d.store, "someone.card", &v, &zip).unwrap_err();
        assert!(e.contains("SHA-256"), "{e}");

        let mut v = good.clone();
        v.tree = "0".repeat(64);
        let e = install(&d.plugins, &d.store, "someone.card", &v, &zip).unwrap_err();
        assert!(e.contains("tree hash"), "{e}");

        // The manifest must name the same id and version.
        let mut v = good.clone();
        v.version = "2.0.0".into();
        assert!(install(&d.plugins, &d.store, "someone.card", &v, &zip).is_err());
        assert!(install(&d.plugins, &d.store, "someone.other", &good, &zip).is_err());

        assert_eq!(listing(&d.plugins), ["mine"]);
        assert!(Record::load(&d.store).plugins.is_empty());
    }

    #[test]
    fn listing_must_match_manifest() {
        let files = good_files("1.0.0");
        let zip = zip_of(&files);
        let good = version_of("1.0.0", &zip, &files);
        let mut cases = Vec::new();
        let mut v = good.clone();
        v.permissions.clear();
        cases.push(("fewer permissions", v));
        let mut v = good.clone();
        v.permissions.push("squad".into());
        cases.push(("more permissions", v));
        let mut v = good.clone();
        v.network.clear();
        cases.push(("fewer hosts", v));
        let mut v = good.clone();
        v.network = vec!["other.example.com".into()];
        cases.push(("other host", v));
        let mut v = good.clone();
        v.api = 2;
        cases.push(("api", v));
        for (what, v) in cases {
            let d = dirs();
            let e = install(&d.plugins, &d.store, "someone.card", &v, &zip).unwrap_err();
            assert!(e.contains("than the index lists"), "{what}: {e}");
            assert_eq!(listing(&d.plugins), ["mine"], "{what}");
            assert!(Record::load(&d.store).plugins.is_empty(), "{what}");
        }
        // Order doesn't matter.
        let mut files = good_files("1.0.0");
        files[0].1 = String::from_utf8(manifest("1.0.0"))
            .unwrap()
            .replace(r#"["fight"]"#, r#"["squad", "fight"]"#)
            .into_bytes();
        let zip = zip_of(&files);
        let mut v = version_of("1.0.0", &zip, &files);
        v.permissions = vec!["fight".into(), "squad".into()];
        let d = dirs();
        install(&d.plugins, &d.store, "someone.card", &v, &zip).unwrap();
    }

    #[test]
    fn sweep_removes_only_leftovers() {
        let d = dirs();
        for f in [".someone.card.new-12", ".someone.card.old-3", ".a.b.c.old-1/sub"] {
            fs::create_dir_all(d.plugins.join(f)).unwrap();
        }
        fs::write(d.plugins.join(".x-y.new-9"), "file").unwrap();
        let keep = [".hidden", ".someone.card.new-", ".someone.card.new-1x", ".Bad.old-1", "someone.card.old-1", "mine"];
        for f in &keep[..5] {
            fs::create_dir(d.plugins.join(f)).unwrap();
        }
        #[cfg(unix)]
        {
            // A leftover that is a link: the link goes, its target stays.
            std::os::unix::fs::symlink(d.plugins.join("mine"), d.plugins.join(".mine.old-5")).unwrap();
        }
        let log = sweep(&d.plugins);
        let mut want: Vec<String> = keep.iter().map(|s| s.to_string()).collect();
        want.sort();
        assert_eq!(listing(&d.plugins), want);
        assert!(d.plugins.join("mine/a.txt").exists());
        assert!(log.iter().all(|l| l.starts_with("removed")), "{log:?}");
        assert_eq!(log.len(), if cfg!(unix) { 5 } else { 4 });
        assert!(sweep(&d.plugins).is_empty());
    }

    #[test]
    fn hand_installed_folder_is_never_replaced() {
        let d = dirs();
        let files = good_files("1.0.0");
        let zip = zip_of(&files);
        let v = version_of("1.0.0", &zip, &files);
        fs::create_dir(d.plugins.join("someone.card")).unwrap();
        fs::write(d.plugins.join("someone.card/own.txt"), "own").unwrap();
        let e = install(&d.plugins, &d.store, "someone.card", &v, &zip).unwrap_err();
        assert_eq!(e, IN_THE_WAY);
        assert_eq!(fs::read_to_string(d.plugins.join("someone.card/own.txt")).unwrap(), "own");
        assert_eq!(listing(&d.plugins), ["mine", "someone.card"]);
        // A stray file or link with the id counts too.
        fs::remove_dir_all(d.plugins.join("someone.card")).unwrap();
        fs::write(d.plugins.join("someone.card"), "file").unwrap();
        assert_eq!(install(&d.plugins, &d.store, "someone.card", &v, &zip).unwrap_err(), IN_THE_WAY);
        assert!(Record::load(&d.store).plugins.is_empty());
    }

    #[test]
    fn uninstall_only_store_installs() {
        let d = dirs();
        assert!(uninstall(&d.plugins, &d.store, "mine").is_err());
        assert!(d.plugins.join("mine/a.txt").exists());
        let files = good_files("1.0.0");
        let zip = zip_of(&files);
        install(&d.plugins, &d.store, "someone.card", &version_of("1.0.0", &zip, &files), &zip).unwrap();
        uninstall(&d.plugins, &d.store, "someone.card").unwrap();
        assert_eq!(listing(&d.plugins), ["mine"]);
        assert!(!Record::load(&d.store).contains("someone.card"));
        assert!(uninstall(&d.plugins, &d.store, "someone.card").is_err());
        // Ids that would escape the folder are refused even if the record names them.
        let mut r = Record::default();
        r.plugins.insert("..".into(), Installed { version: "1".into(), tree: "t".into() });
        r.save(&d.store).unwrap();
        assert!(uninstall(&d.plugins, &d.store, "..").is_err());
        assert!(d.plugins.join("mine/a.txt").exists());
    }

    #[test]
    fn auto_update_rules() {
        let m = manifest::parse(&manifest("1.0.0"), Path::new("/nonexistent")).unwrap();
        let base = Version {
            version: "1.1.0".into(),
            url: "https://example.com/x.zip".into(),
            sha256: "0".repeat(64),
            tree: "0".repeat(64),
            permissions: vec!["fight".into()],
            network: vec!["api.example.com".into()],
            api: 1,
        };
        assert!(auto_update_ok(&m, Trust::Verified, &base, Trust::Verified));
        assert!(auto_update_ok(&m, Trust::Unverified, &base, Trust::Unverified));
        let mut v = base.clone();
        v.permissions.push("squad".into());
        assert!(!auto_update_ok(&m, Trust::Verified, &v, Trust::Verified));
        let mut v = base.clone();
        v.network.push("evil.example.com".into());
        assert!(!auto_update_ok(&m, Trust::Verified, &v, Trust::Verified));
        assert!(!auto_update_ok(&m, Trust::Verified, &base, Trust::Unverified));
        assert!(!auto_update_ok(&m, Trust::Unverified, &base, Trust::Verified));
        // Asking for less is fine.
        let mut v = base.clone();
        v.network.clear();
        assert!(auto_update_ok(&m, Trust::Verified, &v, Trust::Verified));
    }
}
