//! Turns the event stream into WvW fights: decides when a log starts and ends, and whether to keep it.

use std::collections::BTreeSet;

use axidps_protocol::v2::{Event, LogAction};

use crate::buffs::Stacks;
use crate::condi::Known;
use crate::casts::Casts;
use crate::effects::Effects;
use crate::registry::{AgentInfo, Registry};
use crate::rules::{Trigger, WvwRules};
use crate::server::Server;
use crate::squad_markers::SquadMarkers;
use crate::wvw::{Objectives, Teams};

/// `Status.group_state[0]` is the Core group's state; 0 = Live (mirrors the DLL's
/// `GroupState::Live`; this crate deliberately does not depend on the DLL's crates).
const CORE_LIVE: u8 = 0;

/// One recorded fight.
#[derive(Debug, Clone)]
pub struct Fight {
    pub start_ms: u64,
    pub end_ms: u64,
    /// Every event that arrived while recording, in order, starting with the triggering one. A
    /// squad `Combat` exit that ends the fight is included; a Map/Status/LogControl that ends it
    /// is not.
    pub events: Vec<(u64, Event)>,
    /// Every agent known just before the fight started.
    pub start_snapshot: Vec<AgentInfo>,
    pub map: Option<Event>,
    pub pov: Option<u32>,
    pub status: Option<Event>,
    /// `(time_ms, unix_s)` from the latest `Status`.
    pub unix_at: Option<(u64, i64)>,
    pub dropped: u64,
    /// Ids that counted as squad at any point while recording.
    pub squad_seen: BTreeSet<u32>,
    /// Buff stacks known just before the fight started (BUFFINITIAL, and removes of them).
    pub buffs: Stacks,
    /// Casts open just before the fight started (their ends are written inside it).
    pub casts: Casts,
    /// Missiles known just before the fight started (owner, skill, recorded target).
    pub effects: Effects,
    /// Server tick, ping, address and shard ids known just before the fight started.
    pub server: Server,
    /// Squad ground markers known just before the fight started (sc 53 start rows).
    pub squad_markers: SquadMarkers,
    /// WvW objectives known just before the fight started (sc 75 start rows).
    pub wvw: Objectives,
    /// WvW team per colour known just before the fight started (sc 74).
    pub wvw_teams: Teams,
    /// Buff formulas and core stats known just before the fight started (condition model).
    pub condi: Known,
    /// Write the condition model's estimates for other players (else self-check only).
    pub condi_write: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscardReason {
    TooShort,
    SmallSquad,
}

// Exact shape fixed by the plan (Task 6 destructures `Ended { fight, registry }`); one per fight.
#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum FightOutcome {
    /// `registry` is a clone taken at the end, for identities.
    Ended { fight: Fight, registry: Registry },
    Discarded {
        reason: DiscardReason,
        duration_ms: u64,
    },
}

struct Active {
    fight: Fight,
    last_activity_ms: u64,
    /// Squad members currently in combat.
    in_combat: BTreeSet<u32>,
}

pub struct Processor {
    rules: WvwRules,
    registry: Registry,
    /// Buff stacks, kept in and out of fights.
    buffs: Stacks,
    /// Open casts and arcdps's per-agent cast flags, kept in and out of fights.
    casts: Casts,
    /// The missile table, kept in and out of fights.
    effects: Effects,
    /// Server tick and values, kept in and out of fights.
    server: Server,
    squad_markers: SquadMarkers,
    wvw: Objectives,
    wvw_teams: Teams,
    /// Buff formulas and core stats, kept in and out of fights.
    condi: Known,
    active: Option<Active>,
    /// Latest values, kept across idle time and used to seed each new fight.
    map: Option<Event>,
    pov: Option<u32>,
    status: Option<Event>,
    unix_at: Option<(u64, i64)>,
    /// Set when core turns off; cleared by a `Status` with core Live.
    blocked: bool,
    /// Drops reported while no fight was recording; charged to the next fight that starts.
    pending_dropped: u64,
    /// `(group_state, dll_version)` from the ring header, used when no `Status` was seen.
    header_status: Option<([u8; 16], u32)>,
}

fn nonzero(id: u32) -> Option<u32> {
    (id != 0).then_some(id)
}

fn named_ids(ev: &Event) -> [Option<u32>; 2] {
    match *ev {
        Event::AgentAdd { id, .. }
        | Event::AgentName { id, .. }
        | Event::AgentRemove { id }
        | Event::Combat { id, .. }
        | Event::Team { id, .. }
        | Event::Health { id, .. }
        | Event::State { id, .. }
        | Event::Motion { id, .. }
        | Event::BuffStack { dst: id, .. }
        | Event::Barrier { id, .. }
        | Event::Tag { id, .. }
        | Event::Breakbar { id, .. }
        | Event::CastEnd { agent: id, .. }
        | Event::WeaponSwap { id, .. }
        | Event::ActionSpeed { id, .. }
        | Event::Glider { id, .. }
        | Event::AgentFlags { id, .. }
        | Event::AgentInfo { id, .. }
        | Event::GadgetCapture { id, .. }
        | Event::GadgetModel { id, .. }
        | Event::GadgetShape { id, .. }
        | Event::Jump { id, .. }
        | Event::FlyTo { id, .. }
        | Event::Teleport { id, .. }
        | Event::GadgetAnimation { id, .. }
        | Event::Transform { id, .. }
        | Event::AgentMaster { id, .. }
        | Event::AgentGuild { id, .. }
        | Event::AgentGuid { id, .. }
        | Event::Pov { id } => [Some(id), None],
        Event::CastStart { agent, target, .. } => [Some(agent), (target != 0).then_some(target)],
        Event::AttackTarget { id, parent } => [Some(id), Some(parent)],
        Event::BuffApply { dst, src, .. } => [Some(dst), (src != 0).then_some(src)],
        Event::BuffRemove { dst, remover, .. } | Event::BuffRemoveAll { dst, remover, .. } => {
            [Some(dst), (remover != 0).then_some(remover)]
        }
        Event::Strike { src, dst, .. } => [Some(src), Some(dst)],
        Event::BuffDamage { src, dst, .. }
        | Event::Heal { src, dst, .. }
        | Event::RelayHeal { src, dst, .. } => [nonzero(src), Some(dst)],
        Event::EffectAgent { host, src, .. } => [Some(host), nonzero(src)],
        Event::EffectAgentRemove { host, .. } => [Some(host), None],
        Event::EffectGround { owner, .. }
        | Event::MissileCreate { owner, .. }
        | Event::MissileRemove { owner, .. }
        | Event::MissileEffect { owner, .. } => [nonzero(owner), None],
        Event::MissileLaunch { owner, target, .. } => [nonzero(owner), nonzero(target)],
        Event::Status { .. }
        | Event::Map { .. }
        | Event::LogControl { .. }
        | Event::EffectGroundRemove { .. }
        | Event::ServerTick { .. }
        | Event::CoreStats { .. }
        | Event::GearAttrs { .. }
        | Event::GearSkill { .. }
        | Event::BuffFormula { .. }
        | Event::SquadMarker { .. }
        | Event::WvwObjective { .. }
        | Event::Reward { .. }
        | Event::WvwTeam { .. }
        | Event::GroupKey { .. }
        | Event::WvwWorld { .. } => [None, None],
    }
}

impl Processor {
    pub fn new(rules: WvwRules) -> Processor {
        Processor {
            rules,
            registry: Registry::new(),
            buffs: Stacks::default(),
            condi: Known::default(),
            casts: Casts::default(),
            effects: Effects::default(),
            server: Server::default(),
            squad_markers: SquadMarkers::default(),
            wvw: Objectives::default(),
            wvw_teams: Teams::default(),
            active: None,
            map: None,
            pov: None,
            status: None,
            unix_at: None,
            blocked: false,
            pending_dropped: 0,
            header_status: None,
        }
    }

    /// True if `ev` is a `Map` for a different map or shard than the current one. The first
    /// `Map`, and a periodic re-push of the current one, are not changes.
    fn map_changed(&self, ev: &Event) -> bool {
        match (self.map, *ev) {
            (
                Some(Event::Map {
                    map_id: old_map,
                    shard: old_shard,
                    ..
                }),
                Event::Map { map_id, shard, .. },
            ) => (old_map, old_shard) != (map_id, shard),
            _ => false,
        }
    }

    /// A new map: instance ids are reused per map, so forget everything keyed by them.
    fn forget_instances(&mut self) {
        self.registry.reset_instances();
        self.buffs.clear();
        self.casts.clear();
        self.effects.clear();
        self.condi.forget_instances();
        self.server.map_loaded();
        self.pov = None;
    }

    /// Keep the buff stacks current, in and out of fights.
    fn track_buffs(&mut self, time_ms: u64, ev: &Event) {
        match *ev {
            Event::BuffApply { .. }
            | Event::BuffRemove { .. }
            | Event::BuffRemoveAll { .. }
            | Event::BuffStack { .. } => {
                self.buffs.apply(time_ms, ev);
            }
            Event::AgentRemove { id } => self.buffs.forget_agent(id),
            _ => {}
        }
        self.casts.apply(time_ms, ev);
        self.effects.apply(ev);
        self.condi.apply(ev);
        self.server.apply(time_ms, ev);
        self.squad_markers.apply(ev);
        let map = match self.map {
            Some(Event::Map { map_id, .. }) => Some(map_id),
            _ => None,
        };
        self.wvw.apply(ev, map);
        self.wvw_teams.apply(ev);
    }

    fn squad(&self, id: u32) -> bool {
        self.rules.trigger == Trigger::Any || self.registry.is_squad(id)
    }

    pub fn push(&mut self, time_ms: u64, ev: &Event) -> Option<FightOutcome> {
        let mut outcome = self.check_inactive(time_ms);

        match *ev {
            Event::Status {
                unix_s,
                group_state,
                ..
            } => {
                self.status = Some(*ev);
                self.unix_at = Some((time_ms, unix_s));
                if group_state[0] != CORE_LIVE {
                    self.blocked = true;
                    if let Some(a) = self.active.as_mut() {
                        // The log records why it ended: the tripped state, not the start one.
                        a.fight.status = Some(*ev);
                        a.fight.unix_at = Some((time_ms, unix_s));
                        outcome = self.end_at(time_ms);
                    }
                } else {
                    self.blocked = false;
                    self.record(time_ms, ev);
                    if let Some(a) = self.active.as_mut() {
                        a.fight.status = Some(*ev);
                        a.fight.unix_at = Some((time_ms, unix_s));
                    }
                }
            }
            Event::Map { .. } => {
                if self.map_changed(ev) {
                    if self.active.is_some() {
                        outcome = self.end_at(time_ms);
                    }
                    self.forget_instances();
                } else if let Some(a) = self.active.as_mut() {
                    // First sight of the map while recording: it is this fight's map.
                    a.fight.map.get_or_insert(*ev);
                }
                self.map = Some(*ev);
            }
            Event::LogControl { action, .. } => match action {
                LogAction::End => {
                    if self.active.is_some() {
                        outcome = self.end_at(time_ms);
                    }
                }
                LogAction::Start => {
                    if self.active.is_none() && !self.blocked {
                        self.start(time_ms, None);
                    }
                }
            },
            _ => {
                if self.active.is_none() {
                    let triggers =
                        matches!(*ev, Event::Combat { id, in_combat: true } if self.squad(id));
                    if triggers && !self.blocked {
                        // The snapshot is taken before the trigger is applied (one ENTER row).
                        self.start(time_ms, Some(ev));
                    } else {
                        self.registry.apply(time_ms, ev);
                        if let Event::Pov { id } = *ev {
                            self.pov = Some(id);
                        }
                    }
                } else if let Some(o) = self.record_active(time_ms, ev) {
                    outcome = Some(o);
                }
            }
        }
        // After the event: a fight's snapshot (taken in `start`) holds the stacks before it, and
        // the assembler replays the fight's own buff events over it.
        self.track_buffs(time_ms, ev);
        outcome
    }

    /// Apply a historic event (pushed before the app attached to the ring) for state only:
    /// agents, names, teams, combat flags, health, POV, map and status. It never starts,
    /// extends or ends a fight, so replaying a ring cannot rewrite logs already written.
    /// Call it only while no fight is recording (the catch-up right after opening a ring).
    pub fn prime(&mut self, time_ms: u64, ev: &Event) {
        self.track_buffs(time_ms, ev);
        match *ev {
            Event::Status {
                unix_s,
                group_state,
                ..
            } => {
                self.status = Some(*ev);
                self.unix_at = Some((time_ms, unix_s));
                self.blocked = group_state[0] != CORE_LIVE;
            }
            Event::Map { .. } => {
                if self.map_changed(ev) {
                    self.forget_instances();
                }
                self.map = Some(*ev);
            }
            Event::Pov { id } => {
                self.registry.apply(time_ms, ev);
                self.pov = Some(id);
            }
            Event::AgentAdd { .. }
            | Event::AgentName { .. }
            | Event::AgentRemove { .. }
            | Event::Team { .. }
            | Event::Combat { .. }
            | Event::Health { .. }
            | Event::State { .. } => self.registry.apply(time_ms, ev),
            Event::BuffApply { .. }
            | Event::BuffRemove { .. }
            | Event::BuffRemoveAll { .. }
            | Event::BuffStack { .. }
            | Event::CastStart { .. }
            | Event::CastEnd { .. }
            | Event::WeaponSwap { .. }
            | Event::ActionSpeed { .. }
            | Event::Glider { .. }
            | Event::AgentFlags { .. }
            | Event::AgentInfo { .. }
            | Event::GadgetCapture { .. }
            | Event::GadgetModel { .. }
            | Event::GadgetShape { .. }
            | Event::Jump { .. }
            | Event::FlyTo { .. }
            | Event::Teleport { .. }
            | Event::GadgetAnimation { .. }
            | Event::Transform { .. }
            | Event::AgentMaster { .. }
            | Event::AttackTarget { .. }
            | Event::AgentGuild { .. }
            | Event::AgentGuid { .. }
            | Event::Breakbar { .. }
            | Event::EffectGround { .. }
            | Event::EffectGroundRemove { .. }
            | Event::EffectAgent { .. }
            | Event::EffectAgentRemove { .. }
            | Event::MissileCreate { .. }
            | Event::MissileLaunch { .. }
            | Event::MissileRemove { .. }
            | Event::MissileEffect { .. } => self.registry.apply(time_ms, ev),
            Event::Strike { .. }
            | Event::ServerTick { .. }
            | Event::CoreStats { .. }
            | Event::GearAttrs { .. }
            | Event::GearSkill { .. }
            | Event::BuffFormula { .. }
            | Event::SquadMarker { .. }
            | Event::WvwObjective { .. }
            | Event::Reward { .. }
            | Event::WvwTeam { .. }
            | Event::WvwWorld { .. }
            | Event::BuffDamage { .. }
            | Event::Heal { .. }
            | Event::RelayHeal { .. }
            | Event::GroupKey { .. }
            | Event::LogControl { .. }
            | Event::Motion { .. }
            | Event::Barrier { .. }
            | Event::Tag { .. } => {}
        }
    }

    /// A fight is recording.
    pub fn recording(&self) -> bool {
        self.active.is_some()
    }

    /// Start time (game ms) of the fight recording, if any.
    pub fn fight_start_ms(&self) -> Option<u64> {
        self.active.as_ref().map(|a| a.fight.start_ms)
    }

    /// The current map id, if a `Map` was seen.
    pub fn map_id(&self) -> Option<u32> {
        match self.map {
            Some(Event::Map { map_id, .. }) => Some(map_id),
            _ => None,
        }
    }

    /// The current map type (MumbleLink's), if a `Map` was seen.
    pub fn map_type(&self) -> Option<u32> {
        match self.map {
            Some(Event::Map { map_type, .. }) => Some(map_type),
            _ => None,
        }
    }

    /// The agent registry as it is now.
    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    pub fn tick(&mut self, now_ms: u64) -> Option<FightOutcome> {
        self.check_inactive(now_ms)
    }

    /// Count events lost on the ring. They precede whatever the next read delivers, so call
    /// this before pushing that read's events. With no fight recording, they are charged to
    /// the next fight that starts.
    pub fn note_dropped(&mut self, n: u64) {
        match self.active.as_mut() {
            Some(a) => a.fight.dropped = a.fight.dropped.saturating_add(n),
            None => self.pending_dropped = self.pending_dropped.saturating_add(n),
        }
    }

    /// The ring header's group state and DLL version. A fight that starts before any `Status`
    /// event was seen (the app attached after the ring lapped past it) uses these instead.
    pub fn set_header_status(&mut self, group_state: [u8; 16], dll_version: u32) {
        self.header_status = Some((group_state, dll_version));
    }

    /// End a recording fight at its last activity (shutdown).
    pub fn flush(&mut self) -> Option<FightOutcome> {
        let at = self.active.as_ref()?.last_activity_ms;
        self.end_at(at)
    }

    fn check_inactive(&mut self, now_ms: u64) -> Option<FightOutcome> {
        let last = self.active.as_ref()?.last_activity_ms;
        if now_ms.saturating_sub(last) > self.rules.inactive_ms {
            self.end_at(last)
        } else {
            None
        }
    }

    fn record(&mut self, time_ms: u64, ev: &Event) {
        if let Some(a) = self.active.as_mut() {
            a.fight.events.push((time_ms, *ev));
        }
    }

    /// Start a fight. `trigger` is the Combat enter that starts it, applied after the snapshot.
    fn start(&mut self, time_ms: u64, trigger: Option<&Event>) {
        let start_snapshot: Vec<AgentInfo> = self.registry.live_agents().cloned().collect();
        let mut squad_seen = BTreeSet::new();
        let mut in_combat = BTreeSet::new();
        let status = self.status.or_else(|| {
            self.header_status
                .map(|(group_state, dll_version)| Event::Status {
                    unix_s: 0,
                    dll_version,
                    group_state,
                })
        });
        for a in &start_snapshot {
            if self.squad(a.id) {
                squad_seen.insert(a.id);
                if a.in_combat {
                    in_combat.insert(a.id);
                }
            }
        }
        self.active = Some(Active {
            fight: Fight {
                start_ms: time_ms,
                end_ms: time_ms,
                events: Vec::new(),
                start_snapshot,
                map: self.map,
                pov: self.pov,
                status,
                unix_at: self.unix_at,
                dropped: std::mem::take(&mut self.pending_dropped),
                squad_seen,
                buffs: self.buffs.clone(),
                casts: self.casts.clone(),
                effects: self.effects.clone(),
                server: self.server,
                squad_markers: self.squad_markers,
                wvw: self.wvw.clone(),
                wvw_teams: self.wvw_teams,
                condi: self.condi.clone(),
                condi_write: self.rules.condi_write,
            },
            last_activity_ms: time_ms,
            in_combat,
        });
        if let Some(ev) = trigger {
            // Cannot end the fight: it only adds to the in-combat set.
            let _ = self.record_active(time_ms, ev);
        }
    }

    /// Apply and record an event while a fight is recording. Returns an outcome if it ends it.
    fn record_active(&mut self, time_ms: u64, ev: &Event) -> Option<FightOutcome> {
        self.registry.apply(time_ms, ev);
        if let Event::Pov { id } = *ev {
            self.pov = Some(id);
        }
        let seen: Vec<u32> = named_ids(ev)
            .into_iter()
            .flatten()
            .filter(|&id| self.squad(id))
            .collect();
        // A member identified while already in combat (the poll names squad members up to a
        // couple of seconds after they appear) counts as in combat from now on.
        let fighting: Vec<u32> = seen
            .iter()
            .copied()
            .filter(|&id| self.registry.agent(id).is_some_and(|x| x.in_combat))
            .collect();
        let (mut activity, mut empties) = (false, false);
        let a = self.active.as_mut()?;
        a.in_combat.extend(fighting);
        a.fight.events.push((time_ms, *ev));
        if let Event::Pov { id } = *ev {
            a.fight.pov = Some(id);
        }
        a.fight.squad_seen.extend(seen.iter().copied());
        match *ev {
            Event::Strike { src, dst, .. } | Event::BuffDamage { src, dst, .. } => {
                activity = seen.contains(&src) || seen.contains(&dst)
            }
            Event::Combat { id, in_combat } if seen.contains(&id) => {
                activity = true;
                if in_combat {
                    a.in_combat.insert(id);
                } else {
                    empties = a.in_combat.remove(&id) && a.in_combat.is_empty();
                }
            }
            _ => {}
        }
        if activity {
            a.last_activity_ms = time_ms;
        }
        if empties { self.end_at(time_ms) } else { None }
    }

    fn end_at(&mut self, end_ms: u64) -> Option<FightOutcome> {
        let mut fight = self.active.take()?.fight;
        fight.end_ms = end_ms.max(fight.start_ms);
        let duration_ms = fight.end_ms - fight.start_ms;
        if duration_ms < self.rules.min_duration_ms {
            return Some(FightOutcome::Discarded {
                reason: DiscardReason::TooShort,
                duration_ms,
            });
        }
        if fight.squad_seen.len() < self.rules.min_squad {
            return Some(FightOutcome::Discarded {
                reason: DiscardReason::SmallSquad,
                duration_ms,
            });
        }
        Some(FightOutcome::Ended {
            fight,
            registry: self.registry.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assemble::build_log;
    use crate::extension;
    use crate::registry::tests::{add, name_events};
    use axidps_protocol::v2::{AgentKind, NameField};

    fn p() -> Processor {
        Processor::new(WvwRules::default())
    }

    /// Add `n` squad players (ids 1..=n) through AgentAdd + AgentName.
    fn squad(p: &mut Processor, n: u32) {
        for id in 1..=n {
            p.push(0, &add(id, 1));
            for e in name_events(id, NameField::Character, &format!("Char {id}")) {
                p.push(0, &e);
            }
            for e in name_events(id, NameField::Account, &format!(":Alpha.{id:04}")) {
                p.push(0, &e);
            }
        }
    }

    fn enemy(p: &mut Processor, id: u32) {
        p.push(
            0,
            &Event::AgentAdd {
                id,
                kind: AgentKind::Player,
                prof: 1,
                elite: 0,
                species: 0,
                team: 2,
                subgroup: 0,
                wvw_rank: 0,
                wvw_rank_def: 0,
            },
        );
    }

    fn combat(id: u32, in_combat: bool) -> Event {
        Event::Combat { id, in_combat }
    }

    fn strike(src: u32, dst: u32) -> Event {
        Event::Strike {
            src,
            dst,
            skill: 1,
            value: 10,
            overstack: 0,
            result: 0,
        }
    }

    fn ended(o: Option<FightOutcome>) -> (Fight, Registry) {
        match o {
            Some(FightOutcome::Ended { fight, registry }) => (fight, registry),
            other => panic!("expected Ended, got {other:?}"),
        }
    }

    #[test]
    fn starts_on_squad_enter_not_on_enemy_enter() {
        let mut p = p();
        squad(&mut p, 6);
        enemy(&mut p, 100);
        p.push(500, &combat(100, true));
        p.push(600, &strike(100, 1));
        assert!(p.active.is_none());
        p.push(1000, &combat(1, true));
        let a = p.active.as_ref().unwrap();
        assert_eq!(a.fight.start_ms, 1000);
        // R1: snapshot before the trigger, trigger is the first event, exactly once.
        assert_eq!(a.fight.events, vec![(1000, combat(1, true))]);
        assert_eq!(a.fight.start_snapshot.len(), 7);
        assert!(
            !a.fight
                .start_snapshot
                .iter()
                .find(|x| x.id == 1)
                .unwrap()
                .in_combat
        );
        assert_eq!(a.fight.squad_seen.len(), 6);
    }

    #[test]
    fn ends_when_last_squad_member_exits() {
        let mut p = p();
        squad(&mut p, 6);
        assert!(p.push(0, &combat(1, true)).is_none());
        p.push(1000, &combat(2, true));
        assert!(p.push(12_000, &combat(1, false)).is_none());
        let (f, reg) = ended(p.push(15_000, &combat(2, false)));
        assert_eq!((f.start_ms, f.end_ms), (0, 15_000));
        assert_eq!(f.events.last().unwrap().1, combat(2, false));
        assert!(reg.agent(1).is_some());
        assert!(p.active.is_none());
    }

    /// Replay 2026-11-24 (log 21:34:53): 19 members entered combat before the poll had marked
    /// them squad; one member's exit then ended the fight while they still fought. A member who
    /// becomes squad while in combat counts as in combat.
    #[test]
    fn member_identified_while_in_combat_keeps_the_fight() {
        let mut p = p();
        squad(&mut p, 6);
        p.push(0, &combat(1, true));
        p.push(100, &combat(7, true)); // not squad yet
        p.push(200, &add(7, 2));
        for e in name_events(7, NameField::Account, ":Late.0007") {
            p.push(200, &e);
        }
        assert!(p.push(300, &combat(1, false)).is_none());
        assert!(p.active.is_some());
        assert!(p.push(400, &combat(7, false)).is_some(), "now the last one");
    }

    /// Stacks applied before a fight are in its snapshot; a despawn drops them.
    #[test]
    fn buff_stacks_seed_the_fight() {
        let mut p = p();
        squad(&mut p, 6);
        let apply = |dst, inst| Event::BuffApply {
            dst,
            src: 0,
            buff: 740,
            inst,
            duration: 5000,
            level: 0,
            active: true,
        };
        p.push(10, &apply(1, 1));
        p.push(10, &apply(2, 2));
        p.push(20, &Event::AgentRemove { id: 2 });
        p.push(100, &combat(1, true));
        assert_eq!(p.active.as_ref().unwrap().fight.buffs.len(), 1);
    }

    /// A cast open before a fight is in its snapshot; a new map forgets casts.
    #[test]
    fn casts_seed_the_fight() {
        let mut p = p();
        squad(&mut p, 6);
        let start = Event::CastStart {
            agent: 2,
            target: 0,
            skill: 5491,
            kind: 1,
            x: 0,
        };
        p.push(10, &start);
        p.push(100, &combat(1, true));
        let f = &p.active.as_ref().unwrap().fight;
        assert_eq!(f.casts.open(2).map(|o| o.skill), Some(5491));
        assert!(named_ids(&start).contains(&Some(2)));
    }

    /// A missile created before a fight is in its snapshot; effect events name their agents.
    #[test]
    fn effects_seed_the_fight() {
        let mut p = p();
        squad(&mut p, 6);
        let create = Event::MissileCreate {
            id: 4,
            owner: 2,
            skill: 5491,
            skin: 0,
            word_1e: 0,
            pos: [0; 3],
            byte_22: 0,
            byte_23: 0,
        };
        p.push(10, &create);
        p.push(100, &combat(1, true));
        let f = &p.active.as_ref().unwrap().fight;
        let mut fx = f.effects.clone();
        let rows = fx.apply(&Event::MissileRemove {
            id: 4,
            owner: 0,
            skill: 0,
            other: 0,
            pos: [0; 3],
            bits: 0,
        });
        assert_eq!((rows[0].src, rows[0].skill), (2, 5491));
        assert!(named_ids(&create).contains(&Some(2)));
        let agent = Event::EffectAgent {
            host: 6,
            src: 7,
            effect: 1,
            duration: 0,
            tracking: 0,
        };
        assert_eq!(named_ids(&agent), [Some(6), Some(7)]);
    }

    #[test]
    fn non_squad_exit_does_not_end_and_reentry_keeps_going() {
        let mut p = p();
        squad(&mut p, 6);
        enemy(&mut p, 100);
        p.push(0, &combat(1, true));
        p.push(100, &combat(100, true));
        assert!(p.push(200, &combat(100, false)).is_none());
        p.push(300, &combat(2, true));
        assert!(p.push(400, &combat(1, false)).is_none());
        p.push(500, &combat(1, true)); // re-entry
        assert!(p.push(600, &combat(2, false)).is_none());
        assert!(p.active.is_some());
    }

    #[test]
    fn inactivity_ends_at_last_activity() {
        let mut p = p();
        squad(&mut p, 6);
        p.push(0, &combat(1, true));
        p.push(11_000, &strike(1, 100));
        assert!(p.tick(31_000).is_none());
        let (f, _) = ended(p.tick(31_001));
        assert_eq!(f.end_ms, 11_000);
        assert!(p.tick(99_000).is_none());
    }

    #[test]
    fn enemy_only_strikes_are_not_activity_and_push_checks_inactivity() {
        let mut p = p();
        squad(&mut p, 6);
        enemy(&mut p, 100);
        enemy(&mut p, 101);
        p.push(0, &combat(1, true));
        p.push(11_000, &strike(1, 100));
        p.push(30_000, &strike(100, 101)); // enemy-only: not activity
        let (f, _) = ended(p.push(31_001, &strike(100, 101)));
        assert_eq!(f.end_ms, 11_000);
        // The event that revealed the inactivity is not part of the ended fight.
        assert_eq!(f.events.len(), 3);
    }

    #[test]
    fn map_change_ends_fight() {
        let mut p = p();
        squad(&mut p, 6);
        p.push(
            0,
            &Event::Map {
                map_id: 38,
                map_type: 1,
                shard: 0,
                gw2_build: 1,
            },
        );
        p.push(1000, &combat(1, true));
        p.push(12_000, &strike(1, 100));
        let (f, _) = ended(p.push(
            13_000,
            &Event::Map {
                map_id: 95,
                map_type: 1,
                shard: 0,
                gw2_build: 1,
            },
        ));
        assert_eq!(f.end_ms, 13_000);
        assert!(matches!(f.map, Some(Event::Map { map_id: 38, .. })));
        assert!(!f.events.iter().any(|(_, e)| matches!(e, Event::Map { .. })));
        // The new map seeds the next fight (the squad is re-added on the new map).
        squad(&mut p, 6);
        p.push(14_000, &combat(1, true));
        assert!(matches!(
            p.active.as_ref().unwrap().fight.map,
            Some(Event::Map { map_id: 95, .. })
        ));
    }

    /// The server tick is kept between fights and seeds the next one; a map change resets it
    /// (arcdps resets its tick at a map load); in a fight the ticks are recorded.
    #[test]
    fn server_tick_seeds_fight_and_map_change_resets_it() {
        let tick = |tick| Event::ServerTick {
            tick,
            ping: 95,
            server_ip: 0xcfdf_3164, // public-check:ok
            shard_ids: 0x8000_0002, // public-check:ok
        };
        let map = |map_id| Event::Map {
            map_id,
            map_type: 1,
            shard: 0,
            gw2_build: 1,
        };
        let mut p = p();
        squad(&mut p, 6);
        p.push(0, &map(38));
        p.push(500, &tick(100));
        p.push(1000, &combat(1, true));
        p.push(1200, &tick(105));
        {
            let f = &p.active.as_ref().unwrap().fight;
            assert_eq!(f.server.clock.extrapolate(500), (100, 0));
            assert_eq!((f.server.ping, f.server.shard_ids), (95, 0x8000_0002)); // public-check:ok
            assert!(f.events.iter().any(|(t, e)| *t == 1200 && *e == tick(105)));
        }
        assert_eq!(p.server.clock.extrapolate(1200), (105, 0));
        let _ = p.push(13_000, &map(95));
        assert!(!p.server.clock.seeded());
        assert_eq!(p.server.shard_ids, 0);
        // prime keeps it too.
        p.prime(14_000, &tick(200));
        assert_eq!(p.server.clock.extrapolate(14_000), (200, 0));
    }

    #[test]
    fn log_control_end_and_start() {
        let mut p = p();
        // Start while idle starts a fight even though nothing triggers.
        p.push(
            100,
            &Event::LogControl {
                action: LogAction::Start,
                reason: 0,
            },
        );
        assert_eq!(p.active.as_ref().unwrap().fight.start_ms, 100);
        let o = p.push(
            10_100,
            &Event::LogControl {
                action: LogAction::End,
                reason: 0,
            },
        );
        // Not enough squad members known: discarded as a small squad.
        assert!(matches!(
            o,
            Some(FightOutcome::Discarded {
                reason: DiscardReason::SmallSquad,
                duration_ms: 10_000
            })
        ));
        let mut p = Processor::new(WvwRules {
            min_squad: 0,
            ..WvwRules::default()
        });
        p.push(
            100,
            &Event::LogControl {
                action: LogAction::Start,
                reason: 0,
            },
        );
        let (f, _) = ended(p.push(
            10_100,
            &Event::LogControl {
                action: LogAction::End,
                reason: 0,
            },
        ));
        assert_eq!((f.start_ms, f.end_ms), (100, 10_100));
        assert!(f.events.is_empty());
    }

    fn short_long(len: u64) -> Option<FightOutcome> {
        let mut p = p();
        squad(&mut p, 6);
        p.push(0, &combat(1, true));
        p.push(len, &combat(1, false))
    }

    #[test]
    fn too_short_discarded() {
        assert!(matches!(
            short_long(9_999),
            Some(FightOutcome::Discarded {
                reason: DiscardReason::TooShort,
                duration_ms: 9_999
            })
        ));
    }

    #[test]
    fn ten_seconds_kept() {
        let (f, _) = ended(short_long(10_000));
        assert_eq!(f.end_ms - f.start_ms, 10_000);
    }

    #[test]
    fn small_squad_discarded() {
        let mut p = p();
        squad(&mut p, 4);
        p.push(0, &combat(1, true));
        let o = p.push(15_000, &combat(1, false));
        assert!(matches!(
            o,
            Some(FightOutcome::Discarded {
                reason: DiscardReason::SmallSquad,
                ..
            })
        ));
    }

    #[test]
    fn squad_seen_includes_members_who_arrive_mid_fight() {
        let mut p = p();
        squad(&mut p, 4);
        p.push(0, &combat(1, true));
        p.push(1000, &add(5, 1));
        for e in name_events(5, NameField::Account, ":Alpha.0005") {
            p.push(1000, &e);
        }
        let (f, _) = ended(p.push(15_000, &combat(1, false)));
        assert_eq!(f.squad_seen.len(), 5);
    }

    #[test]
    fn trigger_any_starts_on_any_agent() {
        let mut p = Processor::new(WvwRules {
            trigger: Trigger::Any,
            min_squad: 0,
            ..WvwRules::default()
        });
        enemy(&mut p, 100);
        p.push(0, &combat(100, true));
        assert!(p.active.is_some());
        let (f, _) = ended(p.push(10_000, &combat(100, false)));
        assert_eq!(f.end_ms, 10_000);
    }

    fn status(core: u8, unix_s: i64) -> Event {
        let mut group_state = [0u8; 16];
        group_state[0] = core;
        Event::Status {
            unix_s,
            dll_version: 1,
            group_state,
        }
    }

    #[test]
    fn core_trip_ends_fight_and_blocks_new() {
        let mut p = p();
        squad(&mut p, 6);
        p.push(0, &status(0, 1_700_000_000));
        p.push(0, &combat(1, true));
        let (f, reg) = ended(p.push(12_000, &status(2, 1_700_000_012)));
        assert_eq!(f.end_ms, 12_000);
        // The log records the trip, not the Live state the fight started with.
        assert!(matches!(f.status, Some(Event::Status { group_state, .. }) if group_state[0] == 2));
        assert_eq!(f.unix_at, Some((12_000, 1_700_000_012)));
        assert_eq!(group_row(&build_log(&f, &reg), 0), 2);
        assert!(
            !f.events
                .iter()
                .any(|(_, e)| matches!(e, Event::Status { .. }))
        );
        // Blocked: neither a squad enter nor LogControl::Start begins a fight.
        p.push(13_000, &combat(1, false));
        p.push(14_000, &combat(1, true));
        p.push(
            14_000,
            &Event::LogControl {
                action: LogAction::Start,
                reason: 0,
            },
        );
        assert!(p.active.is_none());
        // Live again unblocks.
        p.push(15_000, &status(0, 1_700_000_015));
        p.push(16_000, &combat(1, true));
        assert!(p.active.is_some());
    }

    #[test]
    fn idle_context_seeds_fights_and_unix_pair_carries() {
        let mut p = p();
        squad(&mut p, 6);
        p.push(
            5,
            &Event::Map {
                map_id: 38,
                map_type: 1,
                shard: 0,
                gw2_build: 9,
            },
        );
        p.push(6, &Event::Pov { id: 3 });
        p.push(7, &status(0, 1_700_000_000));
        p.push(1000, &combat(1, true));
        let a = p.active.as_ref().unwrap();
        assert_eq!(a.fight.pov, Some(3));
        assert_eq!(a.fight.unix_at, Some((7, 1_700_000_000)));
        assert!(matches!(a.fight.status, Some(Event::Status { .. })));
        assert!(matches!(a.fight.map, Some(Event::Map { map_id: 38, .. })));
    }

    /// The value of extension group row `group` in a built log.
    fn group_row(l: &axidps_evtc::Log, group: u32) -> i32 {
        l.events
            .iter()
            .find(|e| e.is_statechange == 40 && e.pad == extension::SIG && e.skillid == group)
            .unwrap()
            .value
    }

    #[test]
    fn dropped_adds_up_and_flush_ends_at_last_activity() {
        let mut p = p();
        squad(&mut p, 6);
        p.push(0, &combat(1, true));
        p.note_dropped(3);
        p.note_dropped(2);
        p.push(12_000, &strike(1, 100));
        p.push(13_000, &strike(100, 101));
        let (f, _) = ended(p.flush());
        assert_eq!((f.dropped, f.end_ms), (5, 12_000));
        assert!(p.flush().is_none());
    }

    #[test]
    fn drops_while_idle_go_to_the_next_fight_only() {
        let mut p = p();
        squad(&mut p, 6);
        p.note_dropped(5);
        p.push(0, &combat(1, true));
        let (f, _) = ended(p.push(12_000, &combat(1, false)));
        assert_eq!(f.dropped, 5);
        p.push(20_000, &combat(1, true));
        let (f, _) = ended(p.push(32_000, &combat(1, false)));
        assert_eq!(f.dropped, 0);
    }

    #[test]
    fn header_status_seeds_a_fight_with_no_status_event() {
        let mut p = p();
        squad(&mut p, 6);
        let mut gs = [3u8; 16];
        gs[0] = 0;
        p.set_header_status(gs, 0x0001_0203); // public-check:ok
        p.push(0, &combat(1, true));
        let (f, reg) = ended(p.push(12_000, &combat(1, false)));
        let l = build_log(&f, &reg);
        assert_eq!(group_row(&l, 0), 0);
        let v = l
            .events
            .iter()
            .find(|e| e.skillid == extension::ROW_VERSIONS)
            .unwrap();
        assert_eq!(v.src_agent, 0x0001_0203); // public-check:ok
        // A real Status wins over the header.
        p.push(13_000, &status(0, 1_700_000_013));
        p.push(14_000, &combat(1, true));
        let (f, _) = ended(p.push(26_000, &combat(1, false)));
        assert!(matches!(
            f.status,
            Some(Event::Status { dll_version: 1, .. })
        ));
    }

    fn map(map_id: u32) -> Event {
        Event::Map {
            map_id,
            map_type: 1,
            shard: 0,
            gw2_build: 1,
        }
    }

    #[test]
    fn map_change_forgets_instances() {
        let mut p = Processor::new(WvwRules {
            trigger: Trigger::Any,
            min_squad: 0,
            ..WvwRules::default()
        });
        p.push(0, &map(38));
        enemy(&mut p, 7);
        for e in name_events(7, NameField::Character, "Old Seven") {
            p.push(0, &e);
        }
        p.push(100, &combat(7, true));
        assert!(p.active.is_some());
        p.push(200, &map(95));
        assert!(p.active.is_none());
        // New map, new fight: nothing about instance 7 carries over.
        p.push(1000, &combat(100, true));
        let (f, reg) = ended(p.push(12_000, &combat(100, false)));
        assert_eq!(f.squad_seen, BTreeSet::from([100]));
        assert!(f.start_snapshot.is_empty());
        assert!(reg.agent(7).is_none());
        let l = build_log(&f, &reg);
        assert_eq!(l.agents.len(), 1);
        assert!(!l.agents.iter().any(|a| a.name.starts_with(b"Old Seven")));
    }

    #[test]
    fn agents_removed_before_the_fight_are_not_in_it() {
        // Live 2026-11-24: every agent ever seen got a TEAM_CHANGE row at fight start.
        let mut p = Processor::new(WvwRules {
            trigger: Trigger::Any,
            min_squad: 0,
            ..WvwRules::default()
        });
        enemy(&mut p, 7);
        for e in name_events(7, NameField::Character, "Gone Seven") {
            p.push(0, &e);
        }
        p.push(50, &Event::AgentRemove { id: 7 });
        p.push(1000, &combat(100, true));
        let (f, reg) = ended(p.push(12_000, &combat(100, false)));
        assert!(f.start_snapshot.iter().all(|a| a.id != 7));
        let l = build_log(&f, &reg);
        assert!(!l.agents.iter().any(|a| a.name.starts_with(b"Gone Seven")));
    }

    #[test]
    fn same_map_repush_keeps_fight_and_agents() {
        let mut p = p();
        squad(&mut p, 6);
        p.push(0, &map(38));
        p.push(1000, &combat(1, true));
        assert!(p.push(5000, &map(38)).is_none());
        assert!(p.active.is_some());
        assert!(p.registry.agent(1).is_some());
    }

    #[test]
    fn first_map_while_recording_fills_the_fight_map() {
        let mut p = p();
        squad(&mut p, 6);
        p.push(1000, &combat(1, true));
        assert!(p.push(2000, &map(38)).is_none());
        let (f, _) = ended(p.push(12_000, &combat(1, false)));
        assert!(matches!(f.map, Some(Event::Map { map_id: 38, .. })));
    }

    #[test]
    fn prime_updates_state_but_never_starts_or_ends_fights() {
        let mut p = p();
        let mut evs = vec![status(0, 1_700_000_000), map(38), Event::Pov { id: 1 }];
        for id in 1..=6 {
            evs.push(add(id, 1));
            evs.extend(name_events(
                id,
                NameField::Account,
                &format!(":Alpha.{id:04}"),
            ));
        }
        evs.push(combat(1, true));
        evs.push(strike(1, 100));
        evs.push(Event::LogControl {
            action: LogAction::Start,
            reason: 0,
        });
        evs.push(combat(1, false));
        for e in &evs {
            p.prime(10, e);
        }
        assert!(p.active.is_none());
        assert!(p.registry.is_squad(6));
        assert!(p.registry.agent(100).is_none(), "strikes are not primed");
        assert_eq!(p.pov, Some(1));
        // A live squad enter after priming starts a fight seeded from the primed state.
        p.push(1000, &combat(2, true));
        let a = p.active.as_ref().unwrap();
        assert_eq!(a.fight.squad_seen.len(), 6);
        assert_eq!(a.fight.unix_at, Some((10, 1_700_000_000)));
        assert!(matches!(a.fight.map, Some(Event::Map { map_id: 38, .. })));
        // A primed trip blocks; a primed map change forgets instances.
        let mut p2 = Processor::new(WvwRules::default());
        p2.prime(0, &status(2, 1));
        p2.prime(0, &map(38));
        p2.prime(0, &add(1, 1));
        p2.prime(0, &map(95));
        assert!(p2.registry.agent(1).is_none());
        p2.push(
            1,
            &Event::LogControl {
                action: LogAction::Start,
                reason: 0,
            },
        );
        assert!(p2.active.is_none());
    }

    #[test]
    fn same_timestamp_events_keep_order() {
        let mut p = p();
        squad(&mut p, 6);
        p.push(0, &combat(1, true));
        p.push(0, &combat(2, true));
        p.push(10_000, &combat(1, false));
        let (f, _) = ended(p.push(10_000, &combat(2, false)));
        assert_eq!(f.events.len(), 4);
        assert_eq!(f.end_ms, 10_000);
    }
}
