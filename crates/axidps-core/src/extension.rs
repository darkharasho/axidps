//! The axidps extension block: rows tagged `is_statechange = 40` with signature `"AXID"`.

use axidps_evtc::Event as EvtcEvent;
use axidps_protocol::v2::PROTOCOL_VERSION;

/// `"AXID"` little-endian.
pub const SIG: u32 = 0x4449_5841; // public-check:ok
pub const REVISION: u64 = 1;
/// Ring events dropped during the fight.
pub const ROW_DROPS: u32 = 0x100;
/// `src_agent` = DLL version, `dst_agent` = protocol version.
pub const ROW_VERSIONS: u32 = 0x101;
/// Group rows written: core, buffs, casts, condition, positions, markers, effects, heal, objectives.
pub const GROUPS: usize = 9;

const STATECHANGE_EXTENSION: u8 = 40;

/// The rows that follow LOG_START (registration, 9 group rows, versions), and the drops row
/// that goes just before LOG_END. `dropped` saturates into the row's `value`.
pub fn rows(
    start_ms: u64,
    end_ms: u64,
    group_state: [u8; 16],
    dropped: u64,
    dll_version: u32,
) -> (Vec<EvtcEvent>, EvtcEvent) {
    let row = |time: u64| EvtcEvent {
        time,
        is_statechange: STATECHANGE_EXTENSION,
        ..Default::default()
    };
    let mut out = Vec::with_capacity(GROUPS + 2);
    out.push(EvtcEvent {
        src_agent: u64::from(SIG) | (REVISION << 32),
        pad: 0,
        ..row(start_ms)
    });
    for (i, state) in group_state.iter().take(GROUPS).enumerate() {
        out.push(EvtcEvent {
            pad: SIG,
            skillid: i as u32,
            value: i32::from(*state),
            ..row(start_ms)
        });
    }
    out.push(EvtcEvent {
        pad: SIG,
        skillid: ROW_VERSIONS,
        src_agent: u64::from(dll_version),
        dst_agent: u64::from(PROTOCOL_VERSION),
        ..row(start_ms)
    });
    let drops = EvtcEvent {
        pad: SIG,
        skillid: ROW_DROPS,
        value: i32::try_from(dropped).unwrap_or(i32::MAX),
        ..row(end_ms)
    };
    (out, drops)
}

/// The arcdps_healing_stats addon's signature (Krappa322), as its rows carry it.
pub const HEALING_SIG: u32 = 0x9c9b_3c99; // public-check:ok
/// Its EVTC revision; Elite Insights reads revisions 1 and 2.
pub const HEALING_REVISION: u64 = 2;
/// `is_statechange` of the addon's combat rows (extension event `e10`, EXTENSION_COMBAT).
pub const STATECHANGE_EXTENSION_COMBAT: u8 = 49;
/// The version string axidps writes in the registration row. The addon writes its own version
/// (`2.20rc1` in the owner's logs); readers only display it.
pub const HEALING_VERSION: &[u8] = b"axidps";

/// Healing-extension flags in `is_offcycle` (the addon's `HealingEventFlags`): the target was
/// downed (buff heals only), the target sent the row, the source sent the row.
pub const HEAL_TARGET_DOWNED: u8 = 1 << 5;
pub const HEAL_FROM_DST: u8 = 1 << 6;
pub const HEAL_FROM_SRC: u8 = 1 << 7;

/// The healing extension's registration row, as the addon writes it with extension event `e9` at squad
/// combat start: `is_statechange` 40, `pad` 0, `src_agent` = signature | revision << 32 |
/// version length << 56, `dst_agent` = the version string's bytes.
pub fn healing_registration(time: u64) -> EvtcEvent {
    let mut version = [0u8; 8];
    version[..HEALING_VERSION.len()].copy_from_slice(HEALING_VERSION);
    EvtcEvent {
        time,
        is_statechange: STATECHANGE_EXTENSION,
        src_agent: u64::from(HEALING_SIG)
            | (HEALING_REVISION << 32)
            | ((HEALING_VERSION.len() as u64) << 56),
        dst_agent: u64::from_le_bytes(version),
        ..Default::default()
    }
}
