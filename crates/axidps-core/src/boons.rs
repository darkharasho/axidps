//! Boon uptime and generation, from the buff rows [`crate::buffs::Stacks`] writes, simulated
//! with axilog's boon simulator (the same one that reads our logs).
//!
//! Rows go to one small log per (holder, boon). Each tick re-simulates the pairs that got new
//! rows, to the end of time; the simulation never rewrites the past, so its held segments
//! clipped to `now` are final up to `now`. Sources arrive already resolved: a minion's rows carry
//! its master at the row's time, and each account has one id.
//!
//! A pair re-simulates from a checkpoint, not from its first row: the stacks axilog's simulator
//! held after the rows behind it (read back from the simulator's own output and replayed as
//! apply events), and the segments that ended by then. Anything that would change how the rows
//! behind it simulate (a renamed id, the log-wide stack-id flag, a row older than the
//! checkpoint) drops it, and the pair simulates from its first row again. The totals are the
//! same as simulating every row each tick (`checkpoints_equal_full_resimulation`). The rows stay
//! (a restart needs them).
//!
//! Group and squad credit follow axilog's rollup: one subgroup per player for the whole fight
//! (whatever `groups` says at the tick), and every squad player except the source as a target.
//! The tracker sums a source's ms over its targets; `LiveStats` divides by the target count.

use std::collections::{BTreeMap, BTreeSet};

use axilog_core::analysis::buffs::events::{
    BuffEvent, BuffEventKind, extract_buff_events_with_registry,
};
use axilog_core::analysis::buffs::generation::{HeldSegment, run_segments, run_sim};
use axilog_core::analysis::buffs::simulator::capacity_for;
use axilog_core::analysis::buffs::stack_type_for;
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

/// One simulation's state at a checkpoint.
#[derive(Default)]
struct Part {
    /// The stacks held, as apply events at the time of the last event behind the checkpoint.
    seed: Vec<BuffEvent>,
    /// Segments that ended by then (an intensity stack still held is cut there; its rest comes
    /// from the seed).
    kept: Vec<HeldSegment>,
    /// The latest end in `kept`, and `kept` clipped at or after it.
    kept_end: u64,
    kept_up: u64,
    kept_by_source: BTreeMap<u32, u64>,
}

/// Where a pair's simulation starts.
struct Checkpoint {
    /// The first row after it (row 0 is the marker; 1 with nothing behind).
    row: usize,
    /// The latest time of the rows behind it; every row after it is later. `None` with nothing
    /// behind.
    at: Option<u64>,
    /// The credit simulation, and Regeneration's queue simulation.
    parts: [Part; 2],
}

impl Checkpoint {
    fn start() -> Self {
        Checkpoint {
            row: 1,
            at: None,
            parts: Default::default(),
        }
    }
}

/// One holder's rows for one boon, and its last simulation.
struct Pair {
    /// Row 0 is the stack-id marker slot (sc 0 until any stack-active row is seen, then 27).
    log: RawLog,
    /// Since the checkpoint.
    segments: Vec<HeldSegment>,
    /// Regeneration only: the queue simulation axilog's uptime uses (its credit uses the healing
    /// logic in `segments`).
    up_segments: Option<Vec<HeldSegment>>,
    cp: Checkpoint,
    /// Band-aid boons only (axilog rewrites some of their removals from the other rows of the
    /// same stack): row indices per stack instance, and how many rows they cover.
    by_inst: BTreeMap<u32, Vec<usize>>,
    indexed: usize,
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
    /// The last tick's time.
    clipped_at: Option<u64>,
    /// Re-simulate from checkpoints (off only to test them against full re-simulation).
    checkpoints: bool,
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

/// `kept` and `segments` clipped to `[.., now)`; `kept`'s part comes from its cache when it has
/// ended by `now` (kept segments end where `segments` start, so the two add up).
fn clip_part(
    part: &Part,
    segments: &[HeldSegment],
    now: u64,
    intensity: bool,
) -> (u64, BTreeMap<u32, u64>) {
    if now < part.kept_end {
        let all: Vec<HeldSegment> = part.kept.iter().chain(segments).copied().collect();
        return clip(&all, now, intensity);
    }
    let (mut up, mut by_source) = clip(segments, now, intensity);
    up += part.kept_up;
    for (&src, &ms) in &part.kept_by_source {
        *by_source.entry(src).or_default() += ms;
    }
    (up, by_source)
}

/// Whether axilog's band aid can rewrite `boon`'s removals.
fn band_aid(boon: u32) -> bool {
    stack_type_for(boon).is_some_and(|t| t.band_aid_scope().is_some())
}

/// The stack instance a row names.
fn inst_of(e: &RawEvent) -> u32 {
    if e.is_statechange == STACK_ACTIVE {
        e.dst_agent as u32
    } else {
        e.pad
    }
}

fn seed_event(time: u64, holder: u32, boon: u32, source: u64, ms: u64) -> Option<BuffEvent> {
    Some(BuffEvent {
        time,
        buff_id: boon,
        owner: u64::from(holder),
        agent: source,
        buff_instance: 0,
        kind: BuffEventKind::Apply {
            duration_ms: u32::try_from(ms).ok()?,
            is_shields: false,
        },
    })
}

/// How one simulation runs.
struct Sim<'a> {
    holder: u32,
    boon: u32,
    capacity: u32,
    intensity: bool,
    /// Regeneration's credit simulation (axilog's healing logic).
    healing: Option<&'a dyn Fn(u64) -> i16>,
}

impl Sim<'_> {
    fn run(&self, events: Vec<BuffEvent>, end: u64) -> Vec<HeldSegment> {
        match self.healing {
            Some(h) => run_sim(events, self.capacity, self.intensity, end, true, h).segments,
            None => run_segments(events, self.capacity, self.intensity, end),
        }
    }

    /// The state after `events` (a checkpoint's seed and the events after it, up to the new
    /// one): the stacks held, as a seed, and the segments that ended. `None` when the
    /// simulator's output does not pin the state down.
    fn probe(&self, events: Vec<BuffEvent>) -> Option<(Vec<BuffEvent>, Vec<HeldSegment>)> {
        let Some(s) = events.iter().map(|e| e.time).max() else {
            return Some((Vec::new(), Vec::new()));
        };
        let far = self.run(events.clone(), FAR);
        if !self.intensity {
            // The queue drains at the end: one segment per stack held, in queue order, from the
            // clock (the last event's time); every earlier segment starts before it.
            let split = far.iter().position(|g| g.start >= s).unwrap_or(far.len());
            let (kept, drain) = far.split_at(split);
            if drain.iter().any(|g| g.start < s) {
                return None;
            }
            // A seed re-sorts Regeneration's stacks by healing power: they must tie.
            if let Some(h) = self.healing
                && drain.windows(2).any(|w| h(w[0].source) != h(w[1].source))
            {
                return None;
            }
            let seed = drain
                .iter()
                .map(|g| seed_event(s, self.holder, self.boon, g.source, g.end - g.start))
                .collect::<Option<Vec<_>>>()?;
            return Some((seed, kept.to_vec()));
        }
        // Intensity: run to the far end, the stacks held at `s` are credited last, in expiry
        // order (ties in the simulator's order), each up to its expiry. Their order in the
        // simulator: fill it to capacity with stacks that outlast them, then apply one marker
        // per stack held. Each marker evicts the stack with the least time left (the same
        // order), in its place, and a remove-all then credits every stack in the simulator's
        // order: marker `j` stands where the `j`-th stack to expire was.
        let apply = |agent: u64| BuffEvent {
            time: s,
            buff_id: self.boon,
            owner: u64::from(self.holder),
            agent,
            buff_instance: 0,
            kind: BuffEventKind::Apply {
                duration_ms: u32::MAX,
                is_shields: false,
            },
        };
        const FILL: u64 = u64::MAX / 2;
        let remove_all = BuffEvent {
            kind: BuffEventKind::RemoveAll,
            ..apply(0)
        };
        let mut probe = events;
        probe.push(remove_all);
        // Ended at `s` by a remove-all, the stacks held end there instead: the runs agree on
        // everything before (stacks that end at `s` included).
        let cut = self.run(probe.clone(), FAR);
        let done = far.iter().zip(&cut).take_while(|(a, b)| a == b).count();
        let ends = &far[done..];
        if ends.is_empty() {
            return Some((Vec::new(), far));
        }
        let (k, capacity) = (ends.len(), self.capacity as usize);
        if k > capacity
            || cut.len() != done + k
            || ends
                .iter()
                .any(|g| g.end <= s || g.end - s >= u64::from(u32::MAX))
        {
            return None;
        }
        probe.pop();
        probe.extend((k..capacity).map(|_| apply(FILL)));
        probe.extend((0..k as u64).map(|j| apply(FILL + 1 + j)));
        probe.push(remove_all);
        let marked = self.run(probe, FAR);
        if marked.len() != done + k + capacity || marked[..done] != far[..done] {
            return None;
        }
        let (evicted, order) = marked[done..].split_at(k);
        let same = |a: &HeldSegment, b: &HeldSegment| a.source == b.source && a.start == b.start;
        if evicted
            .iter()
            .zip(ends)
            .any(|(e, g)| !same(e, g) || e.end != s)
        {
            return None;
        }
        let mut seed = Vec::with_capacity(k);
        for g in &order[..k] {
            let j = usize::try_from(g.source.checked_sub(FILL + 1)?).ok()?;
            let stack = ends.get(j)?;
            seed.push(seed_event(
                s,
                self.holder,
                self.boon,
                stack.source,
                stack.end - s,
            )?);
        }
        let mut kept = far[..done].to_vec();
        kept.extend_from_slice(evicted);
        Some((seed, kept))
    }
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
            clipped_at: None,
            checkpoints: true,
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
                for pair in self.pairs.values_mut() {
                    pair.cp = Checkpoint::start();
                }
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
                    cp: Checkpoint::start(),
                    by_inst: BTreeMap::new(),
                    indexed: 1,
                    open: false,
                    fresh: false,
                    up: 0,
                    by_source: BTreeMap::new(),
                }
            });
            if pair.cp.at.is_some_and(|at| now <= at) {
                // Older than the checkpoint: the rows behind it simulate differently.
                pair.cp = Checkpoint::start();
            }
            pair.log.events.push(to_raw(now, r));
            self.dirty.insert(key);
        }
    }

    /// Re-simulate the pairs with new rows (holders in the squad only), and bring the totals to
    /// `now`. `groups` is each player's subgroup for the fight, `squad` whether a player counts
    /// (as holder or source), `target` whether a holder counts as a group and squad target, and
    /// `healing_of` the healing-power lookup Regeneration's stack order uses.
    pub fn tick(
        &mut self,
        now: u64,
        groups: &dyn Fn(u32) -> Option<u8>,
        squad: &dyn Fn(u32) -> bool,
        target: &dyn Fn(u32) -> bool,
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
            let boons = BTreeSet::from([key.1]);
            if band_aid(key.1) {
                index(pair);
            }
            if !self.checkpoints {
                pair.cp = Checkpoint::start();
            }
            let events = events_after(pair, &self.registry, &boons);
            let capacity = self.capacities.get(&key.1).copied().unwrap_or(1);
            let intensity = INTENSITY.contains(&key.1);
            let regeneration = key.1 == REGENERATION;
            let credit = Sim {
                holder: key.0,
                boon: key.1,
                capacity,
                intensity,
                healing: regeneration.then_some(&healing as &dyn Fn(u64) -> i16),
            };
            let queue = Sim {
                healing: None,
                intensity: false,
                ..credit
            };
            if self.checkpoints {
                advance(pair, &events, now, &credit, regeneration.then_some(&queue));
            }
            let after: Vec<BuffEvent> = match pair.cp.at {
                Some(at) => events.into_iter().filter(|e| e.time > at).collect(),
                None => events,
            };
            let seeded = |part: &Part| -> Vec<BuffEvent> {
                part.seed.iter().chain(&after).copied().collect()
            };
            pair.up_segments = regeneration.then(|| queue.run(seeded(&pair.cp.parts[1]), FAR));
            pair.segments = credit.run(seeded(&pair.cp.parts[0]), FAR);
            pair.fresh = true;
        }
        // Time went back (a fight's end is its last activity, before later ticks): every pair is
        // clipped again, closed ones included.
        let back = self.clipped_at.is_some_and(|t| now < t);
        self.clipped_at = Some(now);
        for (&(_, boon), pair) in &mut self.pairs {
            if pair.fresh || pair.open || back {
                let intensity = INTENSITY.contains(&boon);
                let [credit, queue] = &pair.cp.parts;
                (pair.up, pair.by_source) = clip_part(credit, &pair.segments, now, intensity);
                let mut open = credit.kept_end > now || pair.segments.iter().any(|s| s.end > now);
                if let Some(up) = &pair.up_segments {
                    pair.up = clip_part(queue, up, now, intensity).0;
                    open |= queue.kept_end > now || up.iter().any(|s| s.end > now);
                }
                pair.open = open;
                pair.fresh = false;
            }
        }

        self.totals.clear();
        for (&(holder, boon), pair) in &self.pairs {
            if !squad(holder) || (pair.segments.is_empty() && pair.cp.parts[0].kept.is_empty()) {
                continue;
            }
            self.totals
                .entry(holder)
                .or_default()
                .entry(boon)
                .or_default()[0] += pair.up;
            let group = groups(holder);
            let is_target = target(holder);
            for (&src, &ms) in &pair.by_source {
                if !squad(src) {
                    continue;
                }
                let t = self.totals.entry(src).or_default().entry(boon).or_default();
                if src == holder {
                    t[1] += ms;
                    continue;
                }
                if !is_target {
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
    /// the last tick. Here `group` and `squad` are sums over the targets (a player's ms on each
    /// squadmate added up); the feed sends them per target, averaged as axilog does (see
    /// `LiveStats`).
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
                    pair.restart();
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
                    have.restart();
                }
                None => {
                    let mut moved = moved;
                    moved.restart();
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

    /// Rows kept over every pair.
    #[cfg(test)]
    pub fn rows_stored(&self) -> usize {
        self.pairs.values().map(|p| p.log.events.len() - 1).sum()
    }

    /// Start over (a new fight).
    pub fn clear(&mut self) {
        self.pairs.clear();
        self.dirty.clear();
        self.stack_ids = false;
        self.totals.clear();
        self.clipped_at = None;
    }
}

impl Pair {
    /// Simulate from the first row again (rows behind the checkpoint changed or moved).
    fn restart(&mut self) {
        self.cp = Checkpoint::start();
        self.by_inst.clear();
        self.indexed = 1;
    }
}

/// Bring a band-aid pair's instance index up to date.
fn index(pair: &mut Pair) {
    for i in pair.indexed..pair.log.events.len() {
        let inst = inst_of(&pair.log.events[i]);
        if inst != 0 {
            pair.by_inst.entry(inst).or_default().push(i);
        }
    }
    pair.indexed = pair.log.events.len();
}

/// The pair's events after its checkpoint (or all of them). A band-aid boon's are read with
/// the rows behind the checkpoint the band aid looks at: the other rows of the same stacks. (It
/// also reads the whole log for one removal that lets it run, but any removal it rewrites is one
/// of those.)
fn events_after(pair: &Pair, registry: &InstidRegistry, boons: &BTreeSet<u32>) -> Vec<BuffEvent> {
    let Some(at) = pair.cp.at else {
        return extract_buff_events_with_registry(&pair.log, registry, boons);
    };
    let rows = &pair.log.events;
    let mut log = empty_log(pair.log.header.clone());
    log.events.push(rows[0].clone());
    if band_aid(*boons.first().unwrap_or(&0)) {
        let behind: BTreeSet<usize> = rows[pair.cp.row..]
            .iter()
            .filter_map(|e| pair.by_inst.get(&inst_of(e)))
            .flatten()
            .copied()
            .filter(|&i| i < pair.cp.row)
            .collect();
        log.events
            .extend(behind.into_iter().map(|i| rows[i].clone()));
    }
    log.events.extend_from_slice(&rows[pair.cp.row..]);
    extract_buff_events_with_registry(&log, registry, boons)
        .into_iter()
        .filter(|e| e.time > at)
        .collect()
}

/// Move the pair's checkpoint up to the rows before `now` (later rows can still arrive at
/// `now`). `events` are the pair's events after the current checkpoint. Kept where it is when
/// the rows are out of time order or a simulation's state can not be read back.
fn advance(pair: &mut Pair, events: &[BuffEvent], now: u64, credit: &Sim, queue: Option<&Sim>) {
    let rows = &pair.log.events;
    let from = pair.cp.row;
    let mut to = from;
    while to < rows.len() && rows[to].time < now {
        to += 1;
    }
    let Some(at) = rows[from..to].iter().map(|e| e.time).max() else {
        return;
    };
    if rows[to..].iter().any(|e| e.time <= at) {
        return;
    }
    let behind: Vec<BuffEvent> = events
        .iter()
        .filter(|e| pair.cp.at.is_none_or(|old| e.time > old) && e.time <= at)
        .copied()
        .collect();
    let mut probed: [Option<(Vec<BuffEvent>, Vec<HeldSegment>)>; 2] = Default::default();
    for (i, sim) in [Some(credit), queue].into_iter().enumerate() {
        let Some(sim) = sim else {
            continue;
        };
        let events = pair.cp.parts[i]
            .seed
            .iter()
            .chain(&behind)
            .copied()
            .collect();
        let Some(p) = sim.probe(events) else {
            return;
        };
        probed[i] = Some(p);
    }
    pair.cp.row = to;
    pair.cp.at = Some(at);
    for ((part, p), intensity) in pair
        .cp
        .parts
        .iter_mut()
        .zip(probed)
        .zip([credit.intensity, false])
    {
        let Some((seed, ended)) = p else {
            continue;
        };
        let (up, by_source) = clip(&ended, FAR, intensity);
        for (src, ms) in by_source {
            *part.kept_by_source.entry(src).or_default() += ms;
        }
        part.kept_up += up;
        part.kept_end = ended.iter().map(|g| g.end).fold(part.kept_end, u64::max);
        part.kept.extend(ended);
        part.seed = seed;
    }
}

#[cfg(test)]
pub(crate) mod tests {
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
                .tick(now, groups, &|id| groups(id).is_some(), &|_| true, &|_| 0);
        }
        fn get(&self, player: u32, boon: u32) -> [u64; 4] {
            self.tr
                .totals(player)
                .and_then(|m| m.get(&boon))
                .copied()
                .unwrap_or_default()
        }
    }

    /// Seeded WvW-like boon traffic on `players`, as the game sends it: two supports per
    /// subgroup of five pulse their subgroup every 3 s (four duration boons, three might, one
    /// stability) and ten squad players every 6 s; everyone gives themselves a boon every 2 s;
    /// queued duration stacks go active when the one ticking ends; might and stability stack to
    /// 25 (stability ticks one stack at a time, the rest frozen until it ends); two strips and a
    /// duration change every 500 ms. `step(t)` is every 100 ms.
    pub(crate) struct Traffic {
        seed: u64,
        inst: u32,
        players: Vec<u32>,
        enemy: u32,
        /// (holder, boon) -> stacks (inst, ms): a duration boon's queue (its first stack ticks
        /// until the second field), an intensity boon's stacks with their end times (`FROZEN`
        /// for a frozen stability stack, whose duration is in `frozen`).
        held: BTreeMap<(u32, u32), Held>,
        frozen: BTreeMap<u32, u64>,
        out: Vec<Event>,
    }

    const FROZEN: u64 = u64::MAX;

    /// A holder's stacks of one boon (see `Traffic::held`).
    type Held = (Vec<(u32, u64)>, u64);

    const DURATION_BOONS: [u32; 11] = [725, 1187, 30328, 717, 718, 726, 743, 719, 26980, 873, 5974];

    impl Traffic {
        pub(crate) fn new(seed: u64, players: Vec<u32>, enemy: u32) -> Self {
            Traffic {
                seed,
                inst: 1000,
                players,
                enemy,
                held: BTreeMap::new(),
                frozen: BTreeMap::new(),
                out: Vec::new(),
            }
        }

        fn rand(&mut self, n: u64) -> u64 {
            self.seed ^= self.seed << 13;
            self.seed ^= self.seed >> 7;
            self.seed ^= self.seed << 17;
            self.seed % n
        }

        fn duration_boon(&mut self) -> u32 {
            DURATION_BOONS[self.rand(DURATION_BOONS.len() as u64) as usize]
        }

        fn apply(&mut self, t: u64, dst: u32, src: u32, buff: u32, ms: u64) {
            self.inst += 1;
            let inst = self.inst;
            let intensity = INTENSITY.contains(&buff);
            let (stacks, ends) = self.held.entry((dst, buff)).or_default();
            if intensity && stacks.len() >= 25 {
                let (old, _) = stacks.remove(0);
                self.out.push(Event::BuffRemove {
                    dst,
                    remover: 0,
                    inst: old,
                });
            } else if !intensity && stacks.len() >= 9 {
                return;
            }
            let ticking = stacks.iter().filter(|s| s.1 != FROZEN).count();
            let active = if buff == STABILITY {
                ticking < 1
            } else {
                intensity || stacks.is_empty()
            };
            if intensity && !active {
                stacks.push((inst, FROZEN));
                self.frozen.insert(inst, ms);
            } else if intensity {
                stacks.push((inst, t + ms));
            } else {
                if active {
                    *ends = t + ms;
                }
                stacks.push((inst, ms));
            }
            self.out
                .push(apply(dst, src, buff, inst, ms as u32, active));
        }

        fn expire(&mut self, t: u64) {
            for (&(dst, buff), (stacks, ends)) in &mut self.held {
                if INTENSITY.contains(&buff) {
                    stacks.retain(|&(inst, end)| {
                        if end <= t {
                            self.out.push(Event::BuffRemove {
                                dst,
                                remover: 0,
                                inst,
                            });
                        }
                        end > t
                    });
                    // A frozen stability stack starts ticking when a ticking one ends.
                    while stacks.iter().filter(|s| s.1 != FROZEN).count() < 1 {
                        let Some(s) = stacks.iter_mut().find(|s| s.1 == FROZEN) else {
                            break;
                        };
                        let ms = self.frozen.remove(&s.0).unwrap_or(1000);
                        *s = (s.0, t + ms);
                        self.out.push(Event::BuffStack {
                            dst,
                            inst: s.0,
                            change: StackChange::Active,
                            value: ms as u32,
                        });
                    }
                } else if !stacks.is_empty() && *ends <= t {
                    let (inst, _) = stacks.remove(0);
                    self.out.push(Event::BuffRemove {
                        dst,
                        remover: 0,
                        inst,
                    });
                    if let Some(&(next, ms)) = stacks.first() {
                        *ends = t + ms;
                        self.out.push(Event::BuffStack {
                            dst,
                            inst: next,
                            change: StackChange::Active,
                            value: ms as u32,
                        });
                    }
                }
            }
        }

        /// The events at `t`.
        pub(crate) fn step(&mut self, t: u64) -> Vec<Event> {
            self.expire(t);
            let ids = self.players.clone();
            let n = ids.len() as u64;
            let group = |i: usize| i / 5;
            for (k, &src) in ids.iter().enumerate() {
                let phase = t + k as u64 * 300;
                if k % 5 < 2 && phase.is_multiple_of(3000) {
                    for (m, &dst) in ids.iter().enumerate() {
                        if group(m) != group(k) {
                            continue;
                        }
                        for _ in 0..4 {
                            let (b, ms) = (self.duration_boon(), 2000 + self.rand(4000));
                            self.apply(t, dst, src, b, ms);
                        }
                        for _ in 0..3 {
                            let ms = 8000 + self.rand(2000);
                            self.apply(t, dst, src, MIGHT, ms);
                        }
                        let ms = 3000 + self.rand(2000);
                        self.apply(t, dst, src, STABILITY, ms);
                    }
                }
                if k % 5 < 2 && phase.is_multiple_of(6000) {
                    for _ in 0..10 {
                        let dst = ids[self.rand(n) as usize];
                        for _ in 0..2 {
                            let (b, ms) = (self.duration_boon(), 3000 + self.rand(3000));
                            self.apply(t, dst, src, b, ms);
                        }
                    }
                }
                if (t + k as u64 * 100).is_multiple_of(2000) {
                    let b = self.duration_boon();
                    self.apply(t, src, src, b, 4000);
                }
            }
            if t.is_multiple_of(500) {
                for _ in 0..2 {
                    let dst = ids[self.rand(n) as usize];
                    let b = if self.rand(2) == 0 {
                        STABILITY
                    } else {
                        self.duration_boon()
                    };
                    if let Some((stacks, _)) = self.held.get_mut(&(dst, b))
                        && !stacks.is_empty()
                    {
                        // One stack (stability, frozen ones too) or all of them, by an enemy.
                        if b == STABILITY {
                            let i = (self.seed % stacks.len() as u64) as usize;
                            let (inst, _) = stacks.remove(i);
                            self.out.push(Event::BuffRemove {
                                dst,
                                remover: self.enemy,
                                inst,
                            });
                        } else {
                            stacks.clear();
                            self.out.push(Event::BuffRemoveAll {
                                dst,
                                remover: self.enemy,
                                buff: b,
                            });
                        }
                    }
                }
                // A ticking duration stack gets a new duration.
                let dst = ids[self.rand(n) as usize];
                let b = self.duration_boon();
                if let Some((stacks, ends)) = self.held.get_mut(&(dst, b))
                    && let Some(&(inst, _)) = stacks.first()
                {
                    *ends = t + 3000;
                    self.out.push(Event::BuffStack {
                        dst,
                        inst,
                        change: StackChange::Duration,
                        value: 3000,
                    });
                }
            }
            std::mem::take(&mut self.out)
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
        g.tr.tick(60_000, &one_group, &|_| true, &|_| true, &heal);
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

    /// A tick earlier than the last one (the fight ends at its last activity, after ticks past
    /// it) clips every pair to it, including stacks that ended in between.
    #[test]
    fn an_earlier_tick_clips_ended_stacks() {
        let mut f = Feed::new();
        f.ev(1000, apply(1, 1, FURY, 1, 2000, true));
        f.ev(1000, apply(2, 1, PROTECTION, 2, 9000, true));
        f.tick(5000, &one_group);
        assert_eq!(f.get(1, FURY)[0], 2000);
        f.tick(2500, &one_group);
        assert_eq!(f.get(1, FURY), [1500, 1500, 0, 0]);
        assert_eq!(f.get(2, PROTECTION)[0], 1500);
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

    /// Random event streams for one pair (few sources, few durations, so stacks often tie),
    /// cut at a random point: the state read back at the cut, simulated on with the rest,
    /// gives what simulating everything gives, for intensity, queue and healing-ordered boons.
    #[test]
    fn probe_resumes_like_the_full_run() {
        let mut seed = 0x51f1_5eedu64;
        let mut rnd = |n: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % n
        };
        let zero = |_: u64| 0i16;
        let varies = |a: u64| (a % 2) as i16;
        let mut resumed_cases = 0;
        for case in 0..4000 {
            let (intensity, healing): (bool, Option<&dyn Fn(u64) -> i16>) = match case % 4 {
                0 => (true, None),
                1 => (false, None),
                2 => (false, Some(&zero)),
                _ => (false, Some(&varies)),
            };
            let boon = if intensity { MIGHT } else { PROTECTION };
            let sim = Sim {
                holder: 1,
                boon,
                capacity: if intensity {
                    3 + rnd(4) as u32
                } else {
                    2 + rnd(4) as u32
                },
                intensity,
                healing,
            };
            let mut events = Vec::new();
            let mut t = 1000u64;
            for _ in 0..(5 + rnd(40)) {
                t += [0, 0, 100, 500, 1000][rnd(5) as usize];
                let src = 2 + rnd(3);
                let kind = match rnd(12) {
                    0 => BuffEventKind::RemoveAll,
                    1 | 2 => BuffEventKind::RemoveSingle {
                        removed_duration_ms: [500, 1000, 2000, 2500][rnd(4) as usize],
                    },
                    3 => BuffEventKind::Extend {
                        extended_ms: 1000,
                        new_duration_ms: [1000, 2000, 3000][rnd(3) as usize],
                    },
                    _ => BuffEventKind::Apply {
                        duration_ms: [0, 1000, 2000, 3000, 3000, 5000][rnd(6) as usize],
                        is_shields: rnd(4) == 0,
                    },
                };
                events.push(BuffEvent {
                    kind,
                    ..apply_be(t, 1, src, boon, 0, false)
                });
            }
            let cut = rnd(events.len() as u64) as usize;
            // Cut between times, as checkpoints are.
            let at = events[cut].time;
            let (behind, rest): (Vec<_>, Vec<_>) = events.iter().partition(|e| e.time <= at);
            let Some((seed_events, kept)) = sim.probe(behind) else {
                continue;
            };
            resumed_cases += 1;
            let full = sim.run(events.clone(), FAR);
            let mut resumed = kept.clone();
            resumed.extend(sim.run(seed_events.iter().chain(&rest).copied().collect(), FAR));
            if !intensity {
                assert_eq!(resumed, full, "case {case}");
            }
            for now in [at, at + 700, t, t + 2500, t + 60_000] {
                assert_eq!(
                    clip(&resumed, now, intensity),
                    clip(&full, now, intensity),
                    "case {case} at {now}"
                );
            }
        }
        // Only healing that differs between the stacks held keeps a state from being read.
        assert!(resumed_cases > 3400, "{resumed_cases} resumed");
    }

    /// Two trackers on the same rows, one simulating from checkpoints and one from the first
    /// row at every tick: the same totals at every tick (and at an end tick back in time), with
    /// the stack-id flag coming late, the band aid's first qualifying strip coming late, a
    /// rename, rows older than the last tick, and Regeneration ordered by healing power.
    #[test]
    fn checkpoints_equal_full_resimulation() {
        for healing in [0i16, 1] {
            let players: Vec<u32> = (10..22).collect();
            let mut traffic = Traffic::new(0x7357_c0de, players.clone(), ENEMY);
            let mut stacks = Stacks::default();
            let (mut a, mut b) = (BoonTracker::new(), BoonTracker::new());
            b.checkpoints = false;
            let groups = |id: u32| match id {
                10..22 => Some(1 + ((id - 10) / 5) as u8),
                113 => Some(1),
                _ => None,
            };
            let heal = move |id: u32| if healing == 0 { 0 } else { (id % 3) as i16 };
            let tick = |tr: &mut BoonTracker, now| {
                tr.tick(
                    now,
                    &groups,
                    &|id| groups(id).is_some(),
                    &|id| id != 11,
                    &heal,
                );
            };
            let mut renamed = false;
            let mut checkpointed = 0;
            for t in (100..=90_000).step_by(100) {
                let mut evs = traffic.step(t);
                // No stack-active rows for 20 s, no strips for 30 s.
                evs.retain(|e| match e {
                    Event::BuffStack { .. } => t > 20_000,
                    Event::BuffRemove { remover, .. } | Event::BuffRemoveAll { remover, .. } => {
                        *remover == 0 || t > 30_000
                    }
                    _ => true,
                });
                let mut batch: Vec<(u64, Event)> = evs.into_iter().map(|e| (t, e)).collect();
                if t % 7_000 == 0 {
                    // A late row: older than the last tick.
                    batch.push((
                        t - 1_500,
                        apply(12, 15, MIGHT, 900_000 + t as u32, 4000, true),
                    ));
                    batch.push((
                        t - 1_500,
                        apply(14, 12, FURY, 900_001 + t as u32, 2000, false),
                    ));
                }
                for (at, e) in batch {
                    let mut rows = stacks.apply(at, &e);
                    if renamed {
                        for r in &mut rows {
                            for id in [&mut r.src, &mut r.dst] {
                                if *id == 13 {
                                    *id = 113;
                                }
                            }
                        }
                    }
                    a.rows(at, &rows);
                    b.rows(at, &rows);
                }
                if t == 60_000 {
                    a.rename(13, 113);
                    b.rename(13, 113);
                    renamed = true;
                }
                if t % 500 == 0 {
                    tick(&mut a, t);
                    tick(&mut b, t);
                    assert_eq!(a.totals, b.totals, "at {t} (healing {healing})");
                    checkpointed += a.pairs.values().filter(|p| p.cp.at.is_some()).count();
                }
            }
            // The fight ends at its last activity, before the last ticks.
            tick(&mut a, 86_300);
            tick(&mut b, 86_300);
            assert_eq!(a.totals, b.totals, "end tick (healing {healing})");
            assert!(a.totals.len() >= players.len());
            assert!(checkpointed > 10_000, "checkpoints used: {checkpointed}");
        }
    }
}
