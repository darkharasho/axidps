//! Finding plugins: the bundled and player folders, validated, never panicking.

use crate::manifest::{MAX_MANIFEST_BYTES, Manifest, problems};
use crate::{MANIFEST_FILE, SUPPORTED_API};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Plugin {
    /// The plugin folder, canonical (a symlinked folder is followed).
    pub dir: PathBuf,
    pub bundled: bool,
    pub manifest: Option<Manifest>,
    /// The folder's name under the plugins dir.
    pub folder: String,
    pub error: Option<String>,
}

pub fn scan(bundled: &Path, player: &Path) -> Vec<Plugin> {
    scan_logged(bundled, player).0
}

pub fn scan_logged(bundled: &Path, player: &Path) -> (Vec<Plugin>, Vec<String>) {
    let mut log = Vec::new();
    let mut all: Vec<Plugin> = Vec::new();
    for (dir, is_bundled) in [(bundled, true), (player, false)] {
        all.extend(scan_dir(dir, is_bundled, &mut log));
    }
    // The page keys plugins by id, so no two entries may share one. Plugins with a manifest
    // claim their manifest id first (bundled before player, then folder order); a plugin
    // without one is listed under its folder name and yields to any entry already using it.
    let mut owner: HashMap<String, (bool, PathBuf)> = HashMap::new();
    let mut keep = vec![true; all.len()];
    for pass_valid in [true, false] {
        for (i, p) in all.iter().enumerate() {
            if p.manifest.is_some() != pass_valid {
                continue;
            }
            let id = match &p.manifest {
                Some(m) => m.id.clone(),
                None => p.folder.clone(),
            };
            match owner.get(&id) {
                None => {
                    owner.insert(id, (p.bundled, p.dir.clone()));
                }
                Some((kept_bundled, kept_dir)) => {
                    keep[i] = false;
                    let why = match (pass_valid, kept_bundled, p.bundled) {
                        (true, true, false) => {
                            "the bundled plugin with that id is kept".to_owned()
                        }
                        _ if pass_valid => format!(
                            "the plugin with that id in {} is kept (it came first)",
                            kept_dir.display()
                        ),
                        _ => format!(
                            "its folder name is the id of the plugin in {}, which is kept",
                            kept_dir.display()
                        ),
                    };
                    log.push(format!(
                        "plugin \"{id}\" in {} ignored: {why}",
                        p.dir.display()
                    ));
                }
            }
        }
    }
    let out = all
        .into_iter()
        .zip(keep)
        .filter_map(|(p, k)| k.then_some(p))
        .collect();
    (out, log)
}

fn scan_dir(dir: &Path, bundled: bool, log: &mut Vec<String>) -> Vec<Plugin> {
    let rd = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) => {
            if e.kind() != std::io::ErrorKind::NotFound {
                log.push(format!("plugins dir {}: {e}", dir.display()));
            }
            return Vec::new();
        }
    };
    let mut names: Vec<_> = rd.filter_map(Result::ok).map(|e| e.path()).collect();
    names.sort();
    let mut out = Vec::new();
    for path in names {
        // `metadata` follows a symlinked folder; a plain file or a dangling link is skipped.
        if !std::fs::metadata(&path).is_ok_and(|m| m.is_dir()) {
            continue;
        }
        let folder = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let canon = path.canonicalize().unwrap_or_else(|_| path.clone());
        let (manifest, error) = match read_manifest(&canon.join(MANIFEST_FILE)) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                (None, Some(format!("no {MANIFEST_FILE}")))
            }
            Err(e) => (None, Some(format!("cannot read {MANIFEST_FILE}: {e}"))),
            Ok(bytes) => match problems(&bytes, &canon) {
                Ok(m) => (Some(m), None),
                Err(mut p) => (None, Some(p.remove(0))),
            },
        };
        if let Some(e) = &error {
            log.push(format!("plugin folder {folder}: {e}"));
        }
        out.push(Plugin {
            dir: canon,
            bundled,
            manifest,
            folder,
            error,
        });
    }
    out
}

/// The manifest's bytes: a regular file only (a FIFO or device would block or never end), and at
/// most one byte past the size limit, which `problems` then reports as too big.
pub fn read_manifest(path: &Path) -> std::io::Result<Vec<u8>> {
    if !std::fs::metadata(path)?.is_file() {
        return Err(std::io::Error::other("not a regular file"));
    }
    let mut buf = Vec::new();
    std::fs::File::open(path)?
        .take(MAX_MANIFEST_BYTES as u64 + 1)
        .read_to_end(&mut buf)?;
    Ok(buf)
}

/// Valid, a supported `api`, no deeper permission, and no privileged one unless built in: the
/// host serves only these.
pub fn loadable(p: &Plugin) -> bool {
    p.error.is_none()
        && p.manifest.as_ref().is_some_and(|m| {
            m.api == SUPPORTED_API
                && m.deeper().is_empty()
                && (p.bundled || m.privileged().is_empty())
        })
}

/// What the player grants a plugin by enabling it: its permissions and one `net:<host>` per
/// network entry (the page's `grantsOf`).
pub fn grants(m: &Manifest) -> Vec<String> {
    m.permissions
        .iter()
        .cloned()
        .chain(m.network.iter().map(|h| format!("net:{h}")))
        .collect()
}

/// Whether the player has plugin `p` on, by the page's rule (`isEnabled` in plugin-host.js), with
/// `saved` the `plugins` object of the saved ui.json (`{<id>: {enabled, granted}}`): a loadable
/// bundled plugin unless switched off; a loadable player plugin only once enabled, and only while
/// its manifest asks for nothing beyond what was granted then.
pub fn enabled(p: &Plugin, saved: &Value) -> bool {
    let Some(m) = p.manifest.as_ref().filter(|_| loadable(p)) else {
        return false;
    };
    let s = saved.get(&m.id);
    if p.bundled {
        return s.and_then(|s| s.get("enabled")) != Some(&Value::Bool(false));
    }
    let Some(s) = s else { return false };
    if s.get("enabled") != Some(&Value::Bool(true)) {
        return false;
    }
    let Some(granted) = s.get("granted").and_then(Value::as_array) else {
        return false;
    };
    grants(m)
        .iter()
        .all(|g| granted.iter().any(|x| x.as_str() == Some(g)))
}

/// The array for `axidps.plugins([...])`.
pub fn to_page_json(plugins: &[Plugin]) -> Value {
    Value::Array(plugins.iter().map(one).collect())
}

fn one(p: &Plugin) -> Value {
    let mut v = match &p.manifest {
        Some(m) => json!({
            "id": m.id, "name": m.name, "version": m.version, "author": m.author, "api": m.api,
            "permissions": m.permissions, "network": m.network,
            "windows": m.windows.iter().map(|w| json!({
                "id": w.id, "title": w.title, "entry": w.entry, "description": w.description,
                "icon": w.icon, "open": w.open, "place": w.place.as_str(),
                "settings": w.settings,
            })).collect::<Vec<_>>(),
            "bundled": p.bundled,
        }),
        None => json!({
            "id": p.folder, "name": p.folder, "version": "", "author": "", "api": 0,
            "permissions": [], "network": [], "windows": [], "bundled": p.bundled,
        }),
    };
    let error = match (&p.error, &p.manifest) {
        (Some(e), _) => Some(e.as_str()),
        (None, Some(m)) if m.api != SUPPORTED_API => Some("needs a newer axidps"),
        (None, Some(m)) if !p.bundled && !m.privileged().is_empty() => {
            Some("only for built-in plugins")
        }
        (None, Some(m)) if !m.deeper().is_empty() => Some("needs verification"),
        _ => None,
    };
    if let Some(e) = error {
        v["error"] = e.into();
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn manifest(id: &str) -> String {
        format!(
            r#"{{"id":"{id}","name":"n","version":"1.0.0","api":1,"windows":[{{"id":"w","title":"t","entry":"a.html"}}]}}"#
        )
    }

    fn make(dir: &Path, folder: &str, id: &str) -> PathBuf {
        let d = dir.join(folder);
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join(MANIFEST_FILE), manifest(id)).unwrap();
        d
    }

    #[test]
    fn scan_survives_garbage() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path();
        fs::create_dir(p.join("junk")).unwrap();
        fs::write(p.join("junk").join(MANIFEST_FILE), "not json").unwrap();
        fs::create_dir(p.join("empty")).unwrap();
        make(p, "good", "good.one");
        fs::write(p.join("plain-file"), "x").unwrap();
        let r = scan(&p.join("none"), p);
        assert_eq!(r.len(), 3);
        assert_eq!(r.iter().filter(|x| x.error.is_some()).count(), 2);
        let good = r.iter().find(|x| x.folder == "good").unwrap();
        assert!(good.error.is_none() && loadable(good));
    }

    #[test]
    fn player_duplicate_of_bundled_ignored() {
        let b = tempfile::tempdir().unwrap();
        let p = tempfile::tempdir().unwrap();
        make(b.path(), "dup.x", "dup.x");
        make(p.path(), "dup.x", "dup.x");
        let (r, log) = scan_logged(b.path(), p.path());
        assert_eq!(r.len(), 1);
        assert!(r[0].bundled);
        assert!(log.iter().any(|l| l.contains("ignored")), "{log:?}");
        assert!(
            log.iter().any(|l| l.contains("bundled plugin with that id is kept")),
            "{log:?}"
        );
    }

    #[test]
    fn invalid_folder_never_shares_an_id_with_a_valid_plugin() {
        let b = tempfile::tempdir().unwrap();
        let p = tempfile::tempdir().unwrap();
        // Bundled folder "dup.x" is broken, but a player plugin has the id "dup.x".
        fs::create_dir(b.path().join("dup.x")).unwrap();
        make(p.path(), "other", "dup.x");
        // Two broken folders with one name, one per dir.
        fs::create_dir(b.path().join("broken")).unwrap();
        fs::create_dir(p.path().join("broken")).unwrap();
        let (r, log) = scan_logged(b.path(), p.path());
        let ids = to_page_json(&r);
        let mut ids: Vec<&str> = ids
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["id"].as_str().unwrap())
            .collect();
        ids.sort();
        assert_eq!(ids, ["broken", "dup.x"], "{log:?}");
        let valid = r.iter().find(|x| x.manifest.is_some()).unwrap();
        assert!(loadable(valid));
        assert!(log.iter().any(|l| l.contains("folder name is the id")), "{log:?}");
    }

    #[cfg(unix)]
    #[test]
    fn manifest_that_is_not_a_regular_file_does_not_hang() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path().join("fifo");
        fs::create_dir(&d).unwrap();
        let c = std::ffi::CString::new(d.join(MANIFEST_FILE).to_str().unwrap()).unwrap();
        unsafe extern "C" {
            fn mkfifo(path: *const std::ffi::c_char, mode: u32) -> i32;
        }
        assert_eq!(unsafe { mkfifo(c.as_ptr(), 0o600) }, 0);
        let r = scan(&t.path().join("none"), t.path());
        assert_eq!(r.len(), 1);
        assert!(r[0].error.as_deref().unwrap().contains("cannot read"), "{:?}", r[0].error);
    }

    #[test]
    fn huge_manifest_is_reported_not_read_whole() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path().join("big");
        fs::create_dir(&d).unwrap();
        fs::write(d.join(MANIFEST_FILE), vec![b' '; 200 * 1024]).unwrap();
        let r = scan(&t.path().join("none"), t.path());
        assert!(r[0].error.as_deref().unwrap().contains("at most"), "{:?}", r[0].error);
    }

    fn plugin(id: &str, bundled: bool, permissions: &[&str], network: &[&str]) -> Plugin {
        let mut m = crate::manifest::problems(manifest(id).as_bytes(), Path::new("/nonexistent-axidps-test")).unwrap();
        m.permissions = permissions.iter().map(|s| s.to_string()).collect();
        m.network = network.iter().map(|s| s.to_string()).collect();
        Plugin {
            dir: PathBuf::from("/x"),
            bundled,
            manifest: Some(m),
            folder: id.into(),
            error: None,
        }
    }

    #[test]
    fn privileged_only_loads_bundled() {
        let b = plugin("axidps.outline", true, &["outline"], &[]);
        assert!(loadable(&b));
        assert!(enabled(&b, &Value::Null));
        let p = plugin("x.outline", false, &["outline"], &[]);
        assert!(!loadable(&p));
        assert!(!enabled(&p, &json!({"x.outline": {"enabled": true, "granted": ["outline"]}})));
        let page = to_page_json(&[b, p]);
        assert!(page[0].get("error").is_none());
        assert_eq!(page[1]["error"], "only for built-in plugins");
    }

    #[test]
    fn enabled_follows_the_page_rule() {
        let b = plugin("axidps.fight", true, &["fight"], &[]);
        assert!(enabled(&b, &Value::Null), "bundled: on by default");
        assert!(enabled(&b, &json!({"axidps.fight": {"enabled": true}})));
        assert!(enabled(&b, &json!({"axidps.fight": 3})));
        assert!(!enabled(&b, &json!({"axidps.fight": {"enabled": false}})));

        let p = plugin("x.y", false, &["fight"], &["api.example.com"]);
        let all = json!(["fight", "net:api.example.com"]);
        assert!(!enabled(&p, &Value::Null), "player: off by default");
        assert!(!enabled(&p, &json!({})));
        assert!(enabled(&p, &json!({"x.y": {"enabled": true, "granted": all}})));
        assert!(enabled(
            &p,
            &json!({"x.y": {"enabled": true, "granted": ["fight", "net:api.example.com", "extra"]}})
        ));
        assert!(!enabled(&p, &json!({"x.y": {"enabled": false, "granted": all}})));
        assert!(!enabled(&p, &json!({"x.y": {"enabled": "yes", "granted": all}})));
        assert!(!enabled(&p, &json!({"x.y": {"enabled": true}})));
        assert!(
            !enabled(&p, &json!({"x.y": {"enabled": true, "granted": ["fight"]}})),
            "the manifest asks for a host that was never granted"
        );
        assert!(!enabled(&p, &json!({"x.y": {"enabled": true, "granted": "fight"}})));

        let mut broken = p.clone();
        broken.error = Some("bad".into());
        assert!(!enabled(&broken, &json!({"x.y": {"enabled": true, "granted": all}})));
        let deeper = plugin("x.z", false, &["fight", "world"], &[]);
        assert!(
            !enabled(&deeper, &json!({"x.z": {"enabled": true, "granted": ["fight", "world"]}})),
            "not loadable"
        );
        let deeper_b = plugin("x.w", true, &["world"], &[]);
        assert!(!enabled(&deeper_b, &Value::Null));
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_plugin_folder_scans() {
        use std::os::unix::fs::symlink;
        let elsewhere = tempfile::tempdir().unwrap();
        let player = tempfile::tempdir().unwrap();
        let real = make(elsewhere.path(), "real", "com.example.x");
        fs::write(real.join("a.html"), "hi").unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("secret"), "s").unwrap();
        symlink(outside.path().join("secret"), real.join("leak")).unwrap();
        symlink(&real, player.path().join("linked")).unwrap();

        let r = scan(&player.path().join("none"), player.path());
        assert_eq!(r.len(), 1);
        assert!(r[0].error.is_none(), "{:?}", r[0].error);
        assert_eq!(r[0].folder, "linked");
        assert_eq!(r[0].dir, real.canonicalize().unwrap());
        assert!(crate::serve::resolve(&r[0].dir, "a.html").is_some());
        assert!(crate::serve::resolve(&r[0].dir, "leak").is_none());
    }
}
