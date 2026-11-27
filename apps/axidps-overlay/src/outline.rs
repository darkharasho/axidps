//! The character outline's settings from the page to the game. The page sends
//! `{"outline": {...}}` (or `null` for off); this turns it into wire settings
//! (`axidps_overlay_shm::outline`), clamped, and writes the shared file, whose heartbeat this
//! keeps fresh while the host runs.
//!
//! ```json
//! {"on": true, "color": [0.2, 1, 0.9], "thickness": 3, "fill": 0.15,
//!  "fillColor": [0.2, 1, 0.9], "pve": true, "wvw": true}
//! ```

use axidps_overlay_shm::outline::{self as wire, Settings};
use serde_json::Value;

use crate::textures::now_ms;

fn color(v: &Value, def: [f32; 3]) -> [f32; 3] {
    let Some(a) = v.as_array().filter(|a| a.len() == 3) else { return def };
    let mut c = def;
    for (i, x) in a.iter().enumerate() {
        match x.as_f64() {
            Some(f) => c[i] = f as f32,
            None => return def,
        }
    }
    c
}

/// The page's value; null or anything that isn't an object is off. Wrong-typed keys keep their
/// defaults.
pub fn settings(v: &Value) -> Settings {
    let d = Settings::default();
    if !v.is_object() {
        return d;
    }
    let num = |k: &str, def: f32| v.get(k).and_then(Value::as_f64).map_or(def, |f| f as f32);
    let flag = |k: &str, def: bool| v.get(k).and_then(Value::as_bool).unwrap_or(def);
    Settings {
        on: flag("on", d.on),
        pve: flag("pve", d.pve),
        wvw: flag("wvw", d.wvw),
        thickness: num("thickness", d.thickness),
        fill: num("fill", d.fill),
        color: v.get("color").map_or(d.color, |c| color(c, d.color)),
        fill_color: v.get("fillColor").map_or(d.fill_color, |c| color(c, d.fill_color)),
    }
    .clamped()
}

/// The shared outline file, made at start with the outline off.
pub struct File {
    map: memmap2::MmapMut,
}

impl File {
    pub fn create(path: &std::path::Path) -> anyhow::Result<Self> {
        let f = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        f.set_len(wire::FILE_LEN as u64)?;
        let mut map = unsafe { memmap2::MmapMut::map_mut(&f)? };
        wire::write(&mut map, &Settings::default(), now_ms());
        Ok(File { map })
    }

    pub fn set(&mut self, s: &Settings) {
        wire::write(&mut self.map, s, now_ms());
    }

    pub fn heartbeat(&mut self) {
        wire::heartbeat(&mut self.map, now_ms());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn null_and_garbage_are_off() {
        assert_eq!(settings(&json!(null)), Settings::default());
        assert_eq!(settings(&json!([1, 2])), Settings::default());
        assert!(!settings(&json!({"on": "yes"})).on);
    }

    #[test]
    fn fields_parse_and_clamp() {
        let s = settings(&json!({"on": true, "color": [1, 0, 0.5], "thickness": 12, "fill": 0.3,
                                 "fillColor": [0, 0, 1], "pve": false, "wvw": true}));
        assert!(s.on && !s.pve && s.wvw);
        assert_eq!(s.color, [1.0, 0.0, 0.5]);
        assert_eq!(s.thickness, 8.0);
        assert_eq!(s.fill, 0.3);
        assert_eq!(s.fill_color, [0.0, 0.0, 1.0]);
        assert_eq!(settings(&json!({"on": true, "color": [1, 0]})).color, [0.2, 1.0, 0.9]);
    }

    #[test]
    fn file_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("axidps-outline");
        let mut f = File::create(&path).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(bytes.len(), wire::FILE_LEN);
        assert_eq!(wire::read(&bytes, now_ms()).unwrap().1, Settings::default());
        let s = settings(&json!({"on": true, "thickness": 5, "wvw": false}));
        f.set(&s);
        f.heartbeat();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(wire::read(&bytes, now_ms()).unwrap().1, s);
    }
}
