//! Skill casts and the EVTC rows arcdps writes for them.
//!
//! One open cast per agent. arcdps also keeps per-agent flags that outlive the cast they were set
//! by (dodge and the per-kind flags); each is cleared only by the end that consumes it, so they are
//! kept here the same way.
//!
//! Skill timing comes from arcdps's reduction over the content table (`content::timing`). Each
//! agent keeps arcdps's cast progress: reset by a start, grown by elapsed time × the agent's
//! action speed (sc 92; 1.0 for a speed-immune skill or one the table lacks). An end writes it
//! as buff_dmg and compares it with the expected time for is_activation 3/4.
//! Not replicated: arcdps's 10 ms ticker phase (progress is continuous here, ±10 ms) and the
//! breakbar rule (speed 1.0 while the agent's breakbar is active).

use std::collections::BTreeMap;

use axidps_protocol::v2::{AgentState, Event};

/// arcdps's cast statechange codes.
pub const CAST_START: u8 = 67;
pub const CAST_END: u8 = 68;
/// Weapon swap: dst = the new set (raw), value = the previous set.
pub const WEAPON_SWAP: u8 = 11;
/// Stunbreak: value = the ms left of the CC window.
pub const STUNBREAK: u8 = 56;
/// Message statechanges, written
/// here because their state lives on the same arcdps agent record as the cast flags.
pub const TRANSFORMATION: u8 = 73;
pub const GADGET_ANIMATION: u8 = 77;
/// Teleport: dst and value carry the target only for squad members (the assembler's rule).
pub const TELEPORT: u8 = 85;
pub const JUMP: u8 = 86;
pub const FLYTO: u8 = 88;

/// arcdps end site codes used here.
pub mod site {
    pub const INTERRUPTED: u8 = 8;
    pub const DEAD: u8 = 9;
    pub const DOWNED: u8 = 10;
    /// Skipped while a dodge flag is set.
    pub const R: u8 = 12;
    /// One end message, by flag: 15 (`f3c`), 19 (`f3f`), 26 (`f41`).
    pub const MSG_4C: u8 = 15;
    pub const FLAG_3F: u8 = 19;
    pub const AGENT_REMOVED: u8 = 21;
    /// The agent is hidden from me (sc 76 = 0): arcdps ends its cast every frame.
    pub const HIDDEN: u8 = 24;
    /// Only casts with a skill duration.
    pub const MSG_33: u8 = 22;
    /// Only while the dodge flag is set.
    pub const DODGE_END: u8 = 23;
    pub const FLAG_41: u8 = 26;
    /// Only while flag `f40` is set.
    pub const FLAG_40: u8 = 27;
    /// arcdps-only sites (not emitted here), for the is_activation rule: 5 and the 10 ms
    /// ticker's 20.
    pub const FAILED: u8 = 5;
    pub const TICKER: u8 = 20;
}

/// The strike `result` of an interrupt.
const STRIKE_INTERRUPT: u8 = 5;
/// A CC pseudo-hit strike: opens the target's CC window.
const STRIKE_CC: u8 = 12;
/// A downing strike: no stunbreak for 2200 ms after one.
const STRIKE_DOWNED: u8 = 9;
const STUNBREAK_AFTER_DOWNED_MS: u64 = 2200;

/// arcdps's end is_activation: progress reached the expected time (within 40 ms), fell short,
/// not computed (sites 5, 20, 24), or the expected time is 0 (no timing: pseudo-skills and
/// skills the content table lacks).
const ACTIVATION_REACHED: u8 = 3;
const ACTIVATION_SHORT: u8 = 4;
const ACTIVATION_NOT_COMPUTED: u8 = 5;
const ACTIVATION_NO_TIMING: u8 = 6;

/// The site arcdps ends the open cast with when a start of `kind` arrives.
fn supersede_site(kind: u8) -> u8 {
    match kind {
        1 => 3,
        2 => 14,
        3 => 17,
        4 => 16,
        5 => 13,
        6 => 28,
        7 => 29,
        8 => 25,
        9 => 30,
        _ => 0,
    }
}

/// Whether arcdps would find a nonzero skill duration for a cast of `kind`. arcdps reads the
/// skill's timing; without it, the game-skill kinds 1 and 4 count as timed (logs: every kind-1
/// site-22 end had a duration; every site-15 end was kind 5).
fn timed(kind: u8) -> bool {
    matches!(kind, 1 | 4)
}

/// The cast an agent has open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Open {
    pub skill: u32,
    pub kind: u8,
    pub start: u64,
    /// arcdps's expected time (`min(t_a, t_b)`, ms) for kinds 1/4/5; 0 = none.
    pub expected: i32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct AgentCasts {
    open: Option<Open>,
    /// Set by a dodge start, cleared by a site-23 end.
    dodge: bool,
    /// Set by a kind 5 start, cleared by a site-15 end.
    f3c: bool,
    /// Set by a kind 6 pseudo-skill start, cleared by a site-15 end.
    f3f: bool,
    /// Set by a kind 7 pseudo-skill start, cleared by a site-27 end.
    f40: bool,
    /// Set by a kind 8 pseudo-skill start, cleared by a site-15 end.
    f41: bool,
    /// The weapon set of the last swap message (arcdps reads it from the game's equipment
    /// before the swap stores the new one; 0 before the first).
    set: u8,
    /// Hidden from me (polled visible = 0).
    hidden: bool,
    /// The open CC window `(from, to)`.
    cc: Option<(u64, u64)>,
    /// Downed or dead (arcdps reads the character's state).
    down: bool,
    /// The last downing strike on the agent.
    downed_hit: Option<u64>,
    /// Jumping, cleared by a site-23 end.
    jump: bool,
    /// Flying to a target, cleared by a site-23 end.
    flyto: bool,
    /// Set by an "up" state, cleared by a teleport row.
    up: bool,
    /// The last transformation; never cleared but by the next one.
    transform: Transform,
    /// Cast progress (here in ms), as of `progress_at`.
    progress: Progress,
    /// The latest start's activation point `t_a` (ms), 0 for kinds other than
    /// 1/4/5 (their record has none).
    t_a: u32,
    /// The activation marker is still to fire. Armed by a kind 1/4/5 start
    /// with `t_a != 0`; other starts leave it as it was.
    armed: bool,
}

/// arcdps's per-agent cast progress: grows with the action speed, reset by every start and kept
/// growing after the end.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Progress {
    ms: f64,
    at: u64,
    /// The last action speed (sc 92), `None` = 1.0.
    speed: Option<f32>,
    /// The cast's skill ignores action speed (or has no timing record).
    immune: bool,
}

impl Eq for Progress {}

impl Progress {
    fn speed(&self) -> f64 {
        if self.immune {
            1.0
        } else {
            f64::from(self.speed.unwrap_or(1.0))
        }
    }

    fn advance(&mut self, now: u64) {
        self.ms += now.saturating_sub(self.at) as f64 * self.speed();
        self.at = self.at.max(now);
    }
}

/// arcdps's activation marker skill (strike result 11).
pub const ACTIVATION_MARKER_SKILL: u32 = 23279;

/// arcdps's transformation fields on its agent record.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Transform {
    id: u32,
    shields: u8,
    offcycle: u8,
    /// When the last transformation with a duration ends (only set by one with a duration).
    expiry: Option<u64>,
}

/// As arcdps: a stunbreak inside the agent's CC window, if it is up and was not
/// downed in the last 2200 ms; the window closes.
fn stunbreak(now: u64, agent: u32, a: &mut AgentCasts, rows: &mut Vec<Row>) {
    let Some((from, to)) = a.cc else { return };
    let recent_down = a
        .downed_hit
        .is_some_and(|t| now.saturating_sub(t) <= STUNBREAK_AFTER_DOWNED_MS);
    if from < now && now < to && !a.down && !recent_down {
        a.cc = None;
        rows.push(Row {
            statechange: STUNBREAK,
            src: agent,
            value: i32::try_from(to - now).unwrap_or(i32::MAX),
            ..Row::default()
        });
    }
}

/// One cast row, with agents as instance ids (`0` = none); the assembler maps them to addrs and
/// computes `iff` for starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Row {
    pub statechange: u8,
    pub src: u32,
    pub dst: u32,
    pub skill: u32,
    pub value: i32,
    pub buff_dmg: i32,
    pub overstack: u32,
    pub result: u8,
    pub is_activation: u8,
    /// A raw `dst_agent` (not an agent), for the statechanges that pack data there.
    pub dst_raw: Option<u64>,
    pub is_flanking: u8,
    pub is_shields: u8,
    pub is_offcycle: u8,
}

/// A statechange row whose dst is raw data.
fn sc_row(statechange: u8, src: u32, dst: u64) -> Row {
    Row {
        statechange,
        src,
        dst_raw: Some(dst),
        ..Row::default()
    }
}

/// Four i16 packed into one u64, first lowest (arcdps writes them consecutively).
fn pack_i16x4(v: [i16; 4]) -> u64 {
    v.iter()
        .enumerate()
        .fold(0, |acc, (i, &x)| acc | (u64::from(x as u16) << (16 * i)))
}

/// Every agent's open cast and flags, by instance id.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Casts {
    map: BTreeMap<u32, AgentCasts>,
}

/// End `a`'s open cast at `now` with `site`, if it has one.
fn end(now: u64, agent: u32, a: &mut AgentCasts, site: u8, rows: &mut Vec<Row>) {
    if let Some(o) = a.open.take() {
        let ms = i32::try_from(now.saturating_sub(o.start)).unwrap_or(i32::MAX);
        a.progress.advance(now);
        let progress = a.progress.ms;
        let is_activation = match site {
            site::FAILED | site::TICKER | site::HIDDEN => ACTIVATION_NOT_COMPUTED,
            _ if o.expected == 0 => ACTIVATION_NO_TIMING,
            _ if progress >= f64::from(o.expected) - 40.0 => ACTIVATION_REACHED,
            _ => ACTIVATION_SHORT,
        };
        rows.push(Row {
            statechange: CAST_END,
            src: agent,
            skill: o.skill,
            value: ms,
            buff_dmg: progress.round().min(f64::from(i32::MAX)) as i32,
            result: site,
            is_activation,
            ..Row::default()
        });
    }
}

impl Casts {
    /// The cast `agent` has open.
    pub fn open(&self, agent: u32) -> Option<Open> {
        self.map.get(&agent).and_then(|a| a.open)
    }

    /// The TRANSFORMATION row arcdps re-emits at log start (`now`) for a transformed agent:
    /// value = the ms left of the last transformation with a duration (negative once past,
    /// as arcdps's wrapping subtraction), 0 if none had one.
    pub fn transform_row(&self, agent: u32, now: u64) -> Option<Row> {
        let t = self.map.get(&agent)?.transform;
        (t.id != 0).then(|| Row {
            statechange: TRANSFORMATION,
            src: agent,
            skill: t.id,
            value: t.expiry.map_or(0, |e| e.wrapping_sub(now) as i32),
            is_shields: t.shields,
            is_offcycle: t.offcycle,
            ..Row::default()
        })
    }

    /// Apply an event at `now`; the cast rows arcdps would write for it. Besides the cast events,
    /// a down or death (`State`), an interrupt strike and a despawn end the agent's open cast.
    pub fn apply(&mut self, now: u64, ev: &Event) -> Vec<Row> {
        let mut rows = Vec::new();
        match *ev {
            Event::CastStart {
                agent,
                target,
                skill,
                kind,
                x,
            } => {
                let a = self.map.entry(agent).or_default();
                stunbreak(now, agent, a, &mut rows);
                end(now, agent, a, supersede_site(kind), &mut rows);
                let timing = crate::content::timing(skill);
                a.progress = Progress {
                    ms: 0.0,
                    at: now,
                    immune: timing.is_none_or(|t| t.speed_immune),
                    ..a.progress
                };
                if matches!(kind, 1 | 4 | 5) {
                    a.t_a =
                        timing.map_or(0, |t| t.activation_ms.clamp(0, i64::from(u32::MAX)) as u32);
                    a.armed = a.t_a != 0;
                } else {
                    a.t_a = 0;
                }
                a.open = Some(Open {
                    skill,
                    kind,
                    start: now,
                    expected: match kind {
                        1 | 4 | 5 => timing.map_or(0, |t| t.start_value()),
                        _ => 0,
                    },
                });
                match kind {
                    2 => a.dodge = true,
                    5 => a.f3c = true,
                    6 => a.f3f = true,
                    7 => a.f40 = true,
                    8 => a.f41 = true,
                    _ => {}
                }
                // Kind 1: X is the overstack; value = min(t_a, t_b) and buff_dmg = the full
                // animation from the skill's timing (0 for a skill the content table lacks).
                // Every other kind: value = X, capped like arcdps.
                let (value, buff_dmg, overstack) = if kind == 1 {
                    let t = timing;
                    (
                        t.map_or(0, |t| t.start_value()),
                        t.map_or(0, |t| t.full_ms.clamp(0, i64::from(i32::MAX)) as i32),
                        x,
                    )
                } else {
                    (x.min(i32::MAX as u32) as i32, 0, 0)
                };
                rows.push(Row {
                    statechange: CAST_START,
                    src: agent,
                    dst: target,
                    skill,
                    value,
                    buff_dmg,
                    overstack,
                    result: kind,
                    ..Row::default()
                });
                if a.hidden {
                    end(now, agent, a, site::HIDDEN, &mut rows);
                }
            }
            Event::CastEnd { agent, site: s } => {
                let Some(a) = self.map.get_mut(&agent) else {
                    return rows;
                };
                if s == site::FLAG_40 {
                    stunbreak(now, agent, a, &mut rows);
                }
                match s {
                    site::R => {
                        if !a.dodge {
                            end(now, agent, a, s, &mut rows);
                        }
                    }
                    site::MSG_33 => {
                        if a.open.is_some_and(|o| timed(o.kind)) {
                            end(now, agent, a, s, &mut rows);
                        }
                    }
                    site::DODGE_END => {
                        // arcdps's order on site 23: the dodge, then the jump, then the fly-to.
                        if std::mem::take(&mut a.dodge) {
                            end(now, agent, a, s, &mut rows);
                        }
                        if std::mem::take(&mut a.jump) {
                            rows.push(sc_row(JUMP, agent, 0));
                        }
                        if std::mem::take(&mut a.flyto) {
                            rows.push(sc_row(FLYTO, agent, 0));
                        }
                    }
                    site::FLAG_40 => {
                        if std::mem::take(&mut a.f40) {
                            end(now, agent, a, s, &mut rows);
                        }
                    }
                    site::MSG_4C => {
                        if std::mem::take(&mut a.f3c) && a.open.is_some_and(|o| !timed(o.kind)) {
                            end(now, agent, a, site::MSG_4C, &mut rows);
                        }
                        if std::mem::take(&mut a.f3f) {
                            end(now, agent, a, site::FLAG_3F, &mut rows);
                        }
                        if std::mem::take(&mut a.f41) {
                            end(now, agent, a, site::FLAG_41, &mut rows);
                        }
                    }
                    _ => end(now, agent, a, s, &mut rows),
                }
            }
            Event::State { id, state } => {
                let a = self.map.entry(id).or_default();
                a.down = state != AgentState::Up;
                let s = match state {
                    AgentState::Dead => site::DEAD,
                    AgentState::Down => site::DOWNED,
                    AgentState::Up => {
                        self.map.entry(id).or_default().up = true;
                        return rows;
                    }
                };
                end(now, id, a, s, &mut rows);
            }
            Event::Strike {
                dst,
                value,
                result: STRIKE_CC,
                ..
            } => {
                let a = self.map.entry(dst).or_default();
                let to = (now + u64::try_from(value).unwrap_or(0)).saturating_sub(40);
                a.cc = Some((now, to));
            }
            Event::Strike {
                dst,
                result: STRIKE_DOWNED,
                ..
            } => {
                self.map.entry(dst).or_default().downed_hit = Some(now);
            }
            Event::Strike {
                dst,
                result: STRIKE_INTERRUPT,
                ..
            } => {
                if let Some(a) = self.map.get_mut(&dst) {
                    end(now, dst, a, site::INTERRUPTED, &mut rows);
                }
            }
            Event::AgentFlags { id, visible, .. } => {
                let a = self.map.entry(id).or_default();
                a.hidden = visible == 0;
                if a.hidden {
                    end(now, id, a, site::HIDDEN, &mut rows);
                }
            }
            Event::WeaponSwap { id, set } => {
                let a = self.map.entry(id).or_default();
                rows.push(Row {
                    statechange: WEAPON_SWAP,
                    src: id,
                    dst: u32::from(set),
                    value: i32::from(a.set),
                    ..Row::default()
                });
                a.set = set;
            }
            Event::Jump { id, offcycle } => {
                self.map.entry(id).or_default().jump = true;
                rows.push(Row {
                    is_offcycle: offcycle,
                    ..sc_row(JUMP, id, 1)
                });
            }
            Event::FlyTo { id, pos, speed } => {
                self.map.entry(id).or_default().flyto = true;
                rows.push(Row {
                    is_flanking: 1,
                    ..sc_row(FLYTO, id, pack_i16x4([pos[0], pos[1], pos[2], speed]))
                });
            }
            Event::Teleport {
                id,
                kind,
                arg,
                dst,
                value,
            } => {
                let a = self.map.entry(id).or_default();
                if arg != 0 || a.up {
                    a.up = false;
                    rows.push(Row {
                        value: value as i32,
                        overstack: arg,
                        is_offcycle: kind,
                        ..sc_row(TELEPORT, id, dst)
                    });
                }
            }
            Event::GadgetAnimation { id, token } => {
                rows.push(sc_row(GADGET_ANIMATION, id, token));
            }
            Event::Transform {
                id,
                transform,
                offcycle,
                duration,
                shields,
            } => {
                let t = &mut self.map.entry(id).or_default().transform;
                t.id = transform;
                t.shields = u8::from(shields);
                t.offcycle = offcycle;
                if duration != 0 {
                    t.expiry = Some(now + u64::from(duration));
                }
                rows.push(Row {
                    statechange: TRANSFORMATION,
                    src: id,
                    skill: transform,
                    value: duration as i32,
                    is_shields: u8::from(shields),
                    is_offcycle: offcycle,
                    ..Row::default()
                });
            }
            Event::AgentRemove { id } => {
                if let Some(mut a) = self.map.remove(&id) {
                    end(now, id, &mut a, site::AGENT_REMOVED, &mut rows);
                }
            }
            Event::ActionSpeed { id, speed_bits } => {
                let a = self.map.entry(id).or_default();
                a.progress.advance(now);
                a.progress.speed = Some(f32::from_bits(speed_bits));
            }
            _ => {}
        }
        rows
    }

    /// arcdps's activation markers due by `now`: every
    /// armed agent whose progress passed `t_a`, with the time it did, in agent order. Call before
    /// applying the event at `now` (a start resets the progress).
    pub fn activations(&mut self, now: u64) -> Vec<(u64, u32)> {
        let mut out = Vec::new();
        for (&id, a) in self.map.iter_mut().filter(|(_, a)| a.armed) {
            a.progress.advance(now);
            if a.progress.ms > f64::from(a.t_a) {
                a.armed = false;
                let past = (a.progress.ms - f64::from(a.t_a)) / a.progress.speed();
                let at = (now as f64 - past).ceil().max(0.0) as u64;
                out.push((at.min(now), id));
            }
        }
        out.sort_unstable();
        out
    }

    /// Forget everything (a new map reuses instance ids).
    pub fn clear(&mut self) {
        self.map.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DODGE_SKILL: u32 = 23275;

    fn start(agent: u32, target: u32, skill: u32, kind: u8, x: u32) -> Event {
        Event::CastStart {
            agent,
            target,
            skill,
            kind,
            x,
        }
    }

    fn end_ev(agent: u32, site: u8) -> Event {
        Event::CastEnd { agent, site }
    }

    fn end_row(agent: u32, skill: u32, site: u8, ms: i32) -> Row {
        Row {
            statechange: CAST_END,
            src: agent,
            skill,
            value: ms,
            buff_dmg: ms,
            result: site,
            is_activation: 6,
            ..Row::default()
        }
    }

    #[test]
    fn start_rows_by_kind() {
        let mut c = Casts::default();
        assert_eq!(
            c.apply(100, &start(5, 9, 5491, 1, 7)),
            vec![Row {
                statechange: CAST_START,
                src: 5,
                dst: 9,
                skill: 5491,
                value: 800,
                buff_dmg: 1240,
                overstack: 7,
                result: 1,
                ..Row::default()
            }],
            "value/buff_dmg from the skill's timing (as arcdps's logs)"
        );
        assert_eq!(
            (c.apply(120, &start(8, 0, u32::MAX, 1, 0))[0].value),
            0,
            "a skill the table lacks"
        );
        let rows = c.apply(150, &start(6, 0, 30000, 4, 450));
        assert_eq!(
            (rows[0].value, rows[0].overstack, rows[0].result),
            (450, 0, 4)
        );
        let rows = c.apply(150, &start(7, 0, 23285, 3, u32::MAX));
        assert_eq!(rows[0].value, i32::MAX, "capped like arcdps");
    }

    /// The marker fires once per armed cast when progress passes `t_a`, even after the cast
    /// ended; a pseudo-skill start while armed fires at once; a start resets progress.
    #[test]
    fn activation_markers() {
        let mut c = Casts::default();
        let t_a = crate::content::timing(5491).unwrap().activation_ms as u64;
        c.apply(1000, &start(5, 0, 5491, 1, 0));
        assert!(c.activations(1000 + t_a).is_empty(), "strictly past t_a");
        c.apply(1000 + t_a - 100, &end_ev(5, site::MSG_33));
        assert_eq!(c.activations(1000 + t_a + 30), vec![(1000 + t_a, 5)]);
        assert!(c.activations(9000).is_empty(), "once");
        c.apply(10_000, &start(5, 0, 5491, 1, 0));
        c.apply(10_050, &start(5, 0, 23275, 2, 0));
        assert_eq!(c.activations(10_060), vec![(10_050, 5)]);
        c.apply(20_000, &start(5, 0, 23275, 2, 0));
        assert!(
            c.activations(30_000).is_empty(),
            "a pseudo-skill does not arm"
        );
    }

    /// Progress grows with the action speed (sc 92) and gives is_activation 3 once it is within
    /// 40 ms of the expected time; a skill the table lacks runs at speed 1 with no expected time.
    #[test]
    fn end_progress_follows_action_speed() {
        let mut c = Casts::default();
        let speed = |v: f32| Event::ActionSpeed {
            id: 5,
            speed_bits: v.to_bits(),
        };
        c.apply(0, &speed(1.5));
        c.apply(1000, &start(5, 0, 5491, 1, 0));
        c.apply(1200, &speed(1.0));
        let r = c.apply(1700, &end_ev(5, site::MSG_33))[0];
        assert_eq!((r.value, r.buff_dmg, r.is_activation), (700, 800, 3));
        c.apply(2000, &start(5, 0, u32::MAX, 1, 0));
        c.apply(2000, &speed(1.5));
        let r = c.apply(2400, &end_ev(5, site::MSG_33))[0];
        assert_eq!((r.value, r.buff_dmg, r.is_activation), (400, 400, 6));
        c.apply(3000, &start(5, 0, 5491, 1, 0));
        let r = c.apply(3100, &end_ev(5, site::HIDDEN))[0];
        assert_eq!((r.buff_dmg, r.is_activation), (150, 5));
    }

    #[test]
    fn end_measures_from_start_and_new_start_supersedes() {
        let mut c = Casts::default();
        c.apply(1000, &start(5, 0, 5491, 1, 0));
        assert_eq!(
            c.apply(1750, &end_ev(5, site::MSG_33)),
            vec![Row {
                is_activation: 4,
                ..end_row(5, 5491, 22, 750)
            }],
            "750 ms is short of 5491's expected 800 ms"
        );
        assert_eq!(c.open(5), None);
        assert!(
            c.apply(1800, &end_ev(5, site::MSG_33)).is_empty(),
            "nothing open"
        );
        c.apply(2000, &start(5, 0, 5491, 1, 0));
        let rows = c.apply(2300, &start(5, 0, 6000, 4, 0));
        assert_eq!(
            rows[0],
            Row {
                is_activation: 4,
                ..end_row(5, 5491, 16, 300)
            }
        );
        assert_eq!(rows[1].statechange, CAST_START);
        assert_eq!(c.open(5).map(|o| o.skill), Some(6000));
        for (kind, s) in [
            (1, 3),
            (2, 14),
            (3, 17),
            (5, 13),
            (6, 28),
            (7, 29),
            (8, 25),
            (9, 30),
        ] {
            let rows = c.apply(3000, &start(5, 0, 1, kind, 0));
            assert_eq!(rows[0].result, s, "kind {kind}");
        }
    }

    #[test]
    fn site_33_only_for_timed_kinds() {
        let mut c = Casts::default();
        c.apply(0, &start(5, 0, 23285, 3, 0));
        assert!(c.apply(10, &end_ev(5, site::MSG_33)).is_empty());
        c.apply(20, &start(5, 0, 7000, 5, 0));
        assert!(c.apply(30, &end_ev(5, site::MSG_33)).is_empty());
        c.apply(40, &start(5, 0, 7000, 4, 0));
        assert_eq!(c.apply(50, &end_ev(5, site::MSG_33)).len(), 1);
    }

    #[test]
    fn dodge_flag_gates_r_and_0x57() {
        let mut c = Casts::default();
        c.apply(0, &start(5, 0, 23275, 2, 0));
        assert!(c.apply(10, &end_ev(5, site::R)).is_empty(), "dodging");
        assert_eq!(
            c.apply(20, &end_ev(5, site::DODGE_END)),
            vec![end_row(5, 23275, 23, 20)]
        );
        // The flag is consumed: a later site-23 end ends nothing, and 'r' ends normally.
        c.apply(30, &start(5, 0, 5491, 1, 0));
        assert!(c.apply(40, &end_ev(5, site::DODGE_END)).is_empty());
        assert_eq!(c.apply(50, &end_ev(5, site::R))[0].result, 12);
        // The flag outlives the dodge: superseded, the next cast still ends on site 23.
        c.apply(60, &start(5, 0, 23275, 2, 0));
        c.apply(70, &start(5, 0, 5491, 1, 0));
        assert!(c.apply(80, &end_ev(5, site::R)).is_empty());
        assert_eq!(c.apply(90, &end_ev(5, site::DODGE_END))[0].skill, 5491);
    }

    #[test]
    fn msg_4c_by_flags() {
        let mut c = Casts::default();
        // Kind 5: flag `f3c`, untimed, ends with 15.
        c.apply(0, &start(5, 0, 7000, 5, 0));
        assert_eq!(c.apply(10, &end_ev(5, site::MSG_4C))[0].result, 15);
        // Flag consumed.
        c.apply(20, &start(5, 0, 23285, 3, 0));
        assert!(c.apply(30, &end_ev(5, site::MSG_4C)).is_empty());
        // `f3c` set but the open cast is timed: no end, flag still cleared.
        c.apply(40, &start(5, 0, 7000, 5, 0));
        c.apply(50, &start(5, 0, 5491, 1, 0));
        assert!(c.apply(60, &end_ev(5, site::MSG_4C)).is_empty());
        assert!(c.open(5).is_some());
        // Kind 6 → 19, kind 8 → 26.
        c.apply(70, &start(5, 3, 23302, 6, 0));
        assert_eq!(c.apply(80, &end_ev(5, site::MSG_4C))[0].result, 19);
        c.apply(90, &start(5, 3, 23308, 8, 0));
        assert_eq!(c.apply(100, &end_ev(5, site::MSG_4C))[0].result, 26);
    }

    #[test]
    fn flag_40_gates_site_27() {
        let mut c = Casts::default();
        c.apply(0, &start(5, 0, 5491, 1, 0));
        assert!(c.apply(10, &end_ev(5, site::FLAG_40)).is_empty());
        c.apply(20, &start(5, 3, 23303, 7, 0));
        assert_eq!(c.apply(30, &end_ev(5, site::FLAG_40))[0].result, 27);
    }

    #[test]
    fn ungated_sites_end_directly() {
        let mut c = Casts::default();
        c.apply(0, &start(5, 0, 5491, 1, 0));
        assert_eq!(c.apply(5, &end_ev(5, 11))[0].result, 11);
        assert!(c.apply(6, &end_ev(99, 11)).is_empty(), "unknown agent");
    }

    #[test]
    fn down_death_interrupt_and_despawn_end_casts() {
        let mut c = Casts::default();
        let st = |id, state| Event::State { id, state };
        c.apply(0, &start(5, 0, 1, 1, 0));
        assert_eq!(c.apply(10, &st(5, AgentState::Down))[0].result, 10);
        c.apply(20, &start(5, 0, 1, 1, 0));
        assert!(c.apply(25, &st(5, AgentState::Up)).is_empty());
        assert_eq!(c.apply(30, &st(5, AgentState::Dead))[0].result, 9);
        c.apply(40, &start(5, 0, 1, 1, 0));
        let hit = |result| Event::Strike {
            src: 7,
            dst: 5,
            skill: 2,
            value: 0,
            overstack: 0,
            result,
        };
        assert!(c.apply(45, &hit(0)).is_empty());
        assert_eq!(c.apply(50, &hit(5))[0].result, 8);
        c.apply(60, &start(5, 0, 23275, 2, 0));
        assert_eq!(c.apply(70, &Event::AgentRemove { id: 5 })[0].result, 21);
        // Flags are forgotten with the agent.
        c.apply(80, &start(5, 0, 1, 1, 0));
        assert_eq!(c.apply(90, &end_ev(5, site::R)).len(), 1);
    }

    #[test]
    fn weapon_swap_value_is_the_previous_set() {
        let mut c = Casts::default();
        let swap = |id, set| Event::WeaponSwap { id, set };
        let row = |value, dst| Row {
            statechange: WEAPON_SWAP,
            src: 5,
            dst,
            value,
            ..Row::default()
        };
        assert_eq!(c.apply(10, &swap(5, 4)), vec![row(0, 4)], "first: 0");
        assert_eq!(c.apply(20, &swap(5, 2)), vec![row(4, 2)]);
        assert_eq!(
            c.apply(20, &swap(5, 2)),
            vec![row(2, 2)],
            "unchanged still written"
        );
        c.apply(30, &Event::AgentRemove { id: 5 });
        assert_eq!(
            c.apply(40, &swap(5, 5)),
            vec![row(0, 5)],
            "forgotten at despawn"
        );
    }

    /// A statechange row with a raw dst.
    fn sc_row(statechange: u8, src: u32, dst: u64) -> Row {
        Row {
            statechange,
            src,
            dst_raw: Some(dst),
            ..Row::default()
        }
    }

    #[test]
    fn jump_and_flyto_end_on_0x57_after_the_dodge() {
        let mut c = Casts::default();
        let jump = Event::Jump {
            id: 5,
            offcycle: 16,
        };
        assert_eq!(
            c.apply(10, &jump),
            vec![Row {
                is_offcycle: 16,
                ..sc_row(JUMP, 5, 1)
            }]
        );
        let fly = Event::FlyTo {
            id: 5,
            pos: [272, -569, -153],
            speed: 100,
        };
        assert_eq!(
            c.apply(20, &fly),
            vec![Row {
                is_flanking: 1,
                ..sc_row(FLYTO, 5, 0x0064_ff67_fdc7_0110) // public-check:ok
            }]
        );
        c.apply(30, &start(5, 0, DODGE_SKILL, 2, 0));
        // Site 23: the dodge's cast end, then the jump end, then the fly-to end (arcdps's order).
        let rows = c.apply(40, &end_ev(5, site::DODGE_END));
        assert_eq!(
            rows,
            vec![
                end_row(5, DODGE_SKILL, site::DODGE_END, 10),
                sc_row(JUMP, 5, 0),
                sc_row(FLYTO, 5, 0),
            ]
        );
        assert!(
            c.apply(50, &end_ev(5, site::DODGE_END)).is_empty(),
            "flags consumed"
        );
        // An agent with only a jump open: the end needs no cast entry of its own.
        c.apply(60, &Event::Jump { id: 6, offcycle: 0 });
        assert_eq!(
            c.apply(70, &end_ev(6, site::DODGE_END)),
            vec![sc_row(JUMP, 6, 0)]
        );
        // Forgotten at despawn.
        c.apply(80, &Event::Jump { id: 6, offcycle: 0 });
        c.apply(90, &Event::AgentRemove { id: 6 });
        assert!(c.apply(100, &end_ev(6, site::DODGE_END)).is_empty());
    }

    #[test]
    fn teleport_needs_an_arg_or_the_up_flag() {
        let mut c = Casts::default();
        let tp = |arg| Event::Teleport {
            id: 5,
            kind: 16,
            arg,
            dst: 0x1122_3344_5566_7788, // public-check:ok
            value: 0x4000_0000,         // public-check:ok
        };
        let row = |arg| Row {
            value: 0x4000_0000, // public-check:ok
            overstack: arg,
            is_offcycle: 16,
            ..sc_row(TELEPORT, 5, 0x1122_3344_5566_7788) // public-check:ok
        };
        let up = Event::State {
            id: 5,
            state: AgentState::Up,
        };
        assert_eq!(c.apply(10, &tp(6)), vec![row(6)]);
        assert!(c.apply(20, &tp(0)).is_empty(), "arg 0 without up");
        c.apply(30, &up);
        assert_eq!(c.apply(40, &tp(0)), vec![row(0)], "revive: up flag");
        assert!(c.apply(50, &tp(0)).is_empty(), "up flag consumed");
        c.apply(60, &up);
        c.apply(70, &tp(4));
        assert!(c.apply(80, &tp(0)).is_empty(), "any teleport row clears it");
    }

    #[test]
    fn gadget_animation_row_carries_the_token() {
        let mut c = Casts::default();
        let ev = Event::GadgetAnimation {
            id: 7,
            token: 0x1037_542c, // public-check:ok
        };
        assert_eq!(
            c.apply(10, &ev),
            vec![sc_row(GADGET_ANIMATION, 7, 0x1037_542c)] // public-check:ok
        );
    }

    #[test]
    fn transform_rows_and_log_start_state() {
        let mut c = Casts::default();
        let tf = |id, transform, duration, shields| Event::Transform {
            id,
            transform,
            offcycle: 5,
            duration,
            shields,
        };
        let row = |skill, value, is_shields| Row {
            statechange: TRANSFORMATION,
            src: 5,
            skill,
            value,
            is_shields,
            is_offcycle: 5,
            ..Row::default()
        };
        assert_eq!(c.transform_row(5, 0), None, "never transformed");
        assert_eq!(
            c.apply(1000, &tf(5, 844, 1500, false)),
            vec![row(844, 1500, 0)]
        );
        assert_eq!(
            c.transform_row(5, 1200),
            Some(row(844, 1300, 0)),
            "remaining ms"
        );
        // No duration, so the expiry of the earlier one stays (as arcdps's record).
        assert_eq!(c.apply(2000, &tf(5, 900, 0, true)), vec![row(900, 0, 1)]);
        assert_eq!(c.transform_row(5, 2000), Some(row(900, 500, 1)));
        assert_eq!(
            c.transform_row(5, 2600),
            Some(row(900, -100, 1)),
            "past: negative"
        );
        assert_eq!(c.apply(3000, &tf(5, 0, 0, false)), vec![row(0, 0, 0)]);
        assert_eq!(c.transform_row(5, 3000), None, "untransformed");
        // Without any duration the log-start value is 0.
        c.apply(4000, &tf(6, 844, 0, true));
        assert_eq!(c.transform_row(6, 9000).map(|r| r.value), Some(0));
        c.apply(5000, &Event::AgentRemove { id: 6 });
        assert_eq!(c.transform_row(6, 9000), None, "forgotten at despawn");
    }

    #[test]
    fn hidden_agent_ends_casts_with_site_24() {
        use axidps_protocol::v2::NOT_APPLICABLE as NA;
        let mut c = Casts::default();
        let vis = |visible| Event::AgentFlags {
            id: 5,
            targetable: 1,
            visible,
            named: NA,
        };
        c.apply(10, &start(5, 0, 100, 1, 0));
        let rows = c.apply(20, &vis(0));
        assert_eq!(rows.len(), 1);
        assert_eq!(
            (rows[0].statechange, rows[0].result),
            (CAST_END, site::HIDDEN)
        );
        // A cast started while hidden ends at once.
        let rows = c.apply(30, &start(5, 0, 200, 1, 0));
        let got: Vec<_> = rows
            .iter()
            .map(|r| (r.statechange, r.skill, r.result))
            .collect();
        assert_eq!(
            got,
            vec![(CAST_START, 200, 1), (CAST_END, 200, site::HIDDEN)]
        );
        c.apply(40, &vis(1));
        c.apply(50, &start(5, 0, 300, 1, 0));
        assert_eq!(c.open(5).map(|o| o.skill), Some(300), "visible again");
    }

    #[test]
    fn stunbreak_inside_a_cc_window() {
        let cc = |dst, total| Event::Strike {
            src: 9,
            dst,
            skill: 23306,
            value: total,
            overstack: 0,
            result: 12,
        };
        let brk = |agent| end_ev(agent, site::FLAG_40);
        let sb = |c: &mut Casts, now, ev: &Event| -> Vec<(u32, i32)> {
            c.apply(now, ev)
                .into_iter()
                .filter(|r| r.statechange == STUNBREAK)
                .map(|r| (r.src, r.value))
                .collect()
        };
        let mut c = Casts::default();
        // Window [1000, 1000 + 2000 - 40]; a break at 1500 leaves 1460 ms.
        c.apply(1000, &cc(5, 2000));
        assert_eq!(sb(&mut c, 1500, &brk(5)), vec![(5, 1460)]);
        assert!(sb(&mut c, 1600, &brk(5)).is_empty(), "window cleared");
        // A cast start inside the window breaks too.
        c.apply(3000, &cc(5, 1000));
        assert_eq!(sb(&mut c, 3100, &start(5, 0, 1, 1, 0)), vec![(5, 860)]);
        // Outside the window: nothing.
        c.apply(5000, &cc(5, 1000));
        assert!(sb(&mut c, 6000, &brk(5)).is_empty());
        // Downed: nothing; within 2200 ms of a downed hit: nothing.
        let down = |state| Event::State { id: 5, state };
        c.apply(7000, &cc(5, 2000));
        c.apply(7100, &down(AgentState::Down));
        assert!(sb(&mut c, 7200, &brk(5)).is_empty());
        c.apply(7300, &down(AgentState::Up));
        let downed_hit = Event::Strike {
            src: 9,
            dst: 5,
            skill: 1,
            value: 0,
            overstack: 0,
            result: 9,
        };
        c.apply(8000, &downed_hit);
        c.apply(9000, &cc(5, 3000));
        assert!(
            sb(&mut c, 10_100, &brk(5)).is_empty(),
            "2100 ms after the downed hit"
        );
        assert_eq!(sb(&mut c, 10_300, &brk(5)), vec![(5, 1660)]);
    }
}
