//! Texture styling rules from the page to the game. Plugins granted `textures` set rules through
//! the SDK; the page (`plugin-host.js`) merges every such plugin's rules and sends them here as
//! `{"textures": [...]}`. They are turned into wire rules (`axidps_overlay_shm::texstyle`), clamped,
//! and written to the shared rules file, whose heartbeat this keeps fresh while the host runs.
//!
//! JSON rule (every field but `target` optional):
//!
//! ```json
//! {"target": "commander.red", "mode": "oklab", "color": "#d55e00", "brightness": 1,
//!  "contrast": 0, "keepExtremes": 0.8, "normalize": true, "scale": 1, "offsetY": 0,
//!  "opacity": 1,
//!  "outline": {"style": "round", "width": 6, "color": "#ffffff", "outer": "auto",
//!              "softness": 3, "minimap": 1},
//!  "ring": {"radius": 0.45, "thickness": 0.08, "color": "#000000", "outline": "auto"}}
//! ```

use axidps_overlay_shm::texstyle::{
    self as wire, F_AUTO_OUTER, F_NORMALIZE, F_RING, F_RING_OUTLINE, Mode, Outline, Rule,
};
use serde_json::Value;

/// `#rgb`, `#rrggbb`, `#rrggbbaa`, or `[r, g, b]` / `[r, g, b, a]` (0..255).
pub fn color(v: &Value) -> Option<[u8; 4]> {
    if let Some(s) = v.as_str() {
        let h = s.strip_prefix('#')?;
        let n = |i: usize, len: usize| u8::from_str_radix(h.get(i..i + len)?, 16).ok();
        return match h.len() {
            3 => Some([n(0, 1)? * 17, n(1, 1)? * 17, n(2, 1)? * 17, 255]),
            6 => Some([n(0, 2)?, n(2, 2)?, n(4, 2)?, 255]),
            8 => Some([n(0, 2)?, n(2, 2)?, n(4, 2)?, n(6, 2)?]),
            _ => None,
        };
    }
    let a = v.as_array()?;
    if !(3..=4).contains(&a.len()) {
        return None;
    }
    let mut c = [255u8; 4];
    for (i, x) in a.iter().enumerate() {
        c[i] = x.as_f64()?.clamp(0.0, 255.0).round() as u8;
    }
    Some(c)
}

fn num(v: &Value, k: &str) -> Option<f32> {
    v.get(k).and_then(Value::as_f64).map(|f| f as f32)
}

/// One JSON rule, or None when its target or a named value isn't known. Values are clamped by
/// the wire format.
pub fn rule(v: &Value) -> Option<Rule> {
    let mut r = Rule {
        target: wire::target_id(v.get("target")?.as_str()?)?,
        ..Rule::default()
    };
    if let Some(m) = v.get("mode") {
        r.mode = Mode::parse(m.as_str()?)?;
    }
    if let Some(c) = v.get("color") {
        r.color = color(c)?;
    }
    if let Some(b) = v.get("normalize").and_then(Value::as_bool) {
        r.flags = if b { r.flags | F_NORMALIZE } else { r.flags & !F_NORMALIZE };
    }
    r.brightness = num(v, "brightness").unwrap_or(r.brightness);
    r.contrast = num(v, "contrast").unwrap_or(r.contrast);
    r.keep_extremes = num(v, "keepExtremes").unwrap_or(r.keep_extremes);
    r.scale = num(v, "scale").unwrap_or(r.scale);
    r.offset_y = num(v, "offsetY").unwrap_or(r.offset_y);
    r.opacity = num(v, "opacity").unwrap_or(r.opacity);
    if let Some(o) = v.get("outline").filter(|o| !o.is_null()) {
        r.outline = Outline::parse(o.get("style").and_then(Value::as_str).unwrap_or("round"))?;
        r.outline_width = num(o, "width").unwrap_or(4.0);
        r.softness = num(o, "softness").unwrap_or(r.softness);
        r.minimap_width = num(o, "minimap").unwrap_or(r.minimap_width);
        if let Some(c) = o.get("color") {
            r.outline_color = color(c)?;
        }
        match o.get("outer") {
            None => {}
            Some(Value::String(s)) if s == "auto" => r.flags |= F_AUTO_OUTER,
            Some(c) => {
                r.outer_color = color(c)?;
                r.flags &= !F_AUTO_OUTER;
            }
        }
    }
    if let Some(g) = v.get("ring").filter(|g| !g.is_null() && g.as_bool() != Some(false)) {
        r.flags |= F_RING;
        r.ring_radius = num(g, "radius").unwrap_or(r.ring_radius);
        r.ring_thickness = num(g, "thickness").unwrap_or(r.ring_thickness);
        if let Some(c) = g.get("color") {
            r.ring_color = color(c)?;
        }
        match g.get("outline") {
            None | Some(Value::Bool(false)) | Some(Value::Null) => {}
            Some(Value::String(s)) if s == "auto" => r.flags |= F_RING_OUTLINE | F_AUTO_OUTER,
            Some(c) => {
                r.ring_outline_color = color(c)?;
                r.flags |= F_RING_OUTLINE;
                r.flags &= !F_AUTO_OUTER;
            }
        }
    }
    Some(r.clamped())
}

/// The page's list: rules that parse, in order (later ones win a target on the wire).
pub fn rules(v: &Value) -> Vec<Rule> {
    v.as_array()
        .map(|a| a.iter().take(wire::MAX_RULES * 4).filter_map(rule).collect())
        .unwrap_or_default()
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// The shared rules file, made at start with no rules.
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
        wire::write(&mut map, &[], now_ms());
        Ok(File { map })
    }

    pub fn set(&mut self, rules: &[Rule]) {
        wire::write(&mut self.map, rules, now_ms());
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
    fn colours() {
        assert_eq!(color(&json!("#fff")), Some([255, 255, 255, 255]));
        assert_eq!(color(&json!("#d55e00")), Some([213, 94, 0, 255]));
        assert_eq!(color(&json!("#00000080")), Some([0, 0, 0, 128]));
        assert_eq!(color(&json!([300, -2, 7])), Some([255, 0, 7, 255]));
        assert_eq!(color(&json!("red")), None);
        assert_eq!(color(&json!("#12345")), None);
        assert_eq!(color(&json!([1, 2])), None);
    }

    #[test]
    fn a_full_rule() {
        let r = rule(&json!({
            "target": "commander.blue", "mode": "oklab", "color": "#0072b2",
            "brightness": 1.2, "scale": 1.3, "normalize": false,
            "outline": {"style": "double", "width": 6, "color": "#fff", "outer": "#101010"},
            "ring": {"radius": 0.5, "color": [0, 0, 0], "outline": "auto"}
        }))
        .unwrap();
        assert_eq!(r.target, wire::target_id("commander.blue").unwrap());
        assert_eq!(r.mode, Mode::Oklab);
        assert_eq!(r.outline, Outline::Double);
        assert_eq!(r.outer_color, [16, 16, 16, 255]);
        assert_eq!(r.flags & F_NORMALIZE, 0);
        assert_ne!(r.flags & F_RING, 0);
        assert_ne!(r.flags & F_RING_OUTLINE, 0);
        assert_eq!(r.ring_radius, 0.5);
        assert_eq!(r.scale, 1.3);
    }

    #[test]
    fn bad_rules_are_dropped_and_values_clamped() {
        assert!(rule(&json!({"target": "commander.rainbow"})).is_none());
        assert!(rule(&json!({"target": "target", "mode": "glitter"})).is_none());
        assert!(rule(&json!({"target": "target", "color": "nope"})).is_none());
        let r = rule(&json!({"target": "target", "scale": 50, "opacity": -1})).unwrap();
        assert_eq!((r.scale, r.opacity), (3.0, 0.0));
        let list = rules(&json!([{"target": "mentor"}, {"target": "bad"}, 7]));
        assert_eq!(list.len(), 1);
        assert!(rules(&json!({"not": "a list"})).is_empty());
    }

    #[test]
    fn file_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("axidps-texstyle");
        let mut f = File::create(&path).unwrap();
        let r = rule(&json!({"target": "marker.star", "mode": "solid", "color": "#0f0"})).unwrap();
        f.set(&[r]);
        let bytes = std::fs::read(&path).unwrap();
        let (_, got) = wire::read(&bytes, now_ms()).unwrap();
        assert_eq!(got, vec![r]);
    }
}
