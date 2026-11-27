//! The plugin store, host side: fetches the signed index (at start, every 6 hours, and when the
//! Store tab opens on a cache older than 10 minutes), keeps the last accepted copy in
//! `<config>/store/`, installs, updates and uninstalls store plugins, and gives the page its
//! Store tab as `axidps.store(json)`. Network and file work run on one worker thread; the main
//! thread only queues jobs and reads replies.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axidps_plugins::SUPPORTED_API;
use axidps_plugins::index::{self, Index, Listing, Version};
use axidps_plugins::install::{self, Record};
use axidps_plugins::keys::Keys;
use axidps_plugins::logline::clean;
use axidps_plugins::scan;
use axidps_plugins::trust::{self, Ctx, Decision, Signed, Trust};
use serde_json::{Value, json};

pub const INDEX_URL: &str =
    "https://github.com/darkharasho/axidps-plugins/releases/download/index/index.json";
pub const SIG_URL: &str =
    "https://github.com/darkharasho/axidps-plugins/releases/download/index/index.json.sig";
/// Package zips download from the registry's releases only.
pub const PACKAGE_PREFIX: &str = "https://github.com/darkharasho/axidps-plugins/releases/download/";
const INDEX_CAP: usize = 8 * 1024 * 1024;
const SIG_CAP: usize = 4 * 1024;
/// A package unpacks to at most `MAX_UNPACKED`; the zip may carry a little overhead on top.
const ZIP_CAP: usize = install::MAX_UNPACKED as usize + 1024 * 1024;
const FETCH_EVERY: Duration = Duration::from_secs(6 * 60 * 60);
const STALE_MS: u64 = 10 * 60 * 1000;
const NEEDS_NEWER: &str = "needs a newer axidps";

/// Downloads `url`, at most `cap` bytes.
pub trait Fetch: Send {
    fn get(&self, url: &str, cap: usize) -> Result<Vec<u8>, String>;
}

/// The real network: one agent, 15 s for the whole request, a generic user agent.
pub struct Ureq(ureq::Agent);

impl Ureq {
    pub fn new() -> Ureq {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(15)))
            .user_agent("axidps")
            .build()
            .into();
        Ureq(agent)
    }
}

impl Fetch for Ureq {
    fn get(&self, url: &str, cap: usize) -> Result<Vec<u8>, String> {
        let mut r = self.0.get(url).call().map_err(|e| clean(&e.to_string()))?;
        r.body_mut()
            .with_config()
            .limit(cap as u64)
            .read_to_vec()
            .map_err(|e| clean(&e.to_string()))
    }
}

pub enum Event {
    /// A page call (`axidps.store(...)`).
    Page(String),
    /// Files or trust decisions changed: rescan the plugin folders.
    Rescan,
}

enum Job {
    Fetch,
    Show,
    Install(String),
    Uninstall(String),
}

struct Reply {
    job: Job,
    page: Value,
    signed: Signed,
    store_ids: HashSet<String>,
    rescan: bool,
}

/// The main thread's side.
pub struct Store {
    config: PathBuf,
    tx: mpsc::Sender<Job>,
    rx: mpsc::Receiver<Reply>,
    signed: Signed,
    store_ids: HashSet<String>,
    /// The worker's last page payload (without busy marks).
    last: RefCell<Value>,
    busy: RefCell<HashSet<String>>,
    fetching: Cell<bool>,
    last_fetch: Cell<Option<Instant>>,
    /// Jobs sent and not yet answered.
    pending: Cell<usize>,
    out: RefCell<Vec<Event>>,
}

impl Store {
    /// The store for config folder `config`, installing into `<config>/plugins`.
    #[allow(dead_code)]
    pub fn new(config: &Path, keys: Keys) -> Store {
        Store::open(config, &config.join("plugins"), &config.join("ui.json"), keys, Box::new(Ureq::new()))
    }

    /// The store with its plugin folder, saved UI file and network given. Loads the cached
    /// index (`<config>/store/index.json`, `.sig`, `revoked-seq`) and starts the worker.
    pub fn open(config: &Path, plugins: &Path, ui: &Path, keys: Keys, fetch: Box<dyn Fetch>) -> Store {
        let mut core = Core {
            fetch,
            keys,
            config: config.to_path_buf(),
            store_dir: config.join("store"),
            plugins_dir: plugins.to_path_buf(),
            ui: ui.to_path_buf(),
            cache: None,
            signed: Signed::default(),
            seq: 0,
            fetched: None,
            state: "offline",
            error: None,
            errors: HashMap::new(),
            owned: HashSet::new(),
        };
        core.load();
        core.refresh_owned();
        let page = core.page();
        let store_ids = core.store_ids();
        let signed = core.signed.clone();
        let (tx, jobs) = mpsc::channel::<Job>();
        let (replies, rx) = mpsc::channel::<Reply>();
        std::thread::Builder::new()
            .name("store".into())
            .spawn(move || {
                for job in jobs {
                    // The player may have changed folders since: who owns what, hashed once.
                    core.refresh_owned();
                    let rescan = core.run(&job);
                    let reply = Reply {
                        job,
                        page: core.page(),
                        signed: core.signed.clone(),
                        store_ids: core.store_ids(),
                        rescan,
                    };
                    if replies.send(reply).is_err() {
                        break;
                    }
                }
            })
            .expect("store worker thread");
        Store {
            config: config.to_path_buf(),
            tx,
            rx,
            signed,
            store_ids,
            last: RefCell::new(page),
            busy: RefCell::default(),
            fetching: Cell::new(false),
            last_fetch: Cell::new(None),
            pending: Cell::new(0),
            out: RefCell::default(),
        }
    }

    /// The trust context for a scan: the cached index's approvals, passes and revocation.
    pub fn ctx(&self, dev: bool) -> Ctx {
        make_ctx(&self.signed, dev, &self.config, self.store_ids.clone())
    }

    /// A page message's `store` value: `"open"`, `"refresh"`, or `{"install"|"update"|"uninstall": id}`.
    pub fn request(&self, msg: &Value) {
        match msg {
            Value::String(s) if s == "open" => {
                let fresh = self.last.borrow()["fetched"]
                    .as_u64()
                    .is_some_and(|t| now_ms().saturating_sub(t) < STALE_MS);
                if fresh || self.fetching.get() {
                    self.send(Job::Show);
                } else {
                    self.queue_fetch(Instant::now());
                }
            }
            Value::String(s) if s == "refresh" => self.queue_fetch(Instant::now()),
            Value::Object(o) => {
                let job = |k: &str| o.get(k).and_then(Value::as_str).map(str::to_owned);
                let (id, job) = match (job("install").or_else(|| job("update")), job("uninstall")) {
                    (Some(id), _) => (id.clone(), Job::Install(id)),
                    (None, Some(id)) => (id.clone(), Job::Uninstall(id)),
                    _ => return,
                };
                if crate::log_id(&id) == "?" || !self.busy.borrow_mut().insert(id) {
                    return;
                }
                self.send(job);
                self.emit_page();
            }
            _ => {}
        }
    }

    /// Fetches at the first call and every 6 hours after.
    pub fn tick(&mut self, now: Instant) {
        if self.last_fetch.get().is_none_or(|t| now.duration_since(t) >= FETCH_EVERY) {
            self.queue_fetch(now);
        }
    }

    /// What happened since the last poll.
    pub fn poll(&mut self) -> Vec<Event> {
        while let Ok(r) = self.rx.try_recv() {
            self.pending.set(self.pending.get().saturating_sub(1));
            match &r.job {
                Job::Fetch => self.fetching.set(false),
                Job::Install(id) | Job::Uninstall(id) => {
                    self.busy.borrow_mut().remove(id);
                }
                Job::Show => {}
            }
            self.signed = r.signed;
            self.store_ids = r.store_ids;
            *self.last.borrow_mut() = r.page;
            if r.rescan {
                self.out.borrow_mut().push(Event::Rescan);
            }
            self.emit_page();
        }
        std::mem::take(&mut *self.out.borrow_mut())
    }

    fn send(&self, job: Job) {
        if self.tx.send(job).is_ok() {
            self.pending.set(self.pending.get() + 1);
        }
    }

    fn queue_fetch(&self, now: Instant) {
        self.last_fetch.set(Some(now));
        if self.fetching.replace(true) {
            return;
        }
        self.send(Job::Fetch);
        self.emit_page();
    }

    /// The page call for the current state, with busy marks and `loading` while fetching.
    fn emit_page(&self) {
        let mut v = self.last.borrow().clone();
        if self.fetching.get() {
            v["state"] = "loading".into();
        }
        let busy = self.busy.borrow();
        if let Some(list) = v["plugins"].as_array_mut() {
            for p in list {
                let b = p["id"].as_str().is_some_and(|id| busy.contains(id));
                p["busy"] = b.into();
            }
        }
        self.out
            .borrow_mut()
            .push(Event::Page(format!("window.axidps && window.axidps.store && window.axidps.store({v})")));
    }
}

fn make_ctx(signed: &Signed, dev: bool, config: &Path, store_ids: HashSet<String>) -> Ctx {
    Ctx {
        signed: signed.clone(),
        dev,
        dev_key: trust::load_dev_key(config),
        today: today(),
        store_ids,
    }
}

/// The worker's state: the accepted index and everything done with it.
struct Core {
    fetch: Box<dyn Fetch>,
    keys: Keys,
    config: PathBuf,
    store_dir: PathBuf,
    plugins_dir: PathBuf,
    ui: PathBuf,
    cache: Option<Index>,
    signed: Signed,
    /// The highest `revoked.json` seq seen (`revoked-seq`).
    seq: u64,
    fetched: Option<u64>,
    state: &'static str,
    error: Option<String>,
    /// The last install/update/uninstall failure per plugin.
    errors: HashMap<String, String>,
    /// Record ids whose folder is byte for byte what the store installed (`Record::owns`).
    owned: HashSet<String>,
}

impl Core {
    fn load(&mut self) {
        self.seq = std::fs::read_to_string(self.store_dir.join("revoked-seq"))
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0);
        let read = |f: &str| std::fs::read(self.store_dir.join(f));
        if let (Ok(bytes), Ok(sig)) = (read("index.json"), read("index.json.sig")) {
            match index::parse(&bytes, &String::from_utf8_lossy(&sig), &self.keys) {
                Ok(idx) => {
                    let (signed, log) = Signed::from_index(&idx.raw, &self.keys, self.seq);
                    print_log(&log);
                    self.signed = signed;
                    self.cache = Some(idx);
                    self.fetched = std::fs::metadata(self.store_dir.join("index.json"))
                        .and_then(|m| m.modified())
                        .ok()
                        .map(unix_ms);
                    self.state = "ok";
                }
                Err(e) => println!("store: cached index refused: {}", clean(&e)),
            }
        }
    }

    /// Runs one job; true when the plugin folders should be rescanned.
    fn run(&mut self, job: &Job) -> bool {
        match job {
            Job::Show => false,
            Job::Fetch => self.fetch_index(),
            Job::Install(id) => {
                let r = self.install(id);
                self.done(id, "install", r)
            }
            Job::Uninstall(id) => {
                let r = install::uninstall(&self.plugins_dir, &self.store_dir, id);
                self.done(id, "uninstall", r)
            }
        }
    }

    fn done(&mut self, id: &str, what: &str, r: Result<(), String>) -> bool {
        match r {
            Ok(()) => {
                println!("store: {what} {id} done");
                self.errors.remove(id);
                self.refresh_owned();
                true
            }
            Err(e) => {
                let e = clean(&e);
                println!("store: {what} {id} failed: {e}");
                self.errors.insert(id.to_owned(), e);
                false
            }
        }
    }

    fn fetch_index(&mut self) -> bool {
        let got = self
            .fetch
            .get(INDEX_URL, INDEX_CAP)
            .and_then(|b| Ok((b, self.fetch.get(SIG_URL, SIG_CAP)?)));
        let (bytes, sig) = match got {
            Ok(x) => x,
            Err(e) => {
                println!("store: can't reach the registry: {}", clean(&e));
                self.state = "offline";
                self.error = Some(clean(&format!("can't reach the plugin store: {e}")));
                return false;
            }
        };
        let idx = match index::parse(&bytes, &String::from_utf8_lossy(&sig), &self.keys) {
            Ok(i) => i,
            Err(e) => return self.refuse(&format!("the store index was refused: {e}")),
        };
        print_log(&idx.skipped);
        let (signed, log) = Signed::from_index(&idx.raw, &self.keys, self.seq);
        print_log(&log);
        // Never go backwards: an index without a revocation at least as new as the last one
        // seen is an old copy, so the cached one stays.
        if self.seq > 0 && signed.revoked.is_none() {
            return self.refuse(&format!(
                "the store sent an index older than the last revocation seen (seq {}); \
                 keeping the cached one",
                self.seq
            ));
        }
        let before = self.decisions();
        if let Err(e) = self.save_cache(&bytes, &sig) {
            println!("store: can't save the index: {}", clean(&e));
        }
        let seq = signed.revoked.as_ref().map_or(0, |r| r.seq);
        if seq > self.seq {
            match write_atomic(&self.store_dir, "revoked-seq", seq.to_string().as_bytes()) {
                Ok(()) => self.seq = seq,
                Err(e) => println!("store: can't save revoked-seq: {}", clean(&e)),
            }
        }
        self.cache = Some(idx);
        self.signed = signed;
        self.fetched = Some(now_ms());
        self.state = "ok";
        self.error = None;
        let updated = self.auto_update();
        updated || self.decisions() != before
    }

    fn refuse(&mut self, why: &str) -> bool {
        let why = clean(why);
        println!("store: {why}");
        self.state = "error";
        self.error = Some(why);
        false
    }

    fn save_cache(&self, bytes: &[u8], sig: &[u8]) -> Result<(), String> {
        write_atomic(&self.store_dir, "index.json", bytes)?;
        write_atomic(&self.store_dir, "index.json.sig", sig)
    }

    /// Store installs the store still owns: an edited, replaced or missing folder isn't one.
    fn refresh_owned(&mut self) {
        let record = Record::load(&self.store_dir);
        self.owned = record
            .plugins
            .keys()
            .filter(|id| record.owns(id, &self.plugins_dir.join(id)))
            .cloned()
            .collect();
    }

    fn store_ids(&self) -> HashSet<String> {
        self.owned.clone()
    }

    fn ctx(&self) -> Ctx {
        make_ctx(&self.signed, crate::saved_dev(&self.ui), &self.config, self.store_ids())
    }

    /// The player folder's plugins as trusted now.
    fn scan(&self) -> Vec<scan::Plugin> {
        scan::scan_trusted(Path::new(""), &self.plugins_dir, &self.ctx()).0
    }

    /// Each player plugin's trust decision, by folder.
    fn decisions(&self) -> BTreeMap<String, Decision> {
        self.scan().into_iter().map(|p| (p.folder, p.trust)).collect()
    }

    fn listing(&self, id: &str) -> Option<&Listing> {
        self.cache.as_ref()?.plugins.iter().find(|l| l.id == id)
    }

    /// The version to install for `id`, or why there is none.
    fn offered(&self, id: &str) -> Result<(Listing, Version), String> {
        let l = supported(self.listing(id).ok_or_else(|| format!("{id} isn't in the store"))?);
        if l.versions.is_empty() {
            return Err(NEEDS_NEWER.into());
        }
        let v = l.offer(&self.signed).ok_or("it was withdrawn from the store")?.clone();
        Ok((l, v))
    }

    fn install(&self, id: &str) -> Result<(), String> {
        let (_, v) = self.offered(id)?;
        self.download_install(id, &v)
    }

    fn download_install(&self, id: &str, v: &Version) -> Result<(), String> {
        if !registry_url(&v.url) {
            return Err("its download isn't on the plugin store".into());
        }
        let zip = self.fetch.get(&v.url, ZIP_CAP).map_err(|e| format!("download failed: {e}"))?;
        install::install(&self.plugins_dir, &self.store_dir, id, v, &zip)
    }

    /// Updates store installs whose newer version asks for nothing new and keeps the same trust
    /// level, unless the player turned that off. True if anything was installed.
    fn auto_update(&mut self) -> bool {
        let on = crate::saved_ui(&self.ui)
            .and_then(|v| v.get("storeAutoUpdate").cloned())
            != Some(Value::Bool(false));
        if !on {
            return false;
        }
        let plugins = self.scan();
        let record = Record::load(&self.store_dir);
        let mut any = false;
        for (id, have) in &record.plugins {
            let Some(p) = plugins.iter().find(|p| p.store && p.folder == *id) else { continue };
            let Some(m) = p.manifest.as_ref().filter(|m| m.id == *id) else { continue };
            let Ok((l, new)) = self.offered(id) else { continue };
            if !newer(&new.version, &have.version) {
                continue;
            }
            let level = if l.approved(&new, &self.signed) { Trust::Verified } else { Trust::Unverified };
            if !install::auto_update_ok(m, p.trust.level, &new, level) {
                println!("store: {id} {} needs the player's OK to update", new.version);
                continue;
            }
            let r = self.download_install(id, &new);
            any |= self.done(id, &format!("update to {}", new.version), r);
        }
        any
    }

    /// The `axidps.store(...)` payload (busy marks are the main thread's).
    fn page(&self) -> Value {
        let record = Record::load(&self.store_dir);
        let player = scan::scan_trusted(Path::new(""), &self.plugins_dir, &Ctx::none()).0;
        let mut list = Vec::new();
        for raw in self.cache.iter().flat_map(|c| &c.plugins) {
            let id = &raw.id;
            let has_folder = self.plugins_dir.join(id).symlink_metadata().is_ok();
            // Installed only while the folder is byte for byte the store's: a missing one is
            // offered again, an edited or replaced one is the player's.
            let owned = self.owned.contains(id);
            let installed = record.plugins.get(id).filter(|_| owned).map(|i| i.version.clone());
            let hand = !owned
                && (has_folder || player.iter().any(|p| p.manifest.as_ref().is_some_and(|m| m.id == *id)));
            let l = supported(raw);
            let offer = l.offer(&self.signed);
            if l.versions.is_empty() || offer.is_some() || installed.is_some() {
                // Shown: offered, needs a newer axidps, or installed (and so uninstallable).
            } else {
                continue;
            }
            let shown = offer.or(raw.versions.last());
            let update = match (offer, &installed) {
                (Some(o), Some(i)) if newer(&o.version, i) => Some(o.version.clone()),
                _ => None,
            };
            let mut v = json!({
                "id": id, "name": raw.name, "author": raw.author, "description": raw.description,
                "icon": raw.icon, "repo": raw.repo,
                "version": shown.map(|s| s.version.clone()),
                "trust": if offer.is_some_and(|o| l.approved(o, &self.signed)) { "verified" } else { "unverified" },
                "awaiting": l.awaiting_review(&self.signed),
                "permissions": shown.map(|s| s.permissions.clone()).unwrap_or_default(),
                "network": shown.map(|s| s.network.clone()).unwrap_or_default(),
                "installed": installed, "handInstalled": hand, "update": update, "busy": false,
            });
            let error = self.errors.get(id).cloned().or_else(|| {
                if l.versions.is_empty() { Some(NEEDS_NEWER.to_owned()) } else { None }
            });
            if let Some(e) = error {
                v["error"] = e.into();
            }
            list.push(v);
        }
        let mut v = json!({"state": self.state, "fetched": self.fetched, "plugins": list});
        if let Some(e) = &self.error {
            v["error"] = e.clone().into();
        }
        v
    }
}

/// `l` with only the versions this build can load.
fn supported(l: &Listing) -> Listing {
    let mut l = l.clone();
    l.versions.retain(|v| v.api == SUPPORTED_API);
    l
}

/// `a` is a newer semantic version than `b`.
fn newer(a: &str, b: &str) -> bool {
    match (semver::Version::parse(a), semver::Version::parse(b)) {
        (Ok(a), Ok(b)) => a > b,
        _ => false,
    }
}

/// A download from the registry's own releases: nothing that could resolve elsewhere.
fn registry_url(url: &str) -> bool {
    url.strip_prefix(PACKAGE_PREFIX).is_some_and(|rest| {
        !rest.is_empty()
            && !rest.contains("..")
            && rest.bytes().all(|b| b.is_ascii_alphanumeric() || b"-._/+".contains(&b))
    })
}

fn print_log(lines: &[String]) {
    for l in lines {
        println!("store: {}", clean(l));
    }
}

/// Writes `dir/name` through a temp file renamed over it.
fn write_atomic(dir: &Path, name: &str, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    let err = |e: std::io::Error| format!("{}: {e}", dir.join(name).display());
    std::fs::create_dir_all(dir).map_err(err)?;
    let tmp = dir.join(format!(".{name}.tmp-{}", std::process::id()));
    let w = || -> std::io::Result<()> {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        std::fs::rename(&tmp, dir.join(name))
    };
    w().map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        err(e)
    })
}

fn unix_ms(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

fn now_ms() -> u64 {
    unix_ms(SystemTime::now())
}

/// Today, UTC, `YYYY-MM-DD`.
fn today() -> String {
    let days = (now_ms() / 86_400_000) as i64;
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}")
}

/// The proleptic Gregorian date `days` after 1970-01-01 (Howard Hinnant's algorithm).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axidps_plugins::sign::{canonical, key_id};
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    use ed25519_dalek::{Signer, SigningKey};
    use std::io::Write;
    use std::sync::{Arc, Mutex};

    const ID: &str = "someone.card";

    fn key() -> SigningKey {
        SigningKey::from_bytes(&[7u8; 32])
    }

    fn keys() -> Keys {
        let pk = key().verifying_key().to_bytes();
        Keys { approvers: vec![pk], index: vec![pk] }
    }

    /// Canned downloads by URL; anything else is "offline".
    #[derive(Clone, Default)]
    struct Net(Arc<Mutex<HashMap<String, Vec<u8>>>>);

    impl Net {
        fn set(&self, url: &str, bytes: Vec<u8>) {
            self.0.lock().unwrap().insert(url.into(), bytes);
        }
        fn clear(&self) {
            self.0.lock().unwrap().clear();
        }
    }

    impl Fetch for Net {
        fn get(&self, url: &str, cap: usize) -> Result<Vec<u8>, String> {
            let b = self.0.lock().unwrap().get(url).cloned().ok_or("offline")?;
            if b.len() > cap {
                return Err("too big".into());
            }
            Ok(b)
        }
    }

    struct T {
        _t: tempfile::TempDir,
        config: PathBuf,
        plugins: PathBuf,
        ui: PathBuf,
        net: Net,
    }

    fn t() -> T {
        let t = tempfile::tempdir().unwrap();
        let config = t.path().to_path_buf();
        T { plugins: config.join("plugins"), ui: config.join("ui.json"), config, net: Net::default(), _t: t }
    }

    impl T {
        fn store(&self) -> Store {
            Store::open(&self.config, &self.plugins, &self.ui, keys(), Box::new(self.net.clone()))
        }

        /// A package for `ID` `version` asking for `network`, downloadable; its index entry.
        fn publish(&self, version: &str, network: &[&str]) -> Value {
            let manifest = json!({
                "id": ID, "name": "Card", "version": version, "api": 1,
                "permissions": ["fight"], "network": network,
                "windows": [{"id": "card", "title": "Card", "entry": "card.html"}],
            });
            let files: Vec<(String, Vec<u8>)> = vec![
                (axidps_plugins::MANIFEST_FILE.into(), manifest.to_string().into_bytes()),
                ("card.html".into(), format!("<p>{version}</p>").into_bytes()),
            ];
            let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
            for (name, bytes) in &files {
                w.start_file(name.as_str(), zip::write::SimpleFileOptions::default()).unwrap();
                w.write_all(bytes).unwrap();
            }
            let zip = w.finish().unwrap().into_inner();
            let url = format!("{PACKAGE_PREFIX}{ID}-{version}/package.zip");
            let v = json!({
                "version": version, "url": url, "sha256": install::sha256_hex(&zip),
                "tree": axidps_plugins::tree::hex(&axidps_plugins::tree::hash_files(&files)),
                "permissions": ["fight"], "network": network, "api": 1,
            });
            self.net.set(&url, zip);
            v
        }

        /// Serves an index listing `versions` of `ID` with `extra` top-level fields, signed.
        fn serve(&self, versions: Vec<Value>, extra: Value) {
            let mut v = json!({
                "generated": "2026-11-26T00:00:00Z",
                "plugins": [{"id": ID, "name": "Card", "author": "someone", "versions": versions}],
            });
            for (k, x) in extra.as_object().unwrap() {
                v[k] = x.clone();
            }
            let bytes = serde_json::to_vec(&v).unwrap();
            let mut msg = index::INDEX_PREFIX.as_bytes().to_vec();
            msg.extend_from_slice(&bytes);
            self.net.set(SIG_URL, STANDARD.encode(key().sign(&msg).to_bytes()).into_bytes());
            self.net.set(INDEX_URL, bytes);
        }

        fn saved(&self, v: Value) {
            std::fs::write(&self.ui, v.to_string()).unwrap();
        }

        fn record(&self) -> Record {
            Record::load(&self.config.join("store"))
        }
    }

    fn approver_signed(kind: &str, mut o: Value) -> Value {
        o["approver"] = key_id(&key().verifying_key().to_bytes()).into();
        let msg = format!("axidps-{kind}-v1\n{}", canonical(&o).unwrap());
        o["sig"] = STANDARD.encode(key().sign(msg.as_bytes()).to_bytes()).into();
        o
    }

    fn approval(v: &Value) -> Value {
        approver_signed(
            "approval",
            json!({"id": ID, "version": v["version"], "tree": v["tree"], "verified": true, "grant": []}),
        )
    }

    fn revoked(seq: u64, ids: Value) -> Value {
        approver_signed(
            "revoked",
            json!({"seq": seq, "versions": [], "verifications": [], "ids": ids, "passes": []}),
        )
    }

    /// Everything the store says until its jobs are all answered.
    fn settle(s: &mut Store) -> Vec<Event> {
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut out = Vec::new();
        loop {
            out.extend(s.poll());
            if s.pending.get() == 0 {
                return out;
            }
            assert!(Instant::now() < deadline, "store worker didn't answer");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn page(events: &[Event]) -> Value {
        let call = events
            .iter()
            .rev()
            .find_map(|e| match e {
                Event::Page(p) => Some(p),
                Event::Rescan => None,
            })
            .expect("a page call");
        let json = call
            .strip_prefix("window.axidps && window.axidps.store && window.axidps.store(")
            .and_then(|c| c.strip_suffix(')'))
            .unwrap();
        serde_json::from_str(json).unwrap()
    }

    fn rescans(events: &[Event]) -> usize {
        events.iter().filter(|e| matches!(e, Event::Rescan)).count()
    }

    fn ask(s: &mut Store, msg: Value) -> Vec<Event> {
        s.request(&msg);
        settle(s)
    }

    fn card(p: &Value) -> &Value {
        p["plugins"].as_array().unwrap().iter().find(|c| c["id"] == ID).expect("the card")
    }

    #[test]
    fn cache_used_when_offline() {
        let t = t();
        let v = t.publish("1.0.0", &[]);
        t.serve(vec![v.clone()], json!({"approvals": [approval(&v)]}));
        let mut s = t.store();
        let p = page(&ask(&mut s, json!("refresh")));
        assert_eq!(p["state"], "ok");
        assert_eq!(card(&p)["trust"], "verified");
        drop(s);

        // Restarted with no network: the cache lists it, and a fetch keeps it.
        t.net.clear();
        let mut s = t.store();
        assert_eq!(s.ctx(false).signed.approvals.len(), 1, "approvals from the cache");
        let p = page(&ask(&mut s, json!("refresh")));
        assert_eq!(p["state"], "offline");
        assert!(p["error"].is_string());
        assert!(p["fetched"].is_u64());
        assert_eq!(card(&p)["version"], "1.0.0");
        assert_eq!(card(&p)["trust"], "verified");
    }

    #[test]
    fn no_cache_reports_offline() {
        let t = t();
        let mut s = t.store();
        s.tick(Instant::now());
        let ev = settle(&mut s);
        let first = match &ev[0] {
            Event::Page(p) => p.clone(),
            Event::Rescan => panic!("no rescan"),
        };
        assert!(first.contains("\"loading\""), "{first}");
        let p = page(&ev);
        assert_eq!(p["state"], "offline");
        assert_eq!(p["fetched"], Value::Null);
        assert_eq!(p["plugins"], json!([]));
        assert_eq!(rescans(&ev), 0);
        // The 6-hour clock: no second fetch straight away.
        s.tick(Instant::now());
        assert!(s.poll().is_empty());
        s.tick(Instant::now() + FETCH_EVERY);
        assert!(!settle(&mut s).is_empty());
    }

    #[test]
    fn bad_signature_keeps_old_cache() {
        let t = t();
        let v = t.publish("1.0.0", &[]);
        t.serve(vec![v.clone()], json!({}));
        let mut s = t.store();
        ask(&mut s, json!("refresh"));
        let cached = std::fs::read(t.config.join("store/index.json")).unwrap();

        let v2 = t.publish("2.0.0", &[]);
        t.serve(vec![v, v2], json!({}));
        t.net.set(SIG_URL, STANDARD.encode([0u8; 64]).into_bytes());
        let p = page(&ask(&mut s, json!("refresh")));
        assert_eq!(p["state"], "error");
        assert!(p["error"].as_str().unwrap().contains("refused"), "{p}");
        assert_eq!(card(&p)["version"], "1.0.0");
        assert_eq!(std::fs::read(t.config.join("store/index.json")).unwrap(), cached);
    }

    #[test]
    fn stale_index_is_refused_whole() {
        let t = t();
        let v = t.publish("1.0.0", &[]);
        t.serve(vec![v.clone()], json!({"approvals": [approval(&v)], "revoked": revoked(5, json!([]))}));
        let mut s = t.store();
        ask(&mut s, json!("refresh"));
        assert_eq!(std::fs::read_to_string(t.config.join("store/revoked-seq")).unwrap(), "5");
        let cached = std::fs::read(t.config.join("store/index.json")).unwrap();

        // A replayed older copy (seq 3), and one with no revocation at all: both refused.
        for extra in [json!({"revoked": revoked(3, json!([]))}), json!({})] {
            let v2 = t.publish("2.0.0", &[]);
            t.serve(vec![v.clone(), v2], extra);
            let p = page(&ask(&mut s, json!("refresh")));
            assert_eq!(p["state"], "error", "{p}");
            assert_eq!(card(&p)["version"], "1.0.0");
            assert_eq!(card(&p)["trust"], "verified");
            assert_eq!(std::fs::read(t.config.join("store/index.json")).unwrap(), cached);
        }
    }

    #[test]
    fn revocation_triggers_rescan() {
        let t = t();
        let v = t.publish("1.0.0", &[]);
        t.serve(vec![v.clone()], json!({"approvals": [approval(&v)]}));
        let mut s = t.store();
        ask(&mut s, json!("refresh"));
        let ev = ask(&mut s, json!({"install": ID}));
        assert_eq!(rescans(&ev), 1, "an install changes files");
        assert_eq!(card(&page(&ev))["installed"], "1.0.0");
        assert!(s.ctx(false).store_ids.contains(ID));

        // The same index again: nothing changed, no rescan.
        assert_eq!(rescans(&ask(&mut s, json!("refresh"))), 0);

        t.serve(
            vec![v.clone()],
            json!({"approvals": [approval(&v)], "revoked": revoked(2, json!([{"id": ID, "reason": "malware"}]))}),
        );
        let ev = ask(&mut s, json!("refresh"));
        assert_eq!(rescans(&ev), 1);
        let ctx = s.ctx(false);
        let (list, _) = scan::scan_trusted(Path::new(""), &t.plugins, &ctx);
        assert_eq!(list[0].trust.level, Trust::Revoked);
        assert_eq!(std::fs::read_to_string(t.config.join("store/revoked-seq")).unwrap(), "2");
        // Still listed while installed, so it can be uninstalled.
        assert_eq!(card(&page(&ev))["installed"], "1.0.0");
        let ev = ask(&mut s, json!({"uninstall": ID}));
        assert_eq!(rescans(&ev), 1);
        assert!(!t.plugins.join(ID).exists());
        assert_eq!(page(&ev)["plugins"], json!([]), "pulled and not installed: not listed");
    }

    #[test]
    fn auto_update_respects_setting_and_rules() {
        let t = t();
        let v1 = t.publish("1.0.0", &[]);
        t.serve(vec![v1.clone()], json!({}));
        let mut s = t.store();
        ask(&mut s, json!("refresh"));
        ask(&mut s, json!({"install": ID}));

        // Turned off: 1.1.0 is offered as an update, not installed.
        t.saved(json!({"storeAutoUpdate": false}));
        let v11 = t.publish("1.1.0", &[]);
        t.serve(vec![v1.clone(), v11.clone()], json!({}));
        let ev = ask(&mut s, json!("refresh"));
        assert_eq!(t.record().plugins[ID].version, "1.0.0");
        assert_eq!(card(&page(&ev))["update"], "1.1.0");
        assert_eq!(rescans(&ev), 0);

        // On (the default): installed, and the page reloads it.
        t.saved(json!({}));
        let ev = ask(&mut s, json!("refresh"));
        assert_eq!(t.record().plugins[ID].version, "1.1.0");
        assert_eq!(rescans(&ev), 1);
        assert_eq!(card(&page(&ev))["installed"], "1.1.0");
        assert_eq!(card(&page(&ev))["update"], Value::Null);

        // 1.2.0 asks for a network host: the player must OK it.
        let v12 = t.publish("1.2.0", &["api.example.com"]);
        t.serve(vec![v1.clone(), v11.clone(), v12.clone()], json!({}));
        let ev = ask(&mut s, json!("refresh"));
        assert_eq!(t.record().plugins[ID].version, "1.1.0");
        assert_eq!(card(&page(&ev))["update"], "1.2.0");

        // 1.3.0 asks for nothing new but becomes Verified: a different trust level, so it asks.
        let v13 = t.publish("1.3.0", &[]);
        t.serve(vec![v1, v11, v12, v13.clone()], json!({"approvals": [approval(&v13)]}));
        ask(&mut s, json!("refresh"));
        assert_eq!(t.record().plugins[ID].version, "1.1.0");

        // The player's OK.
        let ev = ask(&mut s, json!({"update": ID}));
        assert_eq!(t.record().plugins[ID].version, "1.3.0");
        assert_eq!(rescans(&ev), 1);
    }

    #[test]
    fn page_json_marks_hand_installed() {
        let t = t();
        let mine = t.plugins.join(ID);
        std::fs::create_dir_all(&mine).unwrap();
        std::fs::write(mine.join("mine.txt"), "mine").unwrap();
        let v = t.publish("1.0.0", &[]);
        t.serve(vec![v], json!({}));
        let mut s = t.store();
        let p = page(&ask(&mut s, json!("refresh")));
        let c = card(&p);
        assert_eq!(c["handInstalled"], true);
        assert_eq!(c["installed"], Value::Null);
        assert_eq!(c["busy"], false);
        for f in ["name", "author", "description", "icon", "repo", "version", "awaiting", "permissions", "network"] {
            assert!(!c[f].is_null(), "{f}");
        }

        // Install is refused and the player's folder is left alone; busy while in flight.
        s.request(&json!({"install": ID}));
        let busy = page(&s.out.borrow_mut().drain(..).collect::<Vec<_>>());
        assert_eq!(card(&busy)["busy"], true);
        let p = page(&settle(&mut s));
        assert_eq!(card(&p)["busy"], false);
        assert_eq!(card(&p)["error"], install::IN_THE_WAY);
        assert_eq!(std::fs::read_to_string(mine.join("mine.txt")).unwrap(), "mine");
        assert!(!mine.join(axidps_plugins::MANIFEST_FILE).exists());
    }

    #[test]
    fn missing_folder_offers_reinstall() {
        let t = t();
        let v = t.publish("1.0.0", &[]);
        t.serve(vec![v], json!({}));
        let mut s = t.store();
        ask(&mut s, json!("refresh"));
        ask(&mut s, json!({"install": ID}));
        std::fs::remove_dir_all(t.plugins.join(ID)).unwrap();
        let p = page(&ask(&mut s, json!("open")));
        assert_eq!(card(&p)["installed"], Value::Null);
        assert_eq!(card(&p)["handInstalled"], false);
        let p = page(&ask(&mut s, json!({"install": ID})));
        assert_eq!(card(&p)["installed"], "1.0.0", "{p}");
        assert!(t.plugins.join(ID).join(axidps_plugins::MANIFEST_FILE).exists());
    }

    #[test]
    fn replaced_or_edited_store_folder_counts_as_hand_installed() {
        let t = t();
        let v1 = t.publish("1.0.0", &[]);
        t.serve(vec![v1.clone()], json!({}));
        let mut s = t.store();
        ask(&mut s, json!("refresh"));
        ask(&mut s, json!({"install": ID}));
        let dir = t.plugins.join(ID);

        // The store's folder deleted, the player's own dropped in its place, then a newer
        // version that would auto-update: the player's folder is left alone.
        std::fs::remove_dir_all(&dir).unwrap();
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(dir.join("mine.txt"), "mine").unwrap();
        let v11 = t.publish("1.1.0", &[]);
        t.serve(vec![v1.clone(), v11.clone()], json!({}));
        let p = page(&ask(&mut s, json!("refresh")));
        assert_eq!(card(&p)["handInstalled"], true, "{p}");
        assert_eq!(card(&p)["installed"], Value::Null);
        assert_eq!(card(&p)["update"], Value::Null);
        assert!(!s.ctx(false).store_ids.contains(ID));
        for msg in [json!({"update": ID}), json!({"uninstall": ID})] {
            let p = page(&ask(&mut s, msg));
            assert_eq!(card(&p)["error"], install::IN_THE_WAY);
        }
        assert_eq!(std::fs::read_to_string(dir.join("mine.txt")).unwrap(), "mine");
        assert!(!dir.join(axidps_plugins::MANIFEST_FILE).exists());

        // A store install with one byte edited is the player's too: never auto-updated.
        std::fs::remove_dir_all(&dir).unwrap();
        t.serve(vec![v1.clone()], json!({}));
        ask(&mut s, json!("refresh"));
        ask(&mut s, json!({"install": ID}));
        assert_eq!(t.record().plugins[ID].version, "1.0.0");
        let html = dir.join("card.html");
        let mut bytes = std::fs::read(&html).unwrap();
        bytes[0] ^= 1;
        std::fs::write(&html, &bytes).unwrap();
        t.serve(vec![v1, v11], json!({}));
        let p = page(&ask(&mut s, json!("refresh")));
        assert_eq!(card(&p)["handInstalled"], true);
        assert_eq!(std::fs::read(&html).unwrap(), bytes);
        assert_eq!(t.record().plugins[ID].version, "1.0.0");
    }

    #[test]
    fn unsupported_api_and_foreign_urls_are_refused() {
        let t = t();
        let mut v = t.publish("1.0.0", &[]);
        v["api"] = (SUPPORTED_API + 1).into();
        t.serve(vec![v.clone()], json!({}));
        let mut s = t.store();
        let p = page(&ask(&mut s, json!("refresh")));
        assert_eq!(card(&p)["error"], NEEDS_NEWER);
        let p = page(&ask(&mut s, json!({"install": ID})));
        assert_eq!(card(&p)["error"], NEEDS_NEWER);
        assert!(!t.plugins.join(ID).exists());

        // A download somewhere other than the registry's releases is never fetched.
        let mut v = t.publish("1.0.0", &[]);
        let zip = t.net.0.lock().unwrap()[v["url"].as_str().unwrap()].clone();
        v["url"] = "https://example.com/package.zip".into();
        t.net.set("https://example.com/package.zip", zip);
        t.serve(vec![v], json!({}));
        ask(&mut s, json!("refresh"));
        let p = page(&ask(&mut s, json!({"install": ID})));
        assert!(card(&p)["error"].as_str().unwrap().contains("isn't on the plugin store"), "{p}");
        assert!(!t.plugins.join(ID).exists());

        for (url, ok) in [
            (format!("{PACKAGE_PREFIX}a-1.0.0/p.zip"), true),
            (format!("{PACKAGE_PREFIX}../../evil/p.zip"), false),
            (format!("{PACKAGE_PREFIX}a/p.zip?x=1"), false),
            ("https://github.com/darkharasho/axidps-plugins.evil/releases/download/a".into(), false),
            (PACKAGE_PREFIX.into(), false),
        ] {
            assert_eq!(registry_url(&url), ok, "{url}");
        }
    }

    #[test]
    fn open_fetches_only_a_stale_cache() {
        let t = t();
        t.serve(vec![t.publish("1.0.0", &[])], json!({}));
        let mut s = t.store();
        ask(&mut s, json!("refresh"));
        t.net.clear();
        // Fresh: shown from the cache, no fetch (which would now say offline).
        assert_eq!(page(&ask(&mut s, json!("open")))["state"], "ok");
        s.last.borrow_mut()["fetched"] = (now_ms() - STALE_MS - 1).into();
        assert_eq!(page(&ask(&mut s, json!("open")))["state"], "offline");
    }

    #[test]
    fn dates_are_utc_civil() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_782), (2024, 2, 29));
        assert_eq!(civil_from_days(20_783), (2026, 11, 26));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
        assert_eq!(today().len(), 10);
    }
}
