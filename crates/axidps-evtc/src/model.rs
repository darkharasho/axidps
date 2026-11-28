//! Plain data types mirroring the on-disk EVTC records.

/// Build string written into the header (8 ASCII digits).
pub const FORMAT_BUILD: &str = "20261115";
/// Boss id EVTC logs carry for WvW logs.
pub const BOSS_ID_WVW: u16 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub build: [u8; 8],
    pub revision: u8,
    pub boss_id: u16,
    pub trailing: u8,
}

impl Header {
    pub fn wvw() -> Header {
        let mut build = [0u8; 8];
        build.copy_from_slice(FORMAT_BUILD.as_bytes());
        Header { build, revision: 1, boss_id: BOSS_ID_WVW, trailing: 0 }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Agent {
    pub addr: u64,
    pub prof: u32,
    pub is_elite: u32,
    pub toughness: i16,
    pub concentration: i16,
    pub healing: i16,
    pub hitbox_width: u16,
    pub condition: i16,
    pub hitbox_height: u16,
    pub name: [u8; 64],
    pub pad: [u8; 4],
}

/// Largest prefix of `s` that fits in `max` bytes without splitting a char.
fn clip(s: &str, max: usize) -> &str {
    let mut end = max.min(s.len());
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Builds the `character\0account\0subgroup\0` buffer. Truncates the
/// character first, then the account, so all three NULs fit in 64 bytes.
pub fn player_name(character: &str, account: &str, subgroup: Option<u8>) -> [u8; 64] {
    let sub = subgroup.map(|s| s.to_string()).unwrap_or_default();
    let budget = 64 - 3;
    let sub = clip(&sub, budget);
    let account = clip(account, budget - sub.len());
    let character = clip(character, budget - sub.len() - account.len());
    let mut out = [0u8; 64];
    let mut at = 0;
    for part in [character, account, sub] {
        out[at..at + part.len()].copy_from_slice(part.as_bytes());
        at += part.len() + 1;
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    pub id: u32,
    pub name: [u8; 64],
}

impl Skill {
    /// Name is truncated to 63 bytes so it stays NUL-terminated.
    pub fn named(id: u32, name: &str) -> Skill {
        let name = clip(name, 63);
        let mut buf = [0u8; 64];
        buf[..name.len()].copy_from_slice(name.as_bytes());
        Skill { id, name: buf }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Event {
    pub time: u64,
    pub src_agent: u64,
    pub dst_agent: u64,
    pub value: i32,
    pub buff_dmg: i32,
    pub overstack: u32,
    pub skillid: u32,
    pub src_instid: u16,
    pub dst_instid: u16,
    pub src_master_instid: u16,
    pub dst_master_instid: u16,
    pub iff: u8,
    pub buff: u8,
    pub result: u8,
    pub is_activation: u8,
    pub is_buffremove: u8,
    pub is_ninety: u8,
    pub is_fifty: u8,
    pub is_moving: u8,
    pub is_statechange: u8,
    pub is_flanking: u8,
    pub is_shields: u8,
    pub is_offcycle: u8,
    pub pad: u32,
}

impl Event {
    /// An event from its 64-byte EVTC record (the encoder's layout).
    pub fn from_bytes(b: &[u8; 64]) -> Event {
        let u16_at = |o: usize| u16::from_le_bytes([b[o], b[o + 1]]);
        let u32_at = |o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
        let u64_at = |o: usize| u64::from(u32_at(o)) | (u64::from(u32_at(o + 4)) << 32);
        Event {
            time: u64_at(0),
            src_agent: u64_at(8),
            dst_agent: u64_at(16),
            value: u32_at(24) as i32,
            buff_dmg: u32_at(28) as i32,
            overstack: u32_at(32),
            skillid: u32_at(36),
            src_instid: u16_at(40),
            dst_instid: u16_at(42),
            src_master_instid: u16_at(44),
            dst_master_instid: u16_at(46),
            iff: b[48],
            buff: b[49],
            result: b[50],
            is_activation: b[51],
            is_buffremove: b[52],
            is_ninety: b[53],
            is_fifty: b[54],
            is_moving: b[55],
            is_statechange: b[56],
            is_flanking: b[57],
            is_shields: b[58],
            is_offcycle: b[59],
            pad: u32_at(60),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Log {
    pub header: Header,
    pub agents: Vec<Agent>,
    pub skills: Vec<Skill>,
    pub events: Vec<Event>,
}
