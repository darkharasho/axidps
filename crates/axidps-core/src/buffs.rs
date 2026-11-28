//! Buff stacks and the EVTC rows EVTC logs carry for them.
//!
//! The DLL forwards the game's buff messages raw; this keeps each stack (target + instance id)
//! so removes and stack changes can carry the buff id and times EVTC logs read from the game.

use std::collections::BTreeMap;

use axidps_protocol::v2::{Event, StackChange};

use crate::buff_tables::{MAX_STACKS, TICKING};

/// The EVTC buff statechange codes (its internal kinds are written unchanged).
pub const BUFF_INITIAL: u8 = 18;
pub const STACK_ACTIVE: u8 = 27;
pub const STACK_RESET: u8 = 28;
pub const BUFF_APPLY: u8 = 69;
pub const BUFF_DURATION: u8 = 70;
pub const BUFF_REMOVE_ONE: u8 = 71;
pub const BUFF_REMOVE_ALL: u8 = 72;

/// Infinite duration as the game sends it.
const INFINITE: u32 = 0x7fff_ffff; // public-check:ok

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stack {
    pub buff: u32,
    pub src: u32,
    /// ms; `INFINITE` for no end.
    pub duration: u32,
    pub applied: u64,
    /// When the remaining time was last `duration` (applied, or back-dated on STACKACTIVE).
    pub start: u64,
    pub active: bool,
    /// The EVTC remaining-time counter (the overstack source): set on apply, duration change
    /// and reset, and lowered by up to 1000 ms on each of the target's condition ticks.
    pub left: u32,
    /// The apply's level byte (formula level; 80 in WvW).
    pub level: u8,
}

/// One condition tick a target took, with the ticking stacks active at it: the condition
/// model's input (`crate::condi`). Recorded only when [`Stacks::record_ticks`] is on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fired {
    pub target: u32,
    pub time: u64,
    /// `(src, buff, level, ms)` per active stack, one entry per stack. `ms` is how much of the
    /// second up to the tick the stack was on (1000 for a full second): the game deals a
    /// condition's damage per second of duration, so a stack that lands or ends mid-second
    /// deals that part of a tick, and the EVTC estimates do the same.
    pub stacks: Vec<(u32, u32, u8, u16)>,
}

impl Stack {
    fn remaining(&self, now: u64) -> u32 {
        if !self.active || self.duration >= INFINITE {
            return self.duration;
        }
        let gone = u32::try_from(now.saturating_sub(self.start)).unwrap_or(u32::MAX);
        self.duration.saturating_sub(gone)
    }
}

/// One EVTC row, with agents as instance ids (`0` = none); the assembler maps them to addrs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Row {
    pub statechange: u8,
    pub src: u32,
    pub dst: u32,
    /// Written as `dst_agent` instead of `dst`'s addr (STACKACTIVE carries the instance id).
    pub dst_raw: Option<u64>,
    pub skill: u32,
    pub value: i32,
    pub buff_dmg: i32,
    pub overstack: u32,
    pub buff: u8,
    pub result: u8,
    pub is_buffremove: u8,
    pub is_shields: u8,
    pub is_offcycle: u8,
    pub pad: u32,
}

/// The EVTC iff byte for "unknown".
pub const IFF_UNKNOWN: u8 = 2;

impl Row {
    /// The row's iff byte as far as the row alone tells: a remove with no remover (an expiry or
    /// an overstack) is unknown; everything else is 0. The log writer replaces it with the
    /// teams' relation on removes that have a remover.
    pub fn iff(&self) -> u8 {
        let remove = matches!(self.statechange, BUFF_REMOVE_ONE | BUFF_REMOVE_ALL);
        if remove && self.dst == 0 && self.dst_raw.is_none() {
            IFF_UNKNOWN
        } else {
            0
        }
    }
}

/// A remove row (`kind` 2 single, 3 one stack of a remove-all): the agent losing the buff is the
/// source, the remover the destination; overstack holds the elapsed ms (low 16 bits, capped) and
/// the stack source's instance id (high 16).
fn remove_row(now: u64, dst: u32, remover: u32, inst: u32, s: &Stack, kind: u8) -> Row {
    let elapsed = u32::try_from(now.saturating_sub(s.applied))
        .unwrap_or(u32::MAX)
        .min(0xffff);
    Row {
        statechange: BUFF_REMOVE_ONE,
        src: dst,
        dst: remover,
        skill: s.buff,
        value: s.remaining(now).min(INFINITE) as i32,
        overstack: elapsed | ((s.src & 0xffff) << 16),
        buff: 1,
        is_buffremove: kind,
        is_shields: u8::from(s.active),
        pad: inst,
        ..Row::default()
    }
}

/// The EVTC condition tick period.
const TICK_MS: u64 = 1000;

/// How many ms of the tick's second `(from, at]` a stack on from `applied` to `end` covered.
fn on_ms(from: u64, at: u64, applied: u64, end: u64) -> u16 {
    let ms = end.min(at).saturating_sub(applied.max(from));
    u16::try_from(ms.min(TICK_MS)).unwrap_or(1000)
}

/// A remove with at most this much time left is the stack running out, not a cleanse (the EVTC
/// own is_offcycle threshold on removes).
const EXPIRY_SLACK_MS: u32 = 40;

/// Buff whose presence on the target marks an apply `is_offcycle`.
const OFFCYCLE_MARKER: u32 = 0xf65c;

fn max_stacks(buff: u32) -> Option<u16> {
    MAX_STACKS
        .binary_search_by_key(&buff, |&(b, _)| b)
        .ok()
        .and_then(|i| MAX_STACKS.get(i))
        .map(|&(_, m)| m)
}

/// Whether an exact damage row of `buff` lands only on its target's tick, so its time is the
/// target's tick phase: Bleeding, Burning, Poison and Torment. Confusion also hits when the
/// target uses a skill, so its rows say nothing about the phase.
pub fn phase_source(buff: u32) -> bool {
    matches!(buff, 723 | 736 | 737 | 19426)
}

fn starts_tick(buff: u32) -> bool {
    TICKING.binary_search(&buff).is_ok()
}

/// Every known stack, by `(target, instance id)`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Stacks {
    map: BTreeMap<(u32, u32), Stack>,
    /// Next condition tick per target (EVTC model: starts on the first apply of a ticking buff
    /// and keeps its phase while the target is known).
    ticks: BTreeMap<u32, u64>,
    /// Keep a [`Fired`] per tick in `fired` (the assembler turns it on for its copy).
    record: bool,
    fired: Vec<Fired>,
    /// Ticking stacks removed since their target's last tick (recorded only): EVTC logs queue a
    /// stack for the next tick while it is active, so one that ends mid-second still counts.
    /// `(target, src, buff, level, applied, ended)`.
    ended: Vec<(u32, u32, u32, u8, u64, u64)>,
}

impl Stacks {
    /// Run `agent`'s condition ticks due by `now`: each takes up to 1000 ms off active stacks.
    fn tick(&mut self, agent: u32, now: u64) {
        let Some(next) = self.ticks.get_mut(&agent) else {
            return;
        };
        while *next <= now {
            let at = *next;
            let from = at.saturating_sub(TICK_MS);
            let mut fired = Vec::new();
            let mut ticking = false;
            for s in self
                .map
                .range_mut((agent, 0)..=(agent, u32::MAX))
                .map(|(_, s)| s)
            {
                if s.active {
                    s.left = s.left.saturating_sub(TICK_MS as u32);
                    if starts_tick(s.buff) && s.applied < at && s.remaining(from) > 0 {
                        ticking = true;
                        if self.record {
                            let end = if s.duration >= INFINITE {
                                at
                            } else {
                                s.start.saturating_add(u64::from(s.duration))
                            };
                            fired.push((s.src, s.buff, s.level, on_ms(from, at, s.applied, end)));
                        }
                    }
                }
            }
            // Stacks that ended inside this second still tick once (the EVTC queue).
            let mut i = 0;
            while i < self.ended.len() {
                let (t, src, buff, level, applied, end) = self.ended[i];
                if t == agent && end <= at {
                    if end > from && applied < at {
                        fired.push((src, buff, level, on_ms(from, at, applied, end)));
                        ticking = true;
                    }
                    self.ended.swap_remove(i);
                } else {
                    i += 1;
                }
            }
            if !fired.is_empty() {
                self.fired.push(Fired {
                    target: agent,
                    time: at,
                    stacks: fired,
                });
            }
            if !ticking {
                // Nothing ticked: the timer stops; the next ticking apply restarts it (as in EVTC logs
                // clears its active flag, and the game restarts the phase too).
                self.ticks.remove(&agent);
                return;
            }
            *next += TICK_MS;
        }
    }

    /// A ticking stack ran out before its target's next tick: keep it for that tick. A stack
    /// removed with time left (a cleanse, a strip, a death) does not tick again: the POV's
    /// exact ticks show the game deals nothing for it.
    fn note_end(&mut self, target: u32, s: &Stack, now: u64) {
        if self.record && s.active && starts_tick(s.buff) && s.remaining(now) <= EXPIRY_SLACK_MS {
            self.ended
                .push((target, s.src, s.buff, s.level, s.applied, now));
        }
    }

    /// Whether `agent` holds any stack of a buff matching `pred` (active or not, as the EVTC
    /// per-agent buff walk).
    pub fn holds(&self, agent: u32, pred: impl Fn(u32) -> bool) -> bool {
        self.map
            .range((agent, 0)..=(agent, u32::MAX))
            .any(|(_, s)| pred(s.buff))
    }

    /// Whether `agent` holds an active stack of `buff` with time left at `now`.
    pub fn has_active(&self, agent: u32, buff: u32, now: u64) -> bool {
        self.map
            .range((agent, 0)..=(agent, u32::MAX))
            .any(|(_, s)| s.buff == buff && s.active && s.remaining(now) > 0)
    }

    /// `(buff, level)` of every active instance `agent` holds with time left at `now` (the
    /// condition model's source buffs).
    pub fn active_on(&self, agent: u32, now: u64) -> Vec<(u32, u8)> {
        self.map
            .range((agent, 0)..=(agent, u32::MAX))
            .filter(|(_, s)| s.active && s.remaining(now) > 0)
            .map(|(_, s)| (s.buff, s.level))
            .collect()
    }

    /// The game dealt `target` an exact condition tick at `at` (the POV's own, given or taken):
    /// put the target's tick phase on it. The phase is the target's, so every source's
    /// estimated ticks on it follow; the model's own guess drifts when the game restarts it.
    pub fn sync_tick(&mut self, target: u32, at: u64) {
        let half = TICK_MS / 2;
        // Ticks for earlier seconds run as they were.
        self.tick(target, at.saturating_sub(half));
        match self.ticks.get(&target).copied() {
            // Already ran this second's tick early: keep its phase from here.
            Some(next) if next > at + half => {
                self.ticks.insert(target, at + TICK_MS);
            }
            // This second's tick is still to come (or the model had stopped): run it now.
            _ => {
                self.ticks.insert(target, at);
                self.tick(target, at);
            }
        }
    }

    /// Record every condition tick from now on (see [`Fired`]).
    pub fn record_ticks(&mut self) {
        self.record = true;
    }

    /// Run every target's ticks due by `now`.
    pub fn advance(&mut self, now: u64) {
        let targets: Vec<u32> = self.ticks.keys().copied().collect();
        for t in targets {
            self.tick(t, now);
        }
    }

    /// The ticks recorded so far, in tick order per target.
    pub fn take_fired(&mut self) -> Vec<Fired> {
        std::mem::take(&mut self.fired)
    }

    /// The EVTC overstack and is_offcycle for an apply of `buff` onto `dst`, before it is added:
    /// at max stacks, the least time left on the target's stacks of it.
    fn overstack(&self, dst: u32, buff: u32) -> (u32, u8) {
        let held = self.map.range((dst, 0)..=(dst, u32::MAX)).map(|(_, s)| s);
        let (mut count, mut least, mut marker) = (0u32, INFINITE, false);
        for s in held {
            if s.buff == buff {
                count += 1;
                least = least.min(s.left);
            }
            marker |= s.buff == OFFCYCLE_MARKER;
        }
        let over = match max_stacks(buff) {
            Some(max) if count >= u32::from(max) => least,
            _ => 0,
        };
        (over, u8::from(marker))
    }

    /// Apply a buff event at `now`; the rows an EVTC log would carry for it.
    pub fn apply(&mut self, now: u64, ev: &Event) -> Vec<Row> {
        let target = match *ev {
            Event::BuffApply { dst, .. }
            | Event::BuffRemove { dst, .. }
            | Event::BuffRemoveAll { dst, .. }
            | Event::BuffStack { dst, .. } => dst,
            _ => return Vec::new(),
        };
        self.tick(target, now);
        match *ev {
            Event::BuffApply {
                dst,
                src,
                buff,
                inst,
                duration,
                active,
                level,
            } => {
                let (overstack, is_offcycle) = self.overstack(dst, buff);
                if starts_tick(buff) {
                    self.ticks.entry(dst).or_insert(now + TICK_MS);
                }
                let to_tick = self
                    .ticks
                    .get(&dst)
                    .map_or(0, |&next| next.saturating_sub(now));
                let to_tick = u32::try_from(to_tick).unwrap_or(u32::MAX);
                self.map.insert(
                    (dst, inst),
                    Stack {
                        buff,
                        src,
                        duration,
                        applied: now,
                        start: now,
                        active,
                        left: duration.saturating_sub(to_tick.min(duration)),
                        level,
                    },
                );
                vec![Row {
                    statechange: BUFF_APPLY,
                    src,
                    dst,
                    skill: buff,
                    value: duration.min(INFINITE) as i32,
                    overstack,
                    buff: 1,
                    is_shields: u8::from(active),
                    is_offcycle,
                    pad: inst,
                    ..Row::default()
                }]
            }
            Event::BuffRemove { dst, remover, inst } => {
                let removed = self.map.remove(&(dst, inst));
                removed
                    .map(|s| {
                        self.note_end(dst, &s, now);
                        vec![remove_row(now, dst, remover, inst, &s, 2)]
                    })
                    .unwrap_or_default()
            }
            Event::BuffRemoveAll { dst, remover, buff } => {
                let keys: Vec<(u32, u32)> = self
                    .map
                    .iter()
                    .filter(|&(&(d, _), s)| d == dst && s.buff == buff)
                    .map(|(&k, _)| k)
                    .collect();
                let mut rows = Vec::with_capacity(keys.len() + 1);
                let (mut sum, mut longest) = (0i64, 0u32);
                for k in keys {
                    if let Some(s) = self.map.remove(&k) {
                        self.note_end(dst, &s, now);
                        let left = s.remaining(now);
                        sum += i64::from(left);
                        longest = longest.max(left);
                        rows.push(remove_row(now, dst, remover, k.1, &s, 3));
                    }
                }
                let count = u8::try_from(rows.len()).unwrap_or(u8::MAX);
                rows.push(Row {
                    statechange: BUFF_REMOVE_ALL,
                    src: dst,
                    dst: remover,
                    skill: buff,
                    value: i32::try_from(sum).unwrap_or(i32::MAX),
                    buff_dmg: longest.min(INFINITE) as i32,
                    result: count,
                    buff: 1,
                    is_buffremove: 1,
                    ..Row::default()
                });
                rows
            }
            Event::BuffStack {
                dst,
                inst,
                change,
                value,
            } => {
                let Some(s) = self.map.get_mut(&(dst, inst)) else {
                    return Vec::new();
                };
                match change {
                    StackChange::Active => {
                        s.active = true;
                        s.start = now.saturating_sub(u64::from(s.duration.saturating_sub(value)));
                        vec![Row {
                            statechange: STACK_ACTIVE,
                            src: dst,
                            dst_raw: Some(u64::from(inst)),
                            skill: s.buff,
                            value: value.min(INFINITE) as i32,
                            buff_dmg: s.duration.saturating_sub(value).min(INFINITE) as i32,
                            ..Row::default()
                        }]
                    }
                    StackChange::Reset => {
                        // The game resets only a ticking stack.
                        if !s.active {
                            return Vec::new();
                        }
                        let left = s.remaining(now);
                        let row = Row {
                            statechange: STACK_RESET,
                            src: dst,
                            skill: s.buff,
                            value: value.min(INFINITE) as i32,
                            overstack: left,
                            buff_dmg: s.duration.saturating_sub(left).min(INFINITE) as i32,
                            is_shields: 1,
                            pad: inst,
                            ..Row::default()
                        };
                        s.duration = value;
                        s.left = value;
                        s.start = now;
                        s.active = false;
                        vec![row]
                    }
                    StackChange::Duration => {
                        let old = s.duration;
                        s.duration = value;
                        s.left = value;
                        vec![Row {
                            statechange: BUFF_DURATION,
                            dst,
                            skill: s.buff,
                            value: (i64::from(value) - i64::from(old)) as i32,
                            overstack: value,
                            buff: 1,
                            is_shields: u8::from(s.active),
                            is_offcycle: 1,
                            pad: inst,
                            ..Row::default()
                        }]
                    }
                }
            }
            _ => Vec::new(),
        }
    }

    /// BUFFINITIAL rows for every known stack, as at a log start.
    pub fn initial(&self, now: u64) -> Vec<Row> {
        self.map
            .iter()
            .map(|(&(dst, inst), s)| Row {
                statechange: BUFF_INITIAL,
                src: s.src,
                dst,
                skill: s.buff,
                value: s.remaining(now).min(INFINITE) as i32,
                buff_dmg: s.duration.min(INFINITE) as i32,
                buff: BUFF_INITIAL,
                is_shields: u8::from(s.active),
                pad: inst,
                ..Row::default()
            })
            .collect()
    }

    /// The agent left: its stacks go.
    pub fn forget_agent(&mut self, id: u32) {
        self.map.retain(|&(dst, _), _| dst != id);
        self.ticks.remove(&id);
        self.ended.retain(|e| e.0 != id);
    }

    /// A map change: instance ids are reused.
    pub fn clear(&mut self) {
        self.map.clear();
        self.ticks.clear();
        self.ended.clear();
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn apply_row() {
        let mut s = Stacks::default();
        let r = s.apply(1000, &apply(5, 7, 740, 900, 3000, true));
        assert_eq!(
            r,
            vec![Row {
                statechange: BUFF_APPLY,
                src: 7,
                dst: 5,
                skill: 740,
                value: 3000,
                buff: 1,
                is_shields: 1,
                pad: 900,
                ..Row::default()
            }]
        );
        assert_eq!(s.len(), 1);
    }

    fn overstack_of(rows: &[Row]) -> (u32, u8) {
        (rows[0].overstack, rows[0].is_offcycle)
    }

    #[test]
    fn apply_below_max_has_no_overstack() {
        let mut s = Stacks::default();
        for i in 0..24 {
            s.apply(1000, &apply(5, 7, 740, i, 6000, true));
        }
        assert_eq!(
            overstack_of(&s.apply(1000, &apply(5, 7, 740, 99, 6000, true))),
            (0, 0)
        );
    }

    #[test]
    fn apply_at_max_carries_shortest_left_without_tick() {
        // Might never starts a tick: the EVTC remaining stays the full duration.
        let mut s = Stacks::default();
        s.apply(0, &apply(5, 7, 740, 0, 4000, true));
        for i in 1..25 {
            s.apply(500, &apply(5, 7, 740, i, 6000, true));
        }
        let r = s.apply(3000, &apply(5, 7, 740, 99, 6000, true));
        assert_eq!(overstack_of(&r), (4000, 0));
    }

    #[test]
    fn tick_takes_a_second_off_active_stacks() {
        // Bleeding (736) starts the target's tick (next at 1000): a stack applied at 0 is left
        // 6000 - 1000, then loses 1000 ms per tick.
        let mut s = Stacks::default();
        s.apply(0, &apply(5, 7, 736, 1, 10_000, true));
        for i in 2..27 {
            s.apply(0, &apply(5, 7, 740, i, 6000, true));
        }
        // Ticks at 1000, 2000 and 3000 have passed by 3500.
        let r = s.apply(3500, &apply(5, 7, 740, 99, 6000, true));
        assert_eq!(overstack_of(&r), (2000, 0));
    }

    fn fired_times(s: &mut Stacks) -> Vec<(u64, usize)> {
        s.take_fired()
            .iter()
            .map(|f| (f.time, f.stacks.len()))
            .collect()
    }

    #[test]
    fn a_stack_ending_mid_second_ticks_once_more() {
        // Bleeding 2500 ms from 0: ticks at 1000 and 2000 while held, and at 3000 for the half
        // second it was still on (EVTC logs queue it). Then the timer stops.
        let mut s = Stacks::default();
        s.record_ticks();
        s.apply(0, &apply(5, 7, 736, 1, 2500, true));
        s.apply(
            2500,
            &Event::BuffRemove {
                dst: 5,
                remover: 0,
                inst: 1,
            },
        );
        s.advance(10_000);
        assert_eq!(fired_times(&mut s), vec![(1000, 1), (2000, 1), (3000, 1)]);
    }

    #[test]
    fn fired_stacks_carry_the_part_of_the_second_they_were_on() {
        // Bleeding from 0 (ticks at 1000, 2000, ...); a second stack lands at 1400 and runs out
        // at 2700; the first runs out at 3200.
        let mut s = Stacks::default();
        s.record_ticks();
        s.apply(0, &apply(5, 7, 736, 1, 3200, true));
        s.apply(1400, &apply(5, 8, 736, 2, 1300, true));
        s.apply(
            2700,
            &Event::BuffRemove {
                dst: 5,
                remover: 0,
                inst: 2,
            },
        );
        s.advance(5000);
        let got: Vec<(u64, Vec<(u32, u16)>)> = s
            .take_fired()
            .iter()
            .map(|f| {
                (
                    f.time,
                    f.stacks.iter().map(|&(src, _, _, ms)| (src, ms)).collect(),
                )
            })
            .collect();
        assert_eq!(
            got,
            vec![
                (1000, vec![(7, 1000)]),
                (2000, vec![(7, 1000), (8, 600)]),
                (3000, vec![(7, 1000), (8, 700)]),
                (4000, vec![(7, 200)]),
            ]
        );
    }

    #[test]
    fn the_tick_phase_restarts_after_the_target_is_clear() {
        let mut s = Stacks::default();
        s.record_ticks();
        s.apply(0, &apply(5, 7, 736, 1, 1500, true));
        s.apply(
            1500,
            &Event::BuffRemove {
                dst: 5,
                remover: 0,
                inst: 1,
            },
        );
        s.advance(5000);
        // A new run at 7300: first tick a second later, not on the old 1000 phase.
        s.apply(7300, &apply(5, 7, 736, 2, 3000, true));
        s.advance(9500);
        assert_eq!(
            fired_times(&mut s),
            vec![(1000, 1), (2000, 1), (8300, 1), (9300, 1)]
        );
    }

    #[test]
    fn a_cleansed_stack_does_not_tick_again() {
        // Bleeding 5000 ms from 0, cleansed at 2500 with 2500 ms left: ticks at 1000 and 2000
        // only (the game deals nothing for the part second before the cleanse).
        let mut s = Stacks::default();
        s.record_ticks();
        s.apply(0, &apply(5, 7, 736, 1, 5000, true));
        s.apply(
            2500,
            &Event::BuffRemoveAll {
                dst: 5,
                remover: 9,
                buff: 736,
            },
        );
        s.advance(10_000);
        assert_eq!(fired_times(&mut s), vec![(1000, 1), (2000, 1)]);
    }

    #[test]
    fn an_exact_tick_moves_the_targets_phase() {
        // Model phase from the apply at 0 is 1000, 2000, …; the game ticks at 1400 instead.
        let mut s = Stacks::default();
        s.record_ticks();
        s.apply(0, &apply(5, 7, 736, 1, 10_000, true));
        s.advance(1300);
        assert_eq!(fired_times(&mut s), vec![(1000, 1)]);
        // Model ran 1000 early (within half a second): not run again, phase moves on.
        s.sync_tick(5, 1400);
        s.advance(3500);
        assert_eq!(fired_times(&mut s), vec![(2400, 1), (3400, 1)]);
        // A tick the model has not reached yet runs at the game's time.
        s.sync_tick(5, 4100);
        assert_eq!(fired_times(&mut s), vec![(4100, 1)]);
        s.advance(5200);
        assert_eq!(fired_times(&mut s), vec![(5100, 1)]);
    }

    #[test]
    fn apply_between_ticks_loses_time_to_next_tick() {
        let mut s = Stacks::default();
        s.apply(0, &apply(5, 7, 736, 1, 10_000, true));
        // Next tick at 1000: an apply at 400 is left 6000 - 600.
        for i in 2..27 {
            s.apply(400, &apply(5, 7, 740, i, 6000, true));
        }
        let r = s.apply(500, &apply(5, 7, 740, 99, 6000, true));
        assert_eq!(overstack_of(&r), (5400, 0));
    }

    #[test]
    fn offcycle_when_target_holds_marker_buff() {
        let mut s = Stacks::default();
        s.apply(0, &apply(5, 7, 0xf65c, 1, 10_000, true));
        assert_eq!(
            overstack_of(&s.apply(10, &apply(5, 7, 740, 2, 6000, true))),
            (0, 1)
        );
        assert_eq!(
            overstack_of(&s.apply(10, &apply(6, 7, 740, 3, 6000, true))),
            (0, 0)
        );
    }

    #[test]
    fn duration_change_sets_left() {
        let mut s = Stacks::default();
        for i in 0..25 {
            s.apply(0, &apply(5, 7, 740, i, 6000, true));
        }
        s.apply(
            100,
            &Event::BuffStack {
                dst: 5,
                inst: 3,
                change: StackChange::Duration,
                value: 2500,
            },
        );
        assert_eq!(
            overstack_of(&s.apply(200, &apply(5, 7, 740, 99, 6000, true))),
            (2500, 0)
        );
    }

    #[test]
    fn remove_single_carries_remaining_and_elapsed() {
        let mut s = Stacks::default();
        s.apply(1000, &apply(5, 7, 740, 900, 3000, true));
        let r = s.apply(
            2200,
            &Event::BuffRemove {
                dst: 5,
                remover: 3,
                inst: 900,
            },
        );
        assert_eq!(
            r,
            vec![Row {
                statechange: BUFF_REMOVE_ONE,
                src: 5,
                dst: 3,
                skill: 740,
                value: 1800,
                overstack: 1200 | (7 << 16),
                buff: 1,
                is_buffremove: 2,
                is_shields: 1,
                pad: 900,
                ..Row::default()
            }]
        );
        assert!(s.is_empty());
        assert!(
            s.apply(
                2300,
                &Event::BuffRemove {
                    dst: 5,
                    remover: 3,
                    inst: 900
                }
            )
            .is_empty(),
            "unknown stack"
        );
    }

    #[test]
    fn remove_all_writes_each_stack_then_a_summary() {
        let mut s = Stacks::default();
        s.apply(1000, &apply(5, 7, 740, 1, 3000, true));
        s.apply(1000, &apply(5, 8, 740, 2, 5000, true));
        s.apply(1000, &apply(5, 8, 1187, 3, 5000, true)); // other buff stays
        let r = s.apply(
            2000,
            &Event::BuffRemoveAll {
                dst: 5,
                remover: 9,
                buff: 740,
            },
        );
        assert_eq!(r.len(), 3);
        assert!(
            r[..2]
                .iter()
                .all(|x| x.statechange == BUFF_REMOVE_ONE && x.is_buffremove == 3)
        );
        assert_eq!(
            r[2],
            Row {
                statechange: BUFF_REMOVE_ALL,
                src: 5,
                dst: 9,
                skill: 740,
                value: 2000 + 4000,
                buff_dmg: 4000,
                result: 2,
                buff: 1,
                is_buffremove: 1,
                ..Row::default()
            }
        );
        assert_eq!(s.len(), 1);
    }

    /// Removes with no remover (expiry, overstack) are iff unknown, as EVTC logs carry them;
    /// removes by someone, and every other row, are 0.
    #[test]
    fn removes_without_a_remover_are_iff_unknown() {
        let mut s = Stacks::default();
        let a = s.apply(1000, &apply(5, 7, 740, 1, 3000, true));
        s.apply(1000, &apply(5, 7, 740, 2, 3000, true));
        s.apply(1000, &apply(5, 7, 740, 3, 3000, true));
        assert_eq!(a[0].iff(), 0);
        let gone = s.apply(
            2000,
            &Event::BuffRemove {
                dst: 5,
                remover: 0,
                inst: 1,
            },
        );
        assert_eq!(gone[0].iff(), IFF_UNKNOWN);
        let stripped = s.apply(
            2000,
            &Event::BuffRemove {
                dst: 5,
                remover: 9,
                inst: 2,
            },
        );
        assert_eq!(stripped[0].iff(), 0);
        let all = s.apply(
            2500,
            &Event::BuffRemoveAll {
                dst: 5,
                remover: 0,
                buff: 740,
            },
        );
        assert_eq!(all.len(), 2);
        assert!(all.iter().all(|r| r.iff() == IFF_UNKNOWN));
        let active = Row {
            statechange: STACK_ACTIVE,
            dst_raw: Some(4),
            ..Row::default()
        };
        assert_eq!(active.iff(), 0);
    }

    #[test]
    fn stack_active_reset_and_duration() {
        let mut s = Stacks::default();
        s.apply(1000, &apply(5, 7, 740, 1, 6000, false));
        let ch = |change, value| Event::BuffStack {
            dst: 5,
            inst: 1,
            change,
            value,
        };
        let r = s.apply(2000, &ch(StackChange::Active, 4000));
        assert_eq!(
            r,
            vec![Row {
                statechange: STACK_ACTIVE,
                src: 5,
                dst_raw: Some(1),
                skill: 740,
                value: 4000,
                buff_dmg: 2000,
                ..Row::default()
            }]
        );
        // 1 s later, 3000 remain; reset sets a new duration and stops the stack.
        let r = s.apply(3000, &ch(StackChange::Reset, 6000));
        assert_eq!(
            r,
            vec![Row {
                statechange: STACK_RESET,
                src: 5,
                skill: 740,
                value: 6000,
                overstack: 3000,
                buff_dmg: 3000,
                is_shields: 1,
                pad: 1,
                ..Row::default()
            }]
        );
        let r = s.apply(3500, &ch(StackChange::Duration, 8000));
        assert_eq!(
            r,
            vec![Row {
                statechange: BUFF_DURATION,
                dst: 5,
                skill: 740,
                value: 2000,
                overstack: 8000,
                buff: 1,
                is_offcycle: 1,
                pad: 1,
                ..Row::default()
            }]
        );
    }

    #[test]
    fn initial_rows_and_forget() {
        let mut s = Stacks::default();
        s.apply(1000, &apply(5, 7, 740, 1, 3000, true));
        assert_eq!(
            s.initial(2000),
            vec![Row {
                statechange: BUFF_INITIAL,
                src: 7,
                dst: 5,
                skill: 740,
                value: 2000,
                buff_dmg: 3000,
                buff: BUFF_INITIAL,
                is_shields: 1,
                pad: 1,
                ..Row::default()
            }]
        );
        s.forget_agent(5);
        assert!(s.is_empty());
    }
}
