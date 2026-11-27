//! Finding plugins: the bundled and player folders, validated, never panicking.

use crate::manifest::{MAX_MANIFEST_BYTES, Manifest, problems};
use crate::trust::{Ctx, Decision, Trust, decide};
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
    /// How far it's trusted (set by `scan_trusted`; Unverified with nothing granted otherwise).
    pub trust: Decision,
    /// Installed by the store.
    pub store: bool,
}

pub fn scan(bundled: &Path, player: &Path) -> Vec<Plugin> {
    scan_logged(bundled, player).0
}

pub fn scan_logged(bundled: &Path, player: &Path) -> (Vec<Plugin>, Vec<String>) {
    scan_trusted(bundled, player, &Ctx::none())
}

/// The scan with each plugin's trust decided under `ctx`. A player plugin with a valid manifest
/// is tree-hashed from the folder the scan found; if that fails it can't be verified.
pub fn scan_trusted(bundled: &Path, player: &Path, ctx: &Ctx) -> (Vec<Plugin>, Vec<String>) {
    let (mut plugins, mut log) = scan_untrusted(bundled, player);
    for p in &mut plugins {
        let tree = match &p.manifest {
            Some(_) if !p.bundled => match crate::tree::hash_dir(&p.dir) {
                Ok(h) => Some(crate::tree::hex(&h)),
                Err(e) => {
                    log.push(format!("plugin folder {}: can't hash its files: {e}", p.folder));
                    None
                }
            },
            _ => None,
        };
        p.store = !p.bundled
            && p.manifest.as_ref().is_some_and(|m| ctx.store_ids.contains(&m.id));
        p.trust = decide(p, tree.as_deref(), ctx);
    }
    (plugins, log)
}

fn scan_untrusted(bundled: &Path, player: &Path) -> (Vec<Plugin>, Vec<String>) {
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
        // Hidden folders are never plugins: the store unpacks into `.<id>.new-*` and moves
        // the old copy to `.<id>.old-*`, which must not load (or shadow the real folder).
        if path.file_name().is_some_and(|n| n.as_encoded_bytes().starts_with(b".")) {
            continue;
        }
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
            trust: Decision::default(),
            store: false,
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

/// Valid, a supported `api`, not revoked, every deeper permission granted by its trust, and no
/// privileged one unless built in: the host serves only these.
pub fn loadable(p: &Plugin) -> bool {
    p.error.is_none()
        && p.trust.level != Trust::Revoked
        && p.manifest.as_ref().is_some_and(|m| {
            m.api == SUPPORTED_API && deeper_granted(p, m) && privileged_allowed(p, m)
        })
}

fn deeper_granted(p: &Plugin, m: &Manifest) -> bool {
    m.deeper().iter().all(|d| p.trust.granted.contains(d))
}

fn privileged_allowed(p: &Plugin, m: &Manifest) -> bool {
    p.bundled || m.privileged().is_empty()
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
    v["trust"] = p.trust.level.as_str().into();
    v["granted"] = json!(p.trust.granted);
    v["reason"] = json!(p.trust.reason);
    v["store"] = p.store.into();
    let error = match (&p.error, &p.manifest) {
        (Some(e), _) => Some(e.clone()),
        _ if p.trust.level == Trust::Revoked => Some(format!(
            "Revoked: {}",
            p.trust.reason.as_deref().unwrap_or("no reason given")
        )),
        (None, Some(m)) if m.api != SUPPORTED_API => Some("needs a newer axidps".into()),
        (None, Some(m)) if !privileged_allowed(p, m) => Some("only for built-in plugins".into()),
        (None, Some(m)) if !deeper_granted(p, m) => Some("needs verification".into()),
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
    fn hidden_folders_are_not_scanned() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path();
        make(p, ".x.old-1", "good.one");
        make(p, ".good.one.new-7", "good.one");
        let real = make(p, "good.one", "good.one");
        fs::write(real.join("a.html"), "real").unwrap();
        let r = scan(&p.join("none"), p);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].folder, "good.one");
        assert!(r[0].error.is_none());
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
            trust: Decision::default(),
            store: false,
        }
    }

    #[test]
    fn page_json_carries_trust() {
        let mut p = plugin("x.y", false, &["fight", "world"], &[]);
        let j = to_page_json(std::slice::from_ref(&p));
        assert_eq!(j[0]["trust"], "unverified");
        assert_eq!(j[0]["granted"], json!([]));
        assert_eq!(j[0]["reason"], Value::Null);
        assert_eq!(j[0]["store"], false);
        assert_eq!(j[0]["error"], "needs verification");

        p.trust = Decision { level: Trust::Verified, granted: vec!["world".into()], reason: None };
        p.store = true;
        let j = to_page_json(std::slice::from_ref(&p));
        assert_eq!(j[0]["trust"], "verified");
        assert_eq!(j[0]["granted"], json!(["world"]));
        assert_eq!(j[0]["store"], true);
        assert!(j[0].get("error").is_none(), "{j}");

        p.trust = Decision { level: Trust::Dev, granted: vec![], reason: None };
        assert_eq!(to_page_json(std::slice::from_ref(&p))[0]["trust"], "dev");

        p.trust = Decision { level: Trust::Revoked, granted: vec![], reason: Some("malware".into()) };
        let j = to_page_json(std::slice::from_ref(&p));
        assert_eq!(j[0]["trust"], "revoked");
        assert_eq!(j[0]["reason"], "malware");
        assert_eq!(j[0]["error"], "Revoked: malware");

        let b = tempfile::tempdir().unwrap();
        make(b.path(), "axidps.b", "axidps.b");
        let (r, _) = scan_logged(b.path(), &b.path().join("none"));
        let j = to_page_json(&r);
        assert_eq!(j[0]["trust"], "bundled");
        assert_eq!(j[0]["store"], false);
    }

    #[test]
    fn deeper_loads_only_when_granted() {
        use crate::keys::Keys;
        use crate::trust::Signed;
        use base64::Engine;
        use base64::engine::general_purpose::STANDARD;
        use ed25519_dalek::{Signer, SigningKey};

        let player = tempfile::tempdir().unwrap();
        let d = player.path().join("deep");
        fs::create_dir(&d).unwrap();
        let m = r#"{"id":"example.deep","name":"n","version":"1.0.0","api":1,"permissions":["fight","world"],"windows":[{"id":"w","title":"t","entry":"a.html"}]}"#;
        fs::write(d.join(MANIFEST_FILE), m).unwrap();
        fs::write(d.join("a.html"), "hi").unwrap();
        let tree = crate::tree::hex(&crate::tree::hash_dir(&d).unwrap());
        let none = player.path().join("none");

        let (r, _) = scan_logged(&none, player.path());
        assert_eq!(r[0].trust.level, Trust::Unverified);
        assert!(!loadable(&r[0]));

        let key = SigningKey::from_bytes(&[7u8; 32]);
        let pk = key.verifying_key().to_bytes();
        let mut a = json!({"id": "example.deep", "version": "1.0.0", "tree": tree,
                           "verified": true, "grant": ["world"],
                           "approver": crate::sign::key_id(&pk)});
        let msg = format!("axidps-approval-v1\n{}", crate::sign::canonical(&a).unwrap());
        a["sig"] = json!(STANDARD.encode(key.sign(msg.as_bytes()).to_bytes()));
        let keys = Keys { approvers: vec![pk], index: vec![] };
        let (signed, log) = Signed::from_index(&json!({"approvals": [a]}), &keys, 0);
        assert!(log.is_empty(), "{log:?}");
        let mut ctx = Ctx { signed, today: "2026-11-26".into(), ..Ctx::none() };
        ctx.store_ids.insert("example.deep".into());

        let (r, log) = scan_trusted(&none, player.path(), &ctx);
        assert!(log.is_empty(), "{log:?}");
        assert_eq!(r[0].trust.level, Trust::Verified);
        assert!(r[0].store);
        assert!(loadable(&r[0]));
        let saved = json!({"example.deep": {"enabled": true, "granted": ["fight", "world"]}});
        assert!(enabled(&r[0], &saved));

        // One edited byte and the approval no longer matches.
        fs::write(d.join("a.html"), "hI").unwrap();
        let (r, _) = scan_trusted(&none, player.path(), &ctx);
        assert_eq!(r[0].trust.level, Trust::Unverified);
        assert!(!loadable(&r[0]));
        assert!(!enabled(&r[0], &saved));

        // A symlink inside: can't be hashed, so can't be verified, and the log says so.
        #[cfg(unix)]
        {
            fs::write(d.join("a.html"), "hi").unwrap();
            std::os::unix::fs::symlink(d.join("a.html"), d.join("link")).unwrap();
            let (r, log) = scan_trusted(&none, player.path(), &ctx);
            assert_eq!(r[0].trust.level, Trust::Unverified);
            assert!(r[0].trust.reason.is_some());
            assert!(!loadable(&r[0]));
            assert!(log.iter().any(|l| l.contains("can't hash")), "{log:?}");
        }
    }

    #[test]
    fn bundled_outline_plugin_scans() {
        let dir = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../apps/axidps-overlay/plugins"));
        let (plugins, log) = scan_logged(dir, Path::new("/nonexistent-player-dir"));
        let p = plugins.iter().find(|p| p.manifest.as_ref().is_some_and(|m| m.id == "axidps.outline")).unwrap_or_else(|| panic!("not scanned: {log:?}"));
        assert!(p.bundled && loadable(p));
        assert!(log.iter().all(|l| !l.contains("outline")), "{log:?}");
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
