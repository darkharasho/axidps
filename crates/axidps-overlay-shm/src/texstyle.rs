//! Texture styling rules: what the overlay host hands the DLL so it can restyle the game's UI
//! icons (commander tags, squad markers, targets). Plugins with the `textures` permission send
//! rules through the SDK; the host checks the grant, clamps every value and writes them here.
//!
//! The rules live in their own small shared file, `/dev/shm/axidps-texstyle` on Linux (the temp
//! folder on Windows), so the frame layout stays as it is:
//!
//! ```text
//! 0   u32 magic, u32 version, u64 seq (bumped after each complete write; odd while writing)
//! 16  u32 rule count, u32 reserved, u64 host heartbeat ms
//! 32  RULE_LEN-byte rules (see `Rule::encode`), up to MAX_RULES
//! ```
//!
//! Every multi-byte value is little-endian. The DLL only acts on a table whose heartbeat is
//! fresh, so rules vanish when the host stops.

pub const MAGIC: u32 = 0x5354_5841; // public-check:ok
pub const VERSION: u32 = 1;
pub const SEQ_OFF: usize = 8;
pub const COUNT_OFF: usize = 16;
pub const HB_OFF: usize = 24;
pub const RULES_OFF: usize = 32;
pub const RULE_LEN: usize = 96;
pub const MAX_RULES: usize = 64;
pub const FILE_LEN: usize = RULES_OFF + MAX_RULES * RULE_LEN;
/// A table whose host heartbeat is older than this is ignored.
pub const STALE_MS: u64 = 5_000;

/// The icons a rule can name, by stable wire id. New targets are only ever appended.
pub const TARGETS: &[&str] = &[
    "commander.red",
    "commander.orange",
    "commander.yellow",
    "commander.green",
    "commander.cyan",
    "commander.blue",
    "commander.purple",
    "commander.magenta",
    "commander.white",
    "squad.commander",
    "squad.lieutenant",
    "mentor",
    "marker.arrow",
    "marker.circle",
    "marker.heart",
    "marker.square",
    "marker.star",
    "marker.spiral",
    "marker.triangle",
    "marker.cross",
    "selection.orange",
    "selection.blue",
    "selection.green",
    "selection.red",
    "target",
    "target.personal",
    "target.combined",
    "target.attack",
    "target.attack-selected",
];

pub fn target_id(name: &str) -> Option<u16> {
    TARGETS.iter().position(|t| *t == name).map(|i| i as u16)
}

pub fn target_name(id: u16) -> Option<&'static str> {
    TARGETS.get(id as usize).copied()
}

/// How the icon's own colours become the rule's colour.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum Mode {
    /// Keep the game's colours (outline, ring, scale and opacity still apply).
    #[default]
    Keep = 0,
    /// Oklab: the target hue and chroma, the icon's shading remapped around the target lightness.
    Oklab = 1,
    /// Oklab lightness kept, the target's chroma applied.
    Blend = 2,
    /// The icon's luminance times the target colour.
    Luminance = 3,
    /// Flat target colour, alpha kept.
    Solid = 4,
}

impl Mode {
    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => Mode::Oklab,
            2 => Mode::Blend,
            3 => Mode::Luminance,
            4 => Mode::Solid,
            _ => Mode::Keep,
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "keep" => Mode::Keep,
            "oklab" => Mode::Oklab,
            "blend" => Mode::Blend,
            "luminance" => Mode::Luminance,
            "solid" => Mode::Solid,
            _ => return None,
        })
    }
}

/// The outline drawn under the icon.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum Outline {
    #[default]
    None = 0,
    /// Round: Euclidean distance from the icon's edge.
    Round = 1,
    /// Diamond: Manhattan distance, pointed corners.
    Diamond = 2,
    /// Two bands: the outline colour inside, the outer colour beyond 40% of the width.
    Double = 3,
}

impl Outline {
    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => Outline::Round,
            2 => Outline::Diamond,
            3 => Outline::Double,
            _ => Outline::None,
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "none" => Outline::None,
            "round" => Outline::Round,
            "diamond" => Outline::Diamond,
            "double" => Outline::Double,
            _ => return None,
        })
    }
}

/// Rule flags.
pub const F_RING: u8 = 1;
/// The ring gets its own outline.
pub const F_RING_OUTLINE: u8 = 2;
/// The double outline's outer colour is picked for contrast with the outline colour.
pub const F_AUTO_OUTER: u8 = 4;
/// Lift very dark shading a little (Oklab mode).
pub const F_NORMALIZE: u8 = 8;
const F_ALL: u8 = F_RING | F_RING_OUTLINE | F_AUTO_OUTER | F_NORMALIZE;

/// One target's style. Colours are straight (not premultiplied) sRGB RGBA8.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rule {
    pub target: u16,
    pub mode: Mode,
    pub outline: Outline,
    pub flags: u8,
    pub color: [u8; 4],
    pub outline_color: [u8; 4],
    pub outer_color: [u8; 4],
    pub ring_color: [u8; 4],
    pub ring_outline_color: [u8; 4],
    /// Brightness multiplier, 0..=3.
    pub brightness: f32,
    /// Outline width in pixels of a 128 px icon, 0..=16.
    pub outline_width: f32,
    /// Outline edge falloff exponent, 0.5..=8.
    pub softness: f32,
    /// Outline width on minimap icons, pixels of a 32 px icon, 0..=4.
    pub minimap_width: f32,
    /// Icon scale about its centre, 0.5..=3. Past what fits in the texture, the DLL draws the icon bigger.
    pub scale: f32,
    /// Vertical offset, fraction of the icon height, -0.5..=0.5 (positive is down).
    pub offset_y: f32,
    /// Ring outer radius, fraction of half the icon width, 0.1..=1.
    pub ring_radius: f32,
    /// Ring thickness, fraction of half the icon width, 0.01..=0.5.
    pub ring_thickness: f32,
    /// Oklab contrast, -0.3..=0.3.
    pub contrast: f32,
    /// Oklab: how much the icon's own lightness survives at its extremes, 0..=1.
    pub keep_extremes: f32,
    /// Whole-icon opacity, 0..=1.
    pub opacity: f32,
}

impl Default for Rule {
    fn default() -> Self {
        Rule {
            target: 0,
            mode: Mode::Keep,
            outline: Outline::None,
            flags: F_AUTO_OUTER | F_NORMALIZE,
            color: [255, 255, 255, 255],
            outline_color: [255, 255, 255, 255],
            outer_color: [0, 0, 0, 255],
            ring_color: [0, 0, 0, 255],
            ring_outline_color: [255, 255, 255, 255],
            brightness: 1.0,
            outline_width: 0.0,
            softness: 3.0,
            minimap_width: 1.0,
            scale: 1.0,
            offset_y: 0.0,
            ring_radius: 0.45,
            ring_thickness: 0.08,
            contrast: 0.0,
            keep_extremes: 0.8,
            opacity: 1.0,
        }
    }
}

fn clampf(v: f32, lo: f32, hi: f32, def: f32) -> f32 {
    if v.is_finite() { v.clamp(lo, hi) } else { def }
}

impl Rule {
    /// Every value forced into its range (NaN becomes the default).
    pub fn clamped(mut self) -> Self {
        let d = Rule::default();
        self.flags &= F_ALL;
        self.brightness = clampf(self.brightness, 0.0, 3.0, d.brightness);
        self.outline_width = clampf(self.outline_width, 0.0, 16.0, d.outline_width);
        self.softness = clampf(self.softness, 0.5, 8.0, d.softness);
        self.minimap_width = clampf(self.minimap_width, 0.0, 4.0, d.minimap_width);
        self.scale = clampf(self.scale, 0.5, 3.0, d.scale);
        self.offset_y = clampf(self.offset_y, -0.5, 0.5, d.offset_y);
        self.ring_radius = clampf(self.ring_radius, 0.1, 1.0, d.ring_radius);
        self.ring_thickness = clampf(self.ring_thickness, 0.01, 0.5, d.ring_thickness);
        self.contrast = clampf(self.contrast, -0.3, 0.3, d.contrast);
        self.keep_extremes = clampf(self.keep_extremes, 0.0, 1.0, d.keep_extremes);
        self.opacity = clampf(self.opacity, 0.0, 1.0, d.opacity);
        self
    }

    /// True when the rule changes nothing, so the DLL can leave the icon alone.
    pub fn is_identity(&self) -> bool {
        self.mode == Mode::Keep
            && (self.outline == Outline::None || self.outline_width <= 0.0)
            && self.flags & F_RING == 0
            && self.scale == 1.0
            && self.offset_y == 0.0
            && self.opacity >= 1.0
    }

    pub fn encode(&self) -> [u8; RULE_LEN] {
        let mut b = [0u8; RULE_LEN];
        b[0..2].copy_from_slice(&self.target.to_le_bytes());
        b[2] = self.mode as u8;
        b[3] = self.outline as u8;
        b[4] = self.flags;
        for (i, c) in [
            self.color,
            self.outline_color,
            self.outer_color,
            self.ring_color,
            self.ring_outline_color,
        ]
        .iter()
        .enumerate()
        {
            b[8 + i * 4..12 + i * 4].copy_from_slice(c);
        }
        for (i, v) in self.floats().iter().enumerate() {
            b[28 + i * 4..32 + i * 4].copy_from_slice(&v.to_le_bytes());
        }
        b
    }

    pub fn decode(b: &[u8]) -> Self {
        let col = |i: usize| -> [u8; 4] { b[8 + i * 4..12 + i * 4].try_into().unwrap() };
        let f = |i: usize| f32::from_le_bytes(b[28 + i * 4..32 + i * 4].try_into().unwrap());
        Rule {
            target: u16::from_le_bytes([b[0], b[1]]),
            mode: Mode::from_u8(b[2]),
            outline: Outline::from_u8(b[3]),
            flags: b[4],
            color: col(0),
            outline_color: col(1),
            outer_color: col(2),
            ring_color: col(3),
            ring_outline_color: col(4),
            brightness: f(0),
            outline_width: f(1),
            softness: f(2),
            minimap_width: f(3),
            scale: f(4),
            offset_y: f(5),
            ring_radius: f(6),
            ring_thickness: f(7),
            contrast: f(8),
            keep_extremes: f(9),
            opacity: f(10),
        }
        .clamped()
    }

    fn floats(&self) -> [f32; 11] {
        [
            self.brightness,
            self.outline_width,
            self.softness,
            self.minimap_width,
            self.scale,
            self.offset_y,
            self.ring_radius,
            self.ring_thickness,
            self.contrast,
            self.keep_extremes,
            self.opacity,
        ]
    }
}

/// Write the whole table: seq goes odd, the rules and count are written, seq goes even.
/// Rules past MAX_RULES and duplicate targets (the last one wins) are dropped.
pub fn write(shm: &mut [u8], rules: &[Rule], now_ms: u64) {
    let mut keep: Vec<Rule> = Vec::new();
    for r in rules {
        if target_name(r.target).is_none() {
            continue;
        }
        keep.retain(|k| k.target != r.target);
        keep.push(r.clamped());
    }
    keep.truncate(MAX_RULES);
    let seq = u64::from_le_bytes(shm[SEQ_OFF..SEQ_OFF + 8].try_into().unwrap());
    let odd = (seq | 1).wrapping_add(if seq & 1 == 1 { 2 } else { 0 });
    shm[0..4].copy_from_slice(&MAGIC.to_le_bytes());
    shm[4..8].copy_from_slice(&VERSION.to_le_bytes());
    shm[SEQ_OFF..SEQ_OFF + 8].copy_from_slice(&odd.to_le_bytes());
    std::sync::atomic::fence(std::sync::atomic::Ordering::Release);
    shm[COUNT_OFF..COUNT_OFF + 4].copy_from_slice(&(keep.len() as u32).to_le_bytes());
    for (i, r) in keep.iter().enumerate() {
        let o = RULES_OFF + i * RULE_LEN;
        shm[o..o + RULE_LEN].copy_from_slice(&r.encode());
    }
    heartbeat(shm, now_ms);
    std::sync::atomic::fence(std::sync::atomic::Ordering::Release);
    shm[SEQ_OFF..SEQ_OFF + 8].copy_from_slice(&odd.wrapping_add(1).to_le_bytes());
}

/// The host is alive (written about once a second).
pub fn heartbeat(shm: &mut [u8], now_ms: u64) {
    shm[HB_OFF..HB_OFF + 8].copy_from_slice(&now_ms.to_le_bytes());
}

/// The seq a reader compares to tell whether to read again (None while mid-write or not ours).
pub fn seq(shm: &[u8]) -> Option<u64> {
    if u32::from_le_bytes(shm[0..4].try_into().unwrap()) != MAGIC
        || u32::from_le_bytes(shm[4..8].try_into().unwrap()) != VERSION
    {
        return None;
    }
    let s = u64::from_le_bytes(shm[SEQ_OFF..SEQ_OFF + 8].try_into().unwrap());
    (s & 1 == 0).then_some(s)
}

/// Read the table, or None if it is mid-write, foreign, or its host has gone quiet.
pub fn read(shm: &[u8], now_ms: u64) -> Option<(u64, Vec<Rule>)> {
    let s = seq(shm)?;
    std::sync::atomic::fence(std::sync::atomic::Ordering::Acquire);
    let hb = u64::from_le_bytes(shm[HB_OFF..HB_OFF + 8].try_into().unwrap());
    let n = (u32::from_le_bytes(shm[COUNT_OFF..COUNT_OFF + 4].try_into().unwrap()) as usize)
        .min(MAX_RULES);
    let rules = (0..n)
        .map(|i| Rule::decode(&shm[RULES_OFF + i * RULE_LEN..RULES_OFF + (i + 1) * RULE_LEN]))
        .filter(|r| target_name(r.target).is_some())
        .collect();
    std::sync::atomic::fence(std::sync::atomic::Ordering::Acquire);
    if seq(shm) != Some(s) {
        return None;
    }
    if now_ms.saturating_sub(hb) > STALE_MS {
        return Some((s, Vec::new()));
    }
    Some((s, rules))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(t: &str) -> Rule {
        Rule {
            target: target_id(t).unwrap(),
            mode: Mode::Oklab,
            outline: Outline::Round,
            outline_width: 6.0,
            color: [213, 94, 0, 255],
            flags: F_RING | F_NORMALIZE,
            ..Rule::default()
        }
    }

    #[test]
    fn targets_are_unique_and_named() {
        for (i, t) in TARGETS.iter().enumerate() {
            assert_eq!(target_id(t), Some(i as u16));
        }
        assert_eq!(target_id("nope"), None);
        assert_eq!(target_name(9999), None);
    }

    /// The SDK carries its own copy of the list for plugins; it must match.
    #[test]
    fn sdk_lists_the_same_targets() {
        let sdk = include_str!("../../../apps/axidps-overlay/sdk/axidps.js");
        let start = sdk.find("const TEXTURE_TARGETS = [").expect("SDK list") + 25;
        let end = start + sdk[start..].find("];").unwrap();
        let names: Vec<&str> = sdk[start..end]
            .split(',')
            .map(|s| s.trim().trim_matches('"'))
            .filter(|s| !s.is_empty())
            .collect();
        assert_eq!(names, TARGETS);
    }

    #[test]
    fn rule_round_trips() {
        let r = rule("commander.red");
        assert_eq!(Rule::decode(&r.encode()), r);
    }

    #[test]
    fn decode_clamps_hostile_values() {
        let mut r = rule("marker.star");
        r.brightness = 99.0;
        r.outline_width = f32::NAN;
        r.scale = -4.0;
        r.flags = 0xff;
        let d = Rule::decode(&r.encode());
        assert_eq!(d.brightness, 3.0);
        assert_eq!(d.outline_width, 0.0);
        assert_eq!(d.scale, 0.5);
        assert_eq!(d.flags, F_ALL);
        r.scale = 9.0;
        assert_eq!(Rule::decode(&r.encode()).scale, 3.0);
        r.scale = 2.5;
        assert_eq!(Rule::decode(&r.encode()).scale, 2.5);
    }

    #[test]
    fn identity_rules_are_spotted() {
        assert!(Rule::default().is_identity());
        assert!(!rule("mentor").is_identity());
        let ring = Rule {
            flags: F_RING,
            ..Rule::default()
        };
        assert!(!ring.is_identity());
    }

    #[test]
    fn table_round_trips_and_drops_duplicates_and_unknowns() {
        let mut shm = vec![0u8; FILE_LEN];
        let mut a = rule("commander.red");
        let b = rule("commander.blue");
        let unknown = Rule {
            target: 500,
            ..rule("commander.red")
        };
        write(&mut shm, &[a, b, unknown], 1000);
        a.color = [1, 2, 3, 255];
        write(&mut shm, &[rule("commander.red"), b, a], 1000);
        let (s, rules) = read(&shm, 1500).unwrap();
        assert_eq!(s % 2, 0);
        assert_eq!(rules, vec![b, a]);
        // A second write moves seq on.
        write(&mut shm, &[], 2000);
        let (s2, rules) = read(&shm, 2000).unwrap();
        assert!(s2 > s);
        assert!(rules.is_empty());
    }

    #[test]
    fn stale_or_torn_tables_give_nothing() {
        let mut shm = vec![0u8; FILE_LEN];
        assert!(read(&shm, 0).is_none(), "not ours yet");
        write(&mut shm, &[rule("target")], 1000);
        assert_eq!(read(&shm, 1000 + STALE_MS + 1).unwrap().1, vec![]);
        heartbeat(&mut shm, 9000);
        assert_eq!(read(&shm, 9000).unwrap().1.len(), 1);
        // Mid-write: odd seq.
        shm[SEQ_OFF] |= 1;
        assert!(read(&shm, 9000).is_none());
    }

    #[test]
    fn too_many_rules_are_cut() {
        let many: Vec<Rule> = (0..TARGETS.len() as u16)
            .cycle()
            .take(200)
            .map(|t| Rule {
                target: t,
                ..Rule::default()
            })
            .collect();
        let mut shm = vec![0u8; FILE_LEN];
        write(&mut shm, &many, 0);
        assert_eq!(read(&shm, 0).unwrap().1.len(), TARGETS.len().min(MAX_RULES));
    }
}
