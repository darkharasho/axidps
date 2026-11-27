//! `axidps-plugin.json`: parsing and validation. `problems` reports every problem at once.

use crate::SUPPORTED_API;
use serde_json::Value;
use std::path::Path;

pub const MAX_MANIFEST_BYTES: usize = 64 * 1024;
pub const ICONS: [&str; 13] = [
    "chart", "users", "shield", "swords", "activity", "x", "grid", "layers", "sliders", "plug",
    "download", "columns", "plus",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    BesideMeter,
    BelowMeter,
}

impl Place {
    pub fn as_str(self) -> &'static str {
        match self {
            Place::BesideMeter => "beside-meter",
            Place::BelowMeter => "below-meter",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowDef {
    pub id: String,
    pub title: String,
    pub entry: String,
    pub description: String,
    pub icon: String,
    pub open: bool,
    pub place: Place,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub author: String,
    pub api: u32,
    pub permissions: Vec<String>,
    pub network: Vec<String>,
    pub windows: Vec<WindowDef>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Basic,
    Deeper,
}

/// The permission table: `fight` is basic; the rest need verification.
pub fn level(name: &str) -> Option<Level> {
    match name {
        "fight" => Some(Level::Basic),
        "world" | "position" | "squad" | "enemies" | "textures" => Some(Level::Deeper),
        _ => None,
    }
}

impl Manifest {
    /// The deeper permissions this plugin names, in manifest order.
    pub fn deeper(&self) -> Vec<String> {
        self.permissions
            .iter()
            .filter(|p| level(p) == Some(Level::Deeper))
            .cloned()
            .collect()
    }
}

fn valid_id(s: &str) -> bool {
    (3..=48).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-')
}

fn valid_window_id(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Top-level names that only ever mean this machine or the local network.
const LOCAL_TLDS: [&str; 5] = ["localhost", "local", "lan", "internal", "home.arpa"];

/// A public host name, with an optional leading `*.`; no scheme, port or path. At least two labels
/// after any `*.` (so no `localhost` or `*.com`), not an IP address (a URL parser reads a host
/// whose last label is a number, like `127.0.0.1` or `0x7f.1`, as IPv4), and not under a name
/// that only means this machine or the local network (`.localhost`, `.local`, ...). This keeps a
/// plugin off the player's own machine and LAN by name.
fn valid_host(s: &str) -> bool {
    let h = s.strip_prefix("*.").unwrap_or(s);
    let labels: Vec<&str> = h.split('.').collect();
    let last = labels.last().copied().unwrap_or("").to_ascii_lowercase();
    let numeric = last.bytes().all(|b| b.is_ascii_digit())
        || last
            .strip_prefix("0x")
            .is_some_and(|x| x.bytes().all(|b| b.is_ascii_hexdigit()));
    let lower = h.to_ascii_lowercase();
    let local = LOCAL_TLDS
        .iter()
        .any(|t| lower == *t || lower.ends_with(&format!(".{t}")));
    !h.is_empty()
        && h.len() <= 253
        && labels.len() >= 2
        && !numeric
        && !local
        && labels.iter().all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
}

fn valid_entry(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with('/')
        && !s.contains(['\0', '\\', ':'])
        && s.split('/').all(|c| !c.is_empty() && c != "." && c != "..")
        && s.split('/').next() != Some("_sdk")
}

fn str_field<'a>(
    o: &'a serde_json::Map<String, Value>,
    k: &str,
    ctx: &str,
    out: &mut Vec<String>,
) -> Option<&'a str> {
    match o.get(k) {
        Some(Value::String(s)) if !s.trim().is_empty() => Some(s),
        Some(Value::String(_)) => {
            out.push(format!("{ctx}{k}: must not be empty"));
            None
        }
        Some(_) => {
            out.push(format!("{ctx}{k}: must be a string"));
            None
        }
        None => {
            out.push(format!("{ctx}{k}: missing"));
            None
        }
    }
}

fn str_list(o: &serde_json::Map<String, Value>, k: &str, out: &mut Vec<String>) -> Vec<String> {
    match o.get(k) {
        None => Vec::new(),
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|v| match v {
                Value::String(s) => Some(s.clone()),
                _ => {
                    out.push(format!("{k}: entries must be strings"));
                    None
                }
            })
            .collect(),
        Some(_) => {
            out.push(format!("{k}: must be a list"));
            Vec::new()
        }
    }
}

/// Every problem found, each a line naming its field.
pub fn problems(bytes: &[u8], folder: &Path) -> Result<Manifest, Vec<String>> {
    if bytes.len() > MAX_MANIFEST_BYTES {
        return Err(vec![format!(
            "manifest: {} bytes, at most {MAX_MANIFEST_BYTES}",
            bytes.len()
        )]);
    }
    let v: Value = serde_json::from_slice(bytes)
        .map_err(|e| vec![format!("manifest: not valid JSON ({e})")])?;
    let Some(o) = v.as_object() else {
        return Err(vec!["manifest: must be a JSON object".into()]);
    };
    let mut out = Vec::new();

    let id = str_field(o, "id", "", &mut out).map(str::to_owned);
    if let Some(id) = &id
        && !valid_id(id)
    {
        out.push(format!(
            "id: \"{id}\" must be 3-48 characters of a-z 0-9 . -"
        ));
    }
    let name = str_field(o, "name", "", &mut out).map(str::to_owned);
    let version = str_field(o, "version", "", &mut out).map(str::to_owned);
    if let Some(ver) = &version
        && semver::Version::parse(ver).is_err()
    {
        out.push(format!(
            "version: \"{ver}\" is not a semantic version (like 1.0.0)"
        ));
    }
    let author = match o.get("author") {
        None => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(_) => {
            out.push("author: must be a string".into());
            String::new()
        }
    };
    let mut newer_api = false;
    let api = match o.get("api") {
        Some(Value::Number(n)) => match n.as_u64().and_then(|n| u32::try_from(n).ok()) {
            Some(n) => {
                newer_api = n != SUPPORTED_API;
                n
            }
            None => {
                out.push("api: must be a whole number".into());
                0
            }
        },
        Some(_) => {
            out.push("api: must be a number".into());
            0
        }
        None => {
            out.push("api: missing".into());
            0
        }
    };
    let permissions = str_list(o, "permissions", &mut out);
    for p in &permissions {
        if level(p).is_none() {
            out.push(format!("permissions: unknown permission \"{p}\""));
        }
    }
    let network = str_list(o, "network", &mut out);
    for n in &network {
        if !valid_host(n) {
            out.push(format!(
                "network: \"{n}\" must be a public host name like api.example.com (HTTPS only, no scheme, port or path; not an IP address, localhost or a local-network name)"
            ));
        }
    }

    let mut windows = Vec::new();
    match o.get("windows") {
        Some(Value::Array(a)) => {
            if a.is_empty() || a.len() > 4 {
                let limit = if a.is_empty() {
                    "at least 1"
                } else {
                    "at most 4"
                };
                out.push(format!("windows: {} given, {limit}", a.len()));
            }
            for (i, w) in a.iter().enumerate() {
                if let Some(w) = parse_window(w, i, &mut out) {
                    if windows.iter().any(|x: &WindowDef| x.id == w.id) {
                        out.push(format!("windows[{i}].id: \"{}\" is used twice", w.id));
                    }
                    windows.push(w);
                }
            }
        }
        Some(_) => out.push("windows: must be a list".into()),
        None => out.push("windows: missing".into()),
    }

    if folder.join("_sdk").exists() {
        out.push(
            "folder: a top-level \"_sdk\" folder is not allowed (the host serves /_sdk/)".into(),
        );
    }

    // A manifest for another api may use fields this build doesn't know: say so first.
    if newer_api && !out.is_empty() {
        out.insert(0, "needs a newer axidps".into());
    }

    match (out.is_empty(), id, name, version) {
        (true, Some(id), Some(name), Some(version)) => Ok(Manifest {
            id,
            name,
            version,
            author,
            api,
            permissions,
            network,
            windows,
        }),
        _ => Err(out),
    }
}

fn parse_window(w: &Value, i: usize, out: &mut Vec<String>) -> Option<WindowDef> {
    let ctx = format!("windows[{i}].");
    let Some(o) = w.as_object() else {
        out.push(format!("windows[{i}]: must be an object"));
        return None;
    };
    let before = out.len();
    let id = str_field(o, "id", &ctx, out).map(str::to_owned);
    if let Some(id) = &id
        && !valid_window_id(id)
    {
        out.push(format!("{ctx}id: \"{id}\" must be a-z 0-9 -"));
    }
    let title = str_field(o, "title", &ctx, out).map(str::to_owned);
    let entry = str_field(o, "entry", &ctx, out).map(str::to_owned);
    if let Some(e) = &entry
        && !valid_entry(e)
    {
        out.push(format!(
            "{ctx}entry: \"{e}\" must be a path inside the plugin folder"
        ));
    }
    let description = match o.get("description") {
        Some(Value::String(s)) => s.clone(),
        None => String::new(),
        Some(_) => {
            out.push(format!("{ctx}description: must be a string"));
            String::new()
        }
    };
    let icon = match o.get("icon") {
        Some(Value::String(s)) if ICONS.contains(&s.as_str()) => s.clone(),
        _ => "plug".to_owned(),
    };
    let open = match o.get("open") {
        None => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => {
            out.push(format!("{ctx}open: must be true or false"));
            false
        }
    };
    let place = match o.get("place") {
        None => Place::BesideMeter,
        Some(Value::String(s)) if s == "beside-meter" => Place::BesideMeter,
        Some(Value::String(s)) if s == "below-meter" => Place::BelowMeter,
        Some(_) => {
            out.push(format!(
                "{ctx}place: must be \"beside-meter\" or \"below-meter\""
            ));
            Place::BesideMeter
        }
    };
    if out.len() != before {
        return None;
    }
    Some(WindowDef {
        id: id?,
        title: title?,
        entry: entry?,
        description,
        icon,
        open,
        place,
    })
}

/// `problems`, reduced to the first line.
pub fn parse(bytes: &[u8], folder: &Path) -> Result<Manifest, String> {
    problems(bytes, folder).map_err(|mut p| p.remove(0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::{Plugin, loadable, to_page_json};
    use std::path::PathBuf;

    const EXAMPLE: &str = r#"{
      "id": "example.dps-card", "name": "DPS card", "version": "1.0.0", "author": "someone",
      "api": 1, "permissions": ["fight"], "network": ["api.example.com"],
      "windows": [{"id": "card", "title": "DPS card", "entry": "card.html",
        "description": "your DPS, big", "icon": "chart", "open": false, "place": "beside-meter"}]
    }"#;

    fn tmp() -> PathBuf {
        PathBuf::from("/nonexistent-axidps-test")
    }

    fn with(edit: impl FnOnce(&mut Value)) -> Vec<u8> {
        let mut v: Value = serde_json::from_str(EXAMPLE).unwrap();
        edit(&mut v);
        serde_json::to_vec(&v).unwrap()
    }

    fn plugin(m: Manifest) -> Plugin {
        Plugin {
            dir: tmp(),
            bundled: false,
            folder: m.id.clone(),
            manifest: Some(m),
            error: None,
        }
    }

    fn windows(n: usize) -> Value {
        (0..n)
            .map(|i| serde_json::json!({"id": format!("w{i}"), "title": "t", "entry": "a.html"}))
            .collect()
    }

    #[test]
    fn parse_example_manifest() {
        let m = parse(EXAMPLE.as_bytes(), &tmp()).unwrap();
        assert_eq!(m.id, "example.dps-card");
        assert_eq!(m.windows[0].place, Place::BesideMeter);
        assert_eq!(m.network, ["api.example.com"]);
    }

    #[test]
    fn rejects_bad_id() {
        for id in ["Bad_ID", "ab", &"a".repeat(49)] {
            let e = parse(&with(|v| v["id"] = id.into()), &tmp()).unwrap_err();
            assert!(e.contains("id"), "{e}");
        }
    }

    #[test]
    fn rejects_unknown_permission() {
        let e = parse(
            &with(|v| v["permissions"] = serde_json::json!(["fight", "files"])),
            &tmp(),
        )
        .unwrap_err();
        assert!(e.contains("files"), "{e}");
    }

    #[test]
    fn deeper_permission_valid_not_loadable() {
        let m = parse(
            &with(|v| v["permissions"] = serde_json::json!(["fight", "world"])),
            &tmp(),
        )
        .unwrap();
        assert_eq!(m.deeper(), ["world"]);
        let p = plugin(m);
        let j = to_page_json(std::slice::from_ref(&p));
        assert_eq!(j[0]["error"], "needs verification");
        assert_eq!(j[0]["permissions"], serde_json::json!(["fight", "world"]));
        assert!(!loadable(&p));
    }

    #[test]
    fn problems_lists_all() {
        let e = problems(
            &with(|v| {
                v["id"] = "Bad_ID".into();
                v["windows"] = windows(5);
            }),
            &tmp(),
        )
        .unwrap_err();
        assert_eq!(e.len(), 2, "{e:?}");
        assert!(e.iter().any(|l| l.contains("id")));
        assert!(
            e.iter().any(|l| l.contains("windows: 5 given, at most 4")),
            "{e:?}"
        );
    }

    #[test]
    fn rejects_bad_network() {
        for n in [
            "https://x.com",
            "x.com/path",
            "",
            "x.com:443",
            "*",
            "localhost",
            "LocalHost",
            "api",
            "*.com",
            "*.localhost",
            "x.localhost",
            "127.0.0.1",
            "192.168.1.1",
            "10.0.0.1",
            "1.2.3.4",
            "0x7f.1",
            "0x7f.0.0.0x1",
            "2130706433",
            "*.168.1.1",
            "[::1]",
            "::1",
            "router.local",
            "nas.lan",
            "x.home.arpa",
            "svc.internal",
        ] {
            assert!(
                parse(&with(|v| v["network"] = serde_json::json!([n])), &tmp()).is_err(),
                "{n}"
            );
        }
        for n in [
            "*.example.com",
            "api.example.com",
            "a.b.example.co.uk",
            "1.example.com",
            "x.123abc",
            "allowed.invalid",
            "*.csp.invalid",
        ] {
            assert!(
                parse(&with(|v| v["network"] = serde_json::json!([n])), &tmp()).is_ok(),
                "{n}"
            );
        }
    }

    #[test]
    fn rejects_window_count() {
        for n in [0, 5] {
            let e = parse(&with(|v| v["windows"] = windows(n)), &tmp()).unwrap_err();
            assert!(e.contains("windows"), "{e}");
        }
        assert!(parse(&with(|v| v["windows"] = windows(4)), &tmp()).is_ok());
    }

    #[test]
    fn rejects_duplicate_window_id() {
        let e = parse(&with(|v| v["windows"] = serde_json::json!([
            {"id": "a", "title": "t", "entry": "a.html"}, {"id": "a", "title": "u", "entry": "b.html"}])), &tmp()).unwrap_err();
        assert!(e.contains("twice"), "{e}");
    }

    #[test]
    fn rejects_entry_outside() {
        for entry in ["../x.html", "/x.html", "a/../../x.html", "_sdk/axidps.js"] {
            let b = with(|v| v["windows"][0]["entry"] = entry.into());
            assert!(parse(&b, &tmp()).is_err(), "{entry}");
        }
        assert!(
            parse(
                &with(|v| v["windows"][0]["entry"] = "ui/card.html".into()),
                &tmp()
            )
            .is_ok()
        );
    }

    #[test]
    fn rejects_sdk_folder() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir(d.path().join("_sdk")).unwrap();
        let e = parse(EXAMPLE.as_bytes(), d.path()).unwrap_err();
        assert!(e.contains("_sdk"), "{e}");
    }

    #[test]
    fn rejects_huge_manifest() {
        let b = vec![b' '; 65 * 1024];
        let e = problems(&b, &tmp()).unwrap_err();
        assert_eq!(e.len(), 1);
    }

    #[test]
    fn unknown_icon_falls_back() {
        let m = parse(&with(|v| v["windows"][0]["icon"] = "rocket".into()), &tmp()).unwrap();
        assert_eq!(m.windows[0].icon, "plug");
    }

    #[test]
    fn api_2_listed_not_loaded() {
        let m = parse(&with(|v| v["api"] = 2.into()), &tmp()).unwrap();
        let p = plugin(m);
        assert_eq!(
            to_page_json(std::slice::from_ref(&p))[0]["error"],
            "needs a newer axidps"
        );
        assert!(!loadable(&p));
    }

    #[test]
    fn newer_api_with_other_errors_says_so_first() {
        let e = problems(
            &with(|v| {
                v["api"] = 2.into();
                v["id"] = "X".into();
            }),
            &tmp(),
        )
        .unwrap_err();
        assert_eq!(e[0], "needs a newer axidps");
        assert!(e.len() > 1);
        let e = problems(&with(|v| v["id"] = "X".into()), &tmp()).unwrap_err();
        assert!(e[0].starts_with("id:"));
    }

    #[test]
    fn defaults_and_garbage() {
        let m = parse(br#"{"id":"abc","name":"n","version":"1.0.0","api":1,"windows":[{"id":"w","title":"t","entry":"a.html","place":"below-meter"}]}"#, &tmp()).unwrap();
        assert_eq!(m.windows[0].place, Place::BelowMeter);
        assert!(parse(b"[]", &tmp()).is_err());
        assert!(parse(b"nope", &tmp()).is_err());
        assert!(
            parse(&with(|v| v["version"] = "1.0".into()), &tmp())
                .unwrap_err()
                .contains("version")
        );
    }
}
