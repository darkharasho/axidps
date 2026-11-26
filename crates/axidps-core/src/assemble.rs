//! Turns a finished [`Fight`] and its [`Registry`] into an arcdps-compatible EVTC [`Log`].

use std::collections::{BTreeMap, BTreeSet};

use axidps_evtc::{Agent, Event as EvtcEvent, Header, Log, Skill, player_name};
use axidps_protocol::v2::{AgentKind, AgentState, CaptureOp, Event, MotionKind, NOT_APPLICABLE};

use crate::buffs::{Row, Stacks};
use crate::casts::{self, Casts};
use crate::condi;
use crate::content;
use crate::effects::{self, Effects};
use crate::extension;
use crate::masters::Masters;
use crate::names;
use crate::processor::Fight;
use crate::registry::{AgentInfo, Registry, squad_fields};
use crate::server::{Server, TickRows, instance_start, tick_before};
use crate::skill_names::SKILL_NAMES;

/// `src_agent` of LOG_START / LOG_END rows (`"arc"`).
pub const ARC_ID: u64 = 0x63_7261; // public-check:ok

const ENTER_COMBAT: u8 = 1;
const EXIT_COMBAT: u8 = 2;
const CHANGE_UP: u8 = 3;
const CHANGE_DEAD: u8 = 4;
const CHANGE_DOWN: u8 = 5;
const SPAWN: u8 = 6;
const DESPAWN: u8 = 7;
const HEALTH_UPDATE: u8 = 8;
const LOG_START: u8 = 9;
const LOG_END: u8 = 10;
/// Effect statechanges whose skillid is an effect id (ground and agent effect create).
const EFFECT_GROUND: u8 = 60;
const EFFECT_AGENT: u8 = 62;
const MAX_HEALTH: u8 = 12;
const POINT_OF_VIEW: u8 = 13;
const LANGUAGE: u8 = 14;
const GW2_BUILD: u8 = 15;
/// arcdps's own build string row (from byte 8). axidps writes the arcdps build its header
/// claims, for byte parity.
const ARCBUILD: u8 = 54;
const ARCDPS_BUILD: &[u8; 24] = b"20261115.180220-613-x64\0";
const TEAM_CHANGE: u8 = 22;
const POSITION: u8 = 19;
const VELOCITY: u8 = 20;
const FACING: u8 = 21;
const BARRIER: u8 = 38;
const TAG: u8 = 37;
const GLIDER: u8 = 55;
const ACTION_SPEED: u8 = 92;
const TARGETABLE: u8 = 24;
const VISIBLE: u8 = 76;
const GADGET_NAME: u8 = 78;
const AGENT_INFO: u8 = 89;
const CAPTURE_START: u8 = 80;
const CAPTURE_UPDATE: u8 = 81;
const CAPTURE_STOP: u8 = 82;
const CAPTURE_OUTLINE_POINT: u8 = 83;
const SQUAD_MARKER: u8 = 53;
const WVW_OBJECTIVE: u8 = 75;
const REWARD: u8 = 17;
const WVW_TEAMS: u8 = 74;
const ATTACK_TARGET: u8 = 23;
const GADGET_MODEL: u8 = 87;
const MAP_ID: u8 = 25;
const SHARD_ID: u8 = 16;
const INSTANCE_START: u8 = 42;
const TICK: u8 = 84;
const BREAKBAR_STATE: u8 = 34;
const BREAKBAR_PERCENT: u8 = 35;
const GUILD: u8 = 29;

/// Buff-damage results, as arcdps writes them: 14 a condition tick, 16 and 18 the on-hit procs its
/// condition model computes on their own paths. The proc buff ids are every id arcdps wrote with
/// result 16 or 18 in the owner's logs of 2026-11-24 (15 logs); every other id was 14.
const BUFF_DAMAGE_TICK: u8 = 14;
const BUFF_DAMAGE_PROC: u8 = 16;
const BUFF_DAMAGE_PROC_18: u8 = 18;
const PROCS_16: &[u32] = &[13814, 30285, 37475, 45026, 57042, 57244, 57409, 58512];
const PROCS_18: &[u32] = &[13036, 26646, 72975];

/// The result arcdps writes for buff damage from `buff`.
fn buff_damage_result(buff: u32) -> u8 {
    if PROCS_16.contains(&buff) {
        BUFF_DAMAGE_PROC
    } else if PROCS_18.contains(&buff) {
        BUFF_DAMAGE_PROC_18
    } else {
        BUFF_DAMAGE_TICK
    }
}

/// Group state written when the fight has no `Status` event (should not happen: core is only
/// live after one). 0xFF is outside the defined states 0..=4.
const UNKNOWN_GROUP_STATE: u8 = 0xFF;
/// `Status.group_state` index of group condition (the combat-hit capture, which also gives
/// the heals), and the state byte of a live group.
const CONDITION_GROUP: usize = 3;
const GROUP_LIVE: u8 = 0;

/// The healing addon marks each row with who sent it (it compares instids with its own player's):
/// the source or its master is the sender, the target or its master is. axidps writes the local
/// player's rows (sender = the POV) and relayed ones (sender = the peer). `senders` lists each
/// row's index in `events` with its sender's instid.
fn heal_senders(events: &mut [EvtcEvent], senders: &[(usize, u16)]) {
    for &(at, sender) in senders {
        let e = &mut events[at];
        if e.src_instid == sender || e.src_master_instid == sender {
            e.is_offcycle |= extension::HEAL_FROM_SRC;
        }
        if e.dst_instid == sender || e.dst_master_instid == sender {
            e.is_offcycle |= extension::HEAL_FROM_DST;
        }
    }
}

fn sc(time: u64, kind: u8) -> EvtcEvent {
    EvtcEvent {
        time,
        is_statechange: kind,
        ..Default::default()
    }
}

fn unix_at(f: &Fight, ms: u64) -> i32 {
    let Some((t0, unix0)) = f.unix_at else {
        return 0;
    };
    let delta = (i128::from(ms) - i128::from(t0)) / 1000;
    let secs = i128::from(unix0) + delta;
    i32::try_from(secs).unwrap_or(0)
}

fn marker(f: &Fight, time: u64, kind: u8) -> EvtcEvent {
    let s = unix_at(f, time);
    EvtcEvent {
        src_agent: ARC_ID,
        value: s,
        buff_dmg: s,
        ..sc(time, kind)
    }
}

/// Plain NUL-terminated name, clipped to 63 bytes on a char boundary.
fn plain_name(s: &str) -> [u8; 64] {
    let mut end = s.len().min(63);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = [0u8; 64];
    out[..end].copy_from_slice(&s.as_bytes()[..end]);
    out
}

/// `squad`: the agent is in the POV's squad. `wvw`: the fight is on a WvW map.
fn agent_row(addr: u64, a: &AgentInfo, squad: bool, wvw: bool) -> Agent {
    let (prof, is_elite, name) = match a.kind {
        // Like arcdps: a real name, an account and a subgroup only for squad members.
        AgentKind::Player if squad => (
            a.prof,
            a.elite,
            player_name(&a.character, &a.account, a.subgroup),
        ),
        // Everyone else, allies included (as arcdps does): the WvW
        // rank title on a WvW map, else the elite spec or profession name.
        AgentKind::Player => {
            let title = a.wvw_rank_id.filter(|_| wvw).and_then(names::wvw_title);
            let name = title
                .as_deref()
                .unwrap_or(names::spec_name(a.prof, a.elite));
            (a.prof, a.elite, player_name(name, "", None))
        }
        AgentKind::Npc => (a.species, 0xFFFF_FFFF, plain_name(&a.character)), // public-check:ok
        AgentKind::Gadget => (
            0xFFFF_0000 | (a.species & 0xFFFF), // public-check:ok
            0xFFFF_FFFF,                        // public-check:ok
            plain_name(&a.character),
        ),
        AgentKind::Unknown => (0, 0xFFFF_FFFF, plain_name("axidps:unknown")), // public-check:ok
    };
    Agent {
        addr,
        prof,
        is_elite,
        toughness: 0,
        concentration: 0,
        healing: 0,
        hitbox_width: 0,
        condition: 0,
        hitbox_height: 0,
        name,
        pad: [0; 4],
    }
}

/// Instance ids an event refers to.
fn event_ids(ev: &Event) -> Vec<u32> {
    match *ev {
        Event::AgentAdd { id, .. }
        | Event::AgentRemove { id }
        | Event::Combat { id, .. }
        | Event::Team { id, .. }
        | Event::Health { id, .. }
        | Event::State { id, .. }
        | Event::Motion { id, .. }
        | Event::BuffStack { dst: id, .. }
        | Event::Barrier { id, .. }
        | Event::Tag { id, .. }
        | Event::Breakbar { id, .. } => vec![id],
        Event::BuffApply { dst, src, .. } => [dst, src].into_iter().filter(|&i| i != 0).collect(),
        Event::BuffRemove { dst, remover, .. } | Event::BuffRemoveAll { dst, remover, .. } => {
            [dst, remover].into_iter().filter(|&i| i != 0).collect()
        }
        Event::Strike { src, dst, .. } => vec![src, dst],
        Event::BuffDamage { src, dst, .. }
        | Event::Heal { src, dst, .. }
        | Event::RelayHeal { src, dst, .. } => {
            [src, dst].into_iter().filter(|&i| i != 0).collect()
        }
        Event::CastStart { agent, target, .. } => {
            [agent, target].into_iter().filter(|&i| i != 0).collect()
        }
        Event::CastEnd { agent, .. }
        | Event::WeaponSwap { id: agent, .. }
        | Event::ActionSpeed { id: agent, .. }
        | Event::Glider { id: agent, .. }
        | Event::AgentFlags { id: agent, .. }
        | Event::AgentInfo { id: agent, .. }
        | Event::GadgetCapture { id: agent, .. }
        | Event::GadgetModel { id: agent, .. }
        | Event::Jump { id: agent, .. }
        | Event::FlyTo { id: agent, .. }
        | Event::Teleport { id: agent, .. }
        | Event::GadgetAnimation { id: agent, .. }
        | Event::Transform { id: agent, .. } => vec![agent],
        _ => vec![],
    }
}

/// The name arcdps writes for `id` (from the owner's arcdps logs), or "" when unknown.
fn skill_name(id: u32) -> &'static str {
    SKILL_NAMES
        .binary_search_by_key(&id, |&(i, _)| i)
        .ok()
        .and_then(|i| SKILL_NAMES.get(i))
        .map_or("", |&(_, n)| n)
}

/// Running per-agent state while walking the events in time order.
struct State {
    team: BTreeMap<u32, u32>,
    health: BTreeMap<u32, u16>,
    /// Last MAX_HEALTH written per agent: like arcdps, a row only when it changes.
    max: BTreeMap<u32, u32>,
    /// Agents last seen in combat: an exit row is written only for one of these.
    in_combat: BTreeSet<u32>,
    /// Agents present (start snapshot or added, not removed since): an add for one of these is
    /// an identity update, not a spawn.
    present: BTreeSet<u32>,
    /// Subgroup as of the event being written (0 = not in the squad), and since when.
    subgroup: BTreeMap<u32, (u8, u64)>,
    /// Time spent in each squad subgroup so far, and when it was last entered.
    sub_time: BTreeMap<u32, BTreeMap<u8, (u64, u64)>>,
    /// arcdps's breakbar caches per agent: last state and last percent written. Absent = reset
    /// (log start, spawn), so the next read writes both.
    breakbar: BTreeMap<u32, (u16, f32)>,
    /// arcdps's targetable/visible/named caches per agent; absent = reset (log start, spawn).
    flags: BTreeMap<u32, (u8, u8, u8)>,
    /// Gadgets with a capture open (cleared at log start).
    capturing: BTreeSet<u32>,
    /// Gadget capture outlines (sc 83), from the start snapshot then the fight's events.
    shapes: BTreeMap<u32, Vec<(u32, u32)>>,
    /// Agents downed now: arcdps marks buff damage to them `is_offcycle`.
    down: BTreeSet<u32>,
}

impl State {
    /// The 24/76/78 rows arcdps writes for a flags read: each field on change, unless it does
    /// not apply to the agent. 24 also carries value 1.
    fn flags(&mut self, ctx: &Ctx, time: u64, id: u32, new: (u8, u8, u8)) -> Vec<EvtcEvent> {
        let old = self
            .flags
            .insert(id, new)
            .unwrap_or((u8::MAX, u8::MAX, u8::MAX));
        let mut rows = Vec::new();
        for (sc, n, o) in [
            (TARGETABLE, new.0, old.0),
            (VISIBLE, new.1, old.1),
            (GADGET_NAME, new.2, old.2),
        ] {
            if n != o && n != NOT_APPLICABLE {
                rows.extend(ctx.agent_row(time, sc, id).map(|mut e| {
                    e.dst_agent = u64::from(n);
                    e.value = i32::from(sc == TARGETABLE);
                    e
                }));
            }
        }
        rows
    }

    /// The 34/35 rows arcdps writes for a breakbar read: state on change, percent on change
    /// (compared as floats) when it is at most 1.0.
    fn breakbar(&mut self, ctx: &Ctx, time: u64, id: u32, state: u16, pct: f32) -> Vec<EvtcEvent> {
        let (last_state, last_pct) = self
            .breakbar
            .get(&id)
            .copied()
            .unwrap_or((u16::MAX, f32::NEG_INFINITY));
        let mut rows = Vec::new();
        let mut cache = (last_state, last_pct);
        if state != last_state {
            cache.0 = state;
            rows.extend(ctx.agent_row(time, BREAKBAR_STATE, id).map(|mut e| {
                e.value = i32::from(state);
                e
            }));
        }
        #[allow(clippy::float_cmp)]
        if pct != last_pct && pct <= 1.0 {
            cache.1 = pct;
            rows.extend(ctx.agent_row(time, BREAKBAR_PERCENT, id).map(|mut e| {
                e.value = pct.to_bits() as i32;
                e
            }));
        }
        self.breakbar.insert(id, cache);
        rows
    }

    fn set_subgroup(&mut self, id: u32, sub: u8, time: u64) {
        match self.subgroup.get(&id) {
            Some(&(cur, _)) if cur == sub => return,
            Some(_) => self.close_subgroup(id, time),
            None => {}
        }
        self.subgroup.insert(id, (sub, time));
    }

    fn close_subgroup(&mut self, id: u32, time: u64) {
        if let Some(&(cur, since)) = self.subgroup.get(&id)
            && cur != 0
        {
            let e = self
                .sub_time
                .entry(id)
                .or_default()
                .entry(cur)
                .or_insert((0, 0));
            e.0 += time.saturating_sub(since);
            e.1 = since;
        }
    }

    fn current_subgroup(&self, id: u32) -> Option<u8> {
        self.subgroup.get(&id).map(|&(s, _)| s)
    }

    /// The squad subgroup `id` spent the most time in (a tie goes to the later one); `None` if
    /// it was never in the squad during the fight. Call after [`Self::close_all`].
    fn main_subgroup(&self, id: u32) -> Option<u8> {
        self.sub_time
            .get(&id)?
            .iter()
            .max_by_key(|&(_, &(dur, since))| (dur, since))
            .map(|(&sub, _)| sub)
    }

    fn close_all(&mut self, end: u64) {
        let ids: Vec<u32> = self.subgroup.keys().copied().collect();
        for id in ids {
            self.close_subgroup(id, end);
            if let Some(e) = self.subgroup.get_mut(&id) {
                e.1 = end;
            }
        }
    }
}

impl State {
    fn iff(&self, reg: &Registry, src: u32, dst: u32) -> u8 {
        let team = |id: u32| {
            self.team
                .get(&id)
                .copied()
                .or_else(|| reg.agent(id).and_then(|a| a.team))
                // Team 0 means "no team": the relation is unknown.
                .filter(|&t| t != 0)
        };
        match (team(src), team(dst)) {
            (Some(a), Some(b)) if a == b => 0,
            (Some(_), Some(_)) => 1,
            _ => 2,
        }
    }
}

/// arcdps's sc 23 row: src = the attack target, dst = its parent gadget, src_master = the
/// parent, iff 2 (arcdps's attitude for them in every log). Written even when the agents are not
/// in the agent table, as arcdps.
fn attack_target_row(ctx: &Ctx, time: u64, id: u32, parent: u32) -> Option<EvtcEvent> {
    let mut e = ctx.agent_row(time, ATTACK_TARGET, id)?;
    e.dst_agent = ctx.addr(parent)?;
    e.dst_instid = parent as u16;
    e.src_master_instid = parent as u16;
    e.iff = 2;
    Some(e)
}

/// arcdps's sc 75 row: value = map id, buff_dmg = owner team, skillid = objective id, buff =
/// type, pad = upgrade progress.
fn wvw_row(time: u64, o: crate::wvw::Objective) -> EvtcEvent {
    EvtcEvent {
        value: o.map as i32,
        buff_dmg: o.owner as i32,
        skillid: o.id,
        buff: o.kind,
        pad: o.progress,
        ..sc(time, WVW_OBJECTIVE)
    }
}

/// arcdps's sc 53 row: src = x | y << 32, dst = z, skillid = slot (f32 bits; +inf = empty).
fn squad_marker_row(time: u64, slot: u8, [x, y, z]: [u32; 3]) -> EvtcEvent {
    EvtcEvent {
        src_agent: u64::from(x) | u64::from(y) << 32,
        dst_agent: u64::from(z),
        skillid: u32::from(slot),
        ..sc(time, SQUAD_MARKER)
    }
}

/// arcdps's sc 83 batch for a gadget's outline: one row per point, `dst` = index, `value` /
/// `buff_dmg` = the f32 bits, `overstack` = count. Nothing for counts 0 and 2, as arcdps.
fn outline_rows(ctx: &Ctx, time: u64, id: u32, pts: &[(u32, u32)]) -> Vec<EvtcEvent> {
    if pts.len() == 2 {
        return Vec::new();
    }
    pts.iter()
        .enumerate()
        .filter_map(|(i, &(x, y))| {
            let mut e = ctx.agent_row(time, CAPTURE_OUTLINE_POINT, id)?;
            e.dst_agent = i as u64;
            e.value = x as i32;
            e.buff_dmg = y as i32;
            e.overstack = pts.len() as u32;
            Some(e)
        })
        .collect()
}

struct Ctx<'a> {
    reg: &'a Registry,
    /// A player's lives in this fight -> one addr ([`Lives`]).
    lives: Lives,
}

/// The fight's instance ids in order of first appearance: the agents present at the start,
/// then the fight's events by time.
fn first_seen(f: &Fight) -> Vec<u32> {
    let mut refs: Vec<(u64, u32)> = f
        .events
        .iter()
        .filter(|(t, _)| *t <= f.end_ms)
        .flat_map(|&(t, ev)| event_ids(&ev).into_iter().map(move |id| (t, id)))
        .collect();
    refs.sort_by_key(|(t, _)| *t);
    let mut seen = BTreeSet::new();
    f.start_snapshot
        .iter()
        .map(|a| a.id)
        .chain(refs.into_iter().map(|(_, id)| id))
        .filter(|&id| id != 0 && seen.insert(id))
        .collect()
}

/// arcdps's addr for a player's account GUID: `guid[0..6]`, then `guid[8]`, `guid[9]` as the top
/// bytes (arcdps also adds a per-session base, taken as 0 here). `None` below 2^32, where it could
/// meet a sequential addr (the GUID variant byte `guid[8]` puts real GUIDs far above).
fn guid_addr(g: &[u8; 16]) -> Option<u64> {
    let addr = u64::from_le_bytes([g[0], g[1], g[2], g[3], g[4], g[5], g[8], g[9]]);
    (addr > u64::from(u32::MAX)).then_some(addr)
}

/// A key naming one player (on one team, which goes with it).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum LifeKey<'a> {
    Guid([u8; 16]),
    Name(&'a str),
}

/// A player's lives in one fight.
struct Group {
    /// The first life's registry addr.
    lead: u64,
    guid: Option<[u8; 16]>,
    team: u32,
    members: Vec<(u32, u64)>,
}

/// One agent per player per fight, as arcdps: it keys every player but the POV by account GUID,
/// so a player who leaves range and comes back under a new instance id, or changes character,
/// keeps one addr.
///
/// Lives are matched on the same team (never across teams) by account GUID (`AgentGuid`; not
/// for the POV), else by account, else by character name (one character belongs to one
/// account), unless the two lives have different GUIDs. A player with a GUID is written under
/// its GUID addr ([`guid_addr`]: the same for the whole session, as arcdps); one without keeps
/// its first life's addr in the fight. Squad players are already one addr by account (the
/// registry). With no `AgentGuid` in the session, only the merges by name apply.
#[derive(Default)]
struct Lives {
    /// Instance id -> the addr it is written under, where that is not the registry's.
    alias: BTreeMap<u32, u64>,
    /// `(GUID, team)` -> its player's GUID addr, for ids not in the fight's order (rows naming
    /// an agent [`event_ids`] does not list).
    by_guid: BTreeMap<([u8; 16], u32), u64>,
    pov: Option<u32>,
}

impl Lives {
    fn new(order: &[u32], reg: &Registry, pov: Option<u32>) -> Self {
        let mut groups: Vec<Group> = Vec::new();
        let mut by_key: BTreeMap<(LifeKey, u32), usize> = BTreeMap::new();
        for &id in order {
            let (Some(a), Some(addr)) = (reg.agent(id), reg.addr(id)) else {
                continue;
            };
            let Some(team) = a.team.filter(|_| a.kind == AgentKind::Player) else {
                continue;
            };
            let guid = a.account_guid.filter(|_| pov != Some(id));
            let names = [a.account.as_str(), a.character.as_str()];
            let names = names.into_iter().filter(|n| !n.is_empty());
            // A name never joins two different GUIDs.
            let same_guid = |i: usize| match (guid, groups[i].guid) {
                (Some(a), Some(b)) => a == b,
                _ => true,
            };
            let found = guid
                .and_then(|g| by_key.get(&(LifeKey::Guid(g), team)).copied())
                .or_else(|| {
                    names.clone().find_map(|n| {
                        by_key
                            .get(&(LifeKey::Name(n), team))
                            .copied()
                            .filter(|&i| same_guid(i))
                    })
                });
            let i = found.unwrap_or_else(|| {
                groups.push(Group {
                    lead: addr,
                    guid: None,
                    team,
                    members: Vec::new(),
                });
                groups.len() - 1
            });
            let g = &mut groups[i];
            g.members.push((id, addr));
            g.guid = g.guid.or(guid);
            for k in guid
                .map(LifeKey::Guid)
                .into_iter()
                .chain(names.map(LifeKey::Name))
            {
                by_key.entry((k, team)).or_insert(i);
            }
        }
        // Two teams (or two colliding GUIDs) never share an addr: the first group keeps it.
        let mut claimed = BTreeSet::new();
        let mut lives = Lives {
            pov,
            ..Lives::default()
        };
        for g in &groups {
            let mut to = g.lead;
            if let Some(guid) = g.guid
                && let Some(addr) = guid_addr(&guid)
                && claimed.insert(addr)
            {
                lives.by_guid.insert((guid, g.team), addr);
                to = addr;
            }
            for &(id, addr) in &g.members {
                if addr != to {
                    lives.alias.insert(id, to);
                }
            }
        }
        lives
    }

    /// The GUID addr of a player `order` did not list, if its GUID has one in this fight.
    fn unlisted(&self, id: u32, reg: &Registry) -> Option<u64> {
        if self.by_guid.is_empty() || self.pov == Some(id) {
            return None;
        }
        let a = reg.agent(id).filter(|a| a.kind == AgentKind::Player)?;
        self.by_guid.get(&(a.account_guid?, a.team?)).copied()
    }
}

impl Ctx<'_> {
    /// The addr `id` is written with: its player's in this fight ([`Lives`]), else the
    /// registry's.
    fn addr(&self, id: u32) -> Option<u64> {
        self.lives
            .alias
            .get(&id)
            .copied()
            .or_else(|| self.lives.unlisted(id, self.reg))
            .or_else(|| self.reg.addr(id))
    }

    /// Event skeleton for an agent row; `None` when the registry has no addr (never panics).
    fn agent_row(&self, time: u64, kind: u8, id: u32) -> Option<EvtcEvent> {
        let addr = self.addr(id)?;
        Some(EvtcEvent {
            src_agent: addr,
            src_instid: id as u16,
            ..sc(time, kind)
        })
    }

    /// `sub`: the subgroup at `time`, if the fight has seen one.
    fn combat(&self, time: u64, id: u32, enter: bool, sub: Option<u8>) -> Option<EvtcEvent> {
        let mut e = self.agent_row(time, if enter { ENTER_COMBAT } else { EXIT_COMBAT }, id)?;
        let a = self.reg.agent(id);
        e.dst_agent = u64::from(sub.or(a.and_then(|a| a.subgroup)).unwrap_or(0));
        e.value = a.map_or(0, |a| a.prof as i32);
        e.buff_dmg = a.map_or(0, |a| a.elite as i32);
        Some(e)
    }

    fn state(&self, time: u64, id: u32, state: AgentState) -> Option<EvtcEvent> {
        let kind = match state {
            AgentState::Up => CHANGE_UP,
            AgentState::Dead => CHANGE_DEAD,
            AgentState::Down => CHANGE_DOWN,
        };
        self.agent_row(time, kind, id)
    }

    /// GUILD (sc 29): the guild GUID's 16 bytes in dst_agent, value and buff_dmg, as arcdps
    /// copies them from the Player object (all zero for a player without a guild).
    fn guild(&self, time: u64, id: u32, guid: [u8; 16]) -> Option<EvtcEvent> {
        let mut e = self.agent_row(time, GUILD, id)?;
        let (lo, hi) = guid.split_at(8);
        let (value, buff_dmg) = hi.split_at(4);
        e.dst_agent = u64::from_le_bytes(lo.try_into().ok()?);
        e.value = i32::from_le_bytes(value.try_into().ok()?);
        e.buff_dmg = i32::from_le_bytes(buff_dmg.try_into().ok()?);
        Some(e)
    }

    fn team(&self, time: u64, id: u32, team: u32) -> Option<EvtcEvent> {
        let mut e = self.agent_row(time, TEAM_CHANGE, id)?;
        e.value = team as i32;
        Some(e)
    }

    /// The max health arcdps writes: none for players (0), the value for everyone else. The DLL
    /// sends players' real max health for the condition model's health check.
    fn evtc_max(&self, id: u32, max: u32) -> u32 {
        if self.reg.agent(id).is_some_and(|a| a.kind == AgentKind::Player) {
            0
        } else {
            max
        }
    }

    fn max_health(&self, time: u64, id: u32, max: u32) -> Option<EvtcEvent> {
        let mut e = self.agent_row(time, MAX_HEALTH, id)?;
        e.dst_agent = u64::from(max);
        Some(e)
    }

    /// x and y as two f32 in `dst_agent`, z as f32 in `value` (0 for facing).
    fn motion(&self, time: u64, id: u32, kind: MotionKind, xyz: [u32; 3]) -> Option<EvtcEvent> {
        let (sc, z) = match kind {
            MotionKind::Position => (POSITION, xyz[2]),
            MotionKind::Velocity => (VELOCITY, xyz[2]),
            MotionKind::Facing => (FACING, 0),
        };
        let mut e = self.agent_row(time, sc, id)?;
        e.dst_agent = u64::from(xyz[0]) | (u64::from(xyz[1]) << 32);
        e.value = z as i32;
        Some(e)
    }

    /// A buff row: agents by instance id (0 = none, written as addr 0).
    fn buff(&self, time: u64, r: &Row) -> EvtcEvent {
        let addr = |id: u32| {
            if id == 0 {
                0
            } else {
                self.addr(id).unwrap_or(0)
            }
        };
        EvtcEvent {
            src_agent: addr(r.src),
            dst_agent: r.dst_raw.unwrap_or_else(|| addr(r.dst)),
            src_instid: r.src as u16,
            dst_instid: if r.dst_raw.is_some() { 0 } else { r.dst as u16 },
            skillid: r.skill,
            value: r.value,
            buff_dmg: r.buff_dmg,
            overstack: r.overstack,
            iff: r.iff(),
            buff: r.buff,
            result: r.result,
            is_buffremove: r.is_buffremove,
            is_shields: r.is_shields,
            is_offcycle: r.is_offcycle,
            pad: r.pad,
            ..sc(time, r.statechange)
        }
    }

    /// A gadget model row (87): dst = file reference, value/buff_dmg = propid (low/high).
    fn model(
        &self,
        time: u64,
        id: u32,
        (file, propid, flag36): (u64, u64, bool),
    ) -> Option<EvtcEvent> {
        self.agent_row(time, GADGET_MODEL, id).map(|mut e| {
            e.dst_agent = file;
            e.value = propid as u32 as i32;
            e.buff_dmg = (propid >> 32) as u32 as i32;
            e.is_flanking = u8::from(flag36);
            e
        })
    }

    /// An agent info row (89).
    fn info(&self, time: u64, id: u32, (value, buff_dmg): (u32, u32)) -> Option<EvtcEvent> {
        self.agent_row(time, AGENT_INFO, id).map(|mut e| {
            e.value = value as i32;
            e.buff_dmg = buff_dmg as i32;
            e
        })
    }

    /// A cast row; `None` when the caster has no addr. Starts carry the target and iff.
    fn cast(&self, time: u64, r: &casts::Row, state: &State) -> Option<EvtcEvent> {
        let mut e = self.agent_row(time, r.statechange, r.src)?;
        if let Some(d) = r.dst_raw {
            // arcdps writes a teleport's target only for squad members.
            let target = r.statechange != casts::TELEPORT || self.reg.is_squad(r.src);
            e.dst_agent = if target { d } else { 0 };
            e.value = if target { r.value } else { 0 };
        }
        e.is_flanking = r.is_flanking;
        e.is_shields = r.is_shields;
        e.is_offcycle = r.is_offcycle;
        if r.dst_raw.is_some() {
            e.skillid = r.skill;
            e.overstack = r.overstack;
            return Some(e);
        }
        if r.statechange == casts::WEAPON_SWAP {
            e.dst_agent = u64::from(r.dst);
        } else if r.statechange == casts::CAST_START {
            if r.dst != 0 {
                e.dst_agent = self.addr(r.dst).unwrap_or(0);
                e.dst_instid = r.dst as u16;
            }
            e.iff = state.iff(self.reg, r.src, r.dst);
        }
        e.skillid = r.skill;
        e.value = r.value;
        e.buff_dmg = r.buff_dmg;
        e.overstack = r.overstack;
        e.result = r.result;
        e.is_activation = r.is_activation;
        Some(e)
    }

    /// An effect or missile row. Agents unknown to the registry are written as addr 0 with the
    /// instid kept, as arcdps writes agents missing from its table.
    fn effect(&self, time: u64, r: &effects::Row, state: &State) -> EvtcEvent {
        let addr = |id: u32| {
            if id == 0 {
                0
            } else {
                self.addr(id).unwrap_or(0)
            }
        };
        let t = r.tail;
        let iff = if !r.team_iff {
            t[0]
        } else if r.src == r.dst {
            0
        } else {
            state.iff(self.reg, r.src, r.dst)
        };
        EvtcEvent {
            src_agent: addr(r.src),
            dst_agent: r.dst_raw.unwrap_or_else(|| addr(r.dst)),
            src_instid: r.src as u16,
            dst_instid: if r.dst_raw.is_some() { 0 } else { r.dst as u16 },
            value: r.value,
            buff_dmg: r.buff_dmg,
            overstack: r.overstack,
            skillid: r.skill,
            iff,
            buff: t[1],
            result: t[2],
            is_activation: t[3],
            is_buffremove: t[4],
            is_ninety: t[5],
            is_fifty: t[6],
            is_moving: t[7],
            is_flanking: t[9],
            is_shields: t[10],
            is_offcycle: t[11],
            pad: r.pad,
            ..sc(time, r.statechange)
        }
    }

    fn health(&self, time: u64, id: u32, pct_x100: u16) -> Option<EvtcEvent> {
        let mut e = self.agent_row(time, HEALTH_UPDATE, id)?;
        e.dst_agent = u64::from(pct_x100);
        Some(e)
    }
}

fn arcbuild_row(time: u64) -> EvtcEvent {
    let b = ARCDPS_BUILD;
    let u64_at = |o: usize| u64::from_le_bytes(b[o..o + 8].try_into().unwrap_or_default());
    let i32_at = |o: usize| i32::from_le_bytes(b[o..o + 4].try_into().unwrap_or_default());
    EvtcEvent {
        src_agent: u64_at(0),
        dst_agent: u64_at(8),
        value: i32_at(16),
        buff_dmg: i32_at(20),
        ..sc(time, ARCBUILD)
    }
}

/// The server values for the log-start rows, and the tick at the start. Values unknown at the
/// start come from the fight's first `ServerTick` that has them; with no tick before the start,
/// the first one in the fight is back-extrapolated at the nominal rate (arcdps always has one,
/// from any agent message; axidps reads only the strike's).
fn start_server(f: &Fight) -> (Server, Option<u32>) {
    let mut s = f.server;
    let mut start_tick = s.clock.seeded().then(|| s.clock.extrapolate(f.start_ms).0);
    for &(t, ev) in f.events.iter().filter(|(t, _)| *t <= f.end_ms) {
        if let Event::ServerTick {
            tick,
            server_ip,
            shard_ids,
            ..
        } = ev
        {
            if start_tick.is_none() && tick != 0 {
                start_tick = Some(tick_before(t, tick, f.start_ms));
            }
            if s.server_ip == 0 {
                s.server_ip = server_ip;
            }
            if s.shard_ids == 0 {
                s.shard_ids = shard_ids;
            }
        }
    }
    (s, start_tick)
}

/// A sc 84 row: src = extrapolated tick, dst = ticks extrapolated since the last real one,
/// value = the last ping as arcdps keeps it (an f32, truncated).
fn tick_row(time: u64, (ext, since): (u32, u32), ping: u32) -> EvtcEvent {
    EvtcEvent {
        src_agent: u64::from(ext),
        dst_agent: u64::from(since),
        value: ping as f32 as i32,
        ..sc(time, TICK)
    }
}

pub fn build_log(f: &Fight, reg: &Registry) -> Log {
    build_log_report(f, reg).0
}

/// What the condition model did in one fight (`crate::condi`).
#[derive(Debug, Clone, PartialEq)]
pub struct CondiReport {
    /// Enemy players' health vs strikes and condition damage (the model's totals check).
    pub health: condi::HealthCheck,
    pub verdict: condi::Verdict,
    pub check: condi::SelfCheck,
    pub coverage: condi::Coverage,
    /// Estimated rows written (0 unless `Fight::condi_write`).
    pub written: u32,
}

/// [`build_log`], plus the condition model's self-check and coverage.
pub fn build_log_report(f: &Fight, reg: &Registry) -> (Log, CondiReport) {
    let order = first_seen(f);
    let ctx = Ctx {
        reg,
        lives: Lives::new(&order, reg, f.pov),
    };
    let mut events = Vec::new();
    let mut ids: BTreeSet<u32> = BTreeSet::new();
    let (head, start_tick) = start_server(f);

    // LOG_START, POV, LANGUAGE, ARCBUILD, GW2_BUILD, SHARDID, MAP_ID (arcdps's order).
    events.push(marker(f, f.start_ms, LOG_START));
    if let Some(e) = f
        .pov
        .and_then(|id| ctx.agent_row(f.start_ms, POINT_OF_VIEW, id))
    {
        events.push(e);
    }
    // LANGUAGE: the client's text language (0 = English, the owner's client); ARCBUILD.
    events.push(sc(f.start_ms, LANGUAGE));
    events.push(arcbuild_row(f.start_ms));
    if let Some(Event::Map { gw2_build, .. }) = f.map {
        events.push(EvtcEvent {
            src_agent: u64::from(gw2_build),
            ..sc(f.start_ms, GW2_BUILD)
        });
    }
    // SHARDID: the map-load message's shard ids (src and dst, u16 each). value/buff_dmg
    // (1010 in the owner's logs) are not captured.
    if head.shard_ids != 0 {
        events.push(EvtcEvent {
            src_agent: u64::from(head.shard_ids & 0xffff),
            dst_agent: u64::from(head.shard_ids >> 16),
            ..sc(f.start_ms, SHARD_ID)
        });
    }
    if let Some(Event::Map {
        map_id, map_type, ..
    }) = f.map
    {
        events.push(EvtcEvent {
            src_agent: u64::from(map_id),
            dst_agent: u64::from(map_type),
            ..sc(f.start_ms, MAP_ID)
        });
    }

    // Extension block.
    let (group_state, dll_version) = match f.status {
        Some(Event::Status {
            dll_version,
            group_state,
            ..
        }) => (group_state, dll_version),
        _ => ([UNKNOWN_GROUP_STATE; 16], 0),
    };
    let (ext, drops) = extension::rows(f.start_ms, f.end_ms, group_state, f.dropped, dll_version);
    events.extend(ext);
    // The healing extension's registration, when its rows can be in this log: the condition
    // group (the combat-hit capture) was live, or a heal came in.
    let heals_live = group_state.get(CONDITION_GROUP) == Some(&GROUP_LIVE)
        || f.events
            .iter()
            .any(|(_, e)| matches!(e, Event::Heal { .. } | Event::RelayHeal { .. }));
    if heals_live {
        events.push(extension::healing_registration(f.start_ms));
    }
    // INSTANCESTART after them, where arcdps has it after its own extension rows (50).
    if let Some(tick) = start_tick {
        events.push(EvtcEvent {
            src_agent: instance_start(f.start_ms, tick),
            value: head.server_ip as i32,
            ..sc(f.start_ms, INSTANCE_START)
        });
    }
    // Squad ground markers: all 8 slots at the start while in a squad, then each change.
    let mut squad_markers = f.squad_markers;
    if let Some(slots) = squad_markers.slots {
        events.extend(
            slots
                .iter()
                .enumerate()
                .map(|(i, &p)| squad_marker_row(f.start_ms, i as u8, p)),
        );
    }
    // Attack targets: one row each at the start, then on a parent change.
    let mut attack_parents: BTreeMap<u32, u32> = f
        .start_snapshot
        .iter()
        .filter_map(|a| a.attack_parent.map(|p| (a.id, p)))
        .collect();
    events.extend(
        attack_parents
            .iter()
            .filter_map(|(&id, &p)| attack_target_row(&ctx, f.start_ms, id, p)),
    );
    // WvW objectives: on a WvW map, every cached objective of the map at the start, then each
    // message arcdps writes a row for.
    let map_id = match f.map {
        Some(Event::Map { map_id, .. }) => Some(map_id),
        _ => None,
    };
    let mut wvw = f.wvw.clone();
    let mut wvw_teams = f.wvw_teams;
    if let Some(Event::Map {
        map_id, map_type, ..
    }) = f.map
        && names::is_wvw_map(map_type)
    {
        events.extend(wvw.on_map(map_id).map(|o| wvw_row(f.start_ms, o)));
    }
    // TICK rows: arcdps evaluates every frame; axidps at every fight event and at the end, so a
    // row's time is the event's (arcdps: the frame's), and a catch-up after a gap with no events
    // takes one event per row (arcdps: one frame per row).
    let mut server = f.server;
    let mut tick_rows = start_tick.map(TickRows::new);

    // Snapshot rows, and the state the strike flags are computed from.
    let mut masters = Masters::new(&f.start_snapshot);
    let snapshot_rows = events.len();
    let mut state = State {
        team: BTreeMap::new(),
        health: BTreeMap::new(),
        max: BTreeMap::new(),
        in_combat: BTreeSet::new(),
        present: f.start_snapshot.iter().map(|a| a.id).collect(),
        subgroup: BTreeMap::new(),
        sub_time: BTreeMap::new(),
        breakbar: BTreeMap::new(),
        flags: BTreeMap::new(),
        capturing: BTreeSet::new(),
        shapes: f
            .start_snapshot
            .iter()
            .filter(|a| !a.shape.is_empty())
            .map(|a| (a.id, a.shape.clone()))
            .collect(),
        down: BTreeSet::new(),
    };
    let start_pov_team = f
        .pov
        .and_then(|p| f.start_snapshot.iter().find(|a| a.id == p))
        .and_then(|a| a.team);
    for a in &f.start_snapshot {
        // GUILD for every squad player present at log start, first of its rows (arcdps's
        // log-start pass; it writes no other GUILD rows).
        if a.kind == AgentKind::Player && (f.pov == Some(a.id) || squad_fields(a, start_pov_team)) {
            events.extend(ctx.guild(f.start_ms, a.id, a.guild.unwrap_or_default()));
        }
        // arcdps re-emits every character's transformation (if any, before) and action speed
        // at log start (1.0 from spawn).
        if matches!(a.kind, AgentKind::Player | AgentKind::Npc) {
            if let Some(r) = f.casts.transform_row(a.id, f.start_ms) {
                events.extend(ctx.cast(f.start_ms, &r, &state));
            }
            let speed = a.speed.unwrap_or(1.0f32.to_bits());
            events.extend(ctx.agent_row(f.start_ms, ACTION_SPEED, a.id).map(|mut e| {
                e.value = speed as i32;
                e
            }));
        }
        if let Some(info) = a.info {
            events.extend(ctx.info(f.start_ms, a.id, info));
        }
        if let Some(model) = a.model {
            events.extend(ctx.model(f.start_ms, a.id, model));
        }
        if let Some(fl) = a.flags {
            events.extend(state.flags(&ctx, f.start_ms, a.id, fl));
        }
        if let Some((s, p)) = a.breakbar {
            events.extend(state.breakbar(&ctx, f.start_ms, a.id, s, f32::from_bits(p)));
        }
        if let Some(team) = a.team {
            state.team.insert(a.id, team);
            events.extend(ctx.team(f.start_ms, a.id, team));
        }
        if let Some((pct, max)) = a.health {
            let max = ctx.evtc_max(a.id, max);
            state.health.insert(a.id, pct);
            state.max.insert(a.id, max);
            events.extend(ctx.max_health(f.start_ms, a.id, max));
            events.extend(ctx.health(f.start_ms, a.id, pct));
        }
        if let Some(sub) = a.subgroup {
            state.set_subgroup(a.id, sub, f.start_ms);
        }
        if a.in_combat {
            state.in_combat.insert(a.id);
            events.extend(ctx.combat(f.start_ms, a.id, true, a.subgroup));
        }
        if a.state == Some(AgentState::Down) {
            state.down.insert(a.id);
        }
        // Up is the default, so only a non-Up state is written at the start.
        if let Some(s @ (AgentState::Down | AgentState::Dead)) = a.state {
            events.extend(ctx.state(f.start_ms, a.id, s));
        }
        if reg.addr(a.id).is_some() {
            ids.insert(a.id);
        }
    }

    // BUFFINITIAL for the stacks known at the start, then the fight's buff events over them.
    let mut stacks = f.buffs.clone();
    stacks.record_ticks();
    let mode = condi::GameMode::from_map_type(match f.map {
        Some(Event::Map { map_type, .. }) => Some(map_type),
        _ => None,
    });
    let mut model = condi::Model::new(f.condi.clone(), f.pov, mode);
    // Enemy health as a check on the model's totals (see `condi::HealthCheck`).
    let mut health = condi::HealthCheck::default();
    let mut not_up: BTreeSet<u32> = BTreeSet::new();
    let mut max_hp: BTreeMap<u32, u32> = BTreeMap::new();
    let enemy_player = |id: u32| {
        reg.agent(id)
            .is_some_and(|a| a.kind == AgentKind::Player && !reg.is_squad(id))
    };
    let mut condi_written = 0u32;
    let master_of = |id: u32| reg.agent(id).and_then(|a| a.master);
    let mut skills = BTreeSet::new();
    for r in stacks.initial(f.start_ms) {
        ids.extend(
            [r.src, r.dst]
                .into_iter()
                .filter(|&i| i != 0 && reg.addr(i).is_some()),
        );
        skills.insert(r.skill);
        events.push(ctx.buff(f.start_ms, &r));
    }
    masters.fill(&mut events[snapshot_rows..], &state.present);

    // Fight events: drop everything after end_ms (R8), then stable-sort by time.
    // Events before start_ms (R9) are clamped to start_ms so they stay inside LOG_START.
    let mut fight_events: Vec<(u64, Event)> = f
        .events
        .iter()
        .filter(|(t, _)| *t <= f.end_ms)
        .map(|&(t, ev)| (t.max(f.start_ms), ev))
        .collect();
    fight_events.sort_by_key(|(t, _)| *t);
    let mut casts: Casts = f.casts.clone();
    let mut effects: Effects = f.effects.clone();
    let mut markers: BTreeSet<u32> = BTreeSet::new();
    let mut effect_ids: BTreeSet<u32> = BTreeSet::new();
    // Each event's rows get their masters once the event is done (the next turn, or after the
    // loop): a branch may `continue`.
    let mut senders: Vec<(usize, u16)> = Vec::new();
    let (mut rows_from, mut done) = (events.len(), None);
    for &(time, ev) in &fight_events {
        // Max health per agent, last nonzero (recordings before d4320a4 have 0 for players:
        // no health check for them).
        if let Event::Health { id, max, .. } = ev
            && max != 0
        {
            max_hp.insert(id, max);
        }
        if time >= f.start_ms {
            match ev {
                Event::Health { id, pct_x100, .. } if enemy_player(id) => {
                    if not_up.contains(&id) {
                        health.interrupt(id);
                    } else {
                        health.health(id, pct_x100, max_hp.get(&id).copied().unwrap_or(0));
                    }
                }
                Event::State { id, state } => {
                    if state == AgentState::Up {
                        not_up.remove(&id);
                    } else {
                        not_up.insert(id);
                        health.interrupt(id);
                    }
                }
                Event::AgentRemove { id } => health.interrupt(id),
                Event::Strike {
                    dst, value, result, ..
                } if result <= 7 && value > 0 => health.strike(dst, value),
                Event::BuffDamage { src, dst, value, .. }
                    if f.pov.is_some() && (Some(src) == f.pov || master_of(src) == f.pov) =>
                {
                    health.pov_tick(dst, value)
                }
                _ => {}
            }
        }
        // Condition ticks due before this event: the model's rows (estimates for others).
        stacks.advance(time);
        for fired in stacks.take_fired() {
            if fired.time < f.start_ms {
                continue;
            }
            if let Some(rows) = immune_tick_rows(&ctx, &state, reg, &stacks, &fired) {
                for (src, e) in rows {
                    skills.insert(e.skillid);
                    ids.extend([src, fired.target]);
                    events.push(e);
                }
                continue;
            }
            for t in model.tick(&fired, master_of, |id| stacks.active_on(id, fired.time)) {
                health.model_tick(t.dst, t.damage);
                if f.condi_write
                    && let Some(e) = buff_damage_row(
                        &ctx, &state, reg, t.time, t.src, t.dst, t.buff, t.damage, 0,
                    )
                {
                    skills.insert(t.buff);
                    ids.extend([t.src, t.dst]);
                    events.push(e);
                    condi_written += 1;
                }
            }
        }
        model.known.apply(&ev);
        masters.fill(&mut events[rows_from..], &state.present);
        if let Some(prev) = done.replace(ev) {
            masters.apply(&prev);
        }
        rows_from = events.len();
        server.apply(time, &ev);
        if let Some(rows) = tick_rows.as_mut().filter(|_| server.clock.seeded())
            && let Some(r) = rows.step(&server.clock, time)
        {
            events.push(tick_row(time, r, server.ping));
        }
        ids.extend(event_ids(&ev));
        // Activation markers due by now, before a start resets the progress.
        for (t, agent) in casts.activations(time) {
            if t >= f.start_ms
                && stacks.has_active(agent, CONFUSION, t)
                && let Some(e) = activation_row(&ctx, &state, t, agent)
            {
                skills.insert(casts::ACTIVATION_MARKER_SKILL);
                ids.insert(agent);
                events.push(e);
            }
        }
        // Cast rows first: arcdps ends the open cast before it writes a down/death row.
        for r in casts.apply(time, &ev) {
            ids.extend([r.src, r.dst].into_iter().filter(|&i| i != 0));
            // Only cast rows carry a skill; a transformation's skillid is not one, and arcdps
            // never lists skill 0.
            if matches!(r.statechange, casts::CAST_START | casts::CAST_END) {
                skills.insert(r.skill);
            }
            events.extend(ctx.cast(time, &r, &state));
        }
        for r in effects.apply(&ev) {
            ids.extend([r.src, r.dst].into_iter().filter(|&i| i != 0));
            skills.extend(r.table_skill);
            let e = ctx.effect(time, &r, &state);
            if matches!(e.is_statechange, EFFECT_GROUND | EFFECT_AGENT) {
                effect_ids.insert(e.skillid);
            }
            events.push(e);
        }
        match ev {
            Event::AgentAdd {
                id, team, subgroup, ..
            } => {
                state.set_subgroup(id, subgroup, time);
                if state.present.insert(id) {
                    events.extend(ctx.agent_row(time, SPAWN, id));
                }
                if state.team.insert(id, team) != Some(team) {
                    events.extend(ctx.team(time, id, team));
                }
            }
            Event::AgentRemove { id } => {
                stacks.forget_agent(id);
                state.in_combat.remove(&id);
                state.present.remove(&id);
                state.breakbar.remove(&id);
                state.flags.remove(&id);
                state.capturing.remove(&id);
                state.team.remove(&id);
                state.down.remove(&id);
                events.extend(ctx.agent_row(time, DESPAWN, id));
            }
            Event::Combat { id, in_combat } => {
                // A row only on a change, like arcdps: the game re-sends Combat(true) while an
                // agent stays in combat, and sends Combat(false) when an out-of-combat agent
                // comes into range.
                let changed = if in_combat {
                    state.in_combat.insert(id)
                } else {
                    state.in_combat.remove(&id)
                };
                if changed {
                    let sub = state.current_subgroup(id);
                    events.extend(ctx.combat(time, id, in_combat, sub));
                }
            }
            Event::State { id, state: s } => {
                if s == AgentState::Down {
                    state.down.insert(id);
                } else {
                    state.down.remove(&id);
                }
                events.extend(ctx.state(time, id, s));
            }
            Event::AgentFlags {
                id,
                targetable,
                visible,
                named,
            } => events.extend(state.flags(&ctx, time, id, (targetable, visible, named))),
            Event::AgentInfo {
                id,
                value,
                buff_dmg,
            } => events.extend(ctx.info(time, id, (value, buff_dmg))),
            Event::GadgetModel {
                id,
                file,
                propid,
                flag36,
            } => events.extend(ctx.model(time, id, (file, propid, flag36))),
            Event::GadgetCapture {
                id,
                op,
                progress_bits,
                team,
                by,
            } => {
                let row = |sc| ctx.agent_row(time, sc, id);
                // arcdps reads the outline just before queuing the 80 row: its 83 batch first.
                let outline = state.shapes.get(&id).cloned().unwrap_or_default();
                let start = |team| {
                    outline_rows(&ctx, time, id, &outline)
                        .into_iter()
                        .chain(row(CAPTURE_START).map(|mut e| {
                            e.buff = team;
                            e
                        }))
                        .collect::<Vec<_>>()
                };
                match op {
                    CaptureOp::Progress => {
                        if state.capturing.insert(id) {
                            events.extend(start(team));
                        }
                        events.extend(row(CAPTURE_UPDATE).map(|mut e| {
                            e.value = progress_bits as i32;
                            e.buff = team;
                            e.result = by;
                            e
                        }));
                    }
                    CaptureOp::Start => {
                        state.capturing.insert(id);
                        events.extend(start(team));
                    }
                    CaptureOp::Stop => {
                        state.capturing.remove(&id);
                        events.extend(row(CAPTURE_STOP));
                    }
                }
            }
            Event::WvwTeam { .. } | Event::WvwWorld { .. } => wvw_teams.apply(&ev),
            Event::AttackTarget { id, parent } => {
                if attack_parents.insert(id, parent) != Some(parent) {
                    events.extend(attack_target_row(&ctx, time, id, parent));
                }
            }
            Event::Reward { id, kind } => events.push(EvtcEvent {
                dst_agent: u64::from(id),
                value: i32::from(kind),
                ..sc(time, REWARD)
            }),
            Event::WvwObjective { .. } => {
                if let Some(o) = wvw.apply(&ev, map_id) {
                    events.push(wvw_row(time, o));
                }
            }
            Event::SquadMarker { .. } => {
                if let Some((slot, p)) = squad_markers.apply(&ev) {
                    events.push(squad_marker_row(time, slot, p));
                }
            }
            Event::GadgetShape {
                id,
                index,
                count,
                x,
                y,
            } => {
                let shape = state.shapes.entry(id).or_default();
                if index == 0 {
                    shape.clear();
                }
                if count > 0 && usize::from(index) == shape.len() {
                    shape.push((x, y));
                }
            }
            Event::ActionSpeed { id, speed_bits } => {
                events.extend(ctx.agent_row(time, ACTION_SPEED, id).map(|mut e| {
                    e.value = speed_bits as i32;
                    e
                }));
            }
            Event::Glider { id, gliding } => {
                events.extend(ctx.agent_row(time, GLIDER, id).map(|mut e| {
                    e.value = i32::from(gliding);
                    e
                }));
            }
            Event::Motion { id, kind, xyz } => events.extend(ctx.motion(time, id, kind, xyz)),
            Event::Tag { id, tag, commander } => {
                if tag != 0 {
                    markers.insert(tag);
                }
                events.extend(ctx.agent_row(time, TAG, id).map(|mut e| {
                    e.value = tag as i32;
                    e.buff = u8::from(commander);
                    e
                }));
            }
            Event::Breakbar {
                id,
                state: s,
                pct_bits,
            } => events.extend(state.breakbar(&ctx, time, id, s, f32::from_bits(pct_bits))),
            Event::Barrier { id, pct_x100 } => {
                events.extend(ctx.agent_row(time, BARRIER, id).map(|mut e| {
                    e.dst_agent = u64::from(pct_x100);
                    e
                }));
            }
            Event::BuffApply { .. }
            | Event::BuffRemove { .. }
            | Event::BuffRemoveAll { .. }
            | Event::BuffStack { .. } => {
                for r in stacks.apply(time, &ev) {
                    if r.skill != 0 {
                        skills.insert(r.skill);
                    }
                    events.push(ctx.buff(time, &r));
                }
            }
            Event::Team { id, team } => {
                // The game sends Team first when an agent comes (back) into range: from here on
                // it is a fresh agent, and arcdps writes its enter row again.
                state.in_combat.remove(&id);
                state.team.insert(id, team);
                events.extend(ctx.team(time, id, team));
            }
            Event::Health { id, pct_x100, max } => {
                let max = ctx.evtc_max(id, max);
                state.health.insert(id, pct_x100);
                if state.max.insert(id, max) != Some(max) {
                    events.extend(ctx.max_health(time, id, max));
                }
                events.extend(ctx.health(time, id, pct_x100));
            }
            Event::Strike {
                src,
                dst,
                skill,
                value,
                overstack,
                result,
            } => {
                let (Some(sa), Some(da)) = (ctx.addr(src), ctx.addr(dst)) else {
                    continue;
                };
                skills.insert(skill);
                events.push(EvtcEvent {
                    src_agent: sa,
                    dst_agent: da,
                    value,
                    // Damage into barrier; `value` already includes it (session 2026-11-24, R3b).
                    overstack,
                    skillid: skill,
                    src_instid: src as u16,
                    dst_instid: dst as u16,
                    iff: state.iff(reg, src, dst),
                    result,
                    is_ninety: u8::from(state.health.get(&src).is_some_and(|&h| h > 9000)),
                    is_fifty: u8::from(state.health.get(&dst).is_some_and(|&h| h < 5000)),
                    ..sc(time, 0)
                });
            }
            Event::BuffDamage {
                src,
                dst,
                buff,
                value,
                barrier,
                ..
            } => {
                // The POV's exact ticks: the model's self-check.
                if buff_damage_result(buff) == BUFF_DAMAGE_TICK
                    && src != 0
                    && (f.pov == Some(src) || master_of(src).is_some_and(|m| f.pov == Some(m)))
                {
                    model.check.exact(buff, value);
                }
                // arcdps's model rows; the amounts are the game's own.
                let Some(e) =
                    buff_damage_row(&ctx, &state, reg, time, src, dst, buff, value, barrier)
                else {
                    continue;
                };
                skills.insert(buff);
                events.push(e);
            }
            Event::Heal {
                src,
                dst,
                skill,
                value,
                barrier,
                buff,
            }
            | Event::RelayHeal {
                src,
                dst,
                skill,
                value,
                barrier,
                buff,
                ..
            } => {
                // The healing extension's row: arcdps's event for the hit, signs flipped by the
                // addon, written with e10.
                // Who sent it (src/dst flags) is set once the masters are filled, below: the
                // POV for a local heal, the peer for a relayed one.
                let (sender, relayed_down) = match ev {
                    Event::RelayHeal { sender, downed, .. } => (Some(sender), Some(downed)),
                    _ => (f.pov, None),
                };
                let (Some(sa), Some(da)) =
                    ((src != 0).then(|| ctx.addr(src)).flatten(), ctx.addr(dst))
                else {
                    continue;
                };
                skills.insert(skill);
                let down = relayed_down.unwrap_or_else(|| state.down.contains(&dst));
                let amount = value.saturating_neg();
                if let Some(sender) = sender.and_then(|id| u16::try_from(id).ok()) {
                    senders.push((events.len(), sender));
                }
                events.push(EvtcEvent {
                    src_agent: sa,
                    dst_agent: da,
                    value: if buff { 0 } else { amount },
                    buff_dmg: if buff { amount } else { 0 },
                    overstack: barrier,
                    is_shields: u8::from(barrier != 0),
                    skillid: skill,
                    src_instid: src as u16,
                    dst_instid: dst as u16,
                    iff: state.iff(reg, src, dst),
                    buff: u8::from(buff),
                    // arcdps: 1 while the target is downed; the addon adds its flag for buffs.
                    is_offcycle: u8::from(down)
                        | if buff && down {
                            extension::HEAL_TARGET_DOWNED
                        } else {
                            0
                        },
                    is_ninety: u8::from(state.health.get(&src).is_some_and(|&h| h > 9000)),
                    is_fifty: u8::from(state.health.get(&dst).is_some_and(|&h| h < 5000)),
                    pad: extension::HEALING_SIG,
                    ..sc(time, extension::STATECHANGE_EXTENSION_COMBAT)
                });
            }
            Event::Status { .. }
            | Event::AgentName { .. }
            | Event::Map { .. }
            | Event::Pov { .. }
            | Event::LogControl { .. }
            | Event::CastStart { .. }
            | Event::CastEnd { .. }
            | Event::WeaponSwap { .. }
            | Event::Jump { .. }
            | Event::FlyTo { .. }
            | Event::Teleport { .. }
            | Event::GadgetAnimation { .. }
            | Event::Transform { .. }
            | Event::EffectGround { .. }
            | Event::EffectGroundRemove { .. }
            | Event::EffectAgent { .. }
            | Event::EffectAgentRemove { .. }
            | Event::MissileCreate { .. }
            | Event::MissileLaunch { .. }
            | Event::MissileRemove { .. }
            | Event::MissileEffect { .. }
            | Event::ServerTick { .. }
            | Event::AgentMaster { .. }
            | Event::AgentGuild { .. }
            | Event::AgentGuid { .. }
            | Event::GroupKey { .. }
            | Event::CoreStats { .. }
            | Event::GearAttrs { .. }
            | Event::GearSkill { .. }
            | Event::BuffFormula { .. } => {}
        }
    }
    masters.fill(&mut events[rows_from..], &state.present);
    heal_senders(&mut events, &senders);
    for (t, agent) in casts.activations(f.end_ms) {
        if t >= f.start_ms
            && stacks.has_active(agent, CONFUSION, t)
            && let Some(e) = activation_row(&ctx, &state, t, agent)
        {
            skills.insert(casts::ACTIVATION_MARKER_SKILL);
            ids.insert(agent);
            events.push(e);
        }
    }
    stacks.advance(f.end_ms);
    for fired in stacks.take_fired() {
        if fired.time < f.start_ms {
            continue;
        }
        if let Some(rows) = immune_tick_rows(&ctx, &state, reg, &stacks, &fired) {
            for (src, e) in rows {
                skills.insert(e.skillid);
                ids.extend([src, fired.target]);
                events.push(e);
            }
            continue;
        }
        for t in model.tick(&fired, master_of, |id| stacks.active_on(id, fired.time)) {
            health.model_tick(t.dst, t.damage);
            if f.condi_write
                && let Some(e) =
                    buff_damage_row(&ctx, &state, reg, t.time, t.src, t.dst, t.buff, t.damage, 0)
            {
                skills.insert(t.buff);
                ids.extend([t.src, t.dst]);
                events.push(e);
                condi_written += 1;
            }
        }
    }
    if let Some(rows) = tick_rows.as_mut().filter(|_| server.clock.seeded())
        && let Some(r) = rows.step(&server.clock, f.end_ms)
    {
        events.push(tick_row(f.end_ms, r, server.ping));
    }

    events.push(drops);
    // WVWTEAMS: arcdps writes it at save, time 0, just before the content rows.
    if let Some((src_agent, dst_agent, value, buff_dmg)) = wvw_teams.row() {
        events.push(EvtcEvent {
            src_agent,
            dst_agent,
            value,
            buff_dmg,
            ..sc(0, WVW_TEAMS)
        });
    }
    // Content rows for the log's skills, per skill in id order, just before LOG_END (as arcdps
    // writes them at save).
    for &id in &skills {
        events.extend(content::rows(id));
    }
    events.push(marker(f, f.end_ms, LOG_END));
    state.close_all(f.end_ms);

    // Agent table: referenced agents, the squad, and the POV, one entry per addr.
    ids.extend(f.squad_seen.iter().copied());
    ids.extend(
        reg.live_agents()
            .filter(|a| reg.is_squad(a.id))
            .map(|a| a.id),
    );
    ids.extend(f.pov);
    ids.extend(masters.named());
    let pov_team = f.pov.and_then(|id| reg.agent(id)).and_then(|a| a.team);
    // No map known (no MumbleLink): an axidps log is a WvW log.
    let wvw = match f.map {
        Some(Event::Map { map_type, .. }) => names::is_wvw_map(map_type),
        _ => true,
    };
    let mut table: BTreeMap<u64, Agent> = BTreeMap::new();
    // A player's lives share an addr: its first life in the fight describes it, as in arcdps.
    let rank: BTreeMap<u32, usize> = order.iter().enumerate().map(|(i, &id)| (id, i)).collect();
    let mut ids: Vec<u32> = ids.into_iter().collect();
    ids.sort_by_key(|id| (rank.get(id).copied().unwrap_or(usize::MAX), *id));
    for id in ids {
        let Some(addr) = ctx.addr(id) else { continue };
        let info = reg.agent(id).cloned().unwrap_or_else(|| AgentInfo {
            id,
            kind: AgentKind::Unknown,
            prof: 0,
            elite: 0,
            species: 0,
            team: None,
            subgroup: None,
            character: String::new(),
            account: String::new(),
            health: None,
            in_combat: false,
            state: None,
            breakbar: None,
            speed: None,
            flags: None,
            info: None,
            model: None,
            shape: Vec::new(),
            attack_parent: None,
            wvw_rank_id: None,
            master: None,
            guild: None,
            account_guid: None,
        });
        // A player in the squad at the start stays in it: if the end read lost the squad
        // (subgroup 0, as seen live 2026-11-24), the start's account and subgroup stand.
        let mut info = info;
        if !squad_fields(&info, pov_team)
            && let Some(s) = f.start_snapshot.iter().find(|s| {
                s.id == id
                    && s.kind == info.kind
                    && s.prof == info.prof
                    && squad_fields(s, pov_team)
            })
        {
            info.subgroup = s.subgroup;
            info.account = s.account.clone();
        }
        if info.kind == AgentKind::Player
            && let Some(sub) = state.main_subgroup(id)
        {
            info.subgroup = Some(sub);
        }
        let squad = f.pov == Some(id) || squad_fields(&info, pov_team);
        table
            .entry(addr)
            .or_insert_with(|| agent_row(addr, &info, squad, wvw));
    }

    // idtoguid rows for the species, gadgets, markers and effects in the log, before LOG_END.
    let mut guids: BTreeSet<(u8, u32)> = table
        .values()
        .filter(|a| a.is_elite == u32::MAX)
        .map(|a| (content::GUID_SPECIES, a.prof & 0xffff))
        .collect();
    guids.extend(markers.iter().map(|&m| (content::GUID_MARKER, m)));
    guids.extend(effect_ids.iter().map(|&e| (content::GUID_EFFECT, e)));
    let end = events.pop();
    events.extend(guids.into_iter().filter_map(|(t, id)| content::guid(t, id)));
    events.extend(end);

    model.check_pov_stats(|id| stacks.active_on(id, f.end_ms));
    let report = CondiReport {
        health,
        verdict: model.check.verdict(),
        check: model.check,
        coverage: model.coverage,
        written: condi_written,
    };
    let log = Log {
        header: Header::wvw(),
        agents: table.into_values().collect(),
        skills: skills
            .into_iter()
            .map(|id| Skill::named(id, skill_name(id)))
            .collect(),
        events,
    };
    (log, report)
}

/// arcdps's activation marker (strike result 11): src 0, dst =
/// the caster, skill 23279, value 1, iff 2; written only while the caster has Confusion.
fn activation_row(ctx: &Ctx, state: &State, time: u64, agent: u32) -> Option<EvtcEvent> {
    Some(EvtcEvent {
        dst_agent: ctx.addr(agent)?,
        value: 1,
        skillid: casts::ACTIVATION_MARKER_SKILL,
        dst_instid: agent as u16,
        iff: 2,
        result: ACTIVATION_MARKER_RESULT,
        is_fifty: u8::from(state.health.get(&agent).is_some_and(|&h| h < 5000)),
        ..sc(time, 0)
    })
}

/// arcdps's zero condition-tick rows on a target holding an invulnerability buff (result 6) or
/// an "invert" category buff (13): one per `(src, buff)` of the tick, buff_dmg
/// 0, for every source (the game reports no tick then, and the amount needs no formula).
/// `None` when the target holds neither; the tick then goes to the condition model.
fn immune_tick_rows(
    ctx: &Ctx,
    state: &State,
    reg: &Registry,
    stacks: &Stacks,
    fired: &crate::buffs::Fired,
) -> Option<Vec<(u32, EvtcEvent)>> {
    let result = if stacks.holds(fired.target, |b| crate::content::buff_flags(b).0) {
        IMMUNE_INVULNERABLE
    } else if stacks.holds(fired.target, |b| crate::content::buff_flags(b).1) {
        IMMUNE_INVERT
    } else {
        return None;
    };
    let groups: BTreeSet<(u32, u32)> = fired.stacks.iter().map(|&(s, b, _)| (s, b)).collect();
    Some(
        groups
            .into_iter()
            .filter_map(|(src, buff)| {
                let mut e =
                    buff_damage_row(ctx, state, reg, fired.time, src, fired.target, buff, 0, 0)?;
                e.result = result;
                Some((src, e))
            })
            .collect(),
    )
}

/// Results of a zero condition tick on an invulnerable / "invert" target.
const IMMUNE_INVULNERABLE: u8 = 6;
const IMMUNE_INVERT: u8 = 13;

/// Strike result of the activation marker.
const ACTIVATION_MARKER_RESULT: u8 = 11;
/// Confusion: the buff whose holder gets activation markers (only Confusion
/// was seen).
const CONFUSION: u32 = 861;

/// A buff-damage row (arcdps's model-row shape): exact POV ticks and the
/// condition model's estimates. `None` when either agent has no addr.
#[allow(clippy::too_many_arguments)]
fn buff_damage_row(
    ctx: &Ctx,
    state: &State,
    reg: &Registry,
    time: u64,
    src: u32,
    dst: u32,
    buff: u32,
    value: i32,
    barrier: u32,
) -> Option<EvtcEvent> {
    let sa = (src != 0).then(|| ctx.addr(src)).flatten()?;
    let da = ctx.addr(dst)?;
    Some(EvtcEvent {
        src_agent: sa,
        dst_agent: da,
        buff_dmg: value,
        // Damage into barrier, like arcdps's own rows.
        overstack: barrier,
        is_shields: u8::from(barrier != 0),
        skillid: buff,
        src_instid: src as u16,
        dst_instid: dst as u16,
        iff: state.iff(reg, src, dst),
        buff: 1,
        result: buff_damage_result(buff),
        is_offcycle: u8::from(state.down.contains(&dst)),
        is_ninety: u8::from(state.health.get(&src).is_some_and(|&h| h > 9000)),
        is_fifty: u8::from(state.health.get(&dst).is_some_and(|&h| h < 5000)),
        ..sc(time, 0)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::casts::Casts;
    use crate::registry::tests::{add, name_events};
    use crate::server::Server;
    use axidps_protocol::v2::{AgentState, MotionKind, NameField};

    fn feed(r: &mut Registry, evs: &[Event]) {
        evs.iter().for_each(|e| r.apply(0, e));
    }

    fn npc(id: u32, team: u32) -> Event {
        Event::AgentAdd {
            id,
            kind: AgentKind::Npc,
            prof: 0,
            elite: 0,
            species: 77,
            team,
            subgroup: 0,
            wvw_rank: 0,
            wvw_rank_def: 0,
        }
    }

    fn strike(src: u32, dst: u32, skill: u32) -> Event {
        Event::Strike {
            src,
            dst,
            skill,
            value: 500,
            overstack: 9,
            result: 0,
        }
    }

    /// Squad player id 1 ("Test Char"/"test.1234", subgroup 3, team 1), enemy player id 2, NPC id 3.
    fn world() -> Registry {
        let mut r = Registry::new();
        feed(&mut r, &[add(1, 3), add(2, 0), npc(3, 9)]);
        feed(&mut r, &name_events(1, NameField::Character, "Test Char"));
        feed(&mut r, &name_events(1, NameField::Account, "test.1234"));
        feed(&mut r, &name_events(2, NameField::Character, "Enemy Char"));
        feed(&mut r, &name_events(3, NameField::Character, "Test Npc"));
        r.apply(0, &Event::Pov { id: 1 });
        r
    }

    fn status() -> Event {
        Event::Status {
            unix_s: 0,
            dll_version: 12,
            group_state: [0, 1, 2, 3, 4, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0],
        }
    }

    fn fight(events: Vec<(u64, Event)>) -> Fight {
        Fight {
            start_ms: 1000,
            end_ms: 5000,
            events,
            start_snapshot: vec![],
            map: Some(Event::Map {
                map_id: 38,
                map_type: 11,
                shard: 0,
                gw2_build: 123_456,
            }),
            pov: Some(1),
            status: Some(status()),
            unix_at: Some((500, 1_700_000_000)),
            dropped: 0,
            squad_seen: BTreeSet::from([1]),
            buffs: Stacks::default(),
            casts: Casts::default(),
            effects: crate::effects::Effects::default(),
            server: Server::default(),
            squad_markers: Default::default(),
            wvw: Default::default(),
            wvw_teams: Default::default(),
            condi: Default::default(),
            condi_write: false,
        }
    }

    fn sc_rows(l: &Log, k: u8) -> Vec<&EvtcEvent> {
        l.events.iter().filter(|e| e.is_statechange == k).collect()
    }

    #[test]
    fn log_starts_and_ends_with_markers() {
        let l = build_log(&fight(vec![(2000, strike(1, 3, 10))]), &world());
        let (first, last) = (l.events.first().unwrap(), l.events.last().unwrap());
        assert_eq!(
            (first.is_statechange, first.time, first.src_agent),
            (9, 1000, ARC_ID)
        );
        assert_eq!(first.value, 1_700_000_000); // (1000-500)/1000 = 0 whole seconds
        assert_eq!(first.buff_dmg, first.value);
        assert_eq!(
            (last.is_statechange, last.time, last.src_agent),
            (10, 5000, ARC_ID)
        );
        assert_eq!(last.value, 1_700_000_004); // 4500 ms
        assert_eq!(last.buff_dmg, last.value);
        let mut f = fight(vec![]);
        f.unix_at = None;
        assert_eq!(build_log(&f, &world()).events[0].value, 0);
    }

    #[test]
    fn header_is_wvw_rev1() {
        let h = build_log(&fight(vec![]), &world()).header;
        assert_eq!((h.boss_id, h.revision), (1, 1));
        assert_eq!(h, Header::wvw());
    }

    #[test]
    fn extension_registration_and_group_rows() {
        let l = build_log(&fight(vec![]), &world());
        let ext = sc_rows(&l, 40);
        let reg_row = ext.iter().find(|e| e.pad == 0).unwrap();
        assert_eq!(reg_row.src_agent, 0x4449_5841 | 1 << 32); // public-check:ok
        let groups: Vec<_> = ext
            .iter()
            .filter(|e| e.pad == extension::SIG && e.skillid < 9)
            .collect();
        assert_eq!(groups.len(), 9);
        let want = [0, 1, 2, 3, 4, 0, 0, 0, 2];
        for (i, g) in groups.iter().enumerate() {
            assert_eq!((g.skillid, g.value), (i as u32, want[i]));
        }
        let v = ext.iter().find(|e| e.skillid == 0x101).unwrap();
        assert_eq!(
            (v.src_agent, v.dst_agent),
            (12, u64::from(axidps_protocol::v2::PROTOCOL_VERSION))
        );
        // Group, version and registration rows follow LOG_START directly.
        assert_eq!(l.events[0].is_statechange, 9);
        assert_eq!(l.events[1].is_statechange, 13);
        assert_eq!(l.events[2].is_statechange, 14);
        assert_eq!(l.events[3].is_statechange, 54);
        assert_eq!(l.events[4].is_statechange, 15);
        assert_eq!(l.events[5].is_statechange, 25);
        assert_eq!(l.events[6].pad, 0);
        assert_eq!(l.events[6].is_statechange, 40);
    }

    /// arcdps writes LANGUAGE (14, src = language index, 0 English) and ARCBUILD (54, its build
    /// string from byte 8) right after the POV row, at the log start.
    #[test]
    fn language_and_arcbuild_rows() {
        let l = build_log(&fight(vec![]), &world());
        let lang = sc_rows(&l, 14);
        assert_eq!(lang.len(), 1);
        assert_eq!((lang[0].time, lang[0].src_agent), (1000, 0));
        let b = sc_rows(&l, 54);
        assert_eq!(b.len(), 1);
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&b[0].src_agent.to_le_bytes());
        bytes.extend_from_slice(&b[0].dst_agent.to_le_bytes());
        bytes.extend_from_slice(&b[0].value.to_le_bytes());
        bytes.extend_from_slice(&b[0].buff_dmg.to_le_bytes());
        assert_eq!(&bytes[..], b"20261115.180220-613-x64\0");
        assert_eq!((b[0].time, b[0].overstack, b[0].skillid), (1000, 0, 0));
    }

    fn tick_ev(tick: u32, ping: u32) -> Event {
        Event::ServerTick {
            tick,
            ping,
            server_ip: 0xcfdf_3164, // public-check:ok
            shard_ids: 0x8000_0002, // public-check:ok
        }
    }

    /// A fight whose server tick is known before the start at 40 ms per tick (119 at 960, so
    /// 120 at the start, 1000).
    fn tick_fight() -> Fight {
        let mut f = fight(vec![
            (1050, strike(1, 3, 10)),
            (1500, tick_ev(132, 104)),
            (2000, strike(1, 3, 10)),
            (2100, strike(1, 3, 10)),
        ]);
        for (t, k) in [(200, 100), (600, 110), (960, 119)] {
            f.server.apply(t, &tick_ev(k, 95));
        }
        f
    }

    /// Log-start rows in arcdps's order: 9, 13, 14, 54, 15, 16, 25, then axidps's extension rows
    /// (40) where arcdps has 50, then 42.
    #[test]
    fn shard_and_instance_start_rows_in_header_order() {
        let l = build_log(&tick_fight(), &world());
        let order: Vec<u8> = l.events.iter().take(7).map(|e| e.is_statechange).collect();
        assert_eq!(order, [9, 13, 14, 54, 15, 16, 25]);
        let ext_end = l
            .events
            .iter()
            .rposition(|e| e.is_statechange == 40 && e.time == 1000);
        let i42 = l
            .events
            .iter()
            .position(|e| e.is_statechange == 42)
            .unwrap();
        assert_eq!(Some(i42), ext_end.map(|i| i + 1));
        let s16 = sc_rows(&l, 16);
        assert_eq!(s16.len(), 1);
        assert_eq!(
            (
                s16[0].time,
                s16[0].src_agent,
                s16[0].dst_agent,
                s16[0].value
            ),
            (1000, 2, 32768, 0)
        );
        let s42 = &l.events[i42];
        // start - tick * 40 = 1000 - 4800, as arcdps's wrapping u64.
        assert_eq!(s42.time, 1000);
        assert_eq!(s42.src_agent, 1000u64.wrapping_sub(4800));
        assert_eq!(s42.value, 0xcfdf_3164u32 as i32);
    }

    /// sc 84: arcdps's extrapolation, a row each time it passes start + 25k (first one tick
    /// after the start), evaluated at event times and once at the log end; value = last ping.
    #[test]
    fn tick_rows() {
        let l = build_log(&tick_fight(), &world());
        let rows: Vec<_> = sc_rows(&l, 84)
            .iter()
            .map(|e| (e.time, e.src_agent, e.dst_agent, e.value))
            .collect();
        assert_eq!(
            rows,
            [
                (1050, 121, 2, 95),
                (2100, 146, 14, 104),
                (5000, 218, 86, 104)
            ]
        );
        let end = l
            .events
            .iter()
            .position(|e| e.is_statechange == 10)
            .unwrap();
        let last84 = l
            .events
            .iter()
            .rposition(|e| e.is_statechange == 84)
            .unwrap();
        assert!(last84 < end);
    }

    /// No tick before the start: the first in-fight tick, back-extrapolated at 40 ms per tick,
    /// gives the start tick; its shard ids and address fill 16 and 42.
    #[test]
    fn start_tick_from_the_first_fight_tick() {
        let f = fight(vec![(1089, tick_ev(505_554, 95))]);
        let l = build_log(&f, &world());
        let s42 = sc_rows(&l, 42);
        assert_eq!(s42.len(), 1);
        assert_eq!(
            s42[0].src_agent,
            crate::server::instance_start(1000, 505_552)
        );
        assert_eq!(s42[0].value, 0xcfdf_3164u32 as i32);
        assert_eq!(sc_rows(&l, 16).len(), 1);
        let first = sc_rows(&l, 84)[0];
        assert_eq!((first.time, first.src_agent), (1089, 505_554));
    }

    #[test]
    fn no_tick_no_tick_rows() {
        let l = build_log(&fight(vec![(2000, strike(1, 3, 10))]), &world());
        for k in [16, 42, 84] {
            assert!(sc_rows(&l, k).is_empty(), "{k}");
        }
    }

    #[test]
    fn drops_are_labelled() {
        let mut f = fight(vec![(2000, strike(1, 3, 10))]);
        f.dropped = 7;
        let l = build_log(&f, &world());
        let n = l.events.len();
        let d = &l.events[n - 2];
        assert_eq!(
            (d.is_statechange, d.pad, d.skillid, d.value),
            (40, extension::SIG, 0x100, 7)
        );
        assert_eq!(l.events[n - 1].is_statechange, 10);
        let (_, row) = extension::rows(0, 0, [0; 16], u64::MAX, 0);
        assert_eq!(row.value, i32::MAX);
    }

    #[test]
    fn pov_row_points_at_pov_addr() {
        let r = world();
        let l = build_log(&fight(vec![]), &r);
        let pov = sc_rows(&l, 13);
        assert_eq!(pov.len(), 1);
        assert_eq!(pov[0].src_agent, r.addr(1).unwrap());
        let m = sc_rows(&l, 25)[0];
        assert_eq!((m.src_agent, m.dst_agent), (38, 11));
        assert_eq!(sc_rows(&l, 15)[0].src_agent, 123_456);
    }

    #[test]
    fn no_pov_no_row() {
        let mut f = fight(vec![]);
        f.pov = None;
        assert!(sc_rows(&build_log(&f, &world()), 13).is_empty());
        f.map = None;
        let l = build_log(&f, &world());
        assert!(sc_rows(&l, 15).is_empty() && sc_rows(&l, 25).is_empty());
    }

    #[test]
    fn team_rows_for_snapshot_and_mid_fight() {
        let r = world();
        let mut f = fight(vec![(3000, Event::Team { id: 3, team: 5 })]);
        f.start_snapshot = vec![r.agent(1).unwrap().clone()];
        let l = build_log(&f, &r);
        let teams = sc_rows(&l, 22);
        let a1 = r.addr(1).unwrap();
        assert!(
            teams
                .iter()
                .any(|e| e.time == 1000 && e.src_agent == a1 && e.value == 1)
        );
        assert!(
            teams
                .iter()
                .any(|e| e.time == 3000 && e.src_agent == r.addr(3).unwrap() && e.value == 5)
        );
    }

    #[test]
    fn enter_exit_combat_carry_subgroup_prof_elite() {
        let mut r = world();
        r.apply(
            0,
            &Event::AgentAdd {
                id: 4,
                kind: AgentKind::Player,
                prof: 5,
                elite: 27,
                species: 0,
                team: 1,
                subgroup: 7,
                wvw_rank: 0,
                wvw_rank_def: 0,
            },
        );
        let mut f = fight(vec![
            (
                2000,
                Event::Combat {
                    id: 4,
                    in_combat: true,
                },
            ),
            (
                3000,
                Event::Combat {
                    id: 4,
                    in_combat: false,
                },
            ),
        ]);
        let mut snap = r.agent(1).unwrap().clone();
        snap.in_combat = true;
        snap.health = Some((8000, 25_000));
        f.start_snapshot = vec![snap];
        let l = build_log(&f, &r);
        let a4 = r.addr(4).unwrap();
        let enter = sc_rows(&l, 1);
        assert_eq!(enter.len(), 2); // snapshot + mid-fight
        let e = enter.iter().find(|e| e.src_agent == a4).unwrap();
        assert_eq!(
            (e.dst_agent, e.value, e.buff_dmg, e.src_instid),
            (7, 5, 27, 4)
        );
        let x = sc_rows(&l, 2)[0];
        assert_eq!(
            (x.src_agent, x.dst_agent, x.value, x.buff_dmg),
            (a4, 7, 5, 27)
        );
        let a1 = r.addr(1).unwrap();
        // A player's max health is left out (0), as before the DLL sent it.
        assert_eq!(sc_rows(&l, 12)[0].dst_agent, 0);
        assert_eq!(sc_rows(&l, 12)[0].src_agent, a1);
        assert_eq!(sc_rows(&l, 8)[0].dst_agent, 8000);
    }

    /// The game sends Combat(false) when an out-of-combat agent comes into range (replay
    /// 2026-11-24: 89 of 132 extra exit rows). Nothing changed, so arcdps writes nothing.
    #[test]
    fn exit_combat_only_after_being_in_combat() {
        let off = |id| Event::Combat {
            id,
            in_combat: false,
        };
        let on = |id| Event::Combat {
            id,
            in_combat: true,
        };
        let mut f = fight(vec![
            (1500, off(2)), // never in combat: no row
            (2000, on(3)),
            (2500, off(3)), // real change: row
            (3000, off(3)), // repeat: no row
            (3500, off(1)), // in combat at the start: row
        ]);
        let mut snap = world().agent(1).unwrap().clone();
        snap.in_combat = true;
        f.start_snapshot = vec![snap];
        let r = world();
        let l = build_log(&f, &r);
        let exits: Vec<_> = sc_rows(&l, 2)
            .iter()
            .map(|e| (e.time, e.src_instid))
            .collect();
        assert_eq!(exits, vec![(2500, 3), (3500, 1)]);
    }

    /// The game re-sends Combat(true) about every 0.6 s while an agent stays in combat; arcdps
    /// writes one row. An agent coming back into range gets a Team message first, and arcdps
    /// writes its enter row again (replay 2026-11-24, instances 4371 and 4878).
    #[test]
    fn enter_combat_once_until_out_or_back_in_range() {
        let on = |id| Event::Combat {
            id,
            in_combat: true,
        };
        let f = fight(vec![
            (1500, on(3)),
            (1600, on(3)), // repeat: no row
            (2000, Event::Team { id: 3, team: 9 }),
            (2000, on(3)), // back in range: row
            (2100, on(3)), // repeat: no row
        ]);
        let l = build_log(&f, &world());
        let enters: Vec<_> = sc_rows(&l, 1).iter().map(|e| e.time).collect();
        assert_eq!(enters, vec![1500, 2000]);
    }

    #[test]
    fn state_rows() {
        let r = world();
        let st = |state| Event::State { id: 3, state };
        let f = fight(vec![
            (2000, st(AgentState::Down)),
            (2100, st(AgentState::Dead)),
            (2200, st(AgentState::Up)),
        ]);
        let l = build_log(&f, &r);
        let rows: Vec<_> = l
            .events
            .iter()
            .filter(|e| matches!(e.is_statechange, 3..=5))
            .map(|e| (e.is_statechange, e.time, e.src_agent))
            .collect();
        let a3 = r.addr(3).unwrap();
        assert_eq!(rows, vec![(5, 2000, a3), (4, 2100, a3), (3, 2200, a3)]);
    }

    #[test]
    fn state_before_start_in_snapshot() {
        let mut r = world();
        r.apply(
            0,
            &Event::State {
                id: 3,
                state: AgentState::Down,
            },
        );
        let mut f = fight(vec![]);
        f.start_snapshot = vec![r.agent(3).unwrap().clone()];
        let l = build_log(&f, &r);
        let rows = sc_rows(&l, 5);
        assert_eq!(rows.len(), 1);
        assert_eq!(
            (rows[0].time, rows[0].src_agent),
            (1000, r.addr(3).unwrap())
        );
        assert!(
            sc_rows(&l, 3).is_empty(),
            "an Up agent writes no CHANGEUP at start"
        );
    }

    #[test]
    fn breakbar_strike_row() {
        let r = world();
        let bar = Event::Strike {
            src: 1,
            dst: 3,
            skill: 4242,
            value: 125,
            overstack: 0,
            result: 10,
        };
        let l = build_log(&fight(vec![(2000, bar)]), &r);
        let s: Vec<_> = l.events.iter().filter(|e| e.is_statechange == 0).collect();
        assert_eq!(s.len(), 1);
        assert_eq!((s[0].result, s[0].value, s[0].skillid), (10, 125, 4242));
    }

    #[test]
    fn strike_iff_and_flags() {
        let mut r = world();
        feed(
            &mut r,
            &[Event::Strike {
                src: 8,
                dst: 8,
                skill: 1,
                value: 1,
                overstack: 0,
                result: 0,
            }],
        ); // no team
        let f = fight(vec![
            (
                1500,
                Event::Health {
                    id: 1,
                    pct_x100: 9500,
                    max: 20_000,
                },
            ),
            (
                1600,
                Event::Health {
                    id: 3,
                    pct_x100: 4000,
                    max: 20_000,
                },
            ),
            (2000, strike(1, 3, 10)), // team 1 vs 9, src 95%, dst 40%
            (2100, strike(1, 1, 11)), // same team
            (2200, strike(1, 8, 12)), // unknown team
            (2300, strike(3, 1, 13)), // src 40%, dst 95%
        ]);
        let l = build_log(&f, &r);
        let s: Vec<_> = l.events.iter().filter(|e| e.is_statechange == 0).collect();
        assert_eq!(s.len(), 4);
        assert_eq!((s[0].iff, s[0].is_ninety, s[0].is_fifty), (1, 1, 1));
        assert_eq!(
            (s[0].value, s[0].overstack, s[0].skillid, s[0].time),
            (500, 9, 10, 2000),
            "overstack carries the barrier damage (session 2026-11-24, R3b)"
        );
        assert_eq!((s[1].iff, s[1].is_fifty), (0, 0));
        assert_eq!(s[2].iff, 2);
        assert_eq!((s[3].is_ninety, s[3].is_fifty), (0, 0));
        // Health unknown means both flags 0.
        let l = build_log(&fight(vec![(2000, strike(1, 3, 10))]), &r);
        let e = l.events.iter().find(|e| e.is_statechange == 0).unwrap();
        assert_eq!((e.is_ninety, e.is_fifty), (0, 0));
    }

    #[test]
    fn max_health_row_only_when_it_changes() {
        // Live 2026-11-24: one MAX_HEALTH per health update gave 9612 rows to arcdps's 1315.
        let h = |t, pct, max| {
            (
                t,
                Event::Health {
                    id: 1,
                    pct_x100: pct,
                    max,
                },
            )
        };
        let f = fight(vec![
            h(1500, 9000, 20_000),
            h(1600, 8000, 20_000),
            h(1700, 8000, 25_000),
        ]);
        let l = build_log(&f, &world());
        let max: Vec<i64> = sc_rows(&l, 12).iter().map(|e| e.dst_agent as i64).collect();
        // Agent 1 is a player: one row of 0, whatever the DLL's max (arcdps writes none).
        assert_eq!(max, vec![0]);
        assert_eq!(sc_rows(&l, 8).len(), 3);
        // An NPC (agent 3): a row per change.
        let n = |t, max| (t, Event::Health { id: 3, pct_x100: 9000, max });
        let f = fight(vec![n(1500, 20_000), n(1600, 20_000), n(1700, 25_000)]);
        let l = build_log(&f, &world());
        let a3 = world().addr(3).unwrap();
        let max: Vec<i64> = sc_rows(&l, 12)
            .iter()
            .filter(|e| e.src_agent == a3)
            .map(|e| e.dst_agent as i64)
            .collect();
        assert_eq!(max, vec![20_000, 25_000]);
    }

    #[test]
    fn team_zero_is_unknown_iff() {
        let mut r = world();
        feed(&mut r, &[npc(4, 0)]);
        let l = build_log(
            &fight(vec![(2000, strike(4, 1, 10)), (2100, strike(1, 4, 11))]),
            &r,
        );
        let s: Vec<_> = l.events.iter().filter(|e| e.is_statechange == 0).collect();
        assert_eq!(s.len(), 2);
        assert_eq!((s[0].iff, s[1].iff), (2, 2));
    }

    #[test]
    fn strike_from_unknown_agent_gets_placeholder() {
        let mut r = world();
        r.apply(0, &strike(50, 1, 10));
        let l = build_log(&fight(vec![(2000, strike(50, 1, 10))]), &r);
        let addr = r.addr(50).unwrap();
        let a = l.agents.iter().find(|a| a.addr == addr).unwrap();
        assert_eq!((a.prof, a.is_elite), (0, 0xFFFF_FFFF)); // public-check:ok
        assert!(a.name.starts_with(b"axidps:unknown\0"));
        let e = l.events.iter().find(|e| e.is_statechange == 0).unwrap();
        assert_eq!((e.src_agent, e.src_instid), (addr, 50));
        // An id the registry never saw: no panic, strike skipped.
        let l = build_log(&fight(vec![(2000, strike(999, 1, 10))]), &r);
        assert!(l.events.iter().all(|e| e.is_statechange != 0));
    }

    #[test]
    fn events_sorted_by_time() {
        let f = fight(vec![
            (3000, strike(1, 3, 10)),
            (2000, strike(1, 3, 11)),
            (2000, strike(1, 3, 12)),
            (
                4000,
                Event::Combat {
                    id: 1,
                    in_combat: true,
                },
            ),
        ]);
        let l = build_log(&f, &world());
        let body: Vec<_> = l
            .events
            .iter()
            .filter(|e| e.is_statechange == 0 || e.is_statechange == 1)
            .collect();
        let times: Vec<_> = body.iter().map(|e| e.time).collect();
        assert_eq!(times, vec![2000, 2000, 3000, 4000]);
        assert_eq!((body[0].skillid, body[1].skillid), (11, 12)); // stable
    }

    #[test]
    fn pre_start_events_are_clamped_into_the_bracket() {
        let f = fight(vec![
            (500, strike(1, 3, 10)),
            (2000, strike(1, 3, 11)),
            (700, Event::Team { id: 3, team: 5 }),
        ]);
        let l = build_log(&f, &world());
        assert!(l.events.iter().all(|e| (1000..=5000).contains(&e.time)));
        assert!(l.events.windows(2).all(|w| w[0].time <= w[1].time));
        let s = l.events.iter().find(|e| e.skillid == 10).unwrap();
        assert_eq!(s.time, 1000);
        let order: Vec<_> = l
            .events
            .iter()
            .filter(|e| e.is_statechange == 0)
            .map(|e| e.skillid)
            .collect();
        assert_eq!(order, vec![10, 11]);
    }

    #[test]
    fn mid_fight_agent_add_emits_spawn_and_team() {
        let r = world();
        let f = fight(vec![(2000, npc(3, 9))]);
        let l = build_log(&f, &r);
        let a3 = r.addr(3).unwrap();
        assert!(
            sc_rows(&l, 6)
                .iter()
                .any(|e| e.src_agent == a3 && e.time == 2000)
        );
        assert!(
            sc_rows(&l, 22)
                .iter()
                .any(|e| e.src_agent == a3 && e.value == 9 && e.time == 2000)
        );
    }

    /// `(time, sc, src_instid, dst_instid, src_master, dst_master)` of the rows naming `id`.
    fn master_rows(l: &Log, id: u16) -> Vec<(u64, u8, u16, u16, u16, u16)> {
        l.events
            .iter()
            .filter(|e| e.src_instid == id || e.dst_instid == id)
            .map(|e| {
                (
                    e.time,
                    e.is_statechange,
                    e.src_instid,
                    e.dst_instid,
                    e.src_master_instid,
                    e.dst_master_instid,
                )
            })
            .collect()
    }

    /// arcdps's finalizer writes the src (dst) agent's current master instid on every row with
    /// a src (dst) agent, while the master is a tracked agent.
    #[test]
    fn minion_rows_carry_their_masters_instid() {
        let mut r = world();
        r.apply(0, &npc(4, 1));
        let mut f = fight(vec![
            (1500, strike(4, 3, 10)),
            (1600, Event::AgentMaster { id: 4, master: 1 }),
            (1700, strike(4, 3, 10)),
            (1800, strike(3, 4, 10)),
            (
                1900,
                Event::Health {
                    id: 4,
                    pct_x100: 5000,
                    max: 100,
                },
            ),
            (2000, Event::AgentRemove { id: 1 }),
            (2100, strike(4, 3, 10)),
            (2200, add(1, 3)),
            (2300, strike(4, 3, 10)),
            (2400, Event::AgentRemove { id: 4 }),
            (2500, npc(4, 1)),
            (2600, strike(4, 3, 10)),
        ]);
        f.start_snapshot = r.live_agents().cloned().collect();
        let l = build_log(&f, &r);
        let rows = master_rows(&l, 4);
        let at = |t: u64, sc: u8| -> Vec<(u16, u16)> {
            rows.iter()
                .filter(|x| x.0 == t && x.1 == sc)
                .map(|x| (x.4, x.5))
                .collect()
        };
        assert_eq!(at(1500, 0), vec![(0, 0)], "no master yet");
        assert_eq!(at(1700, 0), vec![(1, 0)], "minion strikes: src master");
        assert_eq!(
            at(1800, 0),
            vec![(0, 1)],
            "strikes on the minion: dst master"
        );
        assert_eq!(
            at(1900, HEALTH_UPDATE),
            vec![(1, 0)],
            "statechange rows too"
        );
        assert_eq!(at(2100, 0), vec![(0, 0)], "master despawned: none");
        assert_eq!(at(2300, 0), vec![(1, 0)], "master back");
        assert_eq!(at(2400, DESPAWN), vec![(1, 0)], "its despawn row keeps it");
        assert_eq!(at(2500, SPAWN), vec![(0, 0)], "a new life has none");
        assert_eq!(at(2600, 0), vec![(0, 0)]);
        assert!(
            l.events
                .iter()
                .filter(|e| e.src_instid != 4 && e.dst_instid != 4)
                .all(|e| e.src_master_instid == 0 && e.dst_master_instid == 0),
            "nothing else has a master"
        );
    }

    /// A 64-byte arcdps row from its hex.
    fn arc_row(hex: &str) -> EvtcEvent {
        let b: Vec<u8> = (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect();
        EvtcEvent::from_bytes(&b.try_into().unwrap())
    }

    /// GUILD (sc 29) rows, as arcdps writes them: one per squad player present
    /// at log start, the POV included, with the guild GUID's 16 bytes in dst/value/buff_dmg and
    /// zeros for a player without a guild; none for players outside the squad and none later.
    #[test]
    fn guild_rows_for_the_squad_at_log_start() {
        // arcdps 20261124-211816: the POV's GUILD row (event 30) and a squad member's without
        // a guild (event 775).
        let pov = arc_row(
            "060c060000000000d007000000000000d16d17ea5bf111ed81a98fb5cfbe7766\
             0000000000000000990c00000000000000000000000000001d00000000000000",
        );
        let none = arc_row(
            "1c0c06000000000072d8d689a34f81b600000000000000000000000000000000\
             0000000000000000990b00000000000000000000000000001d00000000000000",
        );
        let guid: [u8; 16] = [
            0xd1, 0x6d, 0x17, 0xea, 0x5b, 0xf1, 0x11, 0xed, 0x81, 0xa9, 0x8f, 0xb5, 0xcf, 0xbe,
            0x77, 0x66,
        ];
        let mut r = world();
        r.apply(0, &Event::AgentGuild { id: 1, guid });
        r.apply(0, &add(5, 2));
        feed(&mut r, &name_events(5, NameField::Account, ":Other.2222"));
        // Outside the squad: an ally without an account and an enemy, both with a guild.
        r.apply(0, &Event::AgentGuild { id: 2, guid });
        r.apply(0, &player(4, 9, 2, 0, 0, 0));
        r.apply(0, &Event::AgentGuild { id: 4, guid });
        let mut f = fight(vec![
            (2000, Event::AgentGuild { id: 5, guid }),
            (2100, strike(1, 4, 10)),
        ]);
        f.start_snapshot = r.live_agents().cloned().collect();
        let l = build_log(&f, &r);
        let rows = sc_rows(&l, GUILD);
        assert_eq!(rows.len(), 2, "{rows:?}");
        let want = |arc: &EvtcEvent, id: u32| EvtcEvent {
            time: 1000,
            src_agent: r.addr(id).unwrap(),
            src_instid: id as u16,
            ..arc.clone()
        };
        assert_eq!(*rows[0], want(&pov, 1));
        assert_eq!(*rows[1], want(&none, 5), "no guild at the start: zeros");
    }

    /// A master known before the fight applies from its first rows, and the master is in the
    /// agent table even when nothing else names it.
    #[test]
    fn snapshot_master_and_master_in_agent_table() {
        let mut r = world();
        r.apply(0, &npc(4, 9));
        r.apply(0, &npc(6, 9));
        r.apply(0, &Event::AgentMaster { id: 4, master: 6 });
        let mut f = fight(vec![(1500, strike(4, 1, 10))]);
        f.start_snapshot = r.live_agents().cloned().collect();
        let l = build_log(&f, &r);
        let rows = master_rows(&l, 4);
        assert!(
            rows.iter()
                .any(|x| x.0 == 1000 && x.1 == TEAM_CHANGE && x.4 == 6),
            "{rows:?}"
        );
        assert!(rows.iter().any(|x| x.0 == 1500 && x.1 == 0 && x.4 == 6));
        let a6 = r.addr(6).unwrap();
        assert!(l.agents.iter().any(|a| a.addr == a6), "master listed");
    }

    /// A repeated add for an agent that is present (an identity update, or the DLL's periodic
    /// re-push) is not a spawn; only a new life is. Team rows only when the team changes.
    #[test]
    fn repeated_add_is_not_a_spawn() {
        let r0 = world();
        let mut f = fight(vec![
            (2000, npc(3, 9)), // present at the start: no spawn, same team: no row
            (2500, npc(7, 9)), // new: spawn + team
            (3000, npc(7, 9)), // repeat: nothing
            (3200, npc(7, 4)), // team change: team row only
            (3500, Event::AgentRemove { id: 7 }),
            (4000, npc(7, 4)), // back: spawn + team
        ]);
        f.start_snapshot = r0.live_agents().cloned().collect();
        let mut r = r0.clone();
        r.apply(2500, &npc(7, 9));
        let l = build_log(&f, &r);
        let spawns: Vec<_> = sc_rows(&l, 6)
            .iter()
            .map(|e| (e.time, e.src_instid))
            .collect();
        assert_eq!(spawns, vec![(2500, 7), (4000, 7)]);
        let teams: Vec<_> = sc_rows(&l, 22)
            .iter()
            .filter(|e| e.time > 1000)
            .map(|e| (e.time, e.src_instid, e.value))
            .collect();
        assert_eq!(teams, vec![(2500, 7, 9), (3200, 7, 4), (4000, 7, 4)]);
    }

    /// POSITION / VELOCITY / FACING (19, 20, 21): x and y as f32 in `dst_agent`, z in `value`.
    #[test]
    fn motion_rows() {
        let v = |x: f32, y: f32, z: f32| [x.to_bits(), y.to_bits(), z.to_bits()];
        let f = fight(vec![
            (
                2000,
                Event::Motion {
                    id: 1,
                    kind: MotionKind::Position,
                    xyz: v(1.5, -2.0, 3.25),
                },
            ),
            (
                2100,
                Event::Motion {
                    id: 1,
                    kind: MotionKind::Velocity,
                    xyz: v(4.0, 0.0, -1.0),
                },
            ),
            (
                2200,
                Event::Motion {
                    id: 1,
                    kind: MotionKind::Facing,
                    xyz: v(0.5, 0.75, 0.0),
                },
            ),
        ]);
        let r = world();
        let l = build_log(&f, &r);
        let a1 = r.addr(1).unwrap();
        let unpack = |e: &EvtcEvent| {
            (
                f32::from_bits(e.dst_agent as u32),
                f32::from_bits((e.dst_agent >> 32) as u32),
                f32::from_bits(e.value as u32),
            )
        };
        let p = sc_rows(&l, 19)[0];
        assert_eq!(
            (p.src_agent, p.time, unpack(p)),
            (a1, 2000, (1.5, -2.0, 3.25))
        );
        assert_eq!(unpack(sc_rows(&l, 20)[0]), (4.0, 0.0, -1.0));
        let fc = sc_rows(&l, 21)[0];
        assert_eq!((unpack(fc).0, unpack(fc).1, fc.value), (0.5, 0.75, 0));
    }

    /// Swap rows: dst = the new set, value = the previous one (carried from before the fight).
    #[test]
    fn weapon_swap_rows() {
        let r = world();
        let swap = |id, set| Event::WeaponSwap { id, set };
        let mut f = fight(vec![(2000, swap(1, 5)), (2500, swap(1, 2))]);
        let mut before = Casts::default();
        before.apply(800, &swap(1, 4));
        f.casts = before;
        let l = build_log(&f, &r);
        let a1 = r.addr(1).unwrap();
        let rows: Vec<_> = sc_rows(&l, 11)
            .iter()
            .map(|e| (e.time, e.src_agent, e.src_instid, e.dst_agent, e.value))
            .collect();
        assert_eq!(rows, vec![(2000, a1, 1, 5, 4), (2500, a1, 1, 2, 5)]);
    }

    /// A cast open before the fight ends inside it; starts carry the target and iff; a down
    /// ends the open cast before its state row, as arcdps.
    #[test]
    fn cast_rows() {
        let r = world();
        let start = |agent, target, skill| Event::CastStart {
            agent,
            target,
            skill,
            kind: 4,
            x: 450,
        };
        let mut f = fight(vec![
            (2000, Event::CastEnd { agent: 3, site: 11 }),
            (3000, start(1, 3, 6000)),
            (
                3400,
                Event::State {
                    id: 1,
                    state: AgentState::Down,
                },
            ),
        ]);
        let mut before = Casts::default();
        before.apply(800, &start(3, 0, 7000));
        f.casts = before;
        let l = build_log(&f, &r);
        let (a1, a3) = (r.addr(1).unwrap(), r.addr(3).unwrap());
        let starts = sc_rows(&l, 67);
        assert_eq!(starts.len(), 1);
        let s = starts[0];
        assert_eq!(
            (
                s.time,
                s.src_agent,
                s.dst_agent,
                s.skillid,
                s.value,
                s.result,
                s.iff
            ),
            (3000, a1, a3, 6000, 450, 4, 1)
        );
        assert_eq!((s.src_instid, s.dst_instid), (1, 3));
        let ends = sc_rows(&l, 68);
        assert_eq!(ends.len(), 2);
        assert_eq!(
            (
                ends[0].time,
                ends[0].src_agent,
                ends[0].skillid,
                ends[0].value,
                ends[0].result
            ),
            (2000, a3, 7000, 1200, 11)
        );
        assert_eq!(ends[0].dst_agent, 0);
        assert_eq!(
            (ends[1].src_agent, ends[1].result, ends[1].buff_dmg),
            (a1, 10, 400)
        );
        assert_eq!(ends[1].is_activation, 6);
        let pos = |e: &EvtcEvent| l.events.iter().position(|x| std::ptr::eq(x, e)).unwrap();
        assert!(
            pos(ends[1]) < pos(sc_rows(&l, CHANGE_DOWN)[0]),
            "end before the down row"
        );
        for id in [6000, 7000] {
            assert!(
                l.skills.iter().any(|k| k.id == id),
                "skill {id} in the table"
            );
        }
    }

    /// Effect and missile rows: agents as addrs with instids kept, the ground effect's packed
    /// dst_agent, a remove's iff from the teams, the missile's skill (not the effect id) in the
    /// skill table, and a missile created before the fight still known inside it.
    #[test]
    fn effect_and_missile_rows() {
        use axidps_protocol::v2::{MISSILE_OBJECT, MISSILE_STILL};
        let r = world();
        let mut f = fight(vec![
            (
                2000,
                Event::EffectGround {
                    effect: 17_000,
                    owner: 3,
                    duration: 4000,
                    tracking: 5,
                    pos: [1, 2, 3],
                    orient: [4, 5, 6],
                    scale: 0,
                    shields: 0,
                    flags: 8,
                    flanking: 0,
                },
            ),
            (
                2100,
                Event::EffectAgent {
                    host: 1,
                    src: 3,
                    effect: 21_000,
                    duration: 0,
                    tracking: 9,
                },
            ),
            (
                2200,
                Event::MissileLaunch {
                    id: 4,
                    owner: 1,
                    skill: 5491,
                    target: 3,
                    target_pos: [0; 3],
                    pos: [0; 3],
                    radius: 0,
                    speed: 900,
                    flags: 0,
                    motion: 1,
                    bits: MISSILE_OBJECT | MISSILE_STILL,
                },
            ),
            (
                2300,
                Event::MissileRemove {
                    id: 4,
                    owner: 0,
                    skill: 0,
                    other: 3,
                    pos: [0; 3],
                    bits: 0,
                },
            ),
        ]);
        let mut before = crate::effects::Effects::default();
        before.apply(&Event::MissileCreate {
            id: 4,
            owner: 1,
            skill: 5491,
            skin: 0,
            word_1e: 0,
            pos: [0; 3],
            byte_22: 0,
            byte_23: 0,
        });
        f.effects = before;
        let l = build_log(&f, &r);
        let (a1, a3) = (r.addr(1).unwrap(), r.addr(3).unwrap());
        let g = sc_rows(&l, 60);
        assert_eq!(g.len(), 1);
        assert_eq!(
            (g[0].src_agent, g[0].src_instid, g[0].skillid, g[0].pad),
            (a3, 3, 17_000, 5)
        );
        assert_eq!(g[0].dst_agent, 1 | 2 << 16 | 3 << 32 | 4 << 48);
        assert_eq!(g[0].value, 5 | 6 << 16);
        assert_eq!((g[0].is_buffremove, g[0].dst_instid), (8, 0));
        let a = sc_rows(&l, 62);
        assert_eq!(
            (
                a[0].src_agent,
                a[0].dst_agent,
                a[0].src_instid,
                a[0].dst_instid
            ),
            (a3, a1, 3, 1)
        );
        let rm = sc_rows(&l, 59);
        assert_eq!(rm.len(), 1);
        // Owner from the pre-fight create; dst the target recorded at launch; teams 1 vs 9.
        assert_eq!(
            (rm[0].src_agent, rm[0].dst_agent, rm[0].skillid),
            (a1, a3, 5491)
        );
        assert_eq!((rm[0].iff, rm[0].is_shields), (1, 1));
        assert_eq!(sc_rows(&l, 58)[0].iff, 1, "launch iff byte is the motion");
        assert!(l.skills.iter().any(|k| k.id == 5491), "missile skill");
        assert!(
            !l.skills.iter().any(|k| k.id == 17_000 || k.id == 21_000),
            "effect ids"
        );
    }

    /// Condition ticks on a target holding an invulnerability buff (762 Determined) give arcdps's
    /// zero rows with result 6, one per (src, buff), from every source and without
    /// `--condi-write`; once the buff is gone, none.
    #[test]
    fn immune_target_zero_tick_rows() {
        let r = world();
        let apply = |src, buff, inst, duration| Event::BuffApply {
            dst: 2,
            src,
            buff,
            inst,
            duration,
            level: 80,
            active: true,
        };
        let f = fight(vec![
            (1100, apply(2, 762, 1, 1500)),
            (1200, apply(1, 736, 2, 3000)),
            (1300, apply(3, 736, 3, 3000)),
            (1300, apply(3, 736, 4, 3000)),
            (
                2650,
                Event::BuffRemove {
                    dst: 2,
                    remover: 0,
                    inst: 1,
                },
            ),
        ]);
        let l = build_log(&f, &r);
        let got: Vec<_> = l
            .events
            .iter()
            .filter(|e| {
                e.is_statechange == 0 && e.buff == 1 && e.value == 0 && e.is_buffremove == 0
            })
            .map(|e| (e.time, e.src_agent, e.skillid, e.buff_dmg, e.result))
            .collect();
        let (a1, a3) = (r.addr(1).unwrap(), r.addr(3).unwrap());
        assert_eq!(
            got,
            vec![(2200, a1, 736, 0, 6), (2200, a3, 736, 0, 6)],
            "the 3200 tick comes after the removal: no row (the model has no formulas)"
        );
    }

    /// Activation marker (strike result 11): a caster holding Confusion gets one row when its
    /// cast's progress passes `t_a`; without Confusion, none.
    #[test]
    fn activation_marker_rows() {
        let r = world();
        let t_a = crate::content::timing(5491).unwrap().activation_ms as u64;
        let cast = |t: u64| {
            (
                t,
                Event::CastStart {
                    agent: 2,
                    target: 0,
                    skill: 5491,
                    kind: 1,
                    x: 0,
                },
            )
        };
        let confusion = Event::BuffApply {
            dst: 2,
            src: 1,
            buff: 861,
            inst: 7,
            duration: 1000,
            level: 80,
            active: true,
        };
        let f = fight(vec![(1500, confusion), cast(1600), cast(3000)]);
        let l = build_log(&f, &r);
        let got: Vec<_> = l
            .events
            .iter()
            .filter(|e| e.is_statechange == 0 && e.result == 11)
            .map(|e| (e.time, e.src_agent, e.dst_agent, e.skillid, e.value, e.iff))
            .collect();
        assert_eq!(
            got,
            vec![(1600 + t_a, 0, r.addr(2).unwrap(), 23279, 1, 2)],
            "the second cast passes t_a after Confusion ran out"
        );
    }

    /// Stacks known before the fight open it as BUFFINITIAL; the fight's applies and removes follow,
    /// with agents written as addrs and the buff ids in the skill table.
    #[test]
    fn buff_rows() {
        let r = world();
        let mut f = fight(vec![
            (
                2000,
                Event::BuffApply {
                    dst: 1,
                    src: 3,
                    buff: 740,
                    inst: 2,
                    duration: 4000,
                    level: 0,
                    active: true,
                },
            ),
            (
                2500,
                Event::BuffRemove {
                    dst: 1,
                    remover: 3,
                    inst: 2,
                },
            ),
            (
                2600,
                Event::BuffApply {
                    dst: 1,
                    src: 3,
                    buff: 740,
                    inst: 3,
                    duration: 4000,
                    level: 0,
                    active: true,
                },
            ),
            // No remover: an expiry or overstack, written iff unknown like arcdps.
            (
                2700,
                Event::BuffRemove {
                    dst: 1,
                    remover: 0,
                    inst: 3,
                },
            ),
        ]);
        let mut before = Stacks::default();
        before.apply(
            500,
            &Event::BuffApply {
                dst: 1,
                src: 1,
                buff: 1187,
                inst: 1,
                duration: 10_000,
                level: 0,
                active: true,
            },
        );
        f.buffs = before;
        let l = build_log(&f, &r);
        let (a1, a3) = (r.addr(1).unwrap(), r.addr(3).unwrap());
        let init = sc_rows(&l, 18)[0];
        assert_eq!(
            (
                init.time,
                init.src_agent,
                init.dst_agent,
                init.skillid,
                init.value,
                init.buff
            ),
            (1000, a1, a1, 1187, 9500, 18)
        );
        let ap = sc_rows(&l, 69)[0];
        assert_eq!(
            (
                ap.src_agent,
                ap.dst_agent,
                ap.skillid,
                ap.value,
                ap.pad,
                ap.buff
            ),
            (a3, a1, 740, 4000, 2, 1)
        );
        let rm = sc_rows(&l, 71)[0];
        assert_eq!(
            (
                rm.src_agent,
                rm.dst_agent,
                rm.value,
                rm.is_buffremove,
                rm.iff
            ),
            (a1, a3, 3500, 2, 0)
        );
        let gone = sc_rows(&l, 71)[1];
        assert_eq!((gone.dst_agent, gone.iff), (0, 2));
        let ids: Vec<_> = l.skills.iter().map(|s| s.id).collect();
        assert!(ids.contains(&740) && ids.contains(&1187));
    }

    /// BARRIERUPDATE (38): barrier as a share of max health x10000 in `dst_agent`.
    #[test]
    fn barrier_row() {
        let f = fight(vec![(
            2000,
            Event::Barrier {
                id: 1,
                pct_x100: 2500,
            },
        )]);
        let r = world();
        let l = build_log(&f, &r);
        let b = sc_rows(&l, 38)[0];
        assert_eq!(
            (b.src_agent, b.dst_agent, b.value),
            (r.addr(1).unwrap(), 2500, 0)
        );
    }

    fn tick(src: u32, dst: u32, buff: u32, value: i32, barrier: u32) -> Event {
        Event::BuffDamage {
            src,
            dst,
            buff,
            value,
            barrier,
            stacks: 1,
        }
    }

    /// The local player's condition ticks (kind 42) as arcdps writes buff damage: value 0,
    /// buff_dmg, buff 1, result 14 (16 and 18 for the on-hit procs it tells apart), iff and the
    /// ninety/fifty flags like a strike, barrier in overstack with is_shields, is_offcycle while
    /// the target is downed.
    #[test]
    fn buff_damage_rows() {
        let mut r = world();
        r.apply(0, &tick(50, 1, 736, 1, 0));
        let down = |id, state| Event::State { id, state };
        let f = fight(vec![
            (
                1500,
                Event::Health {
                    id: 1,
                    pct_x100: 9500,
                    max: 20_000,
                },
            ),
            (
                1600,
                Event::Health {
                    id: 3,
                    pct_x100: 4000,
                    max: 20_000,
                },
            ),
            (2000, tick(3, 1, 737, 897, 0)),
            (2100, tick(1, 3, 736, 34, 0)),
            (2200, tick(3, 1, 737, 578, 578)),
            (2300, tick(1, 3, 30285, 121, 0)),
            (2400, tick(1, 3, 26646, 80, 0)),
            (2500, down(3, AgentState::Down)),
            (2600, tick(1, 3, 736, 40, 0)),
            (2700, down(3, AgentState::Up)),
            (2800, tick(1, 3, 736, 41, 0)),
            (2900, tick(50, 1, 736, 7, 0)),
            // No source agent, or an id the registry never saw: no row.
            (3000, tick(0, 1, 736, 9, 0)),
            (3100, tick(999, 1, 736, 9, 0)),
        ]);
        let l = build_log(&f, &r);
        let rows: Vec<_> = l
            .events
            .iter()
            .filter(|e| e.is_statechange == 0 && e.buff == 1)
            .collect();
        assert_eq!(rows.len(), 8);
        let a = |id| r.addr(id).unwrap();
        let t = rows[0];
        assert_eq!(
            (t.time, t.src_agent, t.dst_agent, t.src_instid, t.dst_instid),
            (2000, a(3), a(1), 3, 1)
        );
        assert_eq!(
            (t.value, t.buff_dmg, t.overstack, t.skillid, t.result, t.iff),
            (0, 897, 0, 737, 14, 1)
        );
        assert_eq!(
            (t.is_ninety, t.is_fifty, t.is_shields, t.is_offcycle),
            (0, 0, 0, 0)
        );
        assert_eq!((t.is_activation, t.is_buffremove), (0, 0));
        assert_eq!(
            (rows[1].is_ninety, rows[1].is_fifty, rows[1].skillid),
            (1, 1, 736)
        );
        assert_eq!(
            (rows[2].buff_dmg, rows[2].overstack, rows[2].is_shields),
            (578, 578, 1)
        );
        assert_eq!((rows[3].skillid, rows[3].result), (30285, 16));
        assert_eq!((rows[4].skillid, rows[4].result), (26646, 18));
        assert_eq!((rows[5].buff_dmg, rows[5].is_offcycle), (40, 1));
        assert_eq!((rows[6].buff_dmg, rows[6].is_offcycle), (41, 0));
        assert_eq!((rows[7].src_agent, rows[7].iff), (a(50), 2));
        let skills: Vec<u32> = l.skills.iter().map(|s| s.id).collect();
        for id in [736, 737, 30285, 26646] {
            assert!(skills.contains(&id), "{id} in the skill table");
        }
    }

    fn heal(src: u32, dst: u32, skill: u32, value: i32, barrier: u32, buff: bool) -> Event {
        Event::Heal {
            src,
            dst,
            skill,
            value,
            barrier,
            buff,
        }
    }

    fn heal_rows(l: &Log) -> Vec<&EvtcEvent> {
        l.events
            .iter()
            .filter(|e| e.is_statechange == 49 && e.pad == extension::HEALING_SIG)
            .collect()
    }

    /// The local player's heals and barrier (kind 43) as the healing addon's rows: sc 49, pad =
    /// its signature, signs flipped (value or buff_dmg negative), barrier in overstack with
    /// is_shields, is_offcycle = target downed (plus 0x20 on buffs) | 0x80 the POV (or its
    /// minion) is the source | 0x40 it is the target. Values from arcdps's rows of
    /// 20261124-175441 and 20261124-213145.
    #[test]
    fn healing_extension_rows() {
        let mut r = world();
        feed(&mut r, &[add(4, 2), npc(5, 1)]);
        let mut f = fight(vec![
            (2000, heal(4, 1, 9140, 385, 0, false)),
            (2100, heal(4, 1, 63351, 1075, 1075, false)),
            (2200, heal(4, 1, 72115, 2806, 2806, true)),
            (2300, heal(4, 1, 718, 361, 0, true)),
            (
                2350,
                Event::State {
                    id: 4,
                    state: AgentState::Down,
                },
            ),
            (2400, heal(1, 4, 1066, 1587, 0, false)),
            (2450, heal(1, 4, 718, 92, 0, true)),
            (
                2500,
                Event::State {
                    id: 4,
                    state: AgentState::Up,
                },
            ),
            (2600, heal(1, 1, 80252, 122, 0, false)),
            (2700, Event::AgentMaster { id: 5, master: 1 }),
            (2800, heal(5, 4, 12836, 200, 0, false)),
            // No source agent, or an id the registry never saw: no row.
            (2900, heal(0, 1, 718, 9, 0, true)),
            (3000, heal(999, 1, 718, 9, 0, true)),
        ]);
        f.start_snapshot = r.live_agents().cloned().collect();
        let l = build_log(&f, &r);
        let rows = heal_rows(&l);
        assert_eq!(rows.len(), 8);
        let a = |id| r.addr(id).unwrap();
        let t = rows[0];
        assert_eq!(
            (t.time, t.src_agent, t.dst_agent, t.src_instid, t.dst_instid),
            (2000, a(4), a(1), 4, 1)
        );
        assert_eq!(
            (
                t.value,
                t.buff_dmg,
                t.overstack,
                t.skillid,
                t.buff,
                t.result
            ),
            (-385, 0, 0, 9140, 0, 0)
        );
        assert_eq!((t.iff, t.is_shields, t.is_offcycle), (0, 0, 0x40));
        assert_eq!((t.is_activation, t.is_buffremove), (0, 0));
        let fields = |e: &EvtcEvent| {
            (
                e.value,
                e.buff_dmg,
                e.overstack,
                e.buff,
                e.is_shields,
                e.is_offcycle,
            )
        };
        assert_eq!(fields(rows[1]), (-1075, 0, 1075, 0, 1, 0x40));
        assert_eq!(fields(rows[2]), (0, -2806, 2806, 1, 1, 0x40));
        assert_eq!(fields(rows[3]), (0, -361, 0, 1, 0, 0x40));
        // Revive on a downed ally (arcdps: is_offcycle 129), and a buff heal on it (0x80 | 33).
        assert_eq!(fields(rows[4]), (-1587, 0, 0, 0, 0, 0x81));
        assert_eq!(fields(rows[5]), (0, -92, 0, 1, 0, 0xa1));
        // Self heal: both flags (arcdps: 192).
        assert_eq!(fields(rows[6]), (-122, 0, 0, 0, 0, 0xc0));
        // The POV's minion heals: the source's master is the POV.
        let m = rows[7];
        assert_eq!(
            (m.src_instid, m.src_master_instid, m.is_offcycle),
            (5, 1, 0x80)
        );
        let skills: Vec<u32> = l.skills.iter().map(|s| s.id).collect();
        for id in [9140, 63351, 72115, 718, 1066, 80252, 12836] {
            assert!(skills.contains(&id), "{id} in the skill table");
        }
    }

    fn relay_heal(sender: u32, src: u32, dst: u32, skill: u32, downed: bool) -> Event {
        Event::RelayHeal {
            sender,
            src,
            dst,
            skill,
            value: 300,
            barrier: 0,
            buff: false,
            downed,
        }
    }

    /// A relayed heal is the same row as a local one, but the 0x80/0x40 flags are relative to
    /// its sender (the peer), not the POV; local rows keep the POV's flags.
    #[test]
    fn relayed_heal_rows_carry_the_senders_flags() {
        let mut r = world();
        feed(&mut r, &[add(2, 2), add(3, 2), npc(6, 2)]);
        let mut f = fight(vec![
            (2000, relay_heal(2, 2, 3, 9140, false)),
            (2100, relay_heal(3, 2, 3, 9140, false)),
            (2200, Event::AgentMaster { id: 6, master: 2 }),
            (2300, relay_heal(2, 6, 3, 9140, false)),
            (2400, relay_heal(2, 2, 2, 9140, true)),
            (2500, heal(1, 1, 80252, 122, 0, false)),
            (2600, heal(2, 3, 80252, 122, 0, false)),
        ]);
        f.start_snapshot = r.live_agents().cloned().collect();
        let l = build_log(&f, &r);
        let rows = heal_rows(&l);
        assert_eq!(rows.len(), 6);
        let flags: Vec<u8> = rows.iter().map(|e| e.is_offcycle).collect();
        // Sender B, B->C: 0x80; sender C, B->C: 0x40; B's minion->C: 0x80; B->B downed: 0xc1.
        assert_eq!(flags[0], 0x80);
        assert_eq!(flags[1], 0x40);
        assert_eq!(flags[2], 0x80);
        assert_eq!(flags[3], 0xc1);
        // POV-local: self heal 0xc0; a heal between others carries no POV flags.
        assert_eq!(flags[4], 0xc0);
        assert_eq!(flags[5], 0);
        assert_eq!((rows[0].value, rows[0].pad), (-300, extension::HEALING_SIG));
    }

    #[test]
    fn relayed_only_fight_registers_the_extension() {
        let mut r = world();
        feed(&mut r, &[add(2, 2)]);
        let mut f = fight(vec![(2000, relay_heal(2, 2, 2, 9140, false))]);
        f.status = None;
        f.start_snapshot = r.live_agents().cloned().collect();
        let l = build_log(&f, &r);
        assert_eq!(
            sc_rows(&l, 40)
                .into_iter()
                .filter(|e| e.pad == 0 && e.src_agent & 0xffff_ffff == 0x9c9b_3c99) // public-check:ok
                .count(),
            1
        );
    }

    #[test]
    fn relayed_skill_in_skill_table() {
        let mut r = world();
        feed(&mut r, &[add(2, 2), add(3, 2)]);
        let mut f = fight(vec![(2000, relay_heal(2, 2, 3, 41234, false))]);
        f.start_snapshot = r.live_agents().cloned().collect();
        let l = build_log(&f, &r);
        assert!(l.skills.iter().any(|s| s.id == 41234));
    }

    /// The addon's registration row (sc 40, pad 0): signature | revision 2 << 32 | version
    /// length << 56, the version string in dst_agent. Written when group condition is live or
    /// the fight has heals, after axidps's own extension rows.
    #[test]
    fn healing_extension_registration() {
        let reg_rows = |l: &Log| -> Vec<EvtcEvent> {
            sc_rows(l, 40)
                .into_iter()
                .filter(|e| e.pad == 0 && e.src_agent & 0xffff_ffff == 0x9c9b_3c99) // public-check:ok
                .cloned()
                .collect()
        };
        // Condition group not live (state 3), no heals: none.
        assert!(reg_rows(&build_log(&fight(vec![]), &world())).is_empty());
        // Live condition group, no heals: arcdps writes it in every log the addon was loaded.
        let mut f = fight(vec![]);
        f.status = Some(Event::Status {
            unix_s: 0,
            dll_version: 12,
            group_state: [0, 1, 2, 0, 4, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0],
        });
        let l = build_log(&f, &world());
        let rows = reg_rows(&l);
        assert_eq!(rows.len(), 1);
        let e = &rows[0];
        assert_eq!(e.src_agent, 0x0600_0002_9c9b_3c99); // public-check:ok
        assert_eq!(e.dst_agent.to_le_bytes(), *b"axidps\0\0");
        assert_eq!(
            (e.time, e.value, e.skillid, e.pad, e.is_offcycle),
            (1000, 0, 0, 0, 0)
        );
        // After the axidps rows (its versions row is the last of them).
        let at = l.events.iter().position(|x| x == e).unwrap();
        assert_eq!(l.events[at - 1].skillid, extension::ROW_VERSIONS);
        // A heal alone (group state unknown) also writes it.
        let mut f = fight(vec![(2000, heal(1, 1, 718, 5, 0, true))]);
        f.status = None;
        assert_eq!(reg_rows(&build_log(&f, &world())).len(), 1);
    }

    /// arcdps's own registration row from 20261124-212056 (addon version 2.20rc1) decodes as the
    /// layout above.
    #[test]
    fn healing_registration_layout_matches_arcdps() {
        let (src, dst) = (0x0700_0002_9c9b_3c99u64, 0x0031_6372_3032_2e32u64);
        assert_eq!(src & 0xffff_ffff, u64::from(extension::HEALING_SIG)); // public-check:ok
        assert_eq!((src >> 32) & 0xff_ffff, extension::HEALING_REVISION); // public-check:ok
        assert_eq!(src >> 56, 7);
        assert_eq!(&dst.to_le_bytes()[..7], b"2.20rc1");
    }

    /// Content rows (30/31/32/33/91) for each skill in the log, from the generated table, just
    /// before LOG_END.
    #[test]
    fn content_rows_before_log_end() {
        let id = content::tests::KNOWN_SKILL;
        let l = build_log(&fight(vec![(2000, strike(1, 3, id))]), &world());
        let want = content::rows(id);
        assert!(!want.is_empty());
        let n = l.events.len();
        let at = l
            .events
            .windows(want.len())
            .position(|w| w == &want[..])
            .expect("content rows present");
        // Only non-skill idtoguid rows follow them, then LOG_END.
        assert!(
            l.events[at + want.len()..n - 1]
                .iter()
                .all(|e| e.is_statechange == 46 && e.overstack != 2)
        );
        assert_eq!(l.events[n - 1].is_statechange, LOG_END);
    }

    /// idtoguid (46) rows for the log's NPC species and markers, from the generated table, in
    /// (type, id) order just before LOG_END.
    #[test]
    fn guid_rows_before_log_end() {
        let mut r = world();
        feed(
            &mut r,
            &[Event::AgentAdd {
                id: 4,
                kind: AgentKind::Npc,
                prof: 0,
                elite: 0,
                species: 1082,
                team: 9,
                subgroup: 0,
                wvw_rank: 0,
                wvw_rank_def: 0,
            }],
        );
        let tag = Event::Tag {
            id: 1,
            tag: 35,
            commander: true,
        };
        let l = build_log(&fight(vec![(2000, tag), (2100, strike(1, 4, 0))]), &r);
        let n = l.events.len();
        let tail: Vec<_> = l.events[..n - 1]
            .iter()
            .rev()
            .take_while(|e| e.is_statechange == 46 && e.overstack != 2)
            .map(|e| (e.overstack, e.skillid))
            .collect();
        let mut want = vec![(1, 35), (3, 1082)];
        if content::guid(content::GUID_SPECIES, 77).is_some() {
            want.push((3, 77));
        }
        want.sort_unstable();
        want.reverse();
        assert_eq!(tail, want);
        assert_eq!(l.events[n - 1].is_statechange, LOG_END);
    }

    /// BREAKBARSTATE (34) / BREAKBARPERCENT (35): both at the fight start for agents read
    /// before it (arcdps resets its caches at log start), then each on its own change; a percent
    /// above 1.0 is not written; a despawn resets the caches.
    #[test]
    fn breakbar_rows() {
        let mut r = world();
        let bb = |id, state, pct: f32| Event::Breakbar {
            id,
            state,
            pct_bits: pct.to_bits(),
        };
        r.apply(500, &bb(3, 3, 0.0));
        let mut f = fight(vec![
            (2000, bb(3, 3, 0.0)),
            (2300, bb(3, 0, 0.0)),
            (2600, bb(3, 0, 0.5)),
            (2900, bb(3, 0, 1.5)),
            (3200, bb(3, 2, 1.5)),
            (3500, Event::AgentRemove { id: 3 }),
            (3800, bb(3, 2, 1.5)),
        ]);
        f.start_snapshot = vec![r.agent(3).unwrap().clone()];
        let l = build_log(&f, &r);
        let a3 = r.addr(3).unwrap();
        let rows = |k| -> Vec<(u64, u64, i32)> {
            sc_rows(&l, k)
                .iter()
                .map(|e| (e.time, e.src_agent, e.value))
                .collect()
        };
        assert_eq!(
            rows(34),
            vec![(1000, a3, 3), (2300, a3, 0), (3200, a3, 2), (3800, a3, 2)]
        );
        assert_eq!(
            rows(35),
            vec![(1000, a3, 0), (2600, a3, 0.5f32.to_bits() as i32)]
        );
        assert_eq!(sc_rows(&l, 34)[0].src_instid, 3);
    }

    /// Action speed (92): each character at the fight start with its last speed (1.0 if none
    /// seen), then every message, value = the f32 bits. Glider (55): value 0/1.
    #[test]
    fn action_speed_and_glider_rows() {
        let mut r = world();
        let speed = |id, v: f32| Event::ActionSpeed {
            id,
            speed_bits: v.to_bits(),
        };
        r.apply(500, &speed(1, 1.4925));
        let mut f = fight(vec![
            (2000, speed(3, 0.6667)),
            (2000, speed(3, 0.6667)),
            (
                2500,
                Event::Glider {
                    id: 1,
                    gliding: true,
                },
            ),
        ]);
        f.start_snapshot = vec![r.agent(1).unwrap().clone(), r.agent(3).unwrap().clone()];
        let l = build_log(&f, &r);
        let (a1, a3) = (r.addr(1).unwrap(), r.addr(3).unwrap());
        let rows = |k| -> Vec<(u64, u64, i32)> {
            sc_rows(&l, k)
                .iter()
                .map(|e| (e.time, e.src_agent, e.value))
                .collect()
        };
        let bits = |v: f32| v.to_bits() as i32;
        assert_eq!(
            rows(92),
            vec![
                (1000, a1, bits(1.4925)),
                (1000, a3, bits(1.0)),
                (2000, a3, bits(0.6667)),
                (2000, a3, bits(0.6667)),
            ]
        );
        assert_eq!(rows(55), vec![(2500, a1, 1)]);
    }

    /// Message statechanges: jump (86) and fly-to (88) start rows and their ends on site 23,
    /// teleport (85) with its target only for squad members, gadget animation (77) and
    /// transformation (73), re-emitted at the fight start with the ms left. None of their
    /// skillids enter the skill table.
    #[test]
    fn message_statechange_rows() {
        let r = world();
        assert!(r.is_squad(1) && !r.is_squad(2));
        let tp = |id| Event::Teleport {
            id,
            kind: 16,
            arg: 6,
            dst: 0x1122_3344_5566_7788, // public-check:ok
            value: 0x4000_0000,         // public-check:ok
        };
        let end57 = |agent| Event::CastEnd {
            agent,
            site: casts::site::DODGE_END,
        };
        let tf = |id, transform, duration| Event::Transform {
            id,
            transform,
            offcycle: 5,
            duration,
            shields: false,
        };
        let mut f = fight(vec![
            (
                2000,
                Event::Jump {
                    id: 1,
                    offcycle: 16,
                },
            ),
            (2100, end57(1)),
            (
                2200,
                Event::FlyTo {
                    id: 2,
                    pos: [272, -569, -153],
                    speed: 100,
                },
            ),
            (2250, end57(2)),
            (2300, tp(1)),
            (2300, tp(2)),
            (
                2400,
                Event::GadgetAnimation {
                    id: 3,
                    token: 0x1037_542c, // public-check:ok
                },
            ),
            (2500, tf(1, 844, 1500)),
        ]);
        let mut before = Casts::default();
        before.apply(500, &tf(3, 900, 1000));
        f.casts = before;
        f.start_snapshot = vec![r.agent(1).unwrap().clone(), r.agent(3).unwrap().clone()];
        let l = build_log(&f, &r);
        let (a1, a2, a3) = (r.addr(1).unwrap(), r.addr(2).unwrap(), r.addr(3).unwrap());
        // time, src, dst, value, overstack, skillid, is_flanking, is_shields, is_offcycle.
        type R = (u64, u64, u64, i32, u32, u32, u8, u8, u8);
        let rows = |k| -> Vec<R> {
            sc_rows(&l, k)
                .iter()
                .map(|e| {
                    (
                        e.time,
                        e.src_agent,
                        e.dst_agent,
                        e.value,
                        e.overstack,
                        e.skillid,
                        e.is_flanking,
                        e.is_shields,
                        e.is_offcycle,
                    )
                })
                .collect()
        };
        assert_eq!(
            rows(86),
            vec![
                (2000, a1, 1, 0, 0, 0, 0, 0, 16),
                (2100, a1, 0, 0, 0, 0, 0, 0, 0)
            ]
        );
        assert_eq!(
            rows(88),
            vec![
                (2200, a2, 0x0064_ff67_fdc7_0110, 0, 0, 0, 1, 0, 0), // public-check:ok
                (2250, a2, 0, 0, 0, 0, 0, 0, 0)
            ]
        );
        assert_eq!(
            rows(85),
            vec![
                (2300, a1, 0x1122_3344_5566_7788, 0x4000_0000, 6, 0, 0, 0, 16), // public-check:ok
                (2300, a2, 0, 0, 6, 0, 0, 0, 16),
            ]
        );
        assert_eq!(rows(77), vec![(2400, a3, 0x1037_542c, 0, 0, 0, 0, 0, 0)]); // public-check:ok
        assert_eq!(
            rows(73),
            vec![
                (1000, a3, 0, 500, 0, 900, 0, 0, 5),
                (2500, a1, 0, 1500, 0, 844, 0, 0, 5)
            ]
        );
        let ids: Vec<_> = l.skills.iter().map(|s| s.id).collect();
        assert!(
            !ids.contains(&844) && !ids.contains(&900) && !ids.contains(&0),
            "{ids:?}"
        );
    }

    /// Polled flags: at the fight start each applicable flag (24 targetable with value 1, 76
    /// visible, 78 named) and agent info (89) for agents known before it, as arcdps resets its
    /// caches at log start; then one row per changed field; a despawn resets the caches.
    #[test]
    fn agent_flag_and_info_rows() {
        use axidps_protocol::v2::NOT_APPLICABLE as NA;
        let mut r = world();
        let fl = |id, targetable, visible, named| Event::AgentFlags {
            id,
            targetable,
            visible,
            named,
        };
        r.apply(
            500,
            &Event::AgentInfo {
                id: 3,
                value: 0,
                buff_dmg: 0x2a4_c001, // public-check:ok
            },
        );
        r.apply(500, &fl(3, 1, 1, NA));
        let mut f = fight(vec![
            (2000, fl(3, 1, 0, NA)),
            (2100, fl(3, 1, 0, NA)),
            (2200, fl(3, 0, 1, NA)),
            (2300, Event::AgentRemove { id: 3 }),
            (2400, fl(3, 0, 1, NA)),
            (
                2500,
                Event::AgentInfo {
                    id: 2,
                    value: 20,
                    buff_dmg: 0,
                },
            ),
        ]);
        f.start_snapshot = vec![r.agent(3).unwrap().clone()];
        let l = build_log(&f, &r);
        let (a2, a3) = (r.addr(2).unwrap(), r.addr(3).unwrap());
        let rows = |k| -> Vec<(u64, u64, u64, i32, i32)> {
            sc_rows(&l, k)
                .iter()
                .map(|e| (e.time, e.src_agent, e.dst_agent, e.value, e.buff_dmg))
                .collect()
        };
        assert_eq!(
            rows(24),
            vec![
                (1000, a3, 1, 1, 0),
                (2200, a3, 0, 1, 0),
                (2400, a3, 0, 1, 0)
            ]
        );
        assert_eq!(
            rows(76),
            vec![
                (1000, a3, 1, 0, 0),
                (2000, a3, 0, 0, 0),
                (2200, a3, 1, 0, 0),
                (2400, a3, 1, 0, 0)
            ]
        );
        assert!(rows(78).is_empty(), "not applicable to characters");
        assert_eq!(
            rows(89),
            vec![(1000, a3, 0, 0, 0x2a4_c001), (2500, a2, 0, 20, 0)] // public-check:ok
        );
    }

    /// Gadget capture: progress (81, value = f32 bits, buff = team, result = by) starts the
    /// capture (80) when none is open; start writes 80, stop 82 and closes it.
    #[test]
    fn gadget_capture_rows() {
        use axidps_protocol::v2::CaptureOp;
        let r = world();
        let cap = |op, progress: f32, team, by| Event::GadgetCapture {
            id: 3,
            op,
            progress_bits: progress.to_bits(),
            team,
            by,
        };
        let f = fight(vec![
            (2000, cap(CaptureOp::Progress, 0.9, 3, 1)),
            (2100, cap(CaptureOp::Progress, 0.8, 3, 1)),
            (2200, cap(CaptureOp::Stop, 0.0, 0, 0)),
            (2200, cap(CaptureOp::Start, 0.0, 1, 0)),
            (2200, cap(CaptureOp::Progress, 1.0, 1, 0)),
            (2300, cap(CaptureOp::Stop, 0.0, 0, 0)),
            (2400, cap(CaptureOp::Progress, 1.0, 2, 0)),
        ]);
        let l = build_log(&f, &r);
        let got: Vec<_> = l
            .events
            .iter()
            .filter(|e| matches!(e.is_statechange, 80..=82))
            .map(|e| (e.time, e.is_statechange, e.value, e.buff, e.result))
            .collect();
        let b = |v: f32| v.to_bits() as i32;
        assert_eq!(
            got,
            vec![
                (2000, 80, 0, 3, 0),
                (2000, 81, b(0.9), 3, 1),
                (2100, 81, b(0.8), 3, 1),
                (2200, 82, 0, 0, 0),
                (2200, 80, 0, 1, 0),
                (2200, 81, b(1.0), 1, 0),
                (2300, 82, 0, 0, 0),
                (2400, 80, 0, 2, 0),
                (2400, 81, b(1.0), 2, 0),
            ]
        );
        assert!(
            l.events
                .iter()
                .filter(|e| e.is_statechange == 80)
                .all(|e| e.src_instid == 3)
        );
    }

    /// Attack target (23): a row at the start for one known before, and on a parent change.
    #[test]
    fn attack_target_rows() {
        let mut r = world();
        r.apply(500, &Event::AttackTarget { id: 3, parent: 2 });
        let mut f = fight(vec![
            (2000, Event::AttackTarget { id: 3, parent: 2 }),
            (2500, Event::AttackTarget { id: 3, parent: 1 }),
        ]);
        f.start_snapshot = vec![r.agent(3).unwrap().clone()];
        r.apply(2500, &Event::AttackTarget { id: 3, parent: 1 });
        let l = build_log(&f, &r);
        let (a1, a2, a3) = (r.addr(1).unwrap(), r.addr(2).unwrap(), r.addr(3).unwrap());
        let got: Vec<_> = sc_rows(&l, 23)
            .iter()
            .map(|e| {
                (
                    e.time,
                    e.src_agent,
                    e.dst_agent,
                    e.src_instid,
                    e.dst_instid,
                    e.src_master_instid,
                    e.iff,
                )
            })
            .collect();
        assert_eq!(
            got,
            vec![(1000, a3, a2, 3, 2, 2, 2), (2500, a3, a1, 3, 1, 1, 2)]
        );
    }

    /// WvW teams (74): one row at time 0 before the content rows, from the teams known at the
    /// start and in the fight; none when no team is known.
    #[test]
    fn wvw_teams_row() {
        let r = world();
        let mut f = fight(vec![
            (
                2000,
                Event::WvwTeam {
                    team: 433,
                    colour: 2,
                },
            ),
            (
                2100,
                Event::WvwWorld {
                    colour: 3,
                    world: 11006,
                },
            ),
        ]);
        f.wvw_teams.apply(&Event::WvwTeam {
            team: 708,
            colour: 1,
        });
        f.wvw_teams.apply(&Event::WvwWorld {
            colour: 1,
            world: 11005,
        });
        let l = build_log(&f, &r);
        let got: Vec<_> = sc_rows(&l, 74)
            .iter()
            .map(|e| (e.time, e.src_agent, e.dst_agent, e.value, e.buff_dmg))
            .collect();
        assert_eq!(got, vec![(0, 11005, 11006 | 708 << 32, 433, 0)]);
        let i = l
            .events
            .iter()
            .position(|e| e.is_statechange == 74)
            .unwrap();
        assert!(l.events[i..].iter().skip(1).all(|e| e.is_statechange != 0));
        let l = build_log(&fight(vec![]), &r);
        assert!(sc_rows(&l, 74).is_empty());
    }

    /// Reward (17): dst = id, value = kind, everything else 0.
    #[test]
    fn reward_rows() {
        let r = world();
        let f = fight(vec![(2500, Event::Reward { id: 504, kind: 16 })]);
        let l = build_log(&f, &r);
        let got: Vec<_> = sc_rows(&l, 17)
            .iter()
            .map(|e| (e.time, e.src_agent, e.dst_agent, e.value, e.buff_dmg))
            .collect();
        assert_eq!(got, vec![(2500, 0, 504, 16, 0)]);
    }

    /// WvW objectives (75): the cached objectives of the current map at the start, then a row per
    /// progress message on this map and per owner message for an objective already on it.
    #[test]
    fn wvw_objective_rows() {
        let r = world();
        let ev = |op, g: u8, map_id, objective_id, value| Event::WvwObjective {
            op,
            guid: [g; 16],
            map_id,
            objective_id,
            objective_type: 4,
            value,
        };
        let mut f = fight(vec![
            (2000, ev(1, 1, 0, 0, 2780)),
            (2100, ev(0, 3, 95, 40, 1)),
            (2200, ev(1, 9, 0, 0, 433)),
            (2300, ev(0, 9, 38, 22, 0)),
        ]);
        f.wvw.apply(&ev(0, 1, 38, 16, 70), Some(38));
        f.wvw.apply(&ev(1, 1, 0, 0, 708), Some(38));
        f.wvw.apply(&ev(0, 2, 95, 33, 5), Some(38));
        let l = build_log(&f, &r);
        let got: Vec<_> = sc_rows(&l, 75)
            .iter()
            .map(|e| (e.time, e.value, e.buff_dmg, e.skillid, e.buff, e.pad))
            .collect();
        assert_eq!(
            got,
            vec![
                (1000, 38, 708, 16, 4, 70),
                (2000, 38, 2780, 16, 4, 70),
                (2300, 38, 433, 22, 4, 0),
            ]
        );
    }

    /// Squad ground markers (53): all 8 slots at the start when in a squad, then each change;
    /// src = x | y << 32, dst = z, skillid = slot; leaving the squad writes nothing.
    #[test]
    fn squad_marker_rows() {
        let r = world();
        let inf = f32::INFINITY.to_bits();
        let mk = |slot, v: f32| Event::SquadMarker {
            slot,
            x: v.to_bits(),
            y: (v + 1.0).to_bits(),
            z: (v + 2.0).to_bits(),
        };
        let mut f = fight(vec![
            (2000, mk(4, 10.0)),
            (2100, mk(4, 10.0)),
            (2200, mk(0xff, 0.0)),
            (2300, mk(1, 5.0)),
        ]);
        f.squad_markers.apply(&mk(4, 1.0));
        let l = build_log(&f, &r);
        let got: Vec<_> = sc_rows(&l, 53)
            .iter()
            .map(|e| (e.time, e.skillid, e.src_agent, e.dst_agent))
            .collect();
        let row = |t, slot, v: f32| {
            (
                t,
                slot,
                u64::from(v.to_bits()) | u64::from((v + 1.0).to_bits()) << 32,
                u64::from((v + 2.0).to_bits()),
            )
        };
        let empty = |slot| {
            (
                1000,
                slot,
                u64::from(inf) | u64::from(inf) << 32,
                u64::from(inf),
            )
        };
        let mut want: Vec<_> = (0..8).map(empty).collect();
        want[4] = row(1000, 4, 1.0);
        want.push(row(2000, 4, 10.0));
        // After leaving: slot 1 joins again with the others unknown (+inf), only it changes.
        want.push(row(2300, 1, 5.0));
        assert_eq!(got, want);
    }

    /// Capture outline (83): the batch the start snapshot knows is written just before each 80
    /// row; an in-fight shape replaces it; counts 0 and 2 write nothing.
    #[test]
    fn capture_outline_rows_precede_each_start() {
        use axidps_protocol::v2::CaptureOp;
        let mut r = world();
        let b = |v: f32| v.to_bits();
        let pt = |index, count, x: f32, y: f32| Event::GadgetShape {
            id: 3,
            index,
            count,
            x: b(x),
            y: b(y),
        };
        r.apply(500, &pt(0, 1, 600.0, -1200.0));
        let cap = |op| Event::GadgetCapture {
            id: 3,
            op,
            progress_bits: 0,
            team: 1,
            by: 0,
        };
        let mut f = fight(vec![
            (2000, cap(CaptureOp::Start)),
            (2100, pt(0, 3, 1.0, 2.0)),
            (2100, pt(1, 3, 3.0, 4.0)),
            (2100, pt(2, 3, 5.0, 6.0)),
            (2200, cap(CaptureOp::Start)),
            (2300, pt(0, 2, 1.0, 2.0)),
            (2300, pt(1, 2, 3.0, 4.0)),
            (2400, cap(CaptureOp::Start)),
        ]);
        f.start_snapshot = vec![r.agent(3).unwrap().clone()];
        let l = build_log(&f, &r);
        let got: Vec<_> = l
            .events
            .iter()
            .filter(|e| matches!(e.is_statechange, 80 | 83))
            .map(|e| {
                (
                    e.time,
                    e.is_statechange,
                    e.dst_agent,
                    e.value as u32,
                    e.buff_dmg as u32,
                    e.overstack,
                )
            })
            .collect();
        assert_eq!(
            got,
            vec![
                (2000, 83, 0, b(600.0), b(-1200.0), 1),
                (2000, 80, 0, 0, 0, 0),
                (2200, 83, 0, b(1.0), b(2.0), 3),
                (2200, 83, 1, b(3.0), b(4.0), 3),
                (2200, 83, 2, b(5.0), b(6.0), 3),
                (2200, 80, 0, 0, 0, 0),
                (2400, 80, 0, 0, 0, 0),
            ]
        );
    }

    /// Gadget model info (87): at the fight start for gadgets known before it and on each
    /// message; dst = file reference, value/buff_dmg = propid (low/high), is_flanking = bit 36.
    #[test]
    fn gadget_model_rows() {
        let mut r = world();
        let model = |id, flag36| Event::GadgetModel {
            id,
            file: 0x0100_ee78,     // public-check:ok
            propid: 0x1_2000_0047, // public-check:ok
            flag36,
        };
        r.apply(500, &model(3, false));
        let mut f = fight(vec![(2000, model(3, true))]);
        f.start_snapshot = vec![r.agent(3).unwrap().clone()];
        let l = build_log(&f, &r);
        let a3 = r.addr(3).unwrap();
        let got: Vec<_> = sc_rows(&l, 87)
            .iter()
            .map(|e| {
                (
                    e.time,
                    e.src_agent,
                    e.dst_agent,
                    e.value,
                    e.buff_dmg,
                    e.is_flanking,
                )
            })
            .collect();
        assert_eq!(
            got,
            vec![
                (1000, a3, 0x0100_ee78, 0x2000_0047, 1, 0), // public-check:ok
                (2000, a3, 0x0100_ee78, 0x2000_0047, 1, 1), // public-check:ok
            ]
        );
    }

    /// TAG (37): marker id in `value`, commander flag in `buff`.
    #[test]
    fn tag_row() {
        let f = fight(vec![(
            2000,
            Event::Tag {
                id: 1,
                tag: 212,
                commander: true,
            },
        )]);
        let l = build_log(&f, &world());
        let t = sc_rows(&l, 37)[0];
        assert_eq!((t.value, t.buff, t.dst_agent), (212, 1, 0));
    }

    #[test]
    fn events_after_end_are_dropped() {
        let f = fight(vec![
            (4000, strike(1, 3, 10)),
            (5000, strike(1, 3, 11)),
            (6000, strike(1, 3, 12)),
        ]);
        let l = build_log(&f, &world());
        let max = l.events.iter().map(|e| e.time).max().unwrap();
        assert_eq!(max, 5000);
        assert_eq!(l.events.last().unwrap().is_statechange, 10);
        assert_eq!(l.events.iter().filter(|e| e.is_statechange == 0).count(), 2);
        assert_eq!(l.skills.len(), 2);
    }

    #[test]
    fn players_npcs_gadgets_in_agent_table() {
        let mut r = world();
        r.apply(
            0,
            &Event::AgentAdd {
                id: 6,
                kind: AgentKind::Gadget,
                prof: 0,
                elite: 0,
                species: 0x1_2345,
                team: 0,
                subgroup: 0,
                wvw_rank: 0,
                wvw_rank_def: 0,
            },
        );
        let l = build_log(&f_all(), &r);
        let by = |id: u32| {
            l.agents
                .iter()
                .find(|a| a.addr == r.addr(id).unwrap())
                .unwrap()
        };
        let p = by(1);
        assert_eq!((p.prof, p.is_elite), (1, 0));
        assert!(p.name.starts_with(b"Test Char\0test.1234\x003\0"));
        let e = by(2);
        assert_ne!(e.is_elite, 0xFFFF_FFFF); // public-check:ok
        assert!(e.name.starts_with(b"Guardian\0\0\0"), "non-squad, no rank");
        let n = by(3);
        assert_eq!((n.prof, n.is_elite), (77, 0xFFFF_FFFF)); // public-check:ok
        assert!(n.name.starts_with(b"Test Npc\0"));
        let g = by(6);
        assert_eq!((g.prof, g.is_elite), (0xFFFF_0000 | 0x2345, 0xFFFF_FFFF)); // public-check:ok
        // Addresses are unique and sorted.
        assert!(l.agents.windows(2).all(|w| w[0].addr < w[1].addr));
    }

    /// A player: `(id, team, prof, elite, subgroup, wvw_rank)`.
    fn player(id: u32, team: u32, prof: u32, elite: u32, subgroup: u8, wvw_rank: u32) -> Event {
        Event::AgentAdd {
            id,
            kind: AgentKind::Player,
            prof,
            elite,
            species: 0,
            team,
            subgroup,
            wvw_rank,
            wvw_rank_def: 0,
        }
    }

    /// The agent-table name of each of `ids` for a fight on `map_type` (`None`: no map known).
    /// World: squad POV 1 (rank 6000), ally 2 outside the squad (team 1, Guardian, rank 6000,
    /// with a name and an account the DLL found), enemy 4 (team 9, Warrior, rank 9945), enemy 5
    /// (team 9, Evoker, no rank).
    fn table_names(map_type: Option<u32>, ids: &[u32]) -> Vec<Vec<u8>> {
        let mut r = world();
        r.apply(0, &player(1, 1, 1, 0, 3, 6000));
        r.apply(0, &player(2, 1, 1, 0, 0, 6000));
        feed(&mut r, &name_events(2, NameField::Account, ":Ally.1111"));
        r.apply(0, &player(4, 9, 2, 0, 0, 9945));
        feed(&mut r, &name_events(4, NameField::Character, "Spartan"));
        feed(&mut r, &name_events(4, NameField::Account, ":Less.6590"));
        r.apply(0, &player(5, 9, 6, 80, 0, 0));
        let mut f = fight(
            [2, 4, 5]
                .iter()
                .map(|&d| (2000 + u64::from(d), strike(1, d, 10)))
                .collect(),
        );
        f.map = map_type.map(|map_type| Event::Map {
            map_id: 38,
            map_type,
            shard: 0,
            gw2_build: 123_456,
        });
        let l = build_log(&f, &r);
        ids.iter()
            .map(|&id| {
                let a = l.agents.iter().find(|a| a.addr == r.addr(id).unwrap());
                let name = a.unwrap().name;
                let end = name
                    .windows(3)
                    .position(|w| w == [0, 0, 0])
                    .map_or(64, |p| p + 3);
                name[..end].to_vec()
            })
            .collect()
    }

    /// arcdps names every player outside the squad by WvW rank title on a WvW map, allies
    /// included, with no account or subgroup; the squad keeps real names.
    #[test]
    fn wvw_map_non_squad_players_get_rank_titles() {
        let names = table_names(Some(11), &[1, 2, 4, 5]);
        assert!(names[0].starts_with(b"Test Char\0test.1234\x003\0"));
        assert_eq!(names[1], b"Mithril Veteran\0\0\0");
        assert_eq!(names[2], b"Diamond Legend\0\0\0");
        assert_eq!(names[3], b"Evoker\0\0\0", "no title: spec name");
    }

    /// No map known: axidps logs are WvW logs, so titles are written.
    #[test]
    fn unknown_map_counts_as_wvw() {
        let names = table_names(None, &[2, 4]);
        assert_eq!(names[0], b"Mithril Veteran\0\0\0");
        assert_eq!(names[1], b"Diamond Legend\0\0\0");
    }

    /// Off WvW, arcdps names players outside the squad by elite spec, or profession.
    #[test]
    fn non_wvw_map_non_squad_players_get_spec_names() {
        let names = table_names(Some(5), &[1, 2, 4, 5]);
        assert!(names[0].starts_with(b"Test Char\0test.1234\x003\0"));
        assert_eq!(names[1], b"Guardian\0\0\0");
        assert_eq!(names[2], b"Warrior\0\0\0");
        assert_eq!(names[3], b"Evoker\0\0\0");
    }

    #[test]
    fn squad_member_who_reads_as_out_of_squad_at_the_end_keeps_start_identity() {
        // Live 2026-11-24: squad members read as subgroup 0 at the end; arcdps still lists them.
        let r0 = world();
        let mut f = fight(vec![(2000, strike(1, 3, 10))]);
        f.start_snapshot = r0.live_agents().cloned().collect();
        let mut r = r0.clone();
        r.apply(1500, &add(1, 0));
        let l = build_log(&f, &r);
        let p = l
            .agents
            .iter()
            .find(|a| a.addr == r.addr(1).unwrap())
            .unwrap();
        assert!(p.name.starts_with(b"Test Char\0test.1234\x003\0"));
    }

    /// A subgroup move mid-fight: the agent table takes the subgroup the player spent most of
    /// the fight in (replay 2026-11-24: 8 of 11 movers match arcdps, whose own value lags about a
    /// minute), and each combat row carries the subgroup of its own time.
    #[test]
    fn subgroup_move_mid_fight() {
        let table_sub = |move_at: u64| {
            let r0 = world();
            let on = |in_combat| Event::Combat { id: 1, in_combat };
            let mut f = fight(vec![
                (2000, on(true)),
                (move_at, add(1, 5)),
                (4500, on(false)),
            ]);
            f.start_snapshot = r0.live_agents().cloned().collect();
            let mut r = r0.clone();
            r.apply(move_at, &add(1, 5));
            let l = build_log(&f, &r);
            let p = l
                .agents
                .iter()
                .find(|a| a.addr == r.addr(1).unwrap())
                .unwrap()
                .clone();
            let rows: Vec<_> = l
                .events
                .iter()
                .filter(|e| e.is_statechange == 1 || e.is_statechange == 2)
                .map(|e| (e.time, e.dst_agent))
                .collect();
            (p.name, rows)
        };
        // Fight 1000..5000. Moved at 4000: 3 for 3 s, 5 for 1 s.
        let (name, rows) = table_sub(4000);
        assert!(name.starts_with(b"Test Char\0test.1234\x003\0"));
        assert_eq!(rows, vec![(2000, 3), (4500, 5)]);
        // Moved at 1500: 5 for most of it.
        let (name, _) = table_sub(1500);
        assert!(name.starts_with(b"Test Char\0test.1234\x005\0"));
    }

    /// Joined the squad during the fight: only the time in the squad counts.
    #[test]
    fn squad_joiner_takes_its_longest_subgroup() {
        let r0 = world();
        let mut f = fight(vec![(2000, add(4, 2)), (2500, add(4, 6))]);
        f.start_snapshot = r0.live_agents().cloned().collect();
        let mut r = r0.clone();
        r.apply(2000, &add(4, 2));
        feed(&mut r, &name_events(4, NameField::Character, "Joiner"));
        feed(&mut r, &name_events(4, NameField::Account, "join.1111"));
        r.apply(2500, &add(4, 6));
        let l = build_log(&f, &r);
        let p = l
            .agents
            .iter()
            .find(|a| a.addr == r.addr(4).unwrap())
            .unwrap();
        assert!(
            p.name.starts_with(b"Joiner\0join.1111\x006\0"),
            "{:?}",
            &p.name[..24]
        );
    }

    fn f_all() -> Fight {
        fight(vec![
            (2000, strike(1, 2, 10)),
            (2100, strike(1, 3, 10)),
            (
                2200,
                Event::Combat {
                    id: 6,
                    in_combat: true,
                },
            ),
        ])
    }

    #[test]
    fn skills_table_dedup_sorted() {
        let f = fight(vec![
            (2000, strike(1, 3, 30)),
            (2100, strike(1, 3, 10)),
            (2200, strike(3, 1, 30)),
            (2300, strike(1, 3, 20)),
        ]);
        let l = build_log(&f, &world());
        let ids: Vec<_> = l.skills.iter().map(|s| s.id).collect();
        assert_eq!(ids, vec![10, 20, 30]);
        assert!(
            l.skills.iter().all(|s| s.name == [0u8; 64]),
            "unknown ids: no name"
        );
    }

    /// Skill names as arcdps writes them, from the table of the owner's arcdps logs.
    #[test]
    fn skills_table_has_names() {
        let f = fight(vec![(2000, strike(1, 3, 29604))]);
        let l = build_log(&f, &world());
        assert!(l.skills[0].name.starts_with(b"Chilling Nova\0"));
        assert!(
            SKILL_NAMES
                .windows(2)
                .all(|w| matches!(w, [a, b] if a.0 < b.0))
        );
    }

    fn life(id: u32, prof: u32, elite: u32, team: u32, subgroup: u8) -> Event {
        Event::AgentAdd {
            id,
            kind: AgentKind::Player,
            prof,
            elite,
            species: 0,
            team,
            subgroup,
            wvw_rank: 0,
            wvw_rank_def: 0,
        }
    }

    /// Apply `evs` to `r` (as the processor does) and make them the fight's events.
    fn lived(r: &mut Registry, evs: Vec<(u64, Event)>) -> Fight {
        for (t, e) in &evs {
            r.apply(*t, e);
        }
        fight(evs)
    }

    fn strikes_by(l: &Log, skill: u32) -> Vec<(u64, u16)> {
        l.events
            .iter()
            .filter(|e| e.is_statechange == 0 && e.skillid == skill)
            .map(|e| (e.src_agent, e.src_instid))
            .collect()
    }

    fn names(id: u32, field: NameField, s: &str, t: u64) -> Vec<(u64, Event)> {
        name_events(id, field, s)
            .into_iter()
            .map(|e| (t, e))
            .collect()
    }

    /// Recording 20261124-201138: the allied non-squad Vindicator "Rio Rahl" despawned as
    /// instance 397 and came back 27 s later as 493. arcdps (account GUID addr) writes one
    /// agent; each row keeps its life's instance id.
    #[test]
    fn a_named_player_coming_back_under_a_new_id_is_one_agent() {
        let mut r = world();
        let mut evs = vec![(1500, life(397, 9, 69, 1, 0))];
        evs.extend(names(397, NameField::Character, "Rio Rahl", 1500));
        evs.push((1600, strike(397, 3, 71)));
        evs.push((2000, Event::AgentRemove { id: 397 }));
        evs.push((2500, life(493, 9, 69, 1, 0)));
        evs.extend(names(493, NameField::Character, "Rio Rahl", 2500));
        evs.push((2600, strike(493, 3, 72)));
        let f = lived(&mut r, evs);
        assert_ne!(
            r.addr(397),
            r.addr(493),
            "the registry keys non-squad by id"
        );
        let l = build_log(&f, &r);
        let first = r.addr(397).unwrap();
        assert_eq!(strikes_by(&l, 71), vec![(first, 397)]);
        assert_eq!(strikes_by(&l, 72), vec![(first, 493)]);
        assert_eq!(l.agents.iter().filter(|a| a.prof == 9).count(), 1);
        assert!(!l.agents.iter().any(|a| a.addr == r.addr(493).unwrap()));
        // The second life's spawn row is under the shared addr too.
        let spawn = sc_rows(&l, SPAWN);
        assert!(
            spawn
                .iter()
                .any(|e| e.src_instid == 493 && e.src_agent == first)
        );
    }

    /// Recording 20261124-201138 / arcdps 20261124-212736: enemy "Silver Recruit" was instance
    /// 3913 (Virtuoso), then 3944 (Reaper: the same account on another character). arcdps writes
    /// one agent (account GUID); without `AgentGuid` core gets no name for enemies, so they stay
    /// two (`enemy_lives_with_one_guid_are_one_agent` has the GUID).
    #[test]
    fn unnamed_enemy_lives_stay_separate() {
        let mut r = world();
        let f = lived(
            &mut r,
            vec![
                (1500, life(3913, 7, 66, 708, 0)),
                (1600, strike(3913, 1, 71)),
                (2000, Event::AgentRemove { id: 3913 }),
                (2500, life(3944, 8, 34, 708, 0)),
                (2600, strike(3944, 1, 72)),
            ],
        );
        let l = build_log(&f, &r);
        assert_ne!(strikes_by(&l, 71)[0].0, strikes_by(&l, 72)[0].0);
    }

    /// A character name names one player on one team: another team's player with the same name
    /// (a torn read, a stale id from another map) is not merged, nor are different names.
    #[test]
    fn lives_merge_only_on_the_same_team() {
        let mut r = world();
        let mut evs = vec![(1500, life(10, 9, 69, 1, 0))];
        evs.extend(names(10, NameField::Character, "Rio Rahl", 1500));
        evs.push((1600, strike(10, 3, 71)));
        evs.push((2500, life(11, 9, 69, 708, 0)));
        evs.extend(names(11, NameField::Character, "Rio Rahl", 2500));
        evs.push((2600, strike(11, 3, 72)));
        evs.push((2700, life(12, 9, 69, 1, 0)));
        evs.extend(names(12, NameField::Character, "Other Name", 2700));
        evs.push((2800, strike(12, 3, 73)));
        let l = build_log(&lived(&mut r, evs), &r);
        let (a, b, c) = (strikes_by(&l, 71), strikes_by(&l, 72), strikes_by(&l, 73));
        assert_ne!(a[0].0, b[0].0);
        assert_ne!(a[0].0, c[0].0);
    }

    /// An account names a player across characters (arcdps keys by the account GUID): a
    /// non-squad player whose account core knows is one agent after a character change, and
    /// the agent table describes the first life, as arcdps did for "Silver Recruit".
    #[test]
    fn account_merges_across_characters_and_the_first_life_describes_the_agent() {
        let mut r = world();
        let mut evs = vec![(1500, life(20, 7, 66, 1, 0))];
        evs.extend(names(20, NameField::Character, "First Char", 1500));
        evs.extend(names(20, NameField::Account, ":Ally.1234", 1500));
        evs.push((1600, strike(20, 3, 71)));
        evs.push((2000, Event::AgentRemove { id: 20 }));
        evs.push((2500, life(21, 8, 34, 1, 0)));
        evs.extend(names(21, NameField::Character, "Second Char", 2500));
        evs.extend(names(21, NameField::Account, ":Ally.1234", 2500));
        evs.push((2600, strike(21, 3, 72)));
        let l = build_log(&lived(&mut r, evs), &r);
        let addr = strikes_by(&l, 71)[0].0;
        assert_eq!(strikes_by(&l, 72)[0].0, addr);
        let a = l.agents.iter().find(|a| a.addr == addr).unwrap();
        assert_eq!((a.prof, a.is_elite), (7, 66));
    }

    /// A squad player's account moving to a new instance id mid-fight (left range, came back):
    /// one addr, and the first life's rows are still written (they were dropped with the old id).
    #[test]
    fn squad_player_rows_from_an_earlier_life_are_kept() {
        let mut r = world();
        let mut evs = vec![(1500, life(496, 8, 60, 1, 6))];
        evs.extend(names(496, NameField::Character, "Foxy Latinâ", 1500));
        evs.extend(names(496, NameField::Account, ":Athena.4562", 1500));
        evs.push((1600, strike(496, 3, 71)));
        evs.push((2000, Event::AgentRemove { id: 496 }));
        evs.push((2500, life(5641, 8, 60, 1, 6)));
        evs.extend(names(5641, NameField::Character, "Foxy Latinâ", 2500));
        evs.extend(names(5641, NameField::Account, ":Athena.4562", 2500));
        evs.push((2600, strike(5641, 3, 72)));
        let l = build_log(&lived(&mut r, evs), &r);
        let addr = r.addr(5641).unwrap();
        assert_eq!(strikes_by(&l, 71), vec![(addr, 496)]);
        assert_eq!(strikes_by(&l, 72), vec![(addr, 5641)]);
        assert_eq!(l.agents.iter().filter(|a| a.addr == addr).count(), 1);
    }

    /// An account GUID as the game holds it (variant byte 0x81 at [8]).
    const GUID_A: [u8; 16] = [
        0xa7, 0x02, 0x81, 0xbf, 0x12, 0x34, 0x56, 0x78, 0x81, 0xbf, 0xde, 0xf0, 0x11, 0x22, 0x33,
        0x44,
    ];
    const GUID_B: [u8; 16] = [
        0x10, 0x20, 0x30, 0x40, 0x50, 0x60, 0x70, 0x80, 0x9a, 0xb0, 0xc0, 0xd0, 0xe0, 0xf0, 0x01,
        0x02,
    ];

    fn guid(id: u32, g: [u8; 16]) -> Event {
        Event::AgentGuid { id, guid: g }
    }

    /// Silver Recruit (arcdps 20261124-212736): enemy 3913 (Virtuoso), then 3944 (Reaper) of
    /// the same account. With the GUID: one agent under the GUID addr, each row keeping its
    /// life's instance id, the table entry describing the first life.
    #[test]
    fn enemy_lives_with_one_guid_are_one_agent() {
        let mut r = world();
        let f = lived(
            &mut r,
            vec![
                (1500, life(3913, 7, 66, 708, 0)),
                (1500, guid(3913, GUID_A)),
                (1600, strike(3913, 1, 71)),
                (2000, Event::AgentRemove { id: 3913 }),
                (2500, life(3944, 8, 34, 708, 0)),
                (2500, guid(3944, GUID_A)),
                (2600, strike(3944, 1, 72)),
            ],
        );
        let l = build_log(&f, &r);
        let addr = 0xbf81_3412_bf81_02a7; // public-check:ok
        assert_eq!(guid_addr(&GUID_A), Some(addr));
        assert_eq!(strikes_by(&l, 71), vec![(addr, 3913)]);
        assert_eq!(strikes_by(&l, 72), vec![(addr, 3944)]);
        let players: Vec<_> = l.agents.iter().filter(|a| a.addr == addr).collect();
        assert_eq!(players.len(), 1);
        assert_eq!((players[0].prof, players[0].is_elite), (7, 66));
        for old in [r.addr(3913).unwrap(), r.addr(3944).unwrap()] {
            assert!(!l.agents.iter().any(|a| a.addr == old));
        }
        let spawn = sc_rows(&l, SPAWN);
        assert!(
            spawn
                .iter()
                .any(|e| e.src_instid == 3944 && e.src_agent == addr)
        );
    }

    #[test]
    fn different_guids_stay_apart_even_with_one_name() {
        let mut r = world();
        let mut evs = vec![(1500, life(10, 9, 69, 1, 0)), (1500, guid(10, GUID_A))];
        evs.extend(names(10, NameField::Character, "Rio Rahl", 1500));
        evs.push((1600, strike(10, 3, 71)));
        evs.push((2000, Event::AgentRemove { id: 10 }));
        evs.push((2500, life(11, 9, 69, 1, 0)));
        evs.push((2500, guid(11, GUID_B)));
        evs.extend(names(11, NameField::Character, "Rio Rahl", 2500));
        evs.push((2600, strike(11, 3, 72)));
        let l = build_log(&lived(&mut r, evs), &r);
        assert_eq!(strikes_by(&l, 71)[0].0, guid_addr(&GUID_A).unwrap());
        assert_eq!(strikes_by(&l, 72)[0].0, guid_addr(&GUID_B).unwrap());
    }

    /// The same GUID on two teams is two agents: the first keeps the GUID addr.
    #[test]
    fn one_guid_on_two_teams_is_not_merged() {
        let mut r = world();
        let f = lived(
            &mut r,
            vec![
                (1500, life(20, 7, 66, 708, 0)),
                (1500, guid(20, GUID_A)),
                (1600, strike(20, 1, 71)),
                (2000, Event::AgentRemove { id: 20 }),
                (2500, life(21, 7, 66, 9, 0)),
                (2500, guid(21, GUID_A)),
                (2600, strike(21, 1, 72)),
            ],
        );
        let l = build_log(&f, &r);
        assert_eq!(strikes_by(&l, 71)[0].0, guid_addr(&GUID_A).unwrap());
        assert_eq!(strikes_by(&l, 72)[0].0, r.addr(21).unwrap());
    }

    /// A life whose GUID was not read is matched by name to a life with one, and written under
    /// the GUID addr.
    #[test]
    fn a_life_without_guid_joins_its_named_life_with_one() {
        let mut r = world();
        let mut evs = vec![(1500, life(10, 9, 69, 1, 0))];
        evs.extend(names(10, NameField::Character, "Rio Rahl", 1500));
        evs.push((1600, strike(10, 3, 71)));
        evs.push((2000, Event::AgentRemove { id: 10 }));
        evs.push((2500, life(11, 9, 69, 1, 0)));
        evs.push((2500, guid(11, GUID_A)));
        evs.extend(names(11, NameField::Character, "Rio Rahl", 2500));
        evs.push((2600, strike(11, 3, 72)));
        let l = build_log(&lived(&mut r, evs), &r);
        let addr = guid_addr(&GUID_A).unwrap();
        assert_eq!(strikes_by(&l, 71), vec![(addr, 10)]);
        assert_eq!(strikes_by(&l, 72), vec![(addr, 11)]);
    }

    /// arcdps leaves the POV on its own addr; so does core, GUID or not.
    #[test]
    fn the_pov_keeps_its_addr_with_a_guid() {
        let mut r = world();
        r.apply(0, &guid(1, GUID_A));
        let f = fight(vec![(2000, strike(1, 3, 71))]);
        let l = build_log(&f, &r);
        assert_eq!(strikes_by(&l, 71)[0].0, r.addr(1).unwrap());
        assert!(guid_addr(&GUID_A).is_some_and(|a| !l.agents.iter().any(|x| x.addr == a)));
    }

    /// A GUID below 2^32 as an addr could meet a sequential one: not used.
    #[test]
    fn guid_addr_is_never_a_sequential_addr() {
        assert_eq!(guid_addr(&[0; 16]), None);
        let mut g = [0u8; 16];
        g[0] = 0xff;
        g[3] = 0xff;
        assert_eq!(guid_addr(&g), None);
        g[8] = 0x80;
        assert_eq!(guid_addr(&g), Some(0x0080_0000_ff00_00ff)); // public-check:ok
    }

    /// A life seen only in rows `first_seen` does not list (an effect on it) still gets its
    /// player's GUID addr.
    #[test]
    fn a_life_seen_only_in_effect_rows_gets_the_guid_addr() {
        let mut r = world();
        let f = lived(
            &mut r,
            vec![
                (1500, life(3913, 7, 66, 708, 0)),
                (1500, guid(3913, GUID_A)),
                (1600, strike(3913, 1, 71)),
                (2000, Event::AgentRemove { id: 3913 }),
                (2500, life(3944, 8, 34, 708, 0)),
                (2500, guid(3944, GUID_A)),
            ],
        );
        let mut f = f;
        f.events
            .retain(|(_, e)| !matches!(e, Event::AgentAdd { id: 3944, .. }));
        f.events.push((
            2600,
            Event::EffectAgent {
                host: 3944,
                src: 0,
                effect: 21_000,
                duration: 0,
                tracking: 9,
            },
        ));
        assert!(!first_seen(&f).contains(&3944));
        let l = build_log(&f, &r);
        let addr = guid_addr(&GUID_A).unwrap();
        let fx: Vec<_> = sc_rows(&l, EFFECT_AGENT)
            .into_iter()
            .map(|e| (e.dst_agent, e.dst_instid))
            .collect();
        assert_eq!(fx, vec![(addr, 3944)], "the host is dst");
    }
}
