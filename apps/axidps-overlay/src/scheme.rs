//! `axidps-plugin://<id>/<path>`: serves plugin files to the page's frames, with a
//! Content-Security-Policy, and the host's network backstop.
//!
//! `_sdk/<p>` comes from the overlay's `sdk/` folder; anything else from that plugin's folder.
//! An unknown id, a plugin the player hasn't enabled, or a path that does not resolve is a 404. Every HTML 200 carries the plugin's
//! CSP (`axidps_plugins::serve::csp`) as an injected `<meta>`: at Servo `75d20deef` a CSP
//! *header* on a framed document panics the script thread whenever an ancestor has an opaque
//! origin (`components/script/dom/security/csp.rs:225`, `Url::parse("null")`), and the overlay
//! page's origin is opaque.
//!
//! A meta policy does not survive a frame navigating itself away, so `PluginState::guard` (called
//! from both `load_web_resource` delegates) also refuses every http(s) load whose host no enabled
//! plugin declared in `network`, and every `file:` load outside the overlay's page and SDK folders
//! (a `file://host/` URL is a UNC path on Windows: SMB or WebDAV to any server).

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex, RwLock};

use axidps_plugins::manifest::Manifest;
use axidps_plugins::scan::{Plugin, enabled};
use axidps_plugins::serve;
use content_security_policy::Destination;
use http::HeaderValue;
use http::header::CONTENT_TYPE;
use servo::protocol_handler::{
    DoneChannel, FetchContext, HttpStatus, ProtocolHandler, Request, ResourceFetchTiming,
    Response, ResponseBody,
};
use servo::{WebResourceLoad, WebResourceResponse};

pub const SCHEME: &str = "axidps-plugin";

/// What the host knows about the plugins on disk and which of them the player has on: shared by
/// the scheme handler and the `load_web_resource` backstop, recomputed on every rescan and every
/// save of the page's settings.
#[derive(Default)]
pub struct PluginState {
    /// The last scan.
    plugins: RwLock<Vec<Plugin>>,
    /// The saved `plugins` object (`{<id>: {enabled, granted}}`) from ui.json or the page's latest
    /// save.
    saved: RwLock<serde_json::Value>,
    /// Every *enabled* plugin (`scan::enabled`, the page's own rule): id to its folder and
    /// manifest. A plugin the player never enabled is neither served nor reachable, so a frame
    /// can't navigate itself into it (and get its CSP and network).
    map: RwLock<HashMap<String, (PathBuf, Manifest)>>,
    /// The `network` entries of those plugins, recomputed with the map.
    hosts: RwLock<Vec<String>>,
    /// The folders `file:` loads may come from (the overlay's page and SDK), each as a directory
    /// URL to match lexically and as a canonical path.
    files: RwLock<Vec<(url::Url, PathBuf)>>,
    blocked: Mutex<Blocked>,
}

/// How many refused hosts are remembered (each logged once); past that, refusals are logged
/// through a rate limit, so a plugin fetching random hosts can neither grow memory nor flood the
/// log.
const MAX_BLOCKED: usize = 256;

struct Blocked {
    seen: HashSet<String>,
    gate: axidps_plugins::logline::Gate,
}

impl Default for Blocked {
    fn default() -> Self {
        Blocked {
            seen: HashSet::new(),
            gate: axidps_plugins::logline::Gate::named(5, "blocked"),
        }
    }
}

impl PluginState {
    /// A new scan.
    pub fn set(&self, plugins: &[Plugin]) {
        *self.plugins.write().unwrap() = plugins.to_vec();
        self.recompute();
    }

    /// The saved `plugins` object changed (ui.json at start, or a `{"save": ...}` from the page).
    pub fn set_saved(&self, saved: serde_json::Value) {
        *self.saved.write().unwrap() = saved;
        self.recompute();
    }

    /// The folders `file:` loads may come from. A folder that doesn't exist is left out.
    pub fn set_file_roots(&self, dirs: &[PathBuf]) {
        let mut roots = Vec::new();
        for d in dirs {
            let Ok(canon) = d.canonicalize() else { continue };
            for lexical in [d.as_path(), canon.as_path()] {
                if let Ok(u) = url::Url::from_directory_path(lexical) {
                    roots.push((u, canon.clone()));
                }
            }
        }
        *self.files.write().unwrap() = roots;
    }

    /// Whether a `file:` load of `url` stays inside the overlay's own folders. Checked by the URL
    /// first, so nothing outside them is ever looked up on disk: a host (on Windows a UNC path,
    /// i.e. SMB or WebDAV to any server) is refused outright.
    fn file_allowed(&self, url: &url::Url) -> bool {
        if url
            .host_str()
            .is_some_and(|h| !h.is_empty() && !h.eq_ignore_ascii_case("localhost"))
        {
            return false;
        }
        let path = url.path();
        let roots = self.files.read().unwrap();
        if path.to_ascii_lowercase().contains("%5c")
            || !roots.iter().any(|(u, _)| path.starts_with(u.path()))
        {
            return false;
        }
        let Ok(file) = url.to_file_path() else {
            return false;
        };
        // Inside a root by name; a symlink there must not lead out. A file that isn't there
        // (the page's optional version.js) fails to load by itself.
        match file.canonicalize() {
            Ok(file) => roots.iter().any(|(_, c)| file.starts_with(c)),
            Err(_) => true,
        }
    }

    fn recompute(&self) {
        let map: HashMap<_, _> = {
            let plugins = self.plugins.read().unwrap();
            let saved = self.saved.read().unwrap();
            plugins
                .iter()
                .filter(|p| enabled(p, &saved))
                .filter_map(|p| {
                    p.manifest
                        .clone()
                        .map(|m| (m.id.clone(), (p.dir.clone(), m)))
                })
                .collect()
        };
        let mut hosts: Vec<String> = map
            .values()
            .flat_map(|(_, m)| m.network.iter().cloned())
            .collect();
        hosts.sort();
        hosts.dedup();
        *self.hosts.write().unwrap() = hosts;
        *self.map.write().unwrap() = map;
    }

    fn host_allowed(&self, host: &str) -> bool {
        self.hosts
            .read()
            .unwrap()
            .iter()
            .any(|e| serve::host_matches(e, host))
    }

    /// What to refuse (and log) for a load of `url`, or `None` to let it through: an http(s)
    /// load to a host no enabled plugin declared, over plain http, or to a port other than 443; a `file:` load from
    /// anywhere but the overlay's page and SDK folders. Every other scheme (`axidps-plugin://`,
    /// `data:`, `blob:`, ...) passes.
    fn refuse(&self, url: &url::Url) -> Option<String> {
        // Servo hands this delegate every load, `file:` and the plugin scheme included.
        if url.scheme() == "file" {
            return match self.file_allowed(url) {
                true => None,
                false => Some(match url.host_str() {
                    Some(h) if !h.is_empty() => format!("file://{h}"),
                    _ => "file: outside the overlay".to_owned(),
                }),
            };
        }
        if !matches!(url.scheme(), "http" | "https") {
            return None;
        }
        let host = url.host_str().unwrap_or("").to_owned();
        // HTTPS on its default port only (`port()` is `None` for 443): a declared host's other
        // ports are other services. A WebSocket arrives here rewritten to https.
        if url.scheme() == "https" && url.port().is_none() && self.host_allowed(&host) {
            return None;
        }
        Some(match url.port() {
            Some(p) => format!("{host}:{p}"),
            None => host,
        })
    }

    /// `refuse` for one hop of a load. Servo shows this delegate only a load's first URL, on every
    /// hop of a redirect chain, so a declared host could otherwise send a plugin anywhere: an
    /// http(s) load that has been redirected is refused outright, with its first host in the log.
    /// An http(s) page load (a frame navigating, or an `<embed>`/`<object>`) is refused too: the
    /// host list is every enabled plugin's hosts together, so a plugin could otherwise open a page
    /// another plugin declared, and that page would then run inside the overlay. Plugins only
    /// `fetch` from their hosts, which their own CSP limits.
    fn refuse_load(&self, url: &url::Url, is_redirect: bool, dest: Destination) -> Option<String> {
        if matches!(url.scheme(), "http" | "https") {
            let host = url.host_str().unwrap_or("");
            if is_redirect {
                return Some(format!("redirect from {host}"));
            }
            if builtin_allowed(url, dest) {
                return None;
            }
            if matches!(
                dest,
                Destination::Document
                    | Destination::IFrame
                    | Destination::Frame
                    | Destination::Embed
                    | Destination::Object
            ) {
                return Some(format!("page {host}"));
            }
        }
        self.refuse(url)
    }

    /// The log lines for refusing `what` at `now`: `blocked <what>` the first time, for the first
    /// `MAX_BLOCKED` of them; past that, every refusal, at most 5 a second.
    fn blocked_lines(&self, what: &str, now: std::time::Instant) -> Vec<String> {
        let mut b = self.blocked.lock().unwrap();
        let line = format!("blocked {}", axidps_plugins::logline::clean(what));
        if b.seen.contains(what) {
            return Vec::new();
        }
        if b.seen.len() < MAX_BLOCKED {
            b.seen.insert(what.to_owned());
            return vec![line];
        }
        let (pass, dropped) = b.gate.pass(now);
        dropped.into_iter().chain(pass.then_some(line)).collect()
    }

    /// The network backstop: a refused load (`refuse`) gets an empty 403, logged once per host
    /// (`blocked_lines`).
    pub fn guard(&self, load: WebResourceLoad) {
        let url = load.request.url.as_url();
        let Some(host) = self.refuse_load(url, load.request.is_redirect, load.request.destination) else {
            return; // dropped without intercept: the load continues as normal
        };
        for line in self.blocked_lines(&host, std::time::Instant::now()) {
            println!("{line}");
        }
        let resp = WebResourceResponse::new(url.clone())
            .status_code(http::StatusCode::FORBIDDEN)
            .status_message(b"Forbidden".to_vec());
        // Dropping the returned load finishes it with an empty body.
        let _ = load.intercept(resp);
    }
}

/// The overlay's own loads from ArenaNet's public API: a skill's icon address (a `fetch` from
/// `api.guildwars2.com`) and the icon (an image from `render.guildwars2.com`), for the Details
/// window. HTTPS on the default port, nothing else from either host.
const BUILTIN: &[(&str, Destination)] = &[
    ("api.guildwars2.com", Destination::None),
    ("render.guildwars2.com", Destination::Image),
];

fn builtin_allowed(url: &url::Url, dest: Destination) -> bool {
    url.scheme() == "https"
        && url.port().is_none()
        && BUILTIN
            .iter()
            .any(|&(h, d)| d == dest && url.host_str() == Some(h))
}

pub struct PluginScheme {
    state: Arc<PluginState>,
    /// `<overlay>/sdk`.
    sdk: PathBuf,
}

impl PluginScheme {
    pub fn new(state: Arc<PluginState>, sdk: PathBuf) -> Self {
        PluginScheme { state, sdk }
    }

    /// `(file, csp)` for a request: `csp` is that plugin's policy, used for HTML only.
    fn find(&self, id: &str, path: &str) -> Option<(PathBuf, String)> {
        let rel = path.strip_prefix('/')?;
        let map = self.state.map.read().unwrap();
        let (dir, manifest) = map.get(id)?;
        let file = match rel.strip_prefix("_sdk/") {
            Some(p) => serve::resolve(&self.sdk, p)?,
            None => serve::resolve(dir, rel)?,
        };
        Some((file, serve::csp(manifest)))
    }
}

impl ProtocolHandler for PluginScheme {
    fn load<'a>(
        &'a self,
        request: &'a mut Request,
        _done_chan: &mut DoneChannel,
        _context: &FetchContext,
    ) -> Pin<Box<dyn Future<Output = Response> + Send + 'a>> {
        let url = request.current_url();
        let mut r = Response::new(url.clone(), ResourceFetchTiming::new(request.timing_type()));
        let found = self
            .find(url.host_str().unwrap_or(""), url.path())
            .and_then(|(f, csp)| Some((std::fs::read(&f).ok()?, f, csp)));
        match found {
            Some((body, file, csp)) => {
                let mime = serve::mime(&file);
                r.headers
                    .insert(CONTENT_TYPE, HeaderValue::from_static(mime));
                let body = if mime.starts_with("text/html") {
                    serve::with_csp_meta(body, &csp)
                } else {
                    body
                };
                *r.body.lock() = ResponseBody::Done(body);
                r.status = HttpStatus::default();
            }
            None => {
                println!("plugin scheme: not found: {url}");
                *r.body.lock() = ResponseBody::Done(Vec::new());
                r.status = HttpStatus::new_raw(404, b"Not Found".to_vec());
            }
        }
        Box::pin(std::future::ready(r))
    }

    /// Local files, like `file:`. Without this Servo blocks the page's frames as mixed content
    /// ("Blocked as mixed content": a frame's URL must be potentially trustworthy).
    fn is_secure(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;

    /// A bundled plugin `axidps.b` and two player plugins, `p.on` (declares on.example.com) and
    /// `p.off` (declares off.example.com), each with an index.html; the SDK folder with axidps.js.
    fn setup() -> (tempfile::TempDir, PluginScheme) {
        let t = tempfile::tempdir().unwrap();
        let mk = |dir: &str, id: &str, network: &str| {
            let d = t.path().join(dir).join(id);
            fs::create_dir_all(&d).unwrap();
            fs::write(
                d.join(axidps_plugins::MANIFEST_FILE),
                format!(
                    r#"{{"id":"{id}","name":"n","version":"1.0.0","api":1,"permissions":["fight"],
                    "network":[{network}],"windows":[{{"id":"w","title":"t","entry":"index.html"}}]}}"#
                ),
            )
            .unwrap();
            fs::write(d.join("index.html"), "<p>").unwrap();
        };
        mk("bundled", "axidps.b", "");
        mk("player", "p.on", r#""on.example.com""#);
        mk("player", "p.off", r#""off.example.com""#);
        let sdk = t.path().join("sdk");
        fs::create_dir_all(&sdk).unwrap();
        fs::write(sdk.join("axidps.js"), "1").unwrap();
        let state = Arc::new(PluginState::default());
        state.set(&axidps_plugins::scan::scan(
            &t.path().join("bundled"),
            &t.path().join("player"),
        ));
        let scheme = PluginScheme::new(state, sdk);
        (t, scheme)
    }

    #[test]
    fn guard_keeps_file_loads_inside_the_overlay() {
        let t = tempfile::tempdir().unwrap();
        let page = t.path().join("overlay/page");
        let sdk = t.path().join("overlay/sdk");
        fs::create_dir_all(&page).unwrap();
        fs::create_dir_all(&sdk).unwrap();
        fs::write(page.join("index.html"), "<p>").unwrap();
        fs::write(sdk.join("axidps.js"), "1").unwrap();
        fs::write(t.path().join("overlay/secret"), "s").unwrap();
        let st = PluginState::default();
        let file = |p: &std::path::Path| url::Url::from_file_path(p).unwrap();
        // No folders set: every file: load is refused.
        assert!(st.refuse(&file(&page.join("index.html"))).is_some());
        st.set_file_roots(&[page.clone(), page.join("../sdk"), t.path().join("missing")]);
        assert_eq!(st.refuse(&file(&page.join("index.html"))), None);
        let mut q = file(&page.join("index.html"));
        q.set_query(Some("test"));
        q.set_fragment(Some("t=00"));
        assert_eq!(st.refuse(&q), None, "the page's own URL, query and fragment");
        assert_eq!(st.refuse(&file(&page.join("index.html")).join("../sdk/axidps.js").unwrap()), None);
        assert_eq!(st.refuse(&file(&page.join("index.html")).join("version.js").unwrap()), None,
            "a missing file inside the page folder fails to load by itself");
        for bad in [
            file(&t.path().join("overlay/secret")),
            url("file:///etc/hostname"),
            file(&page.join("index.html")).join("../secret").unwrap(),
            url(&format!("{}/..%5Csecret", file(&page).as_str())),
            url(&format!("{}/%2e%2e/secret", file(&page).as_str())),
        ] {
            assert_eq!(st.refuse(&bad).as_deref(), Some("file: outside the overlay"), "{bad}");
        }
        // A host: on Windows a UNC path (SMB/WebDAV), refused without looking.
        let unc = url(&format!("file://attacker.example{}", page.join("index.html").display()));
        assert_eq!(st.refuse(&unc).as_deref(), Some("file://attacker.example"));
        assert_eq!(st.refuse(&url("file://attacker.example/s/x.png")).as_deref(), Some("file://attacker.example"));
        let local = url(&format!("file://localhost{}", page.join("index.html").display()));
        assert_eq!(st.refuse(&local), None, "localhost is the local machine");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(t.path().join("overlay/secret"), page.join("leak")).unwrap();
            assert!(st.refuse(&file(&page.join("leak"))).is_some(), "a symlink out of the folder");
        }
    }

    #[test]
    fn blocked_hosts_are_capped_and_rate_limited() {
        let st = PluginState::default();
        let t0 = std::time::Instant::now();
        for i in 0..MAX_BLOCKED {
            assert_eq!(st.blocked_lines(&format!("h{i}.example"), t0), [format!("blocked h{i}.example")]);
        }
        assert!(st.blocked_lines("h0.example", t0).is_empty(), "once per host");
        let past: Vec<String> = (0..100)
            .flat_map(|i| st.blocked_lines(&format!("x{i}.example"), t0))
            .collect();
        assert_eq!(past.len(), 5, "{past:?}");
        assert_eq!(st.blocked.lock().unwrap().seen.len(), MAX_BLOCKED);
        let later = st.blocked_lines("y.example", t0 + std::time::Duration::from_secs(2));
        assert_eq!(later, ["… 95 blocked lines dropped", "blocked y.example"]);
        assert_eq!(st.blocked_lines("a\nb", t0 + std::time::Duration::from_secs(2)), ["blocked a b"]);
    }

    fn on() -> serde_json::Value {
        json!({"p.on": {"enabled": true, "granted": ["fight", "net:on.example.com"]},
               "p.off": {"enabled": false, "granted": ["fight", "net:off.example.com"]}})
    }

    fn url(s: &str) -> url::Url {
        url::Url::parse(s).unwrap()
    }

    #[test]
    fn find_serves_enabled_plugins_only() {
        let (_t, s) = setup();
        // Nothing saved yet: bundled plugins only.
        assert!(s.find("axidps.b", "/index.html").is_some());
        assert!(s.find("p.on", "/index.html").is_none());
        assert!(s.find("p.off", "/index.html").is_none());
        s.state.set_saved(on());
        assert!(s.find("axidps.b", "/index.html").is_some());
        let (file, csp) = s.find("p.on", "/index.html").unwrap();
        assert!(file.ends_with("p.on/index.html"));
        assert!(csp.contains("connect-src https://on.example.com"), "{csp}");
        assert!(s.find("p.on", "/_sdk/axidps.js").is_some());
        assert!(s.find("p.off", "/index.html").is_none(), "installed, never enabled");
        assert!(s.find("p.off", "/_sdk/axidps.js").is_none());
        assert!(s.find("p.on", "/missing.html").is_none());
        assert!(s.find("p.on", "/../p.off/index.html").is_none());
        // Switched off again (the page saves before it asks for a rescan).
        s.state
            .set_saved(json!({"p.on": {"enabled": false, "granted": []}, "axidps.b": {"enabled": false}}));
        assert!(s.find("p.on", "/index.html").is_none());
        assert!(s.find("axidps.b", "/index.html").is_none());
    }

    #[test]
    fn find_drops_a_plugin_whose_manifest_asks_for_more() {
        let (t, s) = setup();
        s.state.set_saved(on());
        assert!(s.find("p.on", "/index.html").is_some());
        let m = t.path().join("player/p.on").join(axidps_plugins::MANIFEST_FILE);
        let more = fs::read_to_string(&m)
            .unwrap()
            .replace(r#""on.example.com""#, r#""on.example.com", "more.example.com""#);
        fs::write(&m, more).unwrap();
        s.state.set(&axidps_plugins::scan::scan(
            &t.path().join("bundled"),
            &t.path().join("player"),
        ));
        assert!(s.find("p.on", "/index.html").is_none());
        assert!(s.state.refuse(&url("https://more.example.com/")).is_some());
        assert!(s.state.refuse(&url("https://on.example.com/")).is_some());
    }

    #[test]
    fn guard_allows_enabled_plugins_hosts_only() {
        let (_t, s) = setup();
        let st = &s.state;
        assert_eq!(st.refuse(&url("https://on.example.com/x")).as_deref(), Some("on.example.com"));
        st.set_saved(on());
        assert_eq!(st.refuse(&url("https://on.example.com/x")), None);
        assert_eq!(
            st.refuse(&url("https://off.example.com/x")).as_deref(),
            Some("off.example.com"),
            "declared only by a plugin that is not enabled"
        );
        assert_eq!(st.refuse(&url("http://on.example.com/x")).as_deref(), Some("on.example.com"));
        assert_eq!(st.refuse(&url("https://on.example.com:443/x")), None, "the default port");
        assert_eq!(
            st.refuse(&url("https://on.example.com:8443/x")).as_deref(),
            Some("on.example.com:8443"),
            "another port is another service"
        );
        assert_eq!(st.refuse(&url("https://on.example.com:80/x")).as_deref(), Some("on.example.com:80"));
        assert_eq!(st.refuse(&url("https://evil.example/")).as_deref(), Some("evil.example"));
        assert_eq!(st.refuse(&url("axidps-plugin://p.off/index.html")), None);
        st.set_saved(serde_json::Value::Null);
        assert!(st.refuse(&url("https://on.example.com/x")).is_some());
    }

    #[test]
    fn guard_allows_arenanets_api_and_icons_only() {
        let (_t, s) = setup();
        let st = &s.state;
        let api = url("https://api.guildwars2.com/v2/skills?ids=1,2");
        let icon = url("https://render.guildwars2.com/file/ABC/1.png");
        assert_eq!(st.refuse_load(&api, false, Destination::None), None);
        assert_eq!(st.refuse_load(&icon, false, Destination::Image), None);
        // Each host for its own kind of load only, over HTTPS, never as a page or redirected.
        assert!(st.refuse_load(&api, false, Destination::Image).is_some());
        assert!(st.refuse_load(&icon, false, Destination::None).is_some());
        assert!(st.refuse_load(&icon, false, Destination::Document).is_some());
        assert!(st.refuse_load(&api, true, Destination::None).is_some());
        assert!(st.refuse_load(&url("http://render.guildwars2.com/x.png"), false, Destination::Image).is_some());
        assert!(st.refuse_load(&url("https://render.guildwars2.com:8443/x.png"), false, Destination::Image).is_some());
        assert!(st.refuse_load(&url("https://wiki.guildwars2.com/x.png"), false, Destination::Image).is_some());
    }

    #[test]
    fn guard_refuses_redirected_loads() {
        let (_t, s) = setup();
        let st = &s.state;
        st.set_saved(on());
        let fetch = Destination::None;
        assert_eq!(st.refuse_load(&url("https://on.example.com/x"), false, fetch), None);
        assert_eq!(
            st.refuse_load(&url("https://on.example.com/x"), true, fetch).as_deref(),
            Some("redirect from on.example.com"),
            "Servo reports the first URL on every hop, so the target is unknown"
        );
        assert_eq!(st.refuse_load(&url("axidps-plugin://p.on/index.html"), true, fetch), None);
    }

    #[test]
    fn guard_refuses_page_loads_from_declared_hosts() {
        let (_t, s) = setup();
        let st = &s.state;
        st.set_saved(on());
        for dest in [
            Destination::Document,
            Destination::IFrame,
            Destination::Frame,
            Destination::Embed,
            Destination::Object,
        ] {
            assert_eq!(
                st.refuse_load(&url("https://on.example.com/"), false, dest).as_deref(),
                Some("page on.example.com"),
                "{dest:?}"
            );
        }
        for dest in [Destination::None, Destination::Image, Destination::Script, Destination::Style] {
            assert_eq!(st.refuse_load(&url("https://on.example.com/x"), false, dest), None, "{dest:?}");
        }
        let own = url("axidps-plugin://p.on/index.html");
        assert_eq!(st.refuse_load(&own, false, Destination::IFrame), None, "a plugin's own window");
        assert_eq!(st.refuse_load(&url("data:text/html,x"), false, Destination::Document), None);
    }
}
