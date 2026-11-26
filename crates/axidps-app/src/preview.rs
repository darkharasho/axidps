//! The browser preview server behind `axidps plugin preview`: a std-only HTTP/1.1 server on
//! 127.0.0.1, one thread per connection, GET only. It serves the overlay page, the SDK and
//! plugin folders the way the overlay host does, plus a change feed for reload on save.

use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axidps_plugins::manifest::Manifest;
use axidps_plugins::scan::{self, Plugin};
use axidps_plugins::serve::{csp, mime, resolve};
use axidps_plugins::watch::{Snapshot, Watcher};

/// The longest request head accepted.
const MAX_HEAD: usize = 8 * 1024;
/// How long a client may take to send its request head.
const HEAD_TIMEOUT: Duration = Duration::from_secs(10);
const POLL: Duration = Duration::from_millis(250);
const KEEPALIVE: Duration = Duration::from_secs(15);

pub struct Config {
    /// The overlay page folder (`index.html`, `fonts/`).
    pub page: PathBuf,
    pub sdk: PathBuf,
    /// The bundled plugins folder (a folder of plugin folders).
    pub bundled: PathBuf,
    /// The plugin folder being developed.
    pub folder: Option<PathBuf>,
}

pub struct Server {
    pub port: u16,
    /// What the plugin scan said at start (ignored duplicates and the like).
    pub log: Vec<String>,
}

type Subscribers = Arc<Mutex<Vec<Sender<String>>>>;

struct State {
    page: PathBuf,
    sdk: PathBuf,
    bundled: PathBuf,
    /// The canonical plugin folder, and its parent (the "player" folder the scan reads).
    mine: Option<(PathBuf, PathBuf)>,
    subscribers: Subscribers,
    port: u16,
}

impl State {
    /// The bundled plugins plus the plugin folder under development, and which one that is.
    fn plugins(&self) -> (Vec<Plugin>, Vec<String>) {
        let Some((dir, parent)) = &self.mine else {
            return scan::scan_logged(&self.bundled, Path::new(""));
        };
        let (mut all, log) = scan::scan_logged(&self.bundled, parent);
        all.retain(|p| p.bundled || p.dir == *dir);
        (all, log)
    }

    fn is_mine(&self, p: &Plugin) -> bool {
        !p.bundled && self.mine.as_ref().is_some_and(|(dir, _)| p.dir == *dir)
    }

    fn snapshot(&self) -> Snapshot {
        let Some((dir, parent)) = &self.mine else {
            return Snapshot::default();
        };
        let mut snap = Snapshot::take(parent);
        let name = dir.file_name().map(|n| n.to_string_lossy().into_owned());
        snap.plugins
            .retain(|folder, _| Some(folder) == name.as_ref());
        snap
    }
}

/// Binds 127.0.0.1:`port` (0 picks one) and serves in background threads.
pub fn start(config: Config, port: u16) -> io::Result<Server> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port))?;
    let port = listener.local_addr()?.port();
    let mine = match &config.folder {
        Some(f) => {
            let dir = f.canonicalize()?;
            let parent = dir
                .parent()
                .map(Path::to_path_buf)
                .ok_or_else(|| io::Error::other("the plugin folder has no parent folder"))?;
            Some((dir, parent))
        }
        None => None,
    };
    let state = Arc::new(State {
        page: config.page,
        sdk: config.sdk,
        bundled: config.bundled,
        mine,
        subscribers: Arc::default(),
        port,
    });
    let (_, log) = state.plugins();

    // The baseline is taken before any client can connect, so a save is never missed.
    let mut watcher = Watcher::new();
    watcher.tick(Instant::now(), state.snapshot());
    let poll_state = Arc::clone(&state);
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(POLL);
            for id in watcher.tick(Instant::now(), poll_state.snapshot()) {
                poll_state
                    .subscribers
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .retain(|s| s.send(id.clone()).is_ok());
            }
        }
    });
    std::thread::spawn(move || {
        for stream in listener.incoming().filter_map(Result::ok) {
            let state = Arc::clone(&state);
            std::thread::spawn(move || {
                let _ = handle(stream, &state);
            });
        }
    });
    Ok(Server { port, log })
}

enum Reply {
    Status(u16, &'static str),
    File { path: PathBuf, csp: Option<String> },
    Json(String),
    Events,
}

fn handle(mut stream: TcpStream, state: &State) -> io::Result<()> {
    stream.set_read_timeout(Some(HEAD_TIMEOUT))?;
    stream.set_write_timeout(Some(HEAD_TIMEOUT))?;
    let Some(head) = read_head(&mut stream)? else {
        return respond(&mut stream, 400, "text/plain", &[], b"bad request");
    };
    let mut lines = head.split("\r\n");
    let mut request = lines.next().unwrap_or("").split(' ');
    let (method, target) = (request.next().unwrap_or(""), request.next().unwrap_or(""));
    if method != "GET" {
        return respond(
            &mut stream,
            405,
            "text/plain",
            &[("Allow", "GET")],
            b"GET only",
        );
    }
    // A page on another site must not read this server through a rebound name. Only this
    // server's own loopback address and port pass, and only the re-serialised value is used.
    let host = lines
        .filter_map(|l| l.split_once(':'))
        .find(|(k, _)| k.eq_ignore_ascii_case("host"))
        .and_then(|(_, v)| own_host(v.trim(), state.port));
    let Some(host) = host else {
        return respond(&mut stream, 400, "text/plain", &[], b"bad host");
    };
    let path = target.split(['?', '#']).next().unwrap_or("");
    match route(state, path, &host) {
        Reply::Status(code, text) => respond(&mut stream, code, "text/plain", &[], text.as_bytes()),
        Reply::Json(body) => respond(&mut stream, 200, "application/json", &[], body.as_bytes()),
        Reply::File { path, csp } => match std::fs::read(&path) {
            Ok(body) => {
                let headers: Vec<(&str, &str)> = csp
                    .as_deref()
                    .map(|c| ("Content-Security-Policy", c))
                    .into_iter()
                    .collect();
                respond(&mut stream, 200, mime(&path), &headers, &body)
            }
            Err(_) => respond(&mut stream, 404, "text/plain", &[], b"not found"),
        },
        Reply::Events => events(stream, state),
    }
}

/// The request head, or None when it is too long or not UTF-8.
fn read_head(stream: &mut TcpStream) -> io::Result<Option<String>> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
        if buf.len() > MAX_HEAD {
            return Ok(None);
        }
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            return Ok(None);
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    Ok(String::from_utf8(buf).ok())
}

fn route(state: &State, path: &str, host: &str) -> Reply {
    let file = |root: &Path, rel: &str, csp: Option<String>| match resolve(root, rel) {
        Some(path) => Reply::File { path, csp },
        None => Reply::Status(404, "not found"),
    };
    match path {
        "/plugins.json" => {
            let (plugins, _) = state.plugins();
            let mut json = scan::to_page_json(&plugins);
            if let Some(list) = json.as_array_mut() {
                for (entry, plugin) in list.iter_mut().zip(&plugins) {
                    if state.is_mine(plugin) {
                        entry["preview"] = true.into();
                    }
                }
            }
            Reply::Json(json.to_string())
        }
        "/_events" => Reply::Events,
        "/" => file(&state.page, "index.html", None),
        _ => {
            let Some(rest) = path.strip_prefix('/') else {
                return Reply::Status(404, "not found");
            };
            if let Some(f) = rest.strip_prefix("_sdk/fonts/") {
                file(&state.page.join("fonts"), f, None)
            } else if let Some(f) = rest.strip_prefix("_sdk/").or_else(|| rest.strip_prefix("sdk/")) {
                // `/sdk/`: the demo page's own `../sdk/axidps.js` (sdk/ beside page/ when installed).
                file(&state.sdk, f, None)
            } else if let Some(rest) = rest.strip_prefix("plugins/") {
                let Some((id, rel)) = rest.split_once('/') else {
                    return Reply::Status(404, "not found");
                };
                let (plugins, _) = state.plugins();
                let found = plugins
                    .iter()
                    .find(|p| scan::loadable(p) && p.manifest.as_ref().is_some_and(|m| m.id == id));
                let Some((plugin, manifest)) =
                    found.and_then(|p| p.manifest.as_ref().map(|m| (p, m)))
                else {
                    return Reply::Status(404, "not found");
                };
                if let Some(f) = rel.strip_prefix("_sdk/") {
                    file(&state.sdk, f, None)
                } else {
                    file(&plugin.dir, rel, Some(preview_csp(manifest, host)))
                }
            } else {
                file(&state.page, rest, None)
            }
        }
    }
}

/// The overlay's policy for `m` with each `axidps-plugin://<id>` source replaced by where the
/// browser reaches the plugin and the SDK (`http://<host>/plugins/<id>/ http://<host>/_sdk/`); nothing else changes, so an
/// author sees the same allow/deny as in the overlay.
fn preview_csp(m: &Manifest, host: &str) -> String {
    csp(m).replace(
        &format!("axidps-plugin://{}", m.id),
        &format!("http://{host}/plugins/{}/ http://{host}/_sdk/", m.id),
    )
}

/// `host:port` for a Host header that is exactly 127.0.0.1, localhost or [::1] with `port`.
fn own_host(value: &str, port: u16) -> Option<String> {
    let (name, p) = value.rsplit_once(':')?;
    let name = name.to_ascii_lowercase();
    if !matches!(name.as_str(), "127.0.0.1" | "localhost" | "[::1]") || p != port.to_string() {
        return None;
    }
    Some(format!("{name}:{port}"))
}

fn respond(
    stream: &mut TcpStream,
    code: u16,
    content_type: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> io::Result<()> {
    let reason = match code {
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "Error",
    };
    let mut head = format!(
        "HTTP/1.1 {code} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\
         Cache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n",
        body.len()
    );
    for (k, v) in headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()
}

/// A `changed` event per changed plugin id until the client goes away.
fn events(mut stream: TcpStream, state: &State) -> io::Result<()> {
    let (tx, rx) = mpsc::channel();
    state
        .subscribers
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(tx);
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    stream.write_all(
        b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-store\r\n\
          Connection: close\r\n\r\n: ready\r\n\r\n",
    )?;
    stream.flush()?;
    loop {
        let msg = match rx.recv_timeout(KEEPALIVE) {
            Ok(id) => format!("event: changed\ndata: {id}\n\n"),
            Err(RecvTimeoutError::Timeout) => ": keepalive\n\n".to_owned(),
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
        };
        // A write that fails means the client left; the poller drops the sender next time.
        stream.write_all(msg.as_bytes())?;
        stream.flush()?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axidps_plugins::MANIFEST_FILE;
    use std::fs;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpStream;
    use std::time::{Duration, Instant};

    struct Fixture {
        _t: tempfile::TempDir,
        root: PathBuf,
        port: u16,
        _server: Server,
    }

    fn fixture() -> Fixture {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().to_path_buf();
        for d in ["page/fonts", "sdk", "bundled/b.one", "mine/x.y"] {
            fs::create_dir_all(root.join(d)).unwrap();
        }
        fs::write(root.join("page/index.html"), "<p>page").unwrap();
        fs::write(root.join("page/fonts/f.woff2"), "font").unwrap();
        fs::write(root.join("sdk/axidps.js"), "var a;").unwrap();
        fs::write(root.join("root-secret"), "secret").unwrap();
        for (dir, id) in [("bundled/b.one", "b.one"), ("mine/x.y", "x.y")] {
            fs::write(
                root.join(dir).join(MANIFEST_FILE),
                format!(
                    r#"{{"id":"{id}","name":"n","version":"1.0.0","api":1,"windows":[{{"id":"w","title":"t","entry":"card.html"}}]}}"#
                ),
            )
            .unwrap();
            fs::write(root.join(dir).join("card.html"), "<p>card").unwrap();
        }
        let server = start(
            Config {
                page: root.join("page"),
                sdk: root.join("sdk"),
                bundled: root.join("bundled"),
                folder: Some(root.join("mine/x.y")),
            },
            0,
        )
        .unwrap();
        Fixture {
            port: server.port,
            _server: server,
            root,
            _t: t,
        }
    }

    /// (status, lowercased header block, body)
    fn get(port: u16, path: &str) -> (u16, String, Vec<u8>) {
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        write!(
            s,
            "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
        )
        .unwrap();
        let mut raw = Vec::new();
        s.read_to_end(&mut raw).unwrap();
        let at = raw.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
        let head = String::from_utf8(raw[..at].to_vec()).unwrap();
        let status = head.split(' ').nth(1).unwrap().parse().unwrap();
        (status, head.to_lowercase(), raw[at + 4..].to_vec())
    }

    #[test]
    fn serves_page_and_sdk() {
        let f = fixture();
        let (s, h, b) = get(f.port, "/index.html?demo");
        assert_eq!(s, 200);
        assert!(h.contains("content-type: text/html; charset=utf-8"), "{h}");
        assert_eq!(b, b"<p>page");
        assert_eq!(get(f.port, "/").0, 200);
        let (s, h, _) = get(f.port, "/_sdk/axidps.js");
        assert_eq!(s, 200);
        assert!(h.contains("content-type: text/javascript"), "{h}");
        assert_eq!(get(f.port, "/plugins/x.y/_sdk/axidps.js").0, 200);
        assert_eq!(get(f.port, "/sdk/axidps.js").0, 200, "the demo page's ../sdk/axidps.js");
        assert_eq!(get(f.port, "/_sdk/fonts/f.woff2").2, b"font");
        assert_eq!(get(f.port, "/nope.html").0, 404);
    }

    #[test]
    fn plugin_file_has_csp() {
        let f = fixture();
        let (s, h, b) = get(f.port, "/plugins/x.y/card.html");
        assert_eq!(s, 200);
        assert_eq!(b, b"<p>card");
        let m = axidps_plugins::manifest::parse(
            &fs::read(f.root.join("mine/x.y").join(MANIFEST_FILE)).unwrap(),
            &f.root.join("mine/x.y"),
        )
        .unwrap();
        let want = format!(
            "content-security-policy: {}",
            preview_csp(&m, &format!("127.0.0.1:{}", f.port)).to_lowercase()
        );
        assert!(!h.contains("axidps-plugin://"), "{h}");
        assert!(
            h.contains(&format!("http://127.0.0.1:{}/_sdk/", f.port)),
            "{h}"
        );
        assert!(h.contains(&want), "{h}");
        assert_eq!(get(f.port, "/plugins/b.one/card.html").0, 200);
        assert_eq!(get(f.port, "/plugins/unknown.id/card.html").0, 404);
    }

    #[test]
    fn blocks_escape() {
        let f = fixture();
        for p in [
            "/plugins/x.y/../../etc/passwd",
            "/plugins/x.y/..%2f..%2froot-secret",
            "/plugins/x.y/%2e%2e/%2e%2e/root-secret",
            "/_sdk/../root-secret",
            "/../root-secret",
            "/_sdk/fonts/..%2f..%2froot-secret",
        ] {
            assert_eq!(get(f.port, p).0, 404, "{p}");
        }
    }

    #[test]
    fn plugins_json_marks_preview() {
        let f = fixture();
        let (s, h, b) = get(f.port, "/plugins.json");
        assert_eq!(s, 200);
        assert!(h.contains("application/json"), "{h}");
        let v: serde_json::Value = serde_json::from_slice(&b).unwrap();
        let by = |id: &str| {
            v.as_array()
                .unwrap()
                .iter()
                .find(|p| p["id"] == id)
                .unwrap()
        };
        assert_eq!(by("x.y")["preview"], true);
        assert!(by("b.one").get("preview").is_none());
        assert_eq!(v.as_array().unwrap().len(), 2);
    }

    #[test]
    fn rejects_other_methods() {
        let f = fixture();
        let mut s = TcpStream::connect(("127.0.0.1", f.port)).unwrap();
        write!(
            s,
            "POST /index.html HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n\r\n",
            f.port
        )
        .unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).unwrap();
        assert!(out.starts_with("HTTP/1.1 405"), "{out}");
    }

    #[test]
    fn events_report_change() {
        let f = fixture();
        let s = TcpStream::connect(("127.0.0.1", f.port)).unwrap();
        let mut w = s.try_clone().unwrap();
        write!(
            w,
            "GET /_events HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n\r\n",
            f.port
        )
        .unwrap();
        s.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        let mut r = BufReader::new(s);
        let mut line = String::new();
        r.read_line(&mut line).unwrap();
        assert!(line.starts_with("HTTP/1.1 200"), "{line}");
        loop {
            line.clear();
            r.read_line(&mut line).unwrap();
            if line == "\r\n" {
                break;
            }
        }
        // Registered once the head is out; the watcher baseline was taken at start.
        let t0 = Instant::now();
        fs::write(f.root.join("mine/x.y/card.html"), "<p>changed, longer").unwrap();
        let mut seen = String::new();
        while !seen.contains("data: x.y") {
            line.clear();
            r.read_line(&mut line).unwrap();
            seen.push_str(&line);
            assert!(t0.elapsed() < Duration::from_secs(1), "{seen:?}");
        }
        assert!(seen.contains("event: changed"), "{seen:?}");
    }

    #[test]
    fn preview_csp_keeps_other_directives() {
        let t = tempfile::tempdir().unwrap();
        let m = axidps_plugins::manifest::parse(
            br#"{"id":"x.y","name":"n","version":"1.0.0","api":1,"network":["api.example.com","*.cdn.example.org"],"windows":[{"id":"w","title":"t","entry":"card.html"}]}"#,
            t.path(),
        )
        .unwrap();
        let h = "127.0.0.1:8711";
        let p = preview_csp(&m, h);
        assert!(
            p.contains("script-src http://127.0.0.1:8711/plugins/x.y/ "),
            "{p}"
        );
        assert_eq!(
            p.replace(
                &format!("http://{h}/plugins/x.y/ http://{h}/_sdk/"),
                "axidps-plugin://x.y"
            ),
            csp(&m)
        );
    }

    fn get_with_host(port: u16, host: &str, path: &str) -> (u16, String) {
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        write!(
            s,
            "GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"
        )
        .unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).unwrap();
        let status = out.split(' ').nth(1).unwrap().parse().unwrap();
        (status, out.to_lowercase())
    }

    #[test]
    fn host_with_injection_rejected() {
        let f = fixture();
        let host = format!("127.0.0.1:{}; script-src *", f.port);
        let (s, out) = get_with_host(f.port, &host, "/plugins/x.y/card.html");
        assert_eq!(s, 400, "{out}");
        assert!(!out.contains("content-security-policy"), "{out}");
    }

    #[test]
    fn host_wrong_port_rejected() {
        let f = fixture();
        let other = if f.port == 1 { 2 } else { 1 };
        for host in [
            format!("127.0.0.1:{other}"),
            "127.0.0.1".to_owned(),
            format!("evil.example:{}", f.port),
            format!("127.0.0.1:+{}", f.port),
            format!("127.0.0.1:0{}", f.port),
        ] {
            assert_eq!(get_with_host(f.port, &host, "/index.html").0, 400, "{host}");
        }
    }

    #[test]
    fn host_ipv6_loopback_accepted() {
        let f = fixture();
        let (s, out) = get_with_host(
            f.port,
            &format!("[::1]:{}", f.port),
            "/plugins/x.y/card.html",
        );
        assert_eq!(s, 200, "{out}");
        assert!(
            out.contains(&format!("http://[::1]:{}/_sdk/", f.port)),
            "{out}"
        );
    }
}
