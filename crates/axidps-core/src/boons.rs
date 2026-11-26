//! Boon uptime and generation, from the buff rows [`crate::buffs::Stacks`] writes, simulated
//! with axilog's boon simulator (the same one that reads our logs).
//!
//! Rows go to one small log per (holder, boon). Each tick re-simulates the pairs that got new
//! rows, to the end of time; the simulation never rewrites the past, so its held segments
//! clipped to `now` are final up to `now`. Sources arrive already resolved: a minion's rows carry
//! its master at the row's time, and each account has one id.
//!
//! Group and squad credit follow axilog's rollup: one subgroup per player for the whole fight
//! (whatever `groups` says at the tick), and every squad player except the source as a target.

use std::collections::{BTreeMap, BTreeSet};

use axilog_core::analysis::buffs::events::extract_buff_events_with_registry;
use axilog_core::analysis::buffs::generation::{HeldSegment, run_segments, run_sim};
use axilog_core::analysis::buffs::simulator::capacity_for;
use axilog_core::analysis::damage::InstidRegistry;
use axilog_core::evtc::{RawEvent, RawHeader, RawLog};

use crate::buffs::{
    BUFF_APPLY, BUFF_DURATION, BUFF_INITIAL, BUFF_REMOVE_ALL, BUFF_REMOVE_ONE, Row, STACK_ACTIVE,
    STACK_RESET,
};

/// The squad boons, in the order the catalog lists them.
pub const SQUAD_BOONS: [u32; 13] = [
    740, 725, 1187, 30328, 717, 718, 726, 743, 1122, 719, 26980, 873, 5974,
];

/// Intensity boons (might, stability): measured in stacks, credited per stack.
pub const INTENSITY: [u32; 2] = [740, 1122];

const REGENERATION: u32 = 718;

/// Simulation end for a pair: far enough that nothing it holds ends there.
const FAR: u64 = u64::MAX / 4;

/// Totals per player and boon: `[up, self, group, squad]`, in ms (stack-ms for intensity).
type Totals = BTreeMap<u32, BTreeMap<u32, [u64; 4]>>;

/// One holder's rows for one boon, and its last simulation.
struct Pair {
    /// Row 0 is the stack-id marker slot (sc 0 until any stack-active row is seen, then 27).
    log: RawLog,
    segments: Vec<HeldSegment>,
    /// Regeneration only: the queue simulation axilog's uptime uses (its credit uses the healing
    /// logic in `segments`).
    up_segments: Option<Vec<HeldSegment>>,
    /// Some segment ends after the last clip: the totals still move without new rows.
    open: bool,
    /// Simulated since the last clip.
    fresh: bool,
    /// Clipped to the last tick: ms held, and ms per source.
    up: u64,
    by_source: BTreeMap<u32, u64>,
}

pub struct BoonTracker {
    pairs: BTreeMap<(u32, u32), Pair>,
    dirty: BTreeSet<(u32, u32)>,
    /// Any stack-active or stack-reset row seen (axilog reads it log-wide).
    stack_ids: bool,
    totals: Totals,
    /// Masters are resolved before rows arrive, so one empty registry serves every pair.
    registry: InstidRegistry,
    header: RawHeader,
    capacities: BTreeMap<u32, u32>,
}

impl Default for BoonTracker {
    fn default() -> Self {
        Self::new()
    }
}

fn empty_log(header: RawHeader) -> RawLog {
    RawLog {
        header,
        agents: Vec::new(),
        skills: Vec::new(),
        events: Vec::new(),
        guid_map: Vec::new(),
    }
}

/// The header our log writer writes, so axilog takes the same paths as on our logs.
fn header() -> RawHeader {
    RawHeader {
        build: axidps_evtc::FORMAT_BUILD.into(),
        revision: 1,
        boss_id: axidps_evtc::BOSS_ID_WVW,
    }
}

/// A boon's stack capacity: the content table's (what our logs carry), else axilog's own.
fn capacity(boon: u32) -> u32 {
    crate::content::rows(boon)
        .iter()
        .find(|e| e.is_statechange == 30)
        .map(|e| u32::from(e.src_master_instid).min(99))
        .filter(|&c| c > 0)
        .unwrap_or_else(|| capacity_for(boon))
}

/// The stack's holder.
pub fn owner(r: &Row) -> u32 {
    match r.statechange {
        BUFF_INITIAL | BUFF_APPLY | BUFF_DURATION => r.dst,
        _ => r.src,
    }
}

/// The row as our log writer writes it, with ids for addrs.
fn to_raw(t: u64, r: &Row) -> RawEvent {
    RawEvent {
        time: t,
        src_agent: u64::from(r.src),
        dst_agent: r.dst_raw.unwrap_or(u64::from(r.dst)),
        value: r.value,
        buff_dmg: r.buff_dmg,
        overstack: r.overstack,
        skillid: r.skill,
        src_instid: 0,
        dst_instid: 0,
        src_master_instid: 0,
        dst_master_instid: 0,
        iff: r.iff(),
        buff: r.buff,
        result: r.result,
        is_activation: 0,
        is_buffremove: r.is_buffremove,
        is_ninety: 0,
        is_fifty: 0,
        is_moving: 0,
        is_statechange: r.statechange,
        is_flanking: 0,
        is_shields: r.is_shields,
        is_offcycle: r.is_offcycle,
        pad: r.pad,
    }
}

/// `segments` clipped to `[.., now)`: ms held (stack-ms summed for intensity, the union for
/// duration), and ms per source.
fn clip(segments: &[HeldSegment], now: u64, intensity: bool) -> (u64, BTreeMap<u32, u64>) {
    let mut by_source: BTreeMap<u32, u64> = BTreeMap::new();
    let mut spans: Vec<(u64, u64)> = Vec::with_capacity(segments.len());
    for s in segments {
        let end = s.end.min(now);
        if s.start >= end {
            continue;
        }
        spans.push((s.start, end));
        if let Ok(src) = u32::try_from(s.source)
            && src != 0
        {
            *by_source.entry(src).or_default() += end - s.start;
        }
    }
    let up = if intensity {
        spans.iter().map(|(a, b)| b - a).sum()
    } else {
        spans.sort_unstable();
        let (mut up, mut cur) = (0, None::<(u64, u64)>);
        for (a, b) in spans {
            cur = match cur {
                Some((ca, cb)) if a <= cb => Some((ca, cb.max(b))),
                Some((ca, cb)) => {
                    up += cb - ca;
                    Some((a, b))
                }
                None => Some((a, b)),
            };
        }
        up + cur.map_or(0, |(a, b)| b - a)
    };
    (up, by_source)
}

impl BoonTracker {
    pub fn new() -> Self {
        BoonTracker {
            pairs: BTreeMap::new(),
            dirty: BTreeSet::new(),
            stack_ids: false,
            totals: BTreeMap::new(),
            registry: InstidRegistry::build(&empty_log(header())),
            header: header(),
            capacities: SQUAD_BOONS.iter().map(|&b| (b, capacity(b))).collect(),
        }
    }

    /// The rows [`crate::buffs::Stacks`] wrote at `now` (or its `initial` rows at the fight's
    /// start), with sources resolved to their master and to one id per account.
    pub fn rows(&mut self, now: u64, rows: &[Row]) {
        for r in rows {
            if matches!(r.statechange, STACK_ACTIVE | STACK_RESET) && !self.stack_ids {
                self.stack_ids = true;
                // axilog reads the flag log-wide: every pair simulates again with it.
                self.dirty.extend(self.pairs.keys().copied());
            }
            if !self.capacities.contains_key(&r.skill)
                || !matches!(
                    r.statechange,
                    BUFF_INITIAL
                        | STACK_ACTIVE
                        | STACK_RESET
                        | BUFF_APPLY
                        | BUFF_DURATION
                        | BUFF_REMOVE_ONE
                        | BUFF_REMOVE_ALL
                )
            {
                continue;
            }
            let key = (owner(r), r.skill);
            let header = &self.header;
            let pair = self.pairs.entry(key).or_insert_with(|| {
                let mut log = empty_log(header.clone());
                log.events.push(to_raw(0, &Row::default()));
                Pair {
                    log,
                    segments: Vec::new(),
                    up_segments: None,
                    open: false,
                    fresh: false,
                    up: 0,
                    by_source: BTreeMap::new(),
                }
            });
            pair.log.events.push(to_raw(now, r));
            self.dirty.insert(key);
        }
    }

    /// Re-simulate the pairs with new rows (holders in the squad only), and bring the totals to
    /// `now`. `groups` is each player's subgroup for the fight, `squad` whether a player counts
    /// (as holder, source or target), and `healing_of` the healing-power lookup Regeneration's
    /// stack order uses.
    pub fn tick(
        &mut self,
        now: u64,
        groups: &dyn Fn(u32) -> Option<u8>,
        squad: &dyn Fn(u32) -> bool,
        healing_of: &dyn Fn(u32) -> i16,
    ) {
        let marker = if self.stack_ids { STACK_ACTIVE } else { 0 };
        let healing = |a: u64| u32::try_from(a).map_or(0, healing_of);
        for key in std::mem::take(&mut self.dirty) {
            if !squad(key.0) {
                // Kept for when the holder counts.
                self.dirty.insert(key);
                continue;
            }
            let Some(pair) = self.pairs.get_mut(&key) else {
                continue;
            };
            pair.log.events[0].is_statechange = marker;
            let events = extract_buff_events_with_registry(
                &pair.log,
                &self.registry,
                &BTreeSet::from([key.1]),
            );
            let capacity = self.capacities.get(&key.1).copied().unwrap_or(1);
            let intensity = INTENSITY.contains(&key.1);
            let regeneration = key.1 == REGENERATION;
            pair.up_segments =
                regeneration.then(|| run_segments(events.clone(), capacity, intensity, FAR));
            pair.segments =
                run_sim(events, capacity, intensity, FAR, regeneration, &healing).segments;
            pair.fresh = true;
        }
        for (&(_, boon), pair) in &mut self.pairs {
            if pair.fresh || pair.open {
                let intensity = INTENSITY.contains(&boon);
                (pair.up, pair.by_source) = clip(&pair.segments, now, intensity);
                let mut open = pair.segments.iter().any(|s| s.end > now);
                if let Some(up) = &pair.up_segments {
                    pair.up = clip(up, now, intensity).0;
                    open |= up.iter().any(|s| s.end > now);
                }
                pair.open = open;
                pair.fresh = false;
            }
        }

        self.totals.clear();
        for (&(holder, boon), pair) in &self.pairs {
            if !squad(holder) || pair.segments.is_empty() {
                continue;
            }
            self.totals
                .entry(holder)
                .or_default()
                .entry(boon)
                .or_default()[0] += pair.up;
            let group = groups(holder);
            for (&src, &ms) in &pair.by_source {
                if !squad(src) {
                    continue;
                }
                let t = self.totals.entry(src).or_default().entry(boon).or_default();
                if src == holder {
                    t[1] += ms;
                    continue;
                }
                if group.is_some() && groups(src) == group {
                    t[2] += ms;
                }
                t[3] += ms;
            }
        }
    }

    /// `[up, self, group, squad]` per boon for `player`, cumulative since [`Self::clear`], as of
    /// the last tick. `group` and `squad` are SUMS over the targets (a player's ms on each
    /// squadmate added up), not axilog's per-target average: divide by the target count to
    /// compare with axilog.
    pub fn totals(&self, player: u32) -> Option<&BTreeMap<u32, [u64; 4]>> {
        self.totals.get(&player)
    }

    /// The rows given so far under `from` are `to`'s: a player's rows from before its account
    /// was known move to the account's id, as holder and as source.
    pub fn rename(&mut self, from: u32, to: u32) {
        if from == to {
            return;
        }
        let (f, t) = (u64::from(from), u64::from(to));
        let keys: Vec<(u32, u32)> = self.pairs.keys().copied().collect();
        for key in keys {
            let Some(pair) = self.pairs.get_mut(&key) else {
                continue;
            };
            let mut changed = false;
            for e in pair.log.events.iter_mut().skip(1) {
                if e.src_agent == f {
                    e.src_agent = t;
                    changed = true;
                }
                // Stack-active and stack-reset rows carry an instance id there, not an agent.
                if e.dst_agent == f && !matches!(e.is_statechange, STACK_ACTIVE | STACK_RESET) {
                    e.dst_agent = t;
                    changed = true;
                }
            }
            if key.0 != from {
                if changed {
                    self.dirty.insert(key);
                }
                continue;
            }
            let Some(moved) = self.pairs.remove(&key) else {
                continue;
            };
            self.dirty.remove(&key);
            let new_key = (to, key.1);
            match self.pairs.get_mut(&new_key) {
                Some(have) => {
                    have.log.events.extend(moved.log.events.into_iter().skip(1));
                    // Rows in time order, as they would have arrived under one id.
                    have.log.events[1..].sort_by_key(|e| e.time);
                }
                None => {
                    self.pairs.insert(new_key, moved);
                }
            }
            self.dirty.insert(new_key);
        }
    }

    #[cfg(test)]
    pub fn rows_held(&self) -> usize {
        self.pairs.len()
    }

    /// Start over (a new fight).
    pub fn clear(&mut self) {
        self.pairs.clear();
        self.dirty.clear();
        self.stack_ids = false;
        self.totals.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffs::Stacks;
    use axidps_protocol::v2::{Event, StackChange};
    use axilog_core::analysis::buffs::events::{BuffEvent, BuffEventKind};
    use axilog_core::analysis::buffs::generation::{run_segments, run_sim};
    use axilog_core::evtc::{RawAgent, RawEvent, RawHeader, RawLog};

    const MIGHT: u32 = 740;
    const FURY: u32 = 725;
    const PROTECTION: u32 = 717;
    const REGENERATION: u32 = 718;
    const QUICKNESS: u32 = 1187;
    const STABILITY: u32 = 1122;
    const ENEMY: u32 = 90;

    fn apply(dst: u32, src: u32, buff: u32, inst: u32, duration: u32, active: bool) -> Event {
        Event::BuffApply {
            dst,
            src,
            buff,
            inst,
            duration,
            level: 0,
            active,
        }
    }

    /// The processor's side: its `Stacks`, and the rows it hands the tracker.
    struct Feed {
        stacks: Stacks,
        tr: BoonTracker,
    }

    impl Feed {
        fn new() -> Self {
            Feed {
                stacks: Stacks::default(),
                tr: BoonTracker::new(),
            }
        }
        fn ev(&mut self, t: u64, ev: Event) -> Vec<Row> {
            let rows = self.stacks.apply(t, &ev);
            self.tr.rows(t, &rows);
            rows
        }
        fn tick(&mut self, now: u64, groups: &dyn Fn(u32) -> Option<u8>) {
            self.tr
                .tick(now, groups, &|id| groups(id).is_some(), &|_| 0);
        }
        fn get(&self, player: u32, boon: u32) -> [u64; 4] {
            self.tr
                .totals(player)
                .and_then(|m| m.get(&boon))
                .copied()
                .unwrap_or_default()
        }
    }

    fn one_group(id: u32) -> Option<u8> {
        (id < ENEMY).then_some(1)
    }

    fn apply_be(t: u64, owner: u64, src: u64, buff: u32, duration: u32, active: bool) -> BuffEvent {
        BuffEvent {
            time: t,
            buff_id: buff,
            owner,
            agent: src,
            buff_instance: 0,
            kind: BuffEventKind::Apply {
                duration_ms: duration,
                is_shields: active,
            },
        }
    }

    fn by_source(
        segs: &[axilog_core::analysis::buffs::generation::HeldSegment],
    ) -> BTreeMap<u64, u64> {
        let mut m = BTreeMap::new();
        for s in segs {
            *m.entry(s.source).or_default() += s.end - s.start;
        }
        m
    }

    /// Several sources stacking one duration boon: credit follows axilog's simulator (capacity,
    /// the active stack), not "newest apply".
    #[test]
    fn duration_credit_matches_axilog_sim() {
        let applies = [
            (1000, 2, 3000, true),
            (1500, 3, 4000, false),
            (2000, 1, 1000, false),
            (2500, 4, 2500, false),
            (2600, 5, 2000, false),
            (2700, 6, 6000, false),
            (2800, 2, 1500, false),
            (6000, 3, 500, true),
        ];
        let mut f = Feed::new();
        let mut reference = Vec::new();
        for (i, &(t, src, d, active)) in applies.iter().enumerate() {
            f.ev(t, apply(1, src, PROTECTION, 10 + i as u32, d, active));
            reference.push(apply_be(t, 1, u64::from(src), PROTECTION, d, active));
        }
        f.tick(20_000, &one_group);
        let want = by_source(&run_segments(
            reference,
            capacity(PROTECTION),
            false,
            20_000,
        ));
        assert!(want.len() > 2);
        assert_eq!(f.get(1, PROTECTION)[1], want[&1]);
        for src in 2..=6u32 {
            assert_eq!(
                f.get(src, PROTECTION)[3],
                want.get(&u64::from(src)).copied().unwrap_or(0),
                "source {src}"
            );
        }
        assert_eq!(f.get(1, PROTECTION)[0], want.values().sum::<u64>());
    }

    #[test]
    fn intensity_boon_counts_stacks() {
        let mut f = Feed::new();
        for inst in 1..=3 {
            f.ev(1000, apply(1, 1, MIGHT, inst, 1000, true));
        }
        f.tick(5000, &one_group);
        assert_eq!(f.get(1, MIGHT), [3000, 3000, 0, 0]);
    }

    #[test]
    fn self_group_squad_split() {
        let groups = |id: u32| match id {
            1 | 2 => Some(1),
            3 => Some(2),
            _ => None,
        };
        let mut f = Feed::new();
        for (i, dst) in [1, 2, 3, 4].into_iter().enumerate() {
            f.ev(1000, apply(dst, 1, FURY, i as u32 + 1, 1000, true));
        }
        f.tick(5000, &groups);
        assert_eq!(f.get(1, FURY), [1000, 1000, 1000, 2000]);
        assert_eq!(f.get(2, FURY), [1000, 0, 0, 0]);
        assert_eq!(f.get(3, FURY), [1000, 0, 0, 0]);
        // Not in the squad: no uptime kept, no credit.
        assert!(f.tr.totals(4).is_none());
    }

    /// One subgroup per player for the whole fight: the totals so far follow the subgroup given
    /// now, not the one at the time the boon was held.
    #[test]
    fn group_credit_uses_the_fights_subgroup() {
        let mut f = Feed::new();
        f.ev(1000, apply(2, 1, FURY, 1, 3000, true));
        f.tick(2000, &|_| Some(1));
        assert_eq!(f.get(1, FURY), [0, 0, 1000, 1000]);
        f.tick(5000, &|id| Some(if id == 2 { 2 } else { 1 }));
        assert_eq!(f.get(1, FURY), [0, 0, 0, 3000]);
        assert_eq!(f.get(2, FURY)[0], 3000);
    }

    fn blank() -> RawEvent {
        RawEvent {
            time: 0,
            src_agent: 0,
            dst_agent: 0,
            value: 0,
            buff_dmg: 0,
            overstack: 0,
            skillid: 0,
            src_instid: 0,
            dst_instid: 0,
            src_master_instid: 0,
            dst_master_instid: 0,
            iff: 0,
            buff: 0,
            result: 0,
            is_activation: 0,
            is_buffremove: 0,
            is_ninety: 0,
            is_fifty: 0,
            is_moving: 0,
            is_statechange: 0,
            is_flanking: 0,
            is_shields: 0,
            is_offcycle: 0,
            pad: 0,
        }
    }

    fn player(addr: u64, sub: u8) -> RawAgent {
        RawAgent {
            addr,
            prof: 1,
            is_elite: 0,
            toughness: 0,
            concentration: 0,
            healing: 0,
            hitbox_width: 0,
            condition: 0,
            hitbox_height: 0,
            name_raw: format!("P{addr}\0:p{addr}.1234\0{sub}\0").into_bytes(),
        }
    }

    /// A stability stack frozen (inactive) when it is stripped: its remove row carries its full
    /// duration, which axilog's preprocessing rewrites once the log has stack-active rows.
    #[test]
    fn stability_band_aid_runs() {
        let mut f = Feed::new();
        let mut log: Vec<(u64, Row)> = Vec::new();
        let mut push = |f: &mut Feed, t: u64, ev: Event| {
            for r in f.ev(t, ev) {
                log.push((t, r));
            }
        };
        push(&mut f, 1000, apply(1, 2, STABILITY, 1, 8000, true));
        push(&mut f, 1000, apply(1, 2, STABILITY, 2, 5000, false));
        push(&mut f, 1000, apply(1, 2, STABILITY, 3, 6000, false));
        push(
            &mut f,
            2000,
            Event::BuffStack {
                dst: 1,
                inst: 3,
                change: StackChange::Active,
                value: 6000,
            },
        );
        push(
            &mut f,
            3000,
            Event::BuffRemove {
                dst: 1,
                remover: ENEMY,
                inst: 2,
            },
        );
        // Player 2's pair has no stack-active row of its own: the log-wide flag still applies.
        push(&mut f, 1000, apply(2, 1, STABILITY, 4, 5000, false));
        push(
            &mut f,
            3000,
            Event::BuffRemove {
                dst: 2,
                remover: ENEMY,
                inst: 4,
            },
        );
        let end = 20_000;
        f.tick(end, &one_group);

        // axilog's analyze() on a log of the same rows.
        let mut events = vec![RawEvent {
            time: 1000,
            is_statechange: 9,
            ..blank()
        }];
        // Point of view, one team, and in combat: both players are active squad members.
        for (sc, src, value) in [(13, 1, 0), (22, 1, 705), (22, 2, 705), (1, 2, 0)] {
            events.push(RawEvent {
                time: 1000,
                is_statechange: sc,
                src_agent: src,
                value,
                ..blank()
            });
        }
        events.extend(log.iter().map(|(t, r)| to_raw(*t, r)));
        events.push(RawEvent {
            time: end,
            is_statechange: 10,
            ..blank()
        });
        let raw = RawLog {
            header: RawHeader {
                build: axidps_evtc::FORMAT_BUILD.into(),
                revision: 1,
                boss_id: axidps_evtc::BOSS_ID_WVW,
            },
            agents: vec![player(1, 1), player(2, 1)],
            skills: vec![],
            events,
            guid_map: vec![],
        };
        let enc = axilog_core::model::resolve(&raw);
        let m = axilog_core::analysis::analyze(&enc, &raw);
        for p in [1, 2] {
            let theirs = m.boon_uptime[&(u64::from(p), STABILITY)].avg_stacks;
            let mine = f.get(p, STABILITY)[0] as f64 / (end - 1000) as f64;
            assert!(
                (mine - theirs).abs() < 1e-9,
                "{p}: mine {mine} axilog {theirs}"
            );
        }
        assert_eq!(f.get(2, STABILITY)[0], 2000);
        let up = f.get(1, STABILITY)[0];
        // The strip took stack 2 at 3000 (without the rewrite it would run to 6000): 8000 +
        // 2000 + 6000 stack-ms.
        assert_eq!(up, 16_000);
    }

    /// The first stack-active row comes after a pair was simulated: that pair simulates again
    /// with the flag, as axilog reads the whole log at once.
    #[test]
    fn stack_id_flag_resimulates_earlier_pairs() {
        let mut f = Feed::new();
        f.ev(1000, apply(2, 1, STABILITY, 1, 5000, false));
        f.ev(
            3000,
            Event::BuffRemove {
                dst: 2,
                remover: ENEMY,
                inst: 1,
            },
        );
        f.tick(3500, &one_group);
        assert_eq!(f.get(2, STABILITY)[0], 2500);
        f.ev(4000, apply(1, 1, STABILITY, 2, 5000, false));
        f.ev(
            4000,
            Event::BuffStack {
                dst: 1,
                inst: 2,
                change: StackChange::Active,
                value: 5000,
            },
        );
        f.tick(20_000, &one_group);
        assert_eq!(f.get(2, STABILITY)[0], 2000);
    }

    fn regen_feed(f: &mut Feed) -> Vec<BuffEvent> {
        let cap = capacity(REGENERATION);
        let mut reference = Vec::new();
        let mut add = |f: &mut Feed, t: u64, src: u32, d: u32, active: bool, inst: u32| {
            f.ev(t, apply(1, src, REGENERATION, inst, d, active));
            reference.push(apply_be(t, 1, u64::from(src), REGENERATION, d, active));
        };
        add(f, 1000, 1, 10_000, true, 1);
        add(f, 1100, 2, 1000, false, 2);
        for i in 0..cap {
            add(f, 1200 + u64::from(i), 3 + i, 5000, false, 3 + i);
        }
        reference
    }

    /// Regeneration's stacks follow axilog's healing logic (it evicts a different stack when
    /// full), with the healing-power lookup passed in.
    #[test]
    fn regeneration_uses_healing_logic() {
        let mut f = Feed::new();
        let reference = regen_feed(&mut f);
        f.tick(60_000, &one_group);
        let healing = by_source(
            &run_sim(
                reference.clone(),
                capacity(REGENERATION),
                false,
                60_000,
                true,
                &|_| 0,
            )
            .segments,
        );
        let queue = by_source(&run_segments(
            reference.clone(),
            capacity(REGENERATION),
            false,
            60_000,
        ));
        assert_ne!(healing, queue, "the case must tell the two logics apart");
        for (&src, &ms) in &healing {
            let got = if src == 1 {
                f.get(1, REGENERATION)[1]
            } else {
                f.get(src as u32, REGENERATION)[3]
            };
            assert_eq!(got, ms, "source {src}");
        }

        // A healing lookup reaches the simulation.
        let mut g = Feed::new();
        let reference = regen_feed(&mut g);
        let heal = |id: u32| if id == 4 { -5 } else { 0 };
        g.tr.tick(60_000, &one_group, &|_| true, &heal);
        let want = by_source(
            &run_sim(
                reference,
                capacity(REGENERATION),
                false,
                60_000,
                true,
                &|a| heal(a as u32),
            )
            .segments,
        );
        assert_ne!(want, healing);
        assert_eq!(
            g.get(4, REGENERATION)[3],
            want.get(&4).copied().unwrap_or(0)
        );
        assert_eq!(
            g.get(2, REGENERATION)[3],
            want.get(&2).copied().unwrap_or(0)
        );
    }

    /// Regeneration's uptime is axilog's: its stack-count simulation queues stacks (only the
    /// credit uses the healing logic), so held time follows the queue.
    #[test]
    fn regeneration_uptime_uses_the_queue() {
        let mut f = Feed::new();
        let reference = regen_feed(&mut f);
        f.tick(60_000, &one_group);
        let cap = capacity(REGENERATION);
        let union = |segs: &[HeldSegment]| clip(segs, 60_000, false).0;
        let queue = union(&run_segments(reference.clone(), cap, false, 60_000));
        let healing = union(&run_sim(reference, cap, false, 60_000, true, &|_| 0).segments);
        assert_ne!(queue, healing, "the case must tell the two apart");
        assert_eq!(f.get(1, REGENERATION)[0], queue);
    }

    /// A 60 s fight with overlapping sources, removes and stack changes: ticking every second
    /// ends where one tick at the end does.
    #[test]
    fn incremental_equals_full() {
        let mut seed = 0x2545_f491u64;
        let mut rnd = |n: u64| {
            seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            (seed >> 33) % n
        };
        let boons = [MIGHT, PROTECTION, STABILITY, QUICKNESS, REGENERATION, FURY];
        let mut evs: Vec<(u64, Event)> = Vec::new();
        let mut t = 1000u64;
        let mut inst = 1u32;
        while t < 61_000 {
            let dst = 1 + rnd(4) as u32;
            let buff = boons[rnd(boons.len() as u64) as usize];
            let ev = match rnd(10) {
                0 => Event::BuffRemoveAll {
                    dst,
                    remover: ENEMY,
                    buff,
                },
                1 => Event::BuffRemove {
                    dst,
                    remover: ENEMY,
                    inst: 1 + rnd(u64::from(inst)) as u32,
                },
                2 => Event::BuffStack {
                    dst,
                    inst: 1 + rnd(u64::from(inst)) as u32,
                    change: StackChange::Active,
                    value: 2000,
                },
                _ => {
                    inst += 1;
                    apply(
                        dst,
                        1 + rnd(5) as u32,
                        buff,
                        inst,
                        500 + rnd(8000) as u32,
                        rnd(2) == 0,
                    )
                }
            };
            evs.push((t, ev));
            t += rnd(150);
        }
        let groups = |id: u32| (id < ENEMY).then_some(1 + (id % 2) as u8);
        let mut a = Feed::new();
        let mut next = 2000;
        for &(t, ev) in &evs {
            while t >= next {
                a.tick(next, &groups);
                next += 1000;
            }
            a.ev(t, ev);
        }
        let mut b = Feed::new();
        for &(t, ev) in &evs {
            b.ev(t, ev);
        }
        let end = 62_000;
        a.tick(end, &groups);
        b.tick(end, &groups);
        assert_eq!(a.tr.totals, b.tr.totals);
        assert!(a.tr.totals.values().map(|m| m.len()).sum::<usize>() > 10);
    }

    /// A remove-all ends the boon. An agent remove writes no rows, so its stacks run until they
    /// expire (as axilog reads our logs).
    #[test]
    fn remove_all_stops_accrual() {
        let mut f = Feed::new();
        f.ev(1000, apply(2, 1, PROTECTION, 1, 10_000, true));
        f.ev(1000, apply(2, 1, PROTECTION, 2, 10_000, false));
        f.ev(
            3000,
            Event::BuffRemoveAll {
                dst: 2,
                remover: ENEMY,
                buff: PROTECTION,
            },
        );
        f.ev(1000, apply(3, 1, QUICKNESS, 3, 5000, true));
        f.stacks.forget_agent(3);
        f.tick(20_000, &one_group);
        assert_eq!(f.get(2, PROTECTION)[0], 2000);
        assert_eq!(f.get(3, QUICKNESS)[0], 5000);
        assert_eq!(f.get(1, PROTECTION)[3], 2000);
    }

    /// Rows under a raw id join the account id's own rows (holder and source), as one stream.
    #[test]
    fn rename_merges_rows() {
        let mut f = Feed::new();
        f.ev(1000, apply(7, 2, FURY, 1, 3000, true));
        f.ev(1000, apply(1, 7, PROTECTION, 2, 2000, true));
        f.ev(2000, apply(70, 2, FURY, 3, 4000, false));
        f.tr.rename(7, 70);
        f.tick(10_000, &|id| (id < ENEMY || id == 70).then_some(1));
        // Fury on 70: 1000..4000 from the first stack, then the queued one to 8000.
        assert_eq!(f.get(70, FURY)[0], 7000);
        assert_eq!(f.get(70, PROTECTION)[3], 2000);
        assert!(f.tr.totals(7).is_none());
    }

    /// A remove with no remover is an expiry or an overstack: axilog drops it (its simulation
    /// ends the stack itself), so it does not end the stack early. A strip does.
    #[test]
    fn removes_without_a_remover_do_not_end_the_stack() {
        let mut f = Feed::new();
        f.ev(1000, apply(1, 2, PROTECTION, 1, 5000, true));
        f.ev(
            3000,
            Event::BuffRemove {
                dst: 1,
                remover: 0,
                inst: 1,
            },
        );
        f.ev(1000, apply(3, 2, FURY, 2, 5000, true));
        f.ev(
            3000,
            Event::BuffRemove {
                dst: 3,
                remover: ENEMY,
                inst: 2,
            },
        );
        f.tick(20_000, &one_group);
        assert_eq!(f.get(1, PROTECTION)[0], 5000);
        assert_eq!(f.get(3, FURY)[0], 2000);
    }

    #[test]
    fn clear_starts_over() {
        let mut f = Feed::new();
        f.ev(1000, apply(1, 1, FURY, 1, 3000, true));
        f.tick(2000, &one_group);
        assert_eq!(f.get(1, FURY)[0], 1000);
        f.tr.clear();
        f.tick(3000, &one_group);
        assert!(f.tr.totals(1).is_none());
        f.ev(5000, apply(1, 1, FURY, 2, 1000, true));
        f.tick(9000, &one_group);
        assert_eq!(f.get(1, FURY), [1000, 1000, 0, 0]);
    }
}
