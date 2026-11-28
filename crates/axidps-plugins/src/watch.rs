//! Polling change detection for the player's plugin folders (developer mode).

use crate::MANIFEST_FILE;
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::time::{Duration, Instant, SystemTime};

pub const MAX_FILES: usize = 2000;
pub const QUIET: Duration = Duration::from_millis(200);

type Files = BTreeMap<String, (u64, Option<SystemTime>)>;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct PluginSnap {
    /// The manifest's id, else the folder name.
    pub id: String,
    pub files: Files,
    /// More than `MAX_FILES` files: not watched.
    pub too_many: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Snapshot {
    /// Keyed by folder name.
    pub plugins: BTreeMap<String, PluginSnap>,
}

impl Snapshot {
    pub fn take(dir: &Path) -> Snapshot {
        let mut plugins = BTreeMap::new();
        let Ok(rd) = std::fs::read_dir(dir) else {
            return Snapshot { plugins };
        };
        for e in rd.filter_map(Result::ok) {
            let path = e.path();
            // Hidden folders (store install scratch) are never plugins; see `scan`.
            if e.file_name().as_encoded_bytes().starts_with(b".") {
                continue;
            }
            if !std::fs::metadata(&path).is_ok_and(|m| m.is_dir()) {
                continue;
            }
            let folder = e.file_name().to_string_lossy().into_owned();
            let mut snap = PluginSnap {
                id: manifest_id(&path).unwrap_or_else(|| folder.clone()),
                ..Default::default()
            };
            walk(&path, "", &mut snap, 0);
            if snap.too_many {
                snap.files.clear();
            }
            plugins.insert(folder, snap);
        }
        Snapshot { plugins }
    }
}

fn manifest_id(dir: &Path) -> Option<String> {
    // Capped, and never a FIFO or device: see `scan::read_manifest`.
    let bytes = crate::scan::read_manifest(&dir.join(MANIFEST_FILE)).ok()?;
    if bytes.len() > crate::manifest::MAX_MANIFEST_BYTES {
        return None;
    }
    let v: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    v.get("id")?.as_str().map(str::to_owned)
}

fn walk(dir: &Path, prefix: &str, snap: &mut PluginSnap, depth: usize) {
    if depth > 16 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.filter_map(Result::ok) {
        if snap.too_many {
            return;
        }
        let name = e.file_name().to_string_lossy().into_owned();
        let rel = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        // A vanished entry is skipped. Symlinked directories inside a plugin are not descended
        // (no loops); symlinked files are stat'ed through.
        let Ok(lm) = e.path().symlink_metadata() else {
            continue;
        };
        if lm.is_dir() {
            walk(&e.path(), &rel, snap, depth + 1);
        } else if let Ok(m) = std::fs::metadata(e.path())
            && m.is_file()
        {
            if snap.files.len() >= MAX_FILES {
                snap.too_many = true;
                return;
            }
            snap.files.insert(rel, (m.len(), m.modified().ok()));
        }
    }
}

#[derive(Debug, Default)]
pub struct Watcher {
    last: Option<Snapshot>,
    /// folder -> (id, when it last changed)
    pending: HashMap<String, (String, Instant)>,
}

impl Watcher {
    pub fn new() -> Self {
        Self::default()
    }

    /// Ids whose files changed and have been still for `QUIET`; each burst is reported once.
    /// The first snapshot is the baseline.
    pub fn tick(&mut self, now: Instant, snap: Snapshot) -> Vec<String> {
        if let Some(last) = &self.last {
            let empty = PluginSnap::default();
            for folder in last.plugins.keys().chain(snap.plugins.keys()) {
                let (a, b) = (last.plugins.get(folder), snap.plugins.get(folder));
                if a.unwrap_or(&empty).files != b.unwrap_or(&empty).files
                    || a.is_some() != b.is_some()
                {
                    let id = b.or(a).map_or_else(|| folder.clone(), |s| s.id.clone());
                    self.pending.insert(folder.clone(), (id, now));
                }
            }
        }
        self.last = Some(snap);
        let mut ready: Vec<(String, String)> = self
            .pending
            .iter()
            .filter(|(_, (_, at))| now.saturating_duration_since(*at) >= QUIET)
            .map(|(f, (id, _))| (f.clone(), id.clone()))
            .collect();
        ready.sort();
        for (f, _) in &ready {
            self.pending.remove(f);
        }
        ready.into_iter().map(|(_, id)| id).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// A plugin.json that is a FIFO (from a tar or a dev tool) must not block the watcher, which
    /// the overlay starts on its main thread.
    #[test]
    fn fifo_manifest_does_not_block() {
        let t = tempfile::tempdir().unwrap();
        let c = std::ffi::CString::new(t.path().join(MANIFEST_FILE).to_str().unwrap()).unwrap();
        unsafe extern "C" {
            fn mkfifo(path: *const std::ffi::c_char, mode: u32) -> i32;
        }
        assert_eq!(unsafe { mkfifo(c.as_ptr(), 0o600) }, 0);
        let (tx, rx) = std::sync::mpsc::channel();
        let dir = t.path().to_path_buf();
        std::thread::spawn(move || tx.send(manifest_id(&dir)).unwrap());
        let got = rx.recv_timeout(Duration::from_secs(5));
        assert_eq!(got, Ok(None), "blocked or read a FIFO");
    }

    fn snap(n: usize, tag: u64) -> Snapshot {
        let files = (0..n)
            .map(|i| (format!("f{i}.js"), (tag, Some(SystemTime::UNIX_EPOCH))))
            .collect();
        Snapshot {
            plugins: BTreeMap::from([(
                "x.y".to_owned(),
                PluginSnap {
                    id: "x.y".into(),
                    files,
                    too_many: false,
                },
            )]),
        }
    }

    #[test]
    fn hidden_folders_are_not_watched() {
        let t = tempfile::tempdir().unwrap();
        for f in [".x.old-1", "x.y"] {
            fs::create_dir(t.path().join(f)).unwrap();
            fs::write(t.path().join(f).join("a.js"), "1").unwrap();
        }
        let s = Snapshot::take(t.path());
        assert_eq!(s.plugins.keys().collect::<Vec<_>>(), ["x.y"]);
    }

    #[test]
    fn watch_debounces() {
        let mut w = Watcher::new();
        let t = Instant::now();
        assert!(w.tick(t, snap(2, 1)).is_empty());
        assert!(w.tick(t + Duration::from_millis(10), snap(2, 2)).is_empty());
        let t2 = t + Duration::from_millis(10);
        assert!(
            w.tick(t2 + Duration::from_millis(100), snap(2, 2))
                .is_empty()
        );
        assert_eq!(w.tick(t2 + Duration::from_millis(250), snap(2, 2)), ["x.y"]);
        assert!(
            w.tick(t2 + Duration::from_millis(500), snap(2, 2))
                .is_empty()
        );
    }

    #[test]
    fn watch_storm_once() {
        let mut w = Watcher::new();
        let t = Instant::now();
        w.tick(t, snap(300, 0));
        let mut got = Vec::new();
        for (i, tag) in [1u64, 2, 3].into_iter().enumerate() {
            got.extend(w.tick(
                t + Duration::from_millis(50 * (i as u64 + 1)),
                snap(300, tag),
            ));
        }
        for ms in [200, 250, 300, 350, 400, 450, 600] {
            got.extend(w.tick(t + Duration::from_millis(ms), snap(300, 3)));
        }
        assert_eq!(got, ["x.y"]);
    }

    #[test]
    fn watch_vanishing_file() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("x.y");
        fs::create_dir(&p).unwrap();
        fs::write(p.join(MANIFEST_FILE), r#"{"id":"x.y"}"#).unwrap();
        fs::write(p.join("a.js"), "1").unwrap();
        let mut w = Watcher::new();
        let t = Instant::now();
        w.tick(t, Snapshot::take(d.path()));
        fs::remove_file(p.join("a.js")).unwrap();
        w.tick(t + Duration::from_millis(10), Snapshot::take(d.path()));
        assert_eq!(
            w.tick(t + Duration::from_millis(300), Snapshot::take(d.path())),
            ["x.y"]
        );
    }

    #[test]
    fn take_caps_files_and_falls_back_to_folder_name() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("big");
        fs::create_dir(&p).unwrap();
        for i in 0..=MAX_FILES {
            fs::write(p.join(format!("{i}.js")), "").unwrap();
        }
        let s = Snapshot::take(d.path());
        assert!(s.plugins["big"].too_many && s.plugins["big"].files.is_empty());
        assert_eq!(s.plugins["big"].id, "big");
    }
}
