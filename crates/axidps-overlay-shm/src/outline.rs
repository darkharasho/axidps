//! Character outline settings: what the overlay host hands the DLL so it can outline the
//! player's own character. The host checks the plugin's grant, clamps every value and writes
//! them here.
//!
//! The settings live in their own small shared file, `/dev/shm/axidps-outline` on Linux (the
//! temp folder on Windows), next to the texstyle file:
//!
//! ```text
//! 0   u32 magic, u32 version, u64 seq (bumped after each complete write; odd while writing)
//! 16  u64 reserved, u64 host heartbeat ms
//! 32  REC_LEN-byte record (see `Settings::encode`):
//!       +0 u32 flags (bit 0 on, bit 1 pve, bit 2 wvw), +4 f32 thickness, +8 f32 fill,
//!       +12 f32 x3 color, +24 f32 x3 fill_color, +36 12 zero bytes
//! ```
//!
//! Every multi-byte value is little-endian. The DLL only acts on settings whose heartbeat is
//! fresh, so the outline vanishes when the host stops.

pub const MAGIC: u32 = 0x4C4F_5841; // "AXOL" public-check:ok
pub const VERSION: u32 = 1;
pub const SEQ_OFF: usize = 8;
pub const HB_OFF: usize = 24;
pub const REC_OFF: usize = 32;
pub const REC_LEN: usize = 48;
pub const FILE_LEN: usize = REC_OFF + REC_LEN;
/// Settings whose host heartbeat is older than this are ignored.
pub const STALE_MS: u64 = 5_000;

const F_ON: u32 = 1;
const F_PVE: u32 = 2;
const F_WVW: u32 = 4;
const F_ALL: u32 = F_ON | F_PVE | F_WVW;

/// The outline's settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    pub on: bool,
    /// Drawn in PvE (everything that is neither WvW nor PvP).
    pub pve: bool,
    pub wvw: bool,
    /// Outline thickness in pixels, 1..=8.
    pub thickness: f32,
    /// Opacity of the shading inside the outline, 0..=0.6.
    pub fill: f32,
    /// Outline colour, channels 0..=1.
    pub color: [f32; 3],
    /// Colour of the inside shading, channels 0..=1.
    pub fill_color: [f32; 3],
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            on: false,
            pve: true,
            wvw: true,
            thickness: 3.0,
            fill: 0.15,
            color: [0.2, 1.0, 0.9],
            fill_color: [0.2, 1.0, 0.9],
        }
    }
}

fn clampf(v: f32, lo: f32, hi: f32, def: f32) -> f32 {
    if v.is_finite() { v.clamp(lo, hi) } else { def }
}

fn clamp_color(c: [f32; 3], def: [f32; 3]) -> [f32; 3] {
    [
        clampf(c[0], 0.0, 1.0, def[0]),
        clampf(c[1], 0.0, 1.0, def[1]),
        clampf(c[2], 0.0, 1.0, def[2]),
    ]
}

impl Settings {
    /// Every value forced into its range (a non-finite value becomes the default).
    pub fn clamped(mut self) -> Self {
        let d = Settings::default();
        self.thickness = clampf(self.thickness, 1.0, 8.0, d.thickness);
        self.fill = clampf(self.fill, 0.0, 0.6, d.fill);
        self.color = clamp_color(self.color, d.color);
        self.fill_color = clamp_color(self.fill_color, d.fill_color);
        self
    }

    pub fn encode(&self) -> [u8; REC_LEN] {
        let s = self.clamped();
        let mut b = [0u8; REC_LEN];
        let flags = (s.on as u32 * F_ON) | (s.pve as u32 * F_PVE) | (s.wvw as u32 * F_WVW);
        b[0..4].copy_from_slice(&flags.to_le_bytes());
        let floats = [
            s.thickness,
            s.fill,
            s.color[0],
            s.color[1],
            s.color[2],
            s.fill_color[0],
            s.fill_color[1],
            s.fill_color[2],
        ];
        for (i, v) in floats.iter().enumerate() {
            b[4 + i * 4..8 + i * 4].copy_from_slice(&v.to_le_bytes());
        }
        b
    }

    /// `b` must hold at least REC_LEN bytes.
    pub fn decode(b: &[u8]) -> Self {
        let f = |i: usize| f32::from_le_bytes(b[4 + i * 4..8 + i * 4].try_into().unwrap());
        let flags = u32::from_le_bytes(b[0..4].try_into().unwrap()) & F_ALL;
        Settings {
            on: flags & F_ON != 0,
            pve: flags & F_PVE != 0,
            wvw: flags & F_WVW != 0,
            thickness: f(0),
            fill: f(1),
            color: [f(2), f(3), f(4)],
            fill_color: [f(5), f(6), f(7)],
        }
        .clamped()
    }
}

/// Write the settings: seq goes odd, the record is written, seq goes even.
pub fn write(shm: &mut [u8], s: &Settings, now_ms: u64) {
    let seq = u64::from_le_bytes(shm[SEQ_OFF..SEQ_OFF + 8].try_into().unwrap());
    let odd = (seq | 1).wrapping_add(if seq & 1 == 1 { 2 } else { 0 });
    shm[0..4].copy_from_slice(&MAGIC.to_le_bytes());
    shm[4..8].copy_from_slice(&VERSION.to_le_bytes());
    shm[SEQ_OFF..SEQ_OFF + 8].copy_from_slice(&odd.to_le_bytes());
    std::sync::atomic::fence(std::sync::atomic::Ordering::Release);
    shm[REC_OFF..REC_OFF + REC_LEN].copy_from_slice(&s.encode());
    heartbeat(shm, now_ms);
    std::sync::atomic::fence(std::sync::atomic::Ordering::Release);
    shm[SEQ_OFF..SEQ_OFF + 8].copy_from_slice(&odd.wrapping_add(1).to_le_bytes());
}

/// The host is alive (written about once a second).
pub fn heartbeat(shm: &mut [u8], now_ms: u64) {
    shm[HB_OFF..HB_OFF + 8].copy_from_slice(&now_ms.to_le_bytes());
}

/// The seq a reader compares to tell whether to read again (None while mid-write, short or not ours).
pub fn seq(shm: &[u8]) -> Option<u64> {
    if shm.len() < FILE_LEN
        || u32::from_le_bytes(shm[0..4].try_into().unwrap()) != MAGIC
        || u32::from_le_bytes(shm[4..8].try_into().unwrap()) != VERSION
    {
        return None;
    }
    let s = u64::from_le_bytes(shm[SEQ_OFF..SEQ_OFF + 8].try_into().unwrap());
    (s & 1 == 0).then_some(s)
}

/// Read the settings, or None if the file is short, foreign, mid-write, or its host has gone quiet.
pub fn read(shm: &[u8], now_ms: u64) -> Option<(u64, Settings)> {
    let s = seq(shm)?;
    std::sync::atomic::fence(std::sync::atomic::Ordering::Acquire);
    let hb = u64::from_le_bytes(shm[HB_OFF..HB_OFF + 8].try_into().unwrap());
    let settings = Settings::decode(&shm[REC_OFF..REC_OFF + REC_LEN]);
    std::sync::atomic::fence(std::sync::atomic::Ordering::Acquire);
    if seq(shm) != Some(s) || now_ms.saturating_sub(hb) > STALE_MS {
        return None;
    }
    Some((s, settings))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_spec() {
        let d = Settings::default();
        assert!(!d.on && d.pve && d.wvw);
        assert_eq!((d.thickness, d.fill), (3.0, 0.15));
        assert_eq!(d.color, [0.2, 1.0, 0.9]);
        assert_eq!(d.fill_color, [0.2, 1.0, 0.9]);
    }

    #[test]
    fn round_trip() {
        let s = Settings {
            on: true,
            pve: false,
            wvw: true,
            thickness: 5.0,
            fill: 0.4,
            color: [0.1, 0.5, 0.75],
            fill_color: [1.0, 0.25, 0.0],
        };
        let mut f = vec![0u8; FILE_LEN];
        write(&mut f, &s, 1_000);
        let (seq, got) = read(&f, 1_000).expect("fresh file reads");
        assert!(seq > 0 && seq % 2 == 0);
        assert_eq!(got, s);
        write(&mut f, &s, 1_000);
        assert!(read(&f, 1_000).unwrap().0 > seq);
    }

    #[test]
    fn hostile_values_are_clamped() {
        let s = Settings {
            on: true,
            pve: false,
            wvw: true,
            thickness: 99.0,
            fill: -1.0,
            color: [f32::NAN, 2.0, -3.0],
            fill_color: [0.5, f32::INFINITY, 0.0],
        };
        let c = Settings::decode(&s.encode());
        assert_eq!((c.thickness, c.fill), (8.0, 0.0));
        assert_eq!(c.color, [0.2, 1.0, 0.0]);
        assert_eq!(c.fill_color, [0.5, 1.0, 0.0]);
        let mut raw = s.encode();
        raw[0] = 0xff;
        raw[4..8].copy_from_slice(&f32::NAN.to_le_bytes());
        let d = Settings::decode(&raw);
        assert!(d.on && d.pve && d.wvw);
        assert_eq!(d.thickness, 3.0);
    }

    #[test]
    fn torn_write_reads_none() {
        let mut f = vec![0u8; FILE_LEN];
        write(&mut f, &Settings::default(), 1_000);
        assert!(read(&f, 1_000).is_some());
        let s = u64::from_le_bytes(f[SEQ_OFF..SEQ_OFF + 8].try_into().unwrap());
        f[SEQ_OFF..SEQ_OFF + 8].copy_from_slice(&(s + 1).to_le_bytes());
        assert!(read(&f, 1_000).is_none());
    }

    #[test]
    fn stale_heartbeat_reads_none() {
        let mut f = vec![0u8; FILE_LEN];
        write(&mut f, &Settings::default(), 1_000);
        assert!(read(&f, 6_000).is_some());
        assert!(read(&f, 6_001).is_none());
        heartbeat(&mut f, 6_000);
        assert!(read(&f, 6_001).is_some());
    }

    #[test]
    fn bad_magic_short_or_wrong_version_reads_none() {
        let mut f = vec![0u8; FILE_LEN];
        write(&mut f, &Settings::default(), 1_000);
        assert!(read(&f[..FILE_LEN - 1], 1_000).is_none());
        let mut m = f.clone();
        m[0] ^= 0xff;
        assert!(read(&m, 1_000).is_none());
        let mut v = f.clone();
        v[4..8].copy_from_slice(&2u32.to_le_bytes());
        assert!(read(&v, 1_000).is_none());
    }
}
