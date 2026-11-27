// crates/axidps-protocol/src/v2.rs
//! Protocol v2: a multi-producer ring of typed events.
//!
//! Every slot is 64 bytes: a commit sequence, a timestamp, a kind id and a 40-byte payload.
//! Payload fields are encoded explicitly little-endian at fixed offsets, in declaration order;
//! unused payload bytes are zero.

use std::path::PathBuf;

/// File magic: `AXR2` read as a little-endian u32.
pub const MAGIC: u32 = u32::from_le_bytes(*b"AXR2");
pub const PROTOCOL_VERSION: u32 = 2;
pub const SLOT_SIZE: usize = 64;
/// Number of slots. Must be a power of two.
pub const SLOT_COUNT: usize = 65_536;
pub const HEADER_SIZE: usize = 64;
pub const PAYLOAD_SIZE: usize = 40;
/// Total mapped size: header + slots.
pub const SHM_SIZE: usize = HEADER_SIZE + SLOT_SIZE * SLOT_COUNT;
pub const RING_FILE_NAME: &str = "axidps_ring_v2";
/// Maximum number of capture groups reported in the header's `group_state`.
pub const MAX_GROUPS: usize = 16;
/// `dll_version` reserved for `axidps-mock-producer`. Logs carrying it are not real captures,
/// and parity tooling skips them.
pub const MOCK_DLL_VERSION: u32 = u32::MAX;
/// An [`Event::AgentFlags`] flag that does not apply to the agent (e.g. `named` for a player).
pub const NOT_APPLICABLE: u8 = 2;
/// Header flag bit 0: the producer is active.
pub const FLAG_ACTIVE: u32 = 0x1;

/// Where the ring lives. The DLL and the reader must agree on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RingHost {
    /// A Linux/macOS reader: `/tmp`.
    Unix,
    /// The DLL under Wine/Proton: `Z:\tmp`, which is the host's `/tmp`.
    Wine,
    /// Native Windows (DLL and reader): the user's temp folder.
    Windows,
}

impl RingHost {
    /// The host this binary was built for. A Windows build cannot tell Wine apart at compile
    /// time; the DLL detects Wine at load and passes [`RingHost::Wine`] itself.
    pub fn current_build() -> RingHost {
        if cfg!(windows) {
            RingHost::Windows
        } else {
            RingHost::Unix
        }
    }

    /// The host a Windows DLL runs on, from two probes: ntdll exports `wine_get_version`, or
    /// the `HKCU\Software\Wine` key exists. Either means Wine; the key still exists when
    /// Wine-staging hides its exports.
    pub fn from_probes(wine_export: bool, wine_registry: bool) -> RingHost {
        if wine_export || wine_registry {
            RingHost::Wine
        } else {
            RingHost::Windows
        }
    }
}

/// Path of the v2 ring file for `host`.
pub fn ring_path(host: RingHost) -> PathBuf {
    match host {
        RingHost::Unix => PathBuf::from("/tmp").join(RING_FILE_NAME),
        RingHost::Wine => PathBuf::from(format!("Z:\\tmp\\{RING_FILE_NAME}")),
        RingHost::Windows => std::env::temp_dir().join(RING_FILE_NAME),
    }
}

/// Ring header, 64 bytes at the start of the file.
#[repr(C)]
#[derive(Debug)]
pub struct RingHeader {
    pub magic: u32,
    pub protocol: u32,
    /// Next claim index; producers `fetch_add` on it.
    pub reserve: u64,
    pub dll_version: u32,
    /// bit 0 = producer active.
    pub flags: u32,
    pub group_state: [u8; MAX_GROUPS],
    pub _reserved: [u8; 24],
}

/// One ring slot.
#[repr(C)]
#[derive(Debug)]
pub struct Slot {
    /// Commit marker: claim index + 1; 0 while being written.
    pub seq: u64,
    pub time_ms: u64,
    pub kind: u16,
    pub _pad: [u8; 6],
    pub payload: [u8; PAYLOAD_SIZE],
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentKind {
    Unknown = 0,
    Player = 1,
    Npc = 2,
    Gadget = 3,
}

impl AgentKind {
    fn from_u8(b: u8) -> Option<Self> {
        Some(match b {
            0 => Self::Unknown,
            1 => Self::Player,
            2 => Self::Npc,
            3 => Self::Gadget,
            _ => return None,
        })
    }
}

/// A gadget capture message (EVTC sc 80-82).
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureOp {
    /// Progress update (sc 81; also starts the capture, sc 80, if not started).
    Progress = 0,
    /// Capture started (sc 80).
    Start = 1,
    /// Capture stopped (sc 82).
    Stop = 2,
}

impl CaptureOp {
    fn from_u8(b: u8) -> Option<Self> {
        Some(match b {
            0 => Self::Progress,
            1 => Self::Start,
            2 => Self::Stop,
            _ => return None,
        })
    }
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameField {
    Character = 0,
    Account = 1,
}

impl NameField {
    fn from_u8(b: u8) -> Option<Self> {
        Some(match b {
            0 => Self::Character,
            1 => Self::Account,
            _ => return None,
        })
    }
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogAction {
    Start = 1,
    End = 2,
}

impl LogAction {
    fn from_u8(b: u8) -> Option<Self> {
        Some(match b {
            1 => Self::Start,
            2 => Self::End,
            _ => return None,
        })
    }
}

/// An agent's life state; core writes these as arcdps CHANGEUP / CHANGEDOWN / CHANGEDEAD.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentState {
    Up = 1,
    Down = 2,
    Dead = 3,
}

impl AgentState {
    fn from_u8(b: u8) -> Option<Self> {
        Some(match b {
            1 => Self::Up,
            2 => Self::Down,
            3 => Self::Dead,
            _ => return None,
        })
    }
}

/// Which vector a `Motion` event carries (EVTC statechanges 19, 20 and 21).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MotionKind {
    Position = 1,
    Velocity = 2,
    /// x and y only; z is 0.
    Facing = 3,
}

impl MotionKind {
    fn from_u8(b: u8) -> Option<Self> {
        Some(match b {
            1 => Self::Position,
            2 => Self::Velocity,
            3 => Self::Facing,
            _ => return None,
        })
    }
}

/// What a `BuffStack` event changes on one buff instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum StackChange {
    /// The stack starts ticking; `value` is its remaining ms.
    Active = 1,
    /// The stack stops ticking; `value` is its new duration ms.
    Reset = 2,
    /// `value` is the stack's new duration ms.
    Duration = 3,
}

impl StackChange {
    fn from_u8(b: u8) -> Option<Self> {
        Some(match b {
            1 => Self::Active,
            2 => Self::Reset,
            3 => Self::Duration,
            _ => return None,
        })
    }
}

/// Maximum bytes of an `AgentName` chunk.
pub const NAME_CHUNK_MAX: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// Kind 1.
    Status {
        unix_s: i64,
        dll_version: u32,
        group_state: [u8; MAX_GROUPS],
    },
    /// Kind 2. An upsert.
    AgentAdd {
        id: u32,
        kind: AgentKind,
        prof: u32,
        elite: u32,
        species: u32,
        team: u32,
        subgroup: u8,
        /// A player's WvW rank number; 0 = none or unread. Core turns it into
        /// arcdps's rank-title name for players outside the squad.
        wvw_rank: u32,
        /// The per-type index (1..=105) of the player's WvW rank def; 0 = none or unread. Core
        /// uses it when `wvw_rank` is 0.
        wvw_rank_def: u8,
    },
    /// Kind 3. A UTF-8 chunk of a name; `len <= 32` is the used part of `bytes`.
    AgentName {
        id: u32,
        field: NameField,
        offset: u8,
        total: u8,
        len: u8,
        bytes: [u8; NAME_CHUNK_MAX],
    },
    /// Kind 4.
    AgentRemove { id: u32 },
    /// Kind 5.
    Strike {
        src: u32,
        dst: u32,
        skill: u32,
        value: i32,
        overstack: u32,
        result: u8,
    },
    /// Kind 6.
    Combat { id: u32, in_combat: bool },
    /// Kind 7.
    Team { id: u32, team: u32 },
    /// Kind 8.
    Map {
        map_id: u32,
        map_type: u32,
        shard: u32,
        gw2_build: u32,
    },
    /// Kind 9.
    Health { id: u32, pct_x100: u16, max: u32 },
    /// Kind 10.
    Pov { id: u32 },
    /// Kind 11.
    LogControl { action: LogAction, reason: u8 },
    /// Kind 12.
    State { id: u32, state: AgentState },
    /// Kind 13. `xyz` are f32 bit patterns, in game units as arcdps writes them.
    Motion {
        id: u32,
        kind: MotionKind,
        xyz: [u32; 3],
    },
    /// Kind 14. A buff instance added to `dst`. `src` 0 = none; `duration` ms,
    /// `i32::MAX` = infinite; `active` = the stack ticks from the start.
    BuffApply {
        dst: u32,
        src: u32,
        buff: u32,
        inst: u32,
        duration: u32,
        level: u8,
        active: bool,
    },
    /// Kind 15. One buff instance removed from `dst`. `remover` 0 = none.
    BuffRemove { dst: u32, remover: u32, inst: u32 },
    /// Kind 16. Every instance of `buff` removed from `dst`.
    BuffRemoveAll { dst: u32, remover: u32, buff: u32 },
    /// Kind 17. A change to one instance on `dst`.
    BuffStack {
        dst: u32,
        inst: u32,
        change: StackChange,
        value: u32,
    },
    /// Kind 20 (18 and 19 are taken by casts). Barrier as a share of max health x10000.
    Barrier { id: u32, pct_x100: u16 },
    /// Kind 21. A marker on an agent (`tag` = marker definition id; 0 = clear), and whether it is
    /// a commander tag.
    Tag { id: u32, tag: u32, commander: bool },
    /// Kind 18. `agent` starts a cast. `target` 0 = none; `kind` is arcdps's start kind (1-9,
    /// written as the row's `result`); `x` is the start's extra value, or the pseudo-skill's
    /// value.
    CastStart {
        agent: u32,
        target: u32,
        skill: u32,
        kind: u8,
        x: u32,
    },
    /// Kind 19. A message that ends `agent`'s open cast, by arcdps end site code. Core applies
    /// arcdps's conditions (sites 12, 15, 22, 23, 27 depend on the open cast and agent flags).
    CastEnd { agent: u32, site: u8 },
    /// Kind 22. An agent's breakbar as the poll read it: `state` (0 active, 1 recover, 2 immune,
    /// 3 none) and the percent's f32 bits; (3, 0.0) when it has no breakbar object. Sent on
    /// change of either.
    Breakbar { id: u32, state: u16, pct_bits: u32 },
    /// Kind 23. A ground effect created (EVTC sc 60). `owner` 0 = none; `duration`
    /// ms, `u32::MAX` = infinite; `tracking` 0 = not trackable. `pos` is game units /10,
    /// `orient` radians x1000 (z only for a single angle, zeros for none), `scale` and `shields`
    /// x1000, all quantized as arcdps does. `flags` and `flanking` are the message's bytes arcdps
    /// copies.
    EffectGround {
        effect: u32,
        owner: u32,
        duration: u32,
        tracking: u32,
        pos: [i16; 3],
        orient: [i16; 3],
        scale: i16,
        shields: i16,
        flags: u8,
        flanking: u8,
    },
    /// Kind 24. A tracked ground effect ended (sc 61).
    EffectGroundRemove { tracking: u32 },
    /// Kind 25. An effect on agent `host` (sc 62). `src` 0 = none.
    EffectAgent {
        host: u32,
        src: u32,
        effect: u32,
        duration: u32,
        tracking: u32,
    },
    /// Kind 26. A tracked effect on `host` ended (sc 63).
    EffectAgentRemove { host: u32, tracking: u32 },
    /// Kind 27. A missile created (sc 57). `skin` = the missile's skin, `pos` game units /10;
    /// `word_1e`, `byte_22`, `byte_23` are the message fields arcdps copies (meaning unknown).
    MissileCreate {
        id: u32,
        owner: u32,
        skill: u32,
        skin: u32,
        word_1e: u32,
        pos: [i16; 3],
        byte_22: u8,
        byte_23: u8,
    },
    /// Kind 28. A missile launched (sc 58). `owner`, `skill` and `pos` come from
    /// the game missile object when `bits & MISSILE_OBJECT` (else 0: core falls back to the
    /// create). `bits & MISSILE_STILL`: the current position equals the previous one.
    MissileLaunch {
        id: u32,
        owner: u32,
        skill: u32,
        target: u32,
        target_pos: [i16; 3],
        pos: [i16; 3],
        radius: i16,
        speed: i16,
        flags: u32,
        motion: u8,
        bits: u8,
    },
    /// Kind 29. A missile removed (sc 59). `other` is the object's second agent, `pos` its last
    /// position; object fields are valid when `bits & MISSILE_OBJECT`.
    MissileRemove {
        id: u32,
        owner: u32,
        skill: u32,
        other: u32,
        pos: [i16; 3],
        bits: u8,
    },
    /// Kind 30. An effect on a missile (sc 79). `effect` and `value` as arcdps writes
    /// them; `owner` from the object when `bits & MISSILE_OBJECT`.
    MissileEffect {
        id: u32,
        owner: u32,
        effect: u32,
        value: u32,
        bits: u8,
    },
    /// Kind 31. A weapon set switch (EVTC sc 11): agent `id`'s
    /// new `set`, as the message carries it (sent even when unchanged). Core writes the previous
    /// set as the row's value.
    WeaponSwap { id: u32, set: u8 },
    /// Kind 32. An agent's action speed (EVTC sc 92): the f32's
    /// bits, e.g. 1.4925 with Quickness, 0.6667 with Slow. Sent on every message.
    ActionSpeed { id: u32, speed_bits: u32 },
    /// Kind 33. Glide state (sc 55): false when the message's three floats are all
    /// 0.0.
    Glider { id: u32, gliding: bool },
    /// Kind 34. Polled agent flags, each 0/1 or [`NOT_APPLICABLE`]: `targetable` (sc 24;
    /// characters and gadgets), `visible` (sc 76, characters; 0 while stealthed from me) and
    /// `named` (sc 78, gadgets). Sent on change.
    AgentFlags {
        id: u32,
        targetable: u8,
        visible: u8,
        named: u8,
    },
    /// Kind 35. Agent info (sc 89), once per agent life: gadgets `value` = gadget type,
    /// NPCs `buff_dmg` = species flags. Never for players.
    AgentInfo { id: u32, value: u32, buff_dmg: u32 },
    /// Kind 44. A gadget capture message: `progress_bits` = the progress
    /// f32 (on progress), `team` (on progress or start), `by` (on progress); zero otherwise.
    GadgetCapture {
        id: u32,
        op: CaptureOp,
        progress_bits: u32,
        team: u8,
        by: u8,
    },
    /// Kind 45. A gadget's model info (sc 87): `file` = the first 4 wchar of its model file
    /// reference (zero after the terminator), `propid` = the gadget's prop id, `flag36` = bit 36 of
    /// the gadget flags. Sent at first sight and on change.
    GadgetModel {
        id: u32,
        file: u64,
        propid: u64,
        flag36: bool,
    },
    /// Kind 36. A jump start (sc 86), with the row's `offcycle`. The end rides on the site-23
    /// end ([`Event::CastEnd`]), as in arcdps.
    Jump { id: u32, offcycle: u8 },
    /// Kind 37. A fly-to start (sc 88): the target and speed, each quantized like arcdps
    /// (clamped, /10, rounded). The end rides on site 23, like the jump's.
    FlyTo { id: u32, pos: [i16; 3], speed: i16 },
    /// Kind 38. A teleport (sc 85): `kind` and `arg` from the message, `dst` and `value` the target
    /// (x and y as f32 then z). Sent for every such message; core applies arcdps's condition and
    /// its squad-only rule for the target.
    Teleport {
        id: u32,
        kind: u8,
        arg: u32,
        dst: u64,
        value: u32,
    },
    /// Kind 39. A gadget animation (sc 77), with the animation `token`.
    GadgetAnimation { id: u32, token: u64 },
    /// Kind 40. A transformation (sc 73): timed (`duration` ms, `shields` false) or untimed
    /// (`duration` 0, `shields` true). `transform` 0 = back to normal; `offcycle` as arcdps
    /// writes it.
    Transform {
        id: u32,
        transform: u32,
        offcycle: u8,
        duration: u32,
        shields: bool,
    },
    /// Kind 41. The game server's tick and the values arcdps writes with it.
    /// Pushed when the tick on the game's strike messages (arcdps's source) changed and at least
    /// `SERVER_TICK_MIN_MS` passed since the last push; the event time is the "now" of the tick.
    /// `ping` = last ping ms (sc 84 value; 0 = unread), `server_ip` = the game server's IPv4 as
    /// stored in a `sockaddr_in` (sc 42 value; 0 = unread), `shard_ids` = the map load's
    /// shard ids (sc 16: low 16 bits src, high 16 bits dst; 0 = unread).
    ServerTick {
        tick: u32,
        ping: u32,
        server_ip: u32,
        shard_ids: u32,
    },
    /// Kind 46. An agent's master (owner) changed: `master` is its instance id, 0 = none.
    /// NPCs: the server-set master id, when that character exists; gadgets: the owner id. Sent
    /// on change, so a first read of 0 sends nothing. Core writes it as src/dst master instid.
    AgentMaster { id: u32, master: u32 },
    /// Kind 42. A condition (buff) damage tick the game reports for the local player, dealt or
    /// taken: a combat hit on a buff skill with a non-zero stack count. `src` 0 = no
    /// source agent.
    /// `value` = health plus barrier damage (each truncated, as arcdps sums them; never a heal);
    /// `barrier` = the part barrier absorbed; `stacks` = the hit's stack count.
    BuffDamage {
        src: u32,
        dst: u32,
        buff: u32,
        value: i32,
        barrier: u32,
        stacks: u8,
    },
    /// Kind 43. A heal or barrier the game reports for the local player (or its pets and minions),
    /// given or received: a combat hit with a positive amount. Core writes it as a
    /// healing-extension row. `value` = health healed plus barrier given (each truncated, as arcdps
    /// sums them; always > 0); `barrier` = the barrier part; `buff` = the skill is a buff
    /// (regeneration and other heal-over-time ticks), else a direct heal.
    Heal {
        src: u32,
        dst: u32,
        skill: u32,
        value: i32,
        barrier: u32,
        buff: bool,
    },
    /// Kind 60. The 16-byte key of the player's current squad/party group, shared by every
    /// member so core can tell which peers heal-relay with each other. All zero = solo (no
    /// group).
    GroupKey { key: [u8; 16] },
    /// Kind 61. A heal or barrier relayed by a peer player: same meaning as `Heal` (`value` =
    /// health healed plus barrier, always > 0) plus `sender`, the agent id of the peer who sent
    /// it, and `downed`, the target's downed flag as the sender saw it. Produced only by core,
    /// never by the DLL.
    RelayHeal {
        sender: u32,
        src: u32,
        dst: u32,
        skill: u32,
        value: i32,
        barrier: u32,
        buff: bool,
        downed: bool,
    },
    /// Kind 62. A peer entered (`in_combat`) or left combat, as that peer's own client saw it,
    /// sent over the heal relay: `sender` is the peer's agent id. Its relayed heals count over
    /// this combat, as the healing addon counts its peers'. Produced only by core.
    RelayCombat { sender: u32, in_combat: bool },
    /// Kind 47. A player's guild changed: the 16-byte guild GUID as the game keeps it
    /// (client byte order; all zero = none). Sent on change, so a first read of zero sends
    /// nothing. Core writes it as arcdps's GUILD rows (sc 29) for the squad at log start.
    AgentGuild { id: u32, guid: [u8; 16] },
    /// Kind 48. A player's account GUID: 16 bytes (client byte order), the key arcdps gives
    /// every player but the POV, enemies included. Sent once per life when first
    /// read non-zero, again if it changes, and with the identity re-push; never zero. Core merges
    /// a player's lives in a fight by it.
    AgentGuid { id: u32, guid: [u8; 16] },
    /// Kind 49. A player's core stats as the game holds them: level and the effective
    /// attributes (12; index 1 power … 7 condition damage, 9 expertise), clamped to i16. Sent on
    /// change. Source stats for the condition model; whether the server fills them for other
    /// players is still open.
    CoreStats {
        id: u32,
        level: u16,
        attrs: [i16; 12],
    },
    /// Kind 50. One modifier of a buff's formula from game content, sent once per buff id the first
    /// time it is applied in a session. `index` of `count`; `category` = the buff's category
    /// (2 = condition); `target` = the modified stat, `ftype` = the formula type (0..=15),
    /// `base`/`c1`/`c2` = the formula's f32 bits, `mode` = the game-mode context (0 any, 1 not
    /// PvP, 2 PvP, 3 WvW, 4 PvE). `prereq`: bit 7 = read (0 in recordings from before it was
    /// sent), bits 0-3 = which of the four prerequisite refs are set (traits and buffs the
    /// modifier needs).
    BuffFormula {
        buff: u32,
        index: u8,
        count: u8,
        category: u8,
        ftype: u8,
        target: i32,
        /// f32 bits (the enum is `Eq`).
        base: u32,
        c1: u32,
        c2: u32,
        mode: u32,
        prereq: u8,
    },
    /// Kind 51. One point of a gadget's capture outline (EVTC sc 83), read from the gadget's
    /// shape (0 circle, 1 polygon). A circle is one point (radius, the
    /// f32 after it); a polygon `count` points (x, y). `index` of `count`; `count` 0 (index 0)
    /// means the gadget has no outline now. Sent on change. Floats as bits.
    GadgetShape {
        id: u32,
        index: u8,
        count: u8,
        x: u32,
        y: u32,
    },
    /// Kind 52. A squad ground marker slot (EVTC sc 53): x, y, z f32 bits of the marker's position
    /// (slots 0..8, +inf = empty), read while in a squad. Sent per changed slot, all 8 on joining a
    /// squad and with the identity re-push. `slot` 0xff: not in a squad now (core forgets them).
    SquadMarker { slot: u8, x: u32, y: u32, z: u32 },
    /// Kind 53. A WvW objective (claimable) message (EVTC sc 75), read after the game handled
    /// it: `op` 0 = progress (`value` = upgrade progress), 1 = owner (`value` = owner team id).
    /// `guid` = the claimable's; `map_id` and `objective_id`/`objective_type` from the claimable
    /// and its content definition (`objective_id` 0 = not in the table).
    WvwObjective {
        op: u8,
        guid: [u8; 16],
        map_id: u32,
        objective_id: u32,
        objective_type: u8,
        value: u32,
    },
    /// Kind 54. A reward (EVTC sc 17): `kind` = the reward type, `id` = the reward id, as
    /// arcdps writes them.
    Reward { id: u32, kind: u16 },
    /// Kind 55. A WvW team's colour (EVTC sc 74): from the team's content definition:
    /// `team` = its id, `colour` 1 red / 2 blue / 3 green by the definition's GUID (0 = none of
    /// the three). Sent once per team id a session, from the objective owner messages.
    WvwTeam { team: u32, colour: u8 },
    /// Kind 56. An attack target (game agent category 11) and its parent gadget
    /// (EVTC sc 23). Sent on first sight, on a parent change and
    /// with the identity re-push; the attack target's own ident comes as a gadget named
    /// `at<parent pseudo>-<id>-<parent id>`. Core makes the parent its master.
    AttackTarget { id: u32, parent: u32 },
    /// Kind 57. A WvW colour's world (EVTC sc 74 world ids): `colour` 1 red / 2 blue / 3 green,
    /// `world` = the world id (the 11001.. ids). Read when the game resolves the colour's
    /// guest world, as arcdps does; sent on a change.
    WvwWorld { colour: u8, world: u32 },
    /// Kind 58. A player's attribute points from equipped gear, as arcdps rebuilds them:
    /// per equipped item, `round(adjustment ×
    /// multiplier)` for each itemstat entry, summed by attribute id (`attrs[i]` = CHAR_ATTRIBUTE
    /// i: 1 Power … 7 Condition Damage … 9 Expertise). `alt` 0: the itemstats' normal lists; 1:
    /// their alternate (WvW) list where it has entries. The flat parts are [`Event::GearSkill`].
    /// Sent on change.
    GearAttrs { id: u32, alt: u8, attrs: [i16; 12] },
    /// Kind 59. One stat skill of a player's gear (itemstat, infusion, upgrade, rune tier, relic):
    /// `skill` with `n` copies, `index` of `count` (count 0: none). Its modifiers come as
    /// [`Event::BuffFormula`] keyed by the skill id. Sent on change.
    GearSkill {
        id: u32,
        index: u8,
        count: u8,
        skill: u32,
        n: u16,
    },
}

/// Minimum gap between two [`Event::ServerTick`] pushes.
pub const SERVER_TICK_MIN_MS: u64 = 100;

/// `bits` flag of the missile events: the game missile object was read.
pub const MISSILE_OBJECT: u8 = 1;
/// `bits` flag of `MissileLaunch`: the missile's current position equals its previous one.
pub const MISSILE_STILL: u8 = 2;

/// The `AgentName` chunks for `name`: at most `NAME_CHUNK_MAX` bytes each, split on char
/// boundaries, with the whole name clipped to 255 bytes (the `total` field is a u8) on a char
/// boundary. Does not allocate, so it can run on a game thread.
pub fn name_events(id: u32, field: NameField, name: &str) -> NameChunks<'_> {
    let total = floor_char_boundary(name, usize::from(u8::MAX));
    NameChunks {
        id,
        field,
        name: name.get(..total).unwrap_or(""),
        pos: 0,
    }
}

/// Iterator returned by [`name_events`].
pub struct NameChunks<'a> {
    id: u32,
    field: NameField,
    name: &'a str,
    pos: usize,
}

impl Iterator for NameChunks<'_> {
    type Item = Event;

    fn next(&mut self) -> Option<Event> {
        let rest = self.name.get(self.pos..).filter(|r| !r.is_empty())?;
        let len = floor_char_boundary(rest, NAME_CHUNK_MAX);
        let part = rest.get(..len)?.as_bytes();
        let mut bytes = [0u8; NAME_CHUNK_MAX];
        bytes.get_mut(..len)?.copy_from_slice(part);
        let ev = Event::AgentName {
            id: self.id,
            field: self.field,
            offset: self.pos as u8,
            total: self.name.len() as u8,
            len: len as u8,
            bytes,
        };
        self.pos += len;
        Some(ev)
    }
}

/// The largest char boundary of `s` that is at most `max`.
fn floor_char_boundary(s: &str, max: usize) -> usize {
    if s.len() <= max {
        return s.len();
    }
    (0..=max)
        .rev()
        .find(|&i| s.is_char_boundary(i))
        .unwrap_or(0)
}

/// Little-endian payload writer. Out-of-range writes are ignored (never panics).
struct Put {
    buf: [u8; PAYLOAD_SIZE],
    pos: usize,
}

impl Put {
    fn new() -> Self {
        Self {
            buf: [0; PAYLOAD_SIZE],
            pos: 0,
        }
    }
    fn bytes(&mut self, b: &[u8]) -> &mut Self {
        if let Some(dst) = self
            .pos
            .checked_add(b.len())
            .and_then(|e| self.buf.get_mut(self.pos..e))
        {
            dst.copy_from_slice(b);
        }
        self.pos += b.len();
        self
    }
    fn u8(&mut self, v: u8) -> &mut Self {
        self.bytes(&[v])
    }
    fn u16(&mut self, v: u16) -> &mut Self {
        self.bytes(&v.to_le_bytes())
    }
    fn u32(&mut self, v: u32) -> &mut Self {
        self.bytes(&v.to_le_bytes())
    }
    fn i16(&mut self, v: i16) -> &mut Self {
        self.bytes(&v.to_le_bytes())
    }
    fn i16x3(&mut self, v: [i16; 3]) -> &mut Self {
        self.i16(v[0]).i16(v[1]).i16(v[2])
    }
    fn i32(&mut self, v: i32) -> &mut Self {
        self.bytes(&v.to_le_bytes())
    }
    fn i64(&mut self, v: i64) -> &mut Self {
        self.bytes(&v.to_le_bytes())
    }
    fn u64(&mut self, v: u64) -> &mut Self {
        self.bytes(&v.to_le_bytes())
    }
}

/// Little-endian payload reader. Returns `None` when out of range.
struct Take<'a> {
    buf: &'a [u8; PAYLOAD_SIZE],
    pos: usize,
}

impl Take<'_> {
    fn arr<const N: usize>(&mut self) -> Option<[u8; N]> {
        let end = self.pos.checked_add(N)?;
        let out = self.buf.get(self.pos..end)?.try_into().ok()?;
        self.pos = end;
        Some(out)
    }
    fn u8(&mut self) -> Option<u8> {
        self.arr::<1>().map(|a| a[0])
    }
    fn u16(&mut self) -> Option<u16> {
        self.arr().map(u16::from_le_bytes)
    }
    fn u32(&mut self) -> Option<u32> {
        self.arr().map(u32::from_le_bytes)
    }
    fn i16(&mut self) -> Option<i16> {
        self.arr().map(i16::from_le_bytes)
    }
    fn i16x3(&mut self) -> Option<[i16; 3]> {
        Some([self.i16()?, self.i16()?, self.i16()?])
    }
    fn i32(&mut self) -> Option<i32> {
        self.arr().map(i32::from_le_bytes)
    }
    fn i64(&mut self) -> Option<i64> {
        self.arr().map(i64::from_le_bytes)
    }
    fn u64(&mut self) -> Option<u64> {
        self.arr().map(u64::from_le_bytes)
    }
}

impl Event {
    /// The kind id written to the slot.
    pub fn kind(&self) -> u16 {
        match self {
            Event::Status { .. } => 1,
            Event::AgentAdd { .. } => 2,
            Event::AgentName { .. } => 3,
            Event::AgentRemove { .. } => 4,
            Event::Strike { .. } => 5,
            Event::Combat { .. } => 6,
            Event::Team { .. } => 7,
            Event::Map { .. } => 8,
            Event::Health { .. } => 9,
            Event::Pov { .. } => 10,
            Event::LogControl { .. } => 11,
            Event::State { .. } => 12,
            Event::Motion { .. } => 13,
            Event::BuffApply { .. } => 14,
            Event::BuffRemove { .. } => 15,
            Event::BuffRemoveAll { .. } => 16,
            Event::BuffStack { .. } => 17,
            Event::Barrier { .. } => 20,
            Event::Tag { .. } => 21,
            Event::CastStart { .. } => 18,
            Event::CastEnd { .. } => 19,
            Event::Breakbar { .. } => 22,
            Event::EffectGround { .. } => 23,
            Event::EffectGroundRemove { .. } => 24,
            Event::EffectAgent { .. } => 25,
            Event::EffectAgentRemove { .. } => 26,
            Event::MissileCreate { .. } => 27,
            Event::MissileLaunch { .. } => 28,
            Event::MissileRemove { .. } => 29,
            Event::MissileEffect { .. } => 30,
            Event::WeaponSwap { .. } => 31,
            Event::ActionSpeed { .. } => 32,
            Event::Glider { .. } => 33,
            Event::AgentFlags { .. } => 34,
            Event::AgentInfo { .. } => 35,
            Event::GadgetCapture { .. } => 44,
            Event::GadgetModel { .. } => 45,
            Event::Jump { .. } => 36,
            Event::FlyTo { .. } => 37,
            Event::Teleport { .. } => 38,
            Event::GadgetAnimation { .. } => 39,
            Event::Transform { .. } => 40,
            Event::ServerTick { .. } => 41,
            Event::AgentMaster { .. } => 46,
            Event::AgentGuild { .. } => 47,
            Event::AgentGuid { .. } => 48,
            Event::CoreStats { .. } => 49,
            Event::BuffFormula { .. } => 50,
            Event::GadgetShape { .. } => 51,
            Event::SquadMarker { .. } => 52,
            Event::WvwObjective { .. } => 53,
            Event::Reward { .. } => 54,
            Event::WvwTeam { .. } => 55,
            Event::AttackTarget { .. } => 56,
            Event::WvwWorld { .. } => 57,
            Event::GearAttrs { .. } => 58,
            Event::GearSkill { .. } => 59,
            Event::BuffDamage { .. } => 42,
            Event::Heal { .. } => 43,
            Event::GroupKey { .. } => 60,
            Event::RelayHeal { .. } => 61,
            Event::RelayCombat { .. } => 62,
        }
    }

    /// Encode to `(kind, payload)`; unused payload bytes are zero.
    pub fn encode(&self) -> (u16, [u8; PAYLOAD_SIZE]) {
        let mut p = Put::new();
        match *self {
            Event::Status {
                unix_s,
                dll_version,
                group_state,
            } => {
                p.i64(unix_s).u32(dll_version).bytes(&group_state);
            }
            Event::AgentAdd {
                id,
                kind,
                prof,
                elite,
                species,
                team,
                subgroup,
                wvw_rank,
                wvw_rank_def,
            } => {
                p.u32(id)
                    .u8(kind as u8)
                    .u32(prof)
                    .u32(elite)
                    .u32(species)
                    .u32(team)
                    .u8(subgroup)
                    .u32(wvw_rank)
                    .u8(wvw_rank_def);
            }
            Event::AgentName {
                id,
                field,
                offset,
                total,
                len,
                bytes,
            } => {
                p.u32(id)
                    .u8(field as u8)
                    .u8(offset)
                    .u8(total)
                    .u8(len)
                    .bytes(&bytes);
            }
            Event::AgentRemove { id } => {
                p.u32(id);
            }
            Event::Strike {
                src,
                dst,
                skill,
                value,
                overstack,
                result,
            } => {
                p.u32(src)
                    .u32(dst)
                    .u32(skill)
                    .i32(value)
                    .u32(overstack)
                    .u8(result);
            }
            Event::Combat { id, in_combat } => {
                p.u32(id).u8(in_combat as u8);
            }
            Event::Team { id, team } => {
                p.u32(id).u32(team);
            }
            Event::Map {
                map_id,
                map_type,
                shard,
                gw2_build,
            } => {
                p.u32(map_id).u32(map_type).u32(shard).u32(gw2_build);
            }
            Event::Health { id, pct_x100, max } => {
                p.u32(id).u16(pct_x100).u32(max);
            }
            Event::Pov { id } => {
                p.u32(id);
            }
            Event::LogControl { action, reason } => {
                p.u8(action as u8).u8(reason);
            }
            Event::State { id, state } => {
                p.u32(id).u8(state as u8);
            }
            Event::Motion { id, kind, xyz } => {
                p.u32(id).u8(kind as u8).u32(xyz[0]).u32(xyz[1]).u32(xyz[2]);
            }
            Event::BuffApply {
                dst,
                src,
                buff,
                inst,
                duration,
                level,
                active,
            } => {
                p.u32(dst)
                    .u32(src)
                    .u32(buff)
                    .u32(inst)
                    .u32(duration)
                    .u8(level)
                    .u8(active as u8);
            }
            Event::BuffRemove { dst, remover, inst } => {
                p.u32(dst).u32(remover).u32(inst);
            }
            Event::BuffRemoveAll { dst, remover, buff } => {
                p.u32(dst).u32(remover).u32(buff);
            }
            Event::BuffStack {
                dst,
                inst,
                change,
                value,
            } => {
                p.u32(dst).u32(inst).u8(change as u8).u32(value);
            }
            Event::Tag { id, tag, commander } => {
                p.u32(id).u32(tag).u8(commander as u8);
            }
            Event::Barrier { id, pct_x100 } => {
                p.u32(id).u16(pct_x100);
            }
            Event::CastStart {
                agent,
                target,
                skill,
                kind,
                x,
            } => {
                p.u32(agent).u32(target).u32(skill).u8(kind).u32(x);
            }
            Event::Breakbar {
                id,
                state,
                pct_bits,
            } => {
                p.u32(id).u16(state).u32(pct_bits);
            }
            Event::CastEnd { agent, site } => {
                p.u32(agent).u8(site);
            }
            Event::EffectGround {
                effect,
                owner,
                duration,
                tracking,
                pos,
                orient,
                scale,
                shields,
                flags,
                flanking,
            } => {
                p.u32(effect)
                    .u32(owner)
                    .u32(duration)
                    .u32(tracking)
                    .i16x3(pos)
                    .i16x3(orient)
                    .i16(scale)
                    .i16(shields)
                    .u8(flags)
                    .u8(flanking);
            }
            Event::EffectGroundRemove { tracking } => {
                p.u32(tracking);
            }
            Event::EffectAgent {
                host,
                src,
                effect,
                duration,
                tracking,
            } => {
                p.u32(host).u32(src).u32(effect).u32(duration).u32(tracking);
            }
            Event::EffectAgentRemove { host, tracking } => {
                p.u32(host).u32(tracking);
            }
            Event::MissileCreate {
                id,
                owner,
                skill,
                skin,
                word_1e,
                pos,
                byte_22,
                byte_23,
            } => {
                p.u32(id)
                    .u32(owner)
                    .u32(skill)
                    .u32(skin)
                    .u32(word_1e)
                    .i16x3(pos)
                    .u8(byte_22)
                    .u8(byte_23);
            }
            Event::MissileLaunch {
                id,
                owner,
                skill,
                target,
                target_pos,
                pos,
                radius,
                speed,
                flags,
                motion,
                bits,
            } => {
                p.u32(id)
                    .u32(owner)
                    .u32(skill)
                    .u32(target)
                    .i16x3(target_pos)
                    .i16x3(pos)
                    .i16(radius)
                    .i16(speed)
                    .u32(flags)
                    .u8(motion)
                    .u8(bits);
            }
            Event::MissileRemove {
                id,
                owner,
                skill,
                other,
                pos,
                bits,
            } => {
                p.u32(id)
                    .u32(owner)
                    .u32(skill)
                    .u32(other)
                    .i16x3(pos)
                    .u8(bits);
            }
            Event::MissileEffect {
                id,
                owner,
                effect,
                value,
                bits,
            } => {
                p.u32(id).u32(owner).u32(effect).u32(value).u8(bits);
            }
            Event::WeaponSwap { id, set } => {
                p.u32(id).u8(set);
            }
            Event::ActionSpeed { id, speed_bits } => {
                p.u32(id).u32(speed_bits);
            }
            Event::Glider { id, gliding } => {
                p.u32(id).u8(gliding as u8);
            }
            Event::AgentFlags {
                id,
                targetable,
                visible,
                named,
            } => {
                p.u32(id).u8(targetable).u8(visible).u8(named);
            }
            Event::AgentInfo {
                id,
                value,
                buff_dmg,
            } => {
                p.u32(id).u32(value).u32(buff_dmg);
            }
            Event::GadgetCapture {
                id,
                op,
                progress_bits,
                team,
                by,
            } => {
                p.u32(id).u8(op as u8).u32(progress_bits).u8(team).u8(by);
            }
            Event::GadgetModel {
                id,
                file,
                propid,
                flag36,
            } => {
                p.u32(id).u64(file).u64(propid).u8(flag36 as u8);
            }
            Event::Jump { id, offcycle } => {
                p.u32(id).u8(offcycle);
            }
            Event::FlyTo { id, pos, speed } => {
                p.u32(id).i16x3(pos).i16(speed);
            }
            Event::Teleport {
                id,
                kind,
                arg,
                dst,
                value,
            } => {
                p.u32(id).u8(kind).u32(arg).i64(dst as i64).u32(value);
            }
            Event::GadgetAnimation { id, token } => {
                p.u32(id).i64(token as i64);
            }
            Event::Transform {
                id,
                transform,
                offcycle,
                duration,
                shields,
            } => {
                p.u32(id)
                    .u32(transform)
                    .u8(offcycle)
                    .u32(duration)
                    .u8(shields as u8);
            }
            Event::ServerTick {
                tick,
                ping,
                server_ip,
                shard_ids,
            } => {
                p.u32(tick).u32(ping).u32(server_ip).u32(shard_ids);
            }
            Event::AgentMaster { id, master } => {
                p.u32(id).u32(master);
            }
            Event::AgentGuild { id, guid } | Event::AgentGuid { id, guid } => {
                p.u32(id).bytes(&guid);
            }
            Event::CoreStats { id, level, attrs } => {
                p.u32(id).u16(level);
                for a in attrs {
                    p.i16(a);
                }
            }
            Event::BuffFormula {
                buff,
                index,
                count,
                category,
                ftype,
                target,
                base,
                c1,
                c2,
                mode,
                prereq,
            } => {
                p.u32(buff)
                    .u8(index)
                    .u8(count)
                    .u8(category)
                    .u8(ftype)
                    .i32(target)
                    .u32(base)
                    .u32(c1)
                    .u32(c2)
                    .u32(mode)
                    .u8(prereq);
            }
            Event::GadgetShape {
                id,
                index,
                count,
                x,
                y,
            } => {
                p.u32(id).u8(index).u8(count).u32(x).u32(y);
            }
            Event::SquadMarker { slot, x, y, z } => {
                p.u8(slot).u32(x).u32(y).u32(z);
            }
            Event::WvwObjective {
                op,
                guid,
                map_id,
                objective_id,
                objective_type,
                value,
            } => {
                p.u8(op)
                    .bytes(&guid)
                    .u32(map_id)
                    .u32(objective_id)
                    .u8(objective_type)
                    .u32(value);
            }
            Event::Reward { id, kind } => {
                p.u32(id).u16(kind);
            }
            Event::WvwTeam { team, colour } => {
                p.u32(team).u8(colour);
            }
            Event::AttackTarget { id, parent } => {
                p.u32(id).u32(parent);
            }
            Event::WvwWorld { colour, world } => {
                p.u8(colour).u32(world);
            }
            Event::GearAttrs { id, alt, attrs } => {
                p.u32(id).u8(alt);
                for a in attrs {
                    p.i16(a);
                }
            }
            Event::GearSkill {
                id,
                index,
                count,
                skill,
                n,
            } => {
                p.u32(id).u8(index).u8(count).u32(skill).u16(n);
            }
            Event::BuffDamage {
                src,
                dst,
                buff,
                value,
                barrier,
                stacks,
            } => {
                p.u32(src)
                    .u32(dst)
                    .u32(buff)
                    .i32(value)
                    .u32(barrier)
                    .u8(stacks);
            }
            Event::Heal {
                src,
                dst,
                skill,
                value,
                barrier,
                buff,
            } => {
                p.u32(src)
                    .u32(dst)
                    .u32(skill)
                    .i32(value)
                    .u32(barrier)
                    .u8(buff as u8);
            }
            Event::GroupKey { key } => {
                p.bytes(&key);
            }
            Event::RelayHeal {
                sender,
                src,
                dst,
                skill,
                value,
                barrier,
                buff,
                downed,
            } => {
                p.u32(sender)
                    .u32(src)
                    .u32(dst)
                    .u32(skill)
                    .i32(value)
                    .u32(barrier)
                    .u8(buff as u8)
                    .u8(downed as u8);
            }
            Event::RelayCombat { sender, in_combat } => {
                p.u32(sender).u8(in_combat as u8);
            }
        }
        (self.kind(), p.buf)
    }

    /// Decode a slot. `None` for an unknown kind, an unknown enum byte, a `bool` byte other
    /// than 0/1, or an `AgentName` whose `len` exceeds the chunk size.
    pub fn decode(kind: u16, payload: &[u8; PAYLOAD_SIZE]) -> Option<Event> {
        let mut t = Take {
            buf: payload,
            pos: 0,
        };
        Some(match kind {
            1 => Event::Status {
                unix_s: t.i64()?,
                dll_version: t.u32()?,
                group_state: t.arr()?,
            },
            2 => Event::AgentAdd {
                id: t.u32()?,
                kind: AgentKind::from_u8(t.u8()?)?,
                prof: t.u32()?,
                elite: t.u32()?,
                species: t.u32()?,
                team: t.u32()?,
                subgroup: t.u8()?,
                wvw_rank: t.u32()?,
                wvw_rank_def: t.u8()?,
            },
            3 => {
                let id = t.u32()?;
                let field = NameField::from_u8(t.u8()?)?;
                let offset = t.u8()?;
                let total = t.u8()?;
                let len = t.u8()?;
                if usize::from(len) > NAME_CHUNK_MAX {
                    return None;
                }
                Event::AgentName {
                    id,
                    field,
                    offset,
                    total,
                    len,
                    bytes: t.arr()?,
                }
            }
            4 => Event::AgentRemove { id: t.u32()? },
            5 => Event::Strike {
                src: t.u32()?,
                dst: t.u32()?,
                skill: t.u32()?,
                value: t.i32()?,
                overstack: t.u32()?,
                result: t.u8()?,
            },
            6 => Event::Combat {
                id: t.u32()?,
                in_combat: match t.u8()? {
                    0 => false,
                    1 => true,
                    _ => return None,
                },
            },
            7 => Event::Team {
                id: t.u32()?,
                team: t.u32()?,
            },
            8 => Event::Map {
                map_id: t.u32()?,
                map_type: t.u32()?,
                shard: t.u32()?,
                gw2_build: t.u32()?,
            },
            9 => Event::Health {
                id: t.u32()?,
                pct_x100: t.u16()?,
                max: t.u32()?,
            },
            10 => Event::Pov { id: t.u32()? },
            11 => Event::LogControl {
                action: LogAction::from_u8(t.u8()?)?,
                reason: t.u8()?,
            },
            12 => Event::State {
                id: t.u32()?,
                state: AgentState::from_u8(t.u8()?)?,
            },
            13 => Event::Motion {
                id: t.u32()?,
                kind: MotionKind::from_u8(t.u8()?)?,
                xyz: [t.u32()?, t.u32()?, t.u32()?],
            },
            14 => Event::BuffApply {
                dst: t.u32()?,
                src: t.u32()?,
                buff: t.u32()?,
                inst: t.u32()?,
                duration: t.u32()?,
                level: t.u8()?,
                active: match t.u8()? {
                    0 => false,
                    1 => true,
                    _ => return None,
                },
            },
            15 => Event::BuffRemove {
                dst: t.u32()?,
                remover: t.u32()?,
                inst: t.u32()?,
            },
            16 => Event::BuffRemoveAll {
                dst: t.u32()?,
                remover: t.u32()?,
                buff: t.u32()?,
            },
            17 => Event::BuffStack {
                dst: t.u32()?,
                inst: t.u32()?,
                change: StackChange::from_u8(t.u8()?)?,
                value: t.u32()?,
            },
            21 => Event::Tag {
                id: t.u32()?,
                tag: t.u32()?,
                commander: match t.u8()? {
                    0 => false,
                    1 => true,
                    _ => return None,
                },
            },
            20 => Event::Barrier {
                id: t.u32()?,
                pct_x100: t.u16()?,
            },
            18 => Event::CastStart {
                agent: t.u32()?,
                target: t.u32()?,
                skill: t.u32()?,
                kind: t.u8()?,
                x: t.u32()?,
            },
            22 => Event::Breakbar {
                id: t.u32()?,
                state: t.u16()?,
                pct_bits: t.u32()?,
            },
            19 => Event::CastEnd {
                agent: t.u32()?,
                site: t.u8()?,
            },
            23 => Event::EffectGround {
                effect: t.u32()?,
                owner: t.u32()?,
                duration: t.u32()?,
                tracking: t.u32()?,
                pos: t.i16x3()?,
                orient: t.i16x3()?,
                scale: t.i16()?,
                shields: t.i16()?,
                flags: t.u8()?,
                flanking: t.u8()?,
            },
            24 => Event::EffectGroundRemove { tracking: t.u32()? },
            25 => Event::EffectAgent {
                host: t.u32()?,
                src: t.u32()?,
                effect: t.u32()?,
                duration: t.u32()?,
                tracking: t.u32()?,
            },
            26 => Event::EffectAgentRemove {
                host: t.u32()?,
                tracking: t.u32()?,
            },
            27 => Event::MissileCreate {
                id: t.u32()?,
                owner: t.u32()?,
                skill: t.u32()?,
                skin: t.u32()?,
                word_1e: t.u32()?,
                pos: t.i16x3()?,
                byte_22: t.u8()?,
                byte_23: t.u8()?,
            },
            28 => Event::MissileLaunch {
                id: t.u32()?,
                owner: t.u32()?,
                skill: t.u32()?,
                target: t.u32()?,
                target_pos: t.i16x3()?,
                pos: t.i16x3()?,
                radius: t.i16()?,
                speed: t.i16()?,
                flags: t.u32()?,
                motion: t.u8()?,
                bits: t.u8()?,
            },
            29 => Event::MissileRemove {
                id: t.u32()?,
                owner: t.u32()?,
                skill: t.u32()?,
                other: t.u32()?,
                pos: t.i16x3()?,
                bits: t.u8()?,
            },
            30 => Event::MissileEffect {
                id: t.u32()?,
                owner: t.u32()?,
                effect: t.u32()?,
                value: t.u32()?,
                bits: t.u8()?,
            },
            31 => Event::WeaponSwap {
                id: t.u32()?,
                set: t.u8()?,
            },
            32 => Event::ActionSpeed {
                id: t.u32()?,
                speed_bits: t.u32()?,
            },
            34 => Event::AgentFlags {
                id: t.u32()?,
                targetable: t.u8()?,
                visible: t.u8()?,
                named: t.u8()?,
            },
            35 => Event::AgentInfo {
                id: t.u32()?,
                value: t.u32()?,
                buff_dmg: t.u32()?,
            },
            44 => Event::GadgetCapture {
                id: t.u32()?,
                op: CaptureOp::from_u8(t.u8()?)?,
                progress_bits: t.u32()?,
                team: t.u8()?,
                by: t.u8()?,
            },
            45 => Event::GadgetModel {
                id: t.u32()?,
                file: t.u64()?,
                propid: t.u64()?,
                flag36: match t.u8()? {
                    0 => false,
                    1 => true,
                    _ => return None,
                },
            },
            36 => Event::Jump {
                id: t.u32()?,
                offcycle: t.u8()?,
            },
            37 => Event::FlyTo {
                id: t.u32()?,
                pos: t.i16x3()?,
                speed: t.i16()?,
            },
            38 => Event::Teleport {
                id: t.u32()?,
                kind: t.u8()?,
                arg: t.u32()?,
                dst: t.i64()? as u64,
                value: t.u32()?,
            },
            39 => Event::GadgetAnimation {
                id: t.u32()?,
                token: t.i64()? as u64,
            },
            40 => Event::Transform {
                id: t.u32()?,
                transform: t.u32()?,
                offcycle: t.u8()?,
                duration: t.u32()?,
                shields: match t.u8()? {
                    0 => false,
                    1 => true,
                    _ => return None,
                },
            },
            46 => Event::AgentMaster {
                id: t.u32()?,
                master: t.u32()?,
            },
            42 => Event::BuffDamage {
                src: t.u32()?,
                dst: t.u32()?,
                buff: t.u32()?,
                value: t.i32()?,
                barrier: t.u32()?,
                stacks: t.u8()?,
            },
            43 => Event::Heal {
                src: t.u32()?,
                dst: t.u32()?,
                skill: t.u32()?,
                value: t.i32()?,
                barrier: t.u32()?,
                buff: match t.u8()? {
                    0 => false,
                    1 => true,
                    _ => return None,
                },
            },
            60 => Event::GroupKey { key: t.arr()? },
            61 => Event::RelayHeal {
                sender: t.u32()?,
                src: t.u32()?,
                dst: t.u32()?,
                skill: t.u32()?,
                value: t.i32()?,
                barrier: t.u32()?,
                buff: match t.u8()? {
                    0 => false,
                    1 => true,
                    _ => return None,
                },
                downed: match t.u8()? {
                    0 => false,
                    1 => true,
                    _ => return None,
                },
            },
            62 => Event::RelayCombat {
                sender: t.u32()?,
                in_combat: match t.u8()? {
                    0 => false,
                    1 => true,
                    _ => return None,
                },
            },
            33 => Event::Glider {
                id: t.u32()?,
                gliding: match t.u8()? {
                    0 => false,
                    1 => true,
                    _ => return None,
                },
            },
            41 => Event::ServerTick {
                tick: t.u32()?,
                ping: t.u32()?,
                server_ip: t.u32()?,
                shard_ids: t.u32()?,
            },
            47 => Event::AgentGuild {
                id: t.u32()?,
                guid: t.arr()?,
            },
            48 => Event::AgentGuid {
                id: t.u32()?,
                guid: t.arr()?,
            },
            49 => Event::CoreStats {
                id: t.u32()?,
                level: t.u16()?,
                attrs: {
                    let mut a = [0i16; 12];
                    for v in &mut a {
                        *v = t.i16()?;
                    }
                    a
                },
            },
            50 => Event::BuffFormula {
                buff: t.u32()?,
                index: t.u8()?,
                count: t.u8()?,
                category: t.u8()?,
                ftype: t.u8()?,
                target: t.i32()?,
                base: t.u32()?,
                c1: t.u32()?,
                c2: t.u32()?,
                mode: t.u32()?,
                prereq: t.u8()?,
            },
            51 => Event::GadgetShape {
                id: t.u32()?,
                index: t.u8()?,
                count: t.u8()?,
                x: t.u32()?,
                y: t.u32()?,
            },
            52 => Event::SquadMarker {
                slot: t.u8()?,
                x: t.u32()?,
                y: t.u32()?,
                z: t.u32()?,
            },
            53 => Event::WvwObjective {
                op: t.u8()?,
                guid: t.arr()?,
                map_id: t.u32()?,
                objective_id: t.u32()?,
                objective_type: t.u8()?,
                value: t.u32()?,
            },
            54 => Event::Reward {
                id: t.u32()?,
                kind: t.u16()?,
            },
            55 => Event::WvwTeam {
                team: t.u32()?,
                colour: t.u8()?,
            },
            56 => Event::AttackTarget {
                id: t.u32()?,
                parent: t.u32()?,
            },
            57 => Event::WvwWorld {
                colour: t.u8()?,
                world: t.u32()?,
            },
            58 => Event::GearAttrs {
                id: t.u32()?,
                alt: t.u8()?,
                attrs: {
                    let mut a = [0i16; 12];
                    for v in &mut a {
                        *v = t.i16()?;
                    }
                    a
                },
            },
            59 => Event::GearSkill {
                id: t.u32()?,
                index: t.u8()?,
                count: t.u8()?,
                skill: t.u32()?,
                n: t.u16()?,
            },
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::size_of;

    fn all_events() -> Vec<Event> {
        vec![
            Event::Status {
                unix_s: i64::MIN,
                dll_version: u32::MAX,
                group_state: [0xFF; 16],
            },
            Event::Status {
                unix_s: i64::MAX,
                dll_version: 0,
                group_state: [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
            },
            Event::AgentAdd {
                id: u32::MAX,
                kind: AgentKind::Gadget,
                prof: u32::MAX,
                elite: u32::MAX,
                species: u32::MAX,
                team: u32::MAX,
                subgroup: u8::MAX,
                wvw_rank: u32::MAX,
                wvw_rank_def: u8::MAX,
            },
            Event::AgentAdd {
                id: 1,
                kind: AgentKind::Unknown,
                prof: 2,
                elite: 3,
                species: 4,
                team: 5,
                subgroup: 6,
                wvw_rank: 0,
                wvw_rank_def: 0,
            },
            Event::AgentAdd {
                id: 1,
                kind: AgentKind::Player,
                prof: 0,
                elite: 0,
                species: 0,
                team: 0,
                subgroup: 0,
                wvw_rank: 0,
                wvw_rank_def: 0,
            },
            Event::AgentAdd {
                id: 1,
                kind: AgentKind::Npc,
                prof: 0,
                elite: 0,
                species: 0,
                team: 0,
                subgroup: 0,
                wvw_rank: 0,
                wvw_rank_def: 0,
            },
            Event::AgentName {
                id: u32::MAX,
                field: NameField::Account,
                offset: 255,
                total: 255,
                len: 32,
                bytes: [b'a'; 32],
            },
            Event::AgentName {
                id: 7,
                field: NameField::Character,
                offset: 0,
                total: 3,
                len: 3,
                bytes: {
                    let mut b = [0; 32];
                    b[..3].copy_from_slice(b"Abc");
                    b
                },
            },
            Event::AgentRemove { id: u32::MAX },
            Event::Strike {
                src: u32::MAX,
                dst: u32::MAX,
                skill: u32::MAX,
                value: i32::MIN,
                overstack: u32::MAX,
                result: u8::MAX,
            },
            Event::Strike {
                src: 1,
                dst: 2,
                skill: 3,
                value: i32::MAX,
                overstack: 5,
                result: 6,
            },
            Event::Combat {
                id: u32::MAX,
                in_combat: true,
            },
            Event::Combat {
                id: 0,
                in_combat: false,
            },
            Event::Team {
                id: u32::MAX,
                team: u32::MAX,
            },
            Event::Map {
                map_id: u32::MAX,
                map_type: u32::MAX,
                shard: u32::MAX,
                gw2_build: u32::MAX,
            },
            Event::Health {
                id: u32::MAX,
                pct_x100: u16::MAX,
                max: u32::MAX,
            },
            Event::Pov { id: u32::MAX },
            Event::LogControl {
                action: LogAction::Start,
                reason: u8::MAX,
            },
            Event::LogControl {
                action: LogAction::End,
                reason: 0,
            },
            Event::State {
                id: u32::MAX,
                state: AgentState::Dead,
            },
            Event::Motion {
                id: u32::MAX,
                kind: MotionKind::Position,
                xyz: [u32::MAX, 0, 1.5f32.to_bits()],
            },
            Event::Motion {
                id: 3,
                kind: MotionKind::Velocity,
                xyz: [1, 2, 3],
            },
            Event::Motion {
                id: 3,
                kind: MotionKind::Facing,
                xyz: [(-1.0f32).to_bits(), 0, 0],
            },
            Event::BuffApply {
                dst: u32::MAX,
                src: 0,
                buff: 740,
                inst: u32::MAX,
                duration: 0x7fff_ffff, // public-check:ok
                level: 255,
                active: true,
            },
            Event::BuffApply {
                dst: 1,
                src: 2,
                buff: 3,
                inst: 4,
                duration: 5,
                level: 0,
                active: false,
            },
            Event::BuffRemove {
                dst: 1,
                remover: 0,
                inst: u32::MAX,
            },
            Event::BuffRemoveAll {
                dst: u32::MAX,
                remover: 9,
                buff: 1187,
            },
            Event::BuffStack {
                dst: 1,
                inst: 2,
                change: StackChange::Active,
                value: 3,
            },
            Event::BuffStack {
                dst: 1,
                inst: 2,
                change: StackChange::Reset,
                value: u32::MAX,
            },
            Event::BuffStack {
                dst: 1,
                inst: 2,
                change: StackChange::Duration,
                value: 0,
            },
            Event::Barrier {
                id: u32::MAX,
                pct_x100: u16::MAX,
            },
            Event::Tag {
                id: 7,
                tag: 212,
                commander: true,
            },
            Event::Tag {
                id: u32::MAX,
                tag: 0,
                commander: false,
            },
            Event::Breakbar {
                id: u32::MAX,
                state: u16::MAX,
                pct_bits: 0.75f32.to_bits(),
            },
            Event::Breakbar {
                id: 1,
                state: 3,
                pct_bits: 0,
            },
            Event::CastStart {
                agent: u32::MAX,
                target: 0,
                skill: u32::MAX,
                kind: 1,
                x: u32::MAX,
            },
            Event::CastStart {
                agent: 1,
                target: 2,
                skill: 23275,
                kind: 9,
                x: 0,
            },
            Event::CastEnd {
                agent: u32::MAX,
                site: u8::MAX,
            },
            Event::CastEnd { agent: 7, site: 22 },
            Event::EffectGround {
                effect: u32::MAX,
                owner: u32::MAX,
                duration: u32::MAX,
                tracking: u32::MAX,
                pos: [i16::MIN, i16::MAX, -1],
                orient: [i16::MAX, 0, i16::MIN],
                scale: -2,
                shields: i16::MAX,
                flags: u8::MAX,
                flanking: u8::MAX,
            },
            Event::EffectGroundRemove { tracking: u32::MAX },
            Event::EffectAgent {
                host: u32::MAX,
                src: 0,
                effect: u32::MAX,
                duration: 1,
                tracking: u32::MAX,
            },
            Event::EffectAgentRemove {
                host: u32::MAX,
                tracking: u32::MAX,
            },
            Event::MissileCreate {
                id: u32::MAX,
                owner: u32::MAX,
                skill: u32::MAX,
                skin: u32::MAX,
                word_1e: u32::MAX,
                pos: [i16::MIN, 3, i16::MAX],
                byte_22: u8::MAX,
                byte_23: u8::MAX,
            },
            Event::MissileLaunch {
                id: u32::MAX,
                owner: u32::MAX,
                skill: u32::MAX,
                target: u32::MAX,
                target_pos: [i16::MIN, 1, i16::MAX],
                pos: [i16::MAX, -1, i16::MIN],
                radius: i16::MIN,
                speed: i16::MAX,
                flags: u32::MAX,
                motion: u8::MAX,
                bits: u8::MAX,
            },
            Event::MissileRemove {
                id: u32::MAX,
                owner: u32::MAX,
                skill: u32::MAX,
                other: u32::MAX,
                pos: [i16::MIN, 0, i16::MAX],
                bits: u8::MAX,
            },
            Event::MissileEffect {
                id: u32::MAX,
                owner: u32::MAX,
                effect: u32::MAX,
                value: u32::MAX,
                bits: u8::MAX,
            },
            Event::WeaponSwap {
                id: u32::MAX,
                set: u8::MAX,
            },
            Event::ActionSpeed {
                id: u32::MAX,
                speed_bits: 1.4925f32.to_bits(),
            },
            Event::Glider {
                id: u32::MAX,
                gliding: true,
            },
            Event::AgentFlags {
                id: u32::MAX,
                targetable: 1,
                visible: 0,
                named: NOT_APPLICABLE,
            },
            Event::AgentInfo {
                id: u32::MAX,
                value: u32::MAX,
                buff_dmg: 0x2a4_c001, // public-check:ok
            },
            Event::GadgetCapture {
                id: u32::MAX,
                op: CaptureOp::Stop,
                progress_bits: 0.5f32.to_bits(),
                team: u8::MAX,
                by: 3,
            },
            Event::GadgetModel {
                id: u32::MAX,
                file: 0x0100_ee78, // public-check:ok
                propid: u64::MAX,
                flag36: true,
            },
            Event::Jump {
                id: u32::MAX,
                offcycle: 16,
            },
            Event::FlyTo {
                id: u32::MAX,
                pos: [272, -569, i16::MIN],
                speed: i16::MAX,
            },
            Event::Teleport {
                id: u32::MAX,
                kind: 16,
                arg: u32::MAX,
                dst: u64::MAX,
                value: u32::MAX,
            },
            Event::GadgetAnimation {
                id: u32::MAX,
                token: 0x029c_2d09_1037_542c, // public-check:ok
            },
            Event::Transform {
                id: u32::MAX,
                transform: u32::MAX,
                offcycle: u8::MAX,
                duration: u32::MAX,
                shields: true,
            },
            Event::ServerTick {
                tick: u32::MAX,
                ping: 95,
                server_ip: 0xcfdf_3164, // public-check:ok
                shard_ids: 0x8000_0002, // public-check:ok
            },
            Event::AgentMaster {
                id: u32::MAX,
                master: 0x1234_5678, // public-check:ok
            },
            Event::AgentGuild {
                id: u32::MAX,
                guid: [
                    0xd1, 0x6d, 0x17, 0xea, 0x5b, 0xf1, 0x11, 0xed, 0x81, 0xa9, 0x8f, 0xb5, 0xcf,
                    0xbe, 0x77, 0x66,
                ],
            },
            Event::AgentGuid {
                id: u32::MAX,
                guid: [
                    0xa7, 0x02, 0x81, 0xbf, 0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0, 0x11,
                    0x22, 0x33, 0xff,
                ],
            },
            Event::CoreStats {
                id: u32::MAX,
                level: 80,
                attrs: [0, 1000, 1000, 1000, 1000, 0, 0, 1800, 0, 600, -1, i16::MAX],
            },
            Event::BuffFormula {
                buff: 736,
                index: 1,
                count: 3,
                category: 2,
                ftype: 15,
                target: -7,
                base: 22.0f32.to_bits(),
                c1: 0.25f32.to_bits(),
                c2: f32::MAX.to_bits(),
                mode: 0xdead_beef, // public-check:ok
                prereq: 0x8b,
            },
            Event::GadgetShape {
                id: u32::MAX,
                index: 3,
                count: 7,
                x: (-5782.0f32).to_bits(),
                y: f32::MIN.to_bits(),
            },
            Event::SquadMarker {
                slot: 0xff,
                x: f32::INFINITY.to_bits(),
                y: u32::MAX,
                z: (-2818.2f32).to_bits(),
            },
            Event::WvwObjective {
                op: 1,
                guid: [0xab; 16],
                map_id: 1651,
                objective_id: 112,
                objective_type: 10,
                value: u32::MAX,
            },
            Event::Reward {
                id: 3_561_046_054,
                kind: u16::MAX,
            },
            Event::WvwTeam {
                team: 2780,
                colour: 3,
            },
            Event::AttackTarget {
                id: 41,
                parent: u32::MAX,
            },
            Event::WvwWorld {
                colour: 2,
                world: 11_005,
            },
            Event::GearAttrs {
                id: u32::MAX,
                alt: 1,
                attrs: [0, 980, -3, 0, 0, 0, 0, i16::MAX, 0, 50, 0, i16::MIN],
            },
            Event::GearSkill {
                id: 77,
                index: 3,
                count: 9,
                skill: 32_045,
                n: u16::MAX,
            },
            Event::BuffDamage {
                src: u32::MAX,
                dst: u32::MAX,
                buff: u32::MAX,
                value: i32::MIN,
                barrier: u32::MAX,
                stacks: u8::MAX,
            },
            Event::BuffDamage {
                src: 0,
                dst: 1,
                buff: 736,
                value: 0,
                barrier: 0,
                stacks: 1,
            },
            Event::Heal {
                src: u32::MAX,
                dst: u32::MAX,
                skill: u32::MAX,
                value: i32::MIN,
                barrier: u32::MAX,
                buff: true,
            },
            Event::Heal {
                src: 0,
                dst: 1,
                skill: 718,
                value: 527,
                barrier: 0,
                buff: false,
            },
            Event::GroupKey {
                key: [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16],
            },
            Event::RelayHeal {
                sender: u32::MAX,
                src: u32::MAX,
                dst: u32::MAX,
                skill: u32::MAX,
                value: i32::MIN,
                barrier: u32::MAX,
                buff: true,
                downed: true,
            },
            Event::RelayHeal {
                sender: 7,
                src: 7,
                dst: 9,
                skill: 9140,
                value: 385,
                barrier: 0,
                buff: true,
                downed: true,
            },
            Event::RelayCombat {
                sender: u32::MAX,
                in_combat: true,
            },
            Event::RelayCombat {
                sender: 7,
                in_combat: false,
            },
        ]
    }

    #[test]
    fn group_key_round_trips() {
        let ev = Event::GroupKey {
            key: [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16],
        };
        let (kind, p) = ev.encode();
        assert_eq!(kind, 60);
        assert_eq!(
            &p[..16],
            &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]
        );
        assert_eq!(Event::decode(kind, &p), Some(ev));
    }

    #[test]
    fn relay_heal_round_trips() {
        let ev = Event::RelayHeal {
            sender: 7,
            src: 7,
            dst: 9,
            skill: 9140,
            value: 385,
            barrier: 0,
            buff: true,
            downed: true,
        };
        let (kind, p) = ev.encode();
        assert_eq!(kind, 61);
        assert_eq!(Event::decode(kind, &p), Some(ev));
    }

    #[test]
    fn every_kind_round_trips() {
        let mut kinds = std::collections::BTreeSet::new();
        for ev in all_events() {
            let (kind, payload) = ev.encode();
            assert_eq!(Event::decode(kind, &payload), Some(ev), "{ev:?}");
            kinds.insert(kind);
        }
        assert_eq!(
            kinds.into_iter().collect::<Vec<_>>(),
            (1..=62).collect::<Vec<u16>>()
        );
    }

    #[test]
    fn encoding_is_little_endian_with_zero_padding() {
        let (kind, p) = Event::Team {
            id: 0x0102_0304,   // public-check:ok
            team: 0x0A0B_0C0D, // public-check:ok
        }
        .encode();
        assert_eq!(kind, 7);
        assert_eq!(&p[..8], &[4, 3, 2, 1, 0x0D, 0x0C, 0x0B, 0x0A]);
        assert!(p[8..].iter().all(|&b| b == 0));
    }

    /// The WvW rank fields follow `subgroup`; a producer without them (zero bytes) reads as no
    /// rank.
    #[test]
    fn agent_add_wvw_rank_layout() {
        let ev = Event::AgentAdd {
            id: 1,
            kind: AgentKind::Player,
            prof: 2,
            elite: 3,
            species: 0,
            team: 4,
            subgroup: 5,
            wvw_rank: 0x0102_0304, // public-check:ok
            wvw_rank_def: 105,
        };
        let (kind, p) = ev.encode();
        assert_eq!(kind, 2);
        assert_eq!(&p[22..27], &[4, 3, 2, 1, 105]);
        assert!(p[27..].iter().all(|&b| b == 0));
        let mut old = p;
        old[22..27].fill(0);
        assert!(matches!(
            Event::decode(2, &old),
            Some(Event::AgentAdd {
                subgroup: 5,
                wvw_rank: 0,
                wvw_rank_def: 0,
                ..
            })
        ));
    }

    /// Kind 42: `src | dst | buff | value | barrier | stacks`, little-endian.
    #[test]
    fn buff_damage_layout() {
        let (kind, p) = Event::BuffDamage {
            src: 0x0102_0304, // public-check:ok
            dst: 5,
            buff: 736,
            value: -2,
            barrier: 7,
            stacks: 9,
        }
        .encode();
        assert_eq!(kind, 42);
        assert_eq!(&p[..4], &[4, 3, 2, 1]);
        assert_eq!(&p[4..8], &5u32.to_le_bytes());
        assert_eq!(&p[8..12], &736u32.to_le_bytes());
        assert_eq!(&p[12..16], &(-2i32).to_le_bytes());
        assert_eq!(&p[16..21], &[7, 0, 0, 0, 9]);
        assert!(p[21..].iter().all(|&b| b == 0));
    }

    /// Kind 43: `src | dst | skill | value | barrier | buff`, little-endian; a `buff` byte other
    /// than 0/1 does not decode.
    #[test]
    fn heal_layout() {
        let (kind, mut p) = Event::Heal {
            src: 0x0102_0304, // public-check:ok
            dst: 5,
            skill: 718,
            value: 1075,
            barrier: 1075,
            buff: true,
        }
        .encode();
        assert_eq!(kind, 43);
        assert_eq!(&p[..4], &[4, 3, 2, 1]);
        assert_eq!(&p[4..8], &5u32.to_le_bytes());
        assert_eq!(&p[8..12], &718u32.to_le_bytes());
        assert_eq!(&p[12..16], &1075i32.to_le_bytes());
        assert_eq!(&p[16..21], &[0x33, 0x04, 0, 0, 1]);
        assert!(p[21..].iter().all(|&b| b == 0));
        p[20] = 2;
        assert_eq!(Event::decode(43, &p), None);
    }

    #[test]
    fn unknown_kind_is_none() {
        assert_eq!(Event::decode(999, &[0; PAYLOAD_SIZE]), None);
        assert_eq!(Event::decode(0, &[0; PAYLOAD_SIZE]), None);
    }

    #[test]
    fn bad_enum_byte_is_none() {
        let mut p = [0u8; PAYLOAD_SIZE];
        p[4] = 4; // AgentKind
        assert_eq!(Event::decode(2, &p), None);
        p[4] = 2; // NameField
        assert_eq!(Event::decode(3, &p), None);
        p[4] = 2; // bool
        assert_eq!(Event::decode(6, &p), None);
        let mut p = [0u8; PAYLOAD_SIZE];
        assert_eq!(Event::decode(11, &p), None); // LogAction 0
        p[0] = 3;
        assert_eq!(Event::decode(11, &p), None);
        let mut p = [0u8; PAYLOAD_SIZE];
        p[7] = 33; // AgentName len
        assert_eq!(Event::decode(3, &p), None);
    }

    #[test]
    fn state_roundtrip() {
        let ev = Event::State {
            id: 9,
            state: AgentState::Down,
        };
        let (kind, p) = ev.encode();
        assert_eq!(kind, 12);
        assert_eq!(Event::decode(kind, &p), Some(ev));
    }

    #[test]
    fn state_bad_byte_is_none() {
        for b in [0u8, 4] {
            let mut p = [0u8; PAYLOAD_SIZE];
            p[0] = 9;
            p[4] = b;
            assert_eq!(Event::decode(12, &p), None, "state byte {b}");
        }
    }

    #[test]
    fn name_events_chunks_on_char_boundary() {
        let name = format!("{}é{}", "a".repeat(31), "b".repeat(37));
        assert_eq!(name.len(), 70);
        let chunks: Vec<Event> = name_events(7, NameField::Character, &name).collect();
        assert_eq!(chunks.len(), 3);
        let mut joined = Vec::new();
        for c in &chunks {
            let Event::AgentName {
                id,
                field,
                offset,
                total,
                len,
                bytes,
            } = *c
            else {
                panic!("{c:?}")
            };
            assert_eq!((id, field, total), (7, NameField::Character, 70));
            assert_eq!(usize::from(offset), joined.len());
            let part = &bytes[..usize::from(len)];
            assert!(
                std::str::from_utf8(part).is_ok(),
                "chunk splits a char: {part:?}"
            );
            joined.extend_from_slice(part);
        }
        assert_eq!(joined, name.as_bytes());
    }

    #[test]
    fn name_events_clips_to_255_bytes() {
        let name = "é".repeat(200); // 400 bytes
        let total = name_events(1, NameField::Account, &name)
            .map(|e| match e {
                Event::AgentName { len, .. } => usize::from(len),
                _ => 0,
            })
            .sum::<usize>();
        assert_eq!(total, 254); // the last whole char at or below 255 bytes
    }

    #[test]
    fn ring_path_per_host() {
        use std::path::Path;
        assert_eq!(ring_path(RingHost::Unix), Path::new("/tmp/axidps_ring_v2"));
        assert_eq!(
            ring_path(RingHost::Wine),
            Path::new(r"Z:\tmp\axidps_ring_v2")
        );
        let w = ring_path(RingHost::Windows);
        assert!(w.starts_with(std::env::temp_dir()), "{w:?}");
        assert!(w.ends_with("axidps_ring_v2"), "{w:?}");
    }

    #[test]
    fn host_from_probes() {
        assert_eq!(RingHost::from_probes(true, false), RingHost::Wine);
        assert_eq!(
            RingHost::from_probes(false, true),
            RingHost::Wine,
            "exports hidden, registry key present"
        );
        assert_eq!(RingHost::from_probes(false, false), RingHost::Windows);
    }

    #[test]
    fn current_build_host() {
        let want = if cfg!(windows) {
            RingHost::Windows
        } else {
            RingHost::Unix
        };
        assert_eq!(RingHost::current_build(), want);
    }

    #[test]
    fn header_and_slot_are_64_bytes() {
        assert_eq!(size_of::<RingHeader>(), HEADER_SIZE);
        assert_eq!(size_of::<Slot>(), SLOT_SIZE);
        assert!(SLOT_COUNT.is_power_of_two());
        assert_eq!(MAGIC.to_le_bytes(), *b"AXR2");
    }
}
