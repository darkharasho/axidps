//! Core's side of the relay: local heals and condition ticks to wire ones, and peers' back to
//! local events.

use std::collections::{BTreeMap, VecDeque};

use axidps_protocol::v2::{AgentKind, AgentState, Event, MotionKind};

use super::wire::{Endpoint, WireCombat, WireCondi, WireHeal, agent_of, endpoint_of};
use crate::registry::Registry;

/// How long agent positions are kept: enough for a peer's tick to arrive and be matched.
const POSITION_KEEP_MS: u64 = 5_000;
/// A peer's enemy target is the nearest enemy of its build and team within this many game units
/// of where the peer saw it (at the same moment, so mostly a few units off; see `foe`).
const FOE_RADIUS: f32 = 600.0;

/// Recent positions per agent, by game time: names enemy players (who have no name) by where
/// they are.
#[derive(Debug, Default)]
pub struct Positions {
    at: BTreeMap<u32, VecDeque<(u64, [f32; 3])>>,
}

impl Positions {
    /// Note a position change (or forget a despawned agent).
    pub fn note(&mut self, t: u64, ev: &Event) {
        match *ev {
            Event::Motion {
                id,
                kind: MotionKind::Position,
                xyz,
            } => {
                let h = self.at.entry(id).or_default();
                h.push_back((t, xyz.map(f32::from_bits)));
                // Keep the newest sample before the window too: the position over all of it.
                while h.len() > 1 && h[1].0.saturating_add(POSITION_KEEP_MS) < t {
                    h.pop_front();
                }
            }
            Event::AgentRemove { id } => {
                self.at.remove(&id);
            }
            _ => {}
        }
    }

    /// Forget everything (a map change: ids are reused).
    pub fn clear(&mut self) {
        self.at.clear();
    }

    /// Where `id` was at game time `t`: the last sample at or before it (else the first after).
    pub fn at(&self, id: u32, t: u64) -> Option<[f32; 3]> {
        let h = self.at.get(&id)?;
        h.iter()
            .rev()
            .find(|(at, _)| *at <= t)
            .or_else(|| h.front())
            .map(|&(_, p)| p)
    }

    /// The local enemy player a peer's [`Endpoint::Foe`] names at game time `t`: an unnamed
    /// player of that profession, elite and team, the nearest to the peer's position within
    /// [`FOE_RADIUS`]. Same-build enemies stacked together can swap; their totals do not change.
    pub fn foe(&self, reg: &Registry, e: &Endpoint, t: u64) -> Option<u32> {
        let &Endpoint::Foe {
            prof,
            elite,
            team,
            xyz,
        } = e
        else {
            return None;
        };
        let want = xyz.map(|c| c as f32);
        reg.live_agents()
            .filter(|a| {
                a.kind == AgentKind::Player
                    && a.account.is_empty()
                    && a.prof == prof
                    && a.elite == elite
                    && a.team == Some(team)
            })
            .filter_map(|a| {
                let p = self.at(a.id, t)?;
                let d = (0..3).map(|i| (p[i] - want[i]).powi(2)).sum::<f32>().sqrt();
                (d <= FOE_RADIUS).then_some((d, a.id))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, id)| id)
    }
}

/// `id` is the POV or one of its pets or minions.
fn povs(reg: &Registry, id: u32) -> bool {
    let pov = reg.pov();
    pov.is_some() && (pov == Some(id) || reg.agent(id).and_then(|a| a.master) == pov)
}

/// The POV's account name, once known.
pub fn pov_account(reg: &Registry) -> Option<&str> {
    reg.pov()
        .and_then(|id| reg.agent(id))
        .map(|a| a.account.as_str())
        .filter(|a| !a.is_empty())
}

/// The wire heal to share for a local event: only `Heal`, both endpoints known. `unix_ms` is the
/// wall clock time of the event; `downed` is the target's state in the registry.
pub fn outgoing(reg: &Registry, unix_ms: u64, ev: &Event) -> Option<WireHeal> {
    let &Event::Heal {
        src,
        dst,
        skill,
        value,
        barrier,
        buff,
    } = ev
    else {
        return None;
    };
    let value = u32::try_from(value).ok().filter(|&v| v > 0)?;
    let barrier = barrier.min(value);
    Some(WireHeal {
        t_unix_ms: unix_ms,
        src: endpoint_of(reg, src)?,
        dst: endpoint_of(reg, dst)?,
        skill,
        heal: value - barrier,
        barrier,
        buff,
        downed: reg.agent(dst).and_then(|a| a.state) == Some(AgentState::Down),
    })
}

/// The wire tick to share for a local event at game time `t`: only a damaging `BuffDamage` the
/// POV (or its minion) dealt to a known target. An enemy player is named by build, team and
/// position when its position is known.
pub fn outgoing_condi(
    reg: &Registry,
    pos: &Positions,
    t: u64,
    unix_ms: u64,
    ev: &Event,
) -> Option<WireCondi> {
    let &Event::BuffDamage {
        src,
        dst,
        buff,
        value,
        barrier,
        ..
    } = ev
    else {
        return None;
    };
    let damage = u32::try_from(value).ok().filter(|&v| v > 0)?;
    if src == 0 || src == dst || !povs(reg, src) {
        return None;
    }
    let target = reg.agent(dst)?;
    let foe =
        (target.kind == AgentKind::Player && target.account.is_empty() && target.master.is_none())
            .then(|| {
                Some(Endpoint::Foe {
                    prof: target.prof,
                    elite: target.elite,
                    team: target.team?,
                    xyz: pos.at(dst, t)?.map(|c| c.round() as i32),
                })
            })
            .flatten();
    Some(WireCondi {
        t_unix_ms: unix_ms,
        src: endpoint_of(reg, src)?,
        dst: match foe {
            Some(f) => f,
            None => endpoint_of(reg, dst)?,
        },
        buff,
        damage,
        barrier: barrier.min(damage),
    })
}

/// The local event for a peer's condition tick, like [`incoming`]. `None` for the POV's own
/// batches, an unknown sender, a source that is not the sender or its minion, or a zero tick.
/// A target that does not resolve is `dst` 0: the tick still counts for its source.
pub fn incoming_condi(
    reg: &Registry,
    pos: &Positions,
    from: &str,
    c: &WireCondi,
    clock: (u64, u64),
    floor: u64,
) -> Option<(u64, Event)> {
    if pov_account(reg) == Some(from) {
        return None;
    }
    let sender = agent_of(reg, &Endpoint::Player(from.to_string()))?;
    let src = agent_of(reg, &c.src)?;
    if src != sender && reg.agent(src).and_then(|a| a.master) != Some(sender) {
        return None;
    }
    let value = i32::try_from(c.damage).ok().filter(|&v| v > 0)?;
    // Where the target was when the tick landed, not when it arrived (the floor only orders).
    let at = local_time(c.t_unix_ms, clock, 0);
    let dst = match c.dst {
        Endpoint::Foe { .. } => pos.foe(reg, &c.dst, at),
        ref e => agent_of(reg, e),
    }
    .unwrap_or(0);
    Some((
        at.max(floor),
        Event::RelayBuffDamage {
            sender,
            src,
            dst,
            buff: c.buff,
            value,
            barrier: c.barrier.min(c.damage),
        },
    ))
}

/// The local event for a peer's heal, at the local time for its `t_unix_ms`: `clock` is
/// `(event time ms, unix ms at it)`, `floor` the last time pushed (times never go backwards).
/// `None` for the POV's own batches, an unknown sender, an unresolved endpoint or a zero heal.
pub fn incoming(
    reg: &Registry,
    from: &str,
    h: &WireHeal,
    clock: (u64, u64),
    floor: u64,
) -> Option<(u64, Event)> {
    if pov_account(reg) == Some(from) {
        return None;
    }
    let sender = agent_of(reg, &Endpoint::Player(from.to_string()))?;
    let src = agent_of(reg, &h.src)?;
    let dst = agent_of(reg, &h.dst)?;
    let total = h.heal.saturating_add(h.barrier);
    let value = i32::try_from(total).unwrap_or(i32::MAX);
    if value <= 0 {
        return None;
    }
    let barrier = h.barrier.min(value.unsigned_abs());
    let t = local_time(h.t_unix_ms, clock, floor);
    Some((
        t,
        Event::RelayHeal {
            sender,
            src,
            dst,
            skill: h.skill,
            value,
            barrier,
            buff: h.buff,
            downed: h.downed,
        },
    ))
}

/// The local time for a peer's `t_unix_ms` (see [`incoming`]).
fn local_time(t_unix_ms: u64, clock: (u64, u64), floor: u64) -> u64 {
    let mapped = i128::from(clock.0) + i128::from(t_unix_ms) - i128::from(clock.1);
    u64::try_from(mapped.max(0)).unwrap_or(u64::MAX).max(floor)
}

/// The local event for a peer's own combat change, like [`incoming`]. `None` for the POV's
/// own batches or an unknown sender.
pub fn incoming_combat(
    reg: &Registry,
    from: &str,
    c: &WireCombat,
    clock: (u64, u64),
    floor: u64,
) -> Option<(u64, Event)> {
    if pov_account(reg) == Some(from) {
        return None;
    }
    let sender = agent_of(reg, &Endpoint::Player(from.to_string()))?;
    Some((
        local_time(c.t_unix_ms, clock, floor),
        Event::RelayCombat {
            sender,
            in_combat: c.in_combat,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::tests::{add, name_events};
    use axidps_protocol::v2::NameField;

    /// POV `:a.1` (id 1) and player `:b.2` (id 2).
    fn reg() -> Registry {
        let mut r = Registry::new();
        for (id, acc) in [(1, ":a.1"), (2, ":b.2")] {
            r.apply(0, &add(id, 1));
            for e in name_events(id, NameField::Account, acc) {
                r.apply(0, &e);
            }
        }
        r.apply(0, &Event::Pov { id: 1 });
        r
    }

    fn wire(src: &str, dst: &str, t_unix_ms: u64) -> WireHeal {
        WireHeal {
            t_unix_ms,
            src: Endpoint::Player(src.into()),
            dst: Endpoint::Player(dst.into()),
            skill: 9,
            heal: 300,
            barrier: 40,
            buff: true,
            downed: false,
        }
    }

    #[test]
    fn pov_account_known() {
        assert_eq!(pov_account(&reg()), Some(":a.1"));
        assert_eq!(pov_account(&Registry::new()), None);
    }

    #[test]
    fn outgoing_heal_from_event() {
        let mut r = reg();
        let heal = Event::Heal {
            src: 1,
            dst: 2,
            skill: 9,
            value: 340,
            barrier: 40,
            buff: true,
        };
        assert_eq!(
            outgoing(&r, 5_000, &heal),
            Some(wire(":a.1", ":b.2", 5_000))
        );
        r.apply(
            0,
            &Event::State {
                id: 2,
                state: AgentState::Down,
            },
        );
        let got = outgoing(&r, 5_000, &heal).unwrap();
        assert!(got.downed);
        // Not a heal, or an unknown endpoint: nothing.
        assert_eq!(outgoing(&r, 5_000, &Event::Pov { id: 1 }), None);
        let unknown = Event::Heal {
            src: 1,
            dst: 99,
            skill: 9,
            value: 1,
            barrier: 0,
            buff: false,
        };
        assert_eq!(outgoing(&r, 5_000, &unknown), None);
    }

    #[test]
    fn incoming_maps_to_relay_heal() {
        let r = reg();
        let h = WireHeal {
            downed: true,
            ..wire(":b.2", ":a.1", 1_000_100)
        };
        let got = incoming(&r, ":b.2", &h, (10_000, 1_000_000), 10_000);
        assert_eq!(
            got,
            Some((
                10_100,
                Event::RelayHeal {
                    sender: 2,
                    src: 2,
                    dst: 1,
                    skill: 9,
                    value: 340,
                    barrier: 40,
                    buff: true,
                    downed: true,
                }
            ))
        );
    }

    #[test]
    fn unknown_accounts_drop() {
        let r = reg();
        let clock = (10_000, 1_000_000);
        let h = wire(":zz.9", ":a.1", 1_000_000);
        assert_eq!(incoming(&r, ":b.2", &h, clock, 0), None);
        // An unknown sender drops too.
        let h = wire(":b.2", ":a.1", 1_000_000);
        assert_eq!(incoming(&r, ":zz.9", &h, clock, 0), None);
        // A zero heal is no heal.
        let h = WireHeal {
            heal: 0,
            barrier: 0,
            ..wire(":b.2", ":a.1", 1_000_000)
        };
        assert_eq!(incoming(&r, ":b.2", &h, clock, 0), None);
    }

    #[test]
    fn relay_times_never_go_backwards() {
        let r = reg();
        let h = wire(":b.2", ":a.1", 999_500);
        let (t, _) = incoming(&r, ":b.2", &h, (10_000, 1_000_000), 10_200).unwrap();
        assert_eq!(t, 10_200);
        // Before game time 0 clamps at the floor too.
        let h = wire(":b.2", ":a.1", 1);
        let (t, _) = incoming(&r, ":b.2", &h, (10, 1_000_000), 0).unwrap();
        assert_eq!(t, 0);
    }

    /// `reg()` plus an enemy player (id 7, prof 2, elite 18, team 9) and another of the same
    /// build (id 8), at x = 1000 and x = 3000.
    fn foes() -> (Registry, Positions) {
        let mut r = reg();
        let mut pos = Positions::default();
        for (id, x) in [(7, 1000.0f32), (8, 3000.0)] {
            r.apply(
                0,
                &Event::AgentAdd {
                    id,
                    kind: AgentKind::Player,
                    prof: 2,
                    elite: 18,
                    species: 0,
                    team: 9,
                    subgroup: 0,
                    wvw_rank: 0,
                    wvw_rank_def: 0,
                },
            );
            let at = |x: f32| Event::Motion {
                id,
                kind: MotionKind::Position,
                xyz: [x.to_bits(), 0f32.to_bits(), 0f32.to_bits()],
            };
            pos.note(10_000, &at(x));
            // Both move on later: the match is by where they were at the tick.
            pos.note(12_000, &at(x + 2000.0));
        }
        (r, pos)
    }

    fn tick(dst: Endpoint) -> WireCondi {
        WireCondi {
            t_unix_ms: 1_000_500,
            src: Endpoint::Player(":b.2".into()),
            dst,
            buff: 736,
            damage: 200,
            barrier: 20,
        }
    }

    #[test]
    fn outgoing_condi_names_enemies_by_build_and_place() {
        let (r, pos) = foes();
        let ev = |src, dst, value| Event::BuffDamage {
            src,
            dst,
            buff: 736,
            value,
            barrier: 20,
            stacks: 1,
        };
        assert_eq!(
            outgoing_condi(&r, &pos, 11_000, 5, &ev(1, 7, 200)),
            Some(WireCondi {
                t_unix_ms: 5,
                src: Endpoint::Player(":a.1".into()),
                dst: Endpoint::Foe {
                    prof: 2,
                    elite: 18,
                    team: 9,
                    xyz: [1000, 0, 0]
                },
                buff: 736,
                damage: 200,
                barrier: 20,
            })
        );
        // A named target is named.
        assert_eq!(
            outgoing_condi(&r, &pos, 11_000, 5, &ev(1, 2, 200))
                .unwrap()
                .dst,
            Endpoint::Player(":b.2".into())
        );
        // Ticks taken, by others, zero or on oneself are not shared.
        assert_eq!(outgoing_condi(&r, &pos, 11_000, 5, &ev(7, 1, 200)), None);
        assert_eq!(outgoing_condi(&r, &pos, 11_000, 5, &ev(2, 7, 200)), None);
        assert_eq!(outgoing_condi(&r, &pos, 11_000, 5, &ev(1, 7, 0)), None);
        assert_eq!(outgoing_condi(&r, &pos, 11_000, 5, &ev(1, 1, 50)), None);
    }

    #[test]
    fn incoming_condi_matches_the_nearest_foe_when_it_landed() {
        let (r, pos) = foes();
        let clock = (10_000, 1_000_000);
        let foe = |x| Endpoint::Foe {
            prof: 2,
            elite: 18,
            team: 9,
            xyz: [x, 0, 0],
        };
        // At 10_500 id 8 is at 3000: a peer seeing it near there names it.
        let got = incoming_condi(&r, &pos, ":b.2", &tick(foe(2900)), clock, 0);
        assert_eq!(
            got,
            Some((
                10_500,
                Event::RelayBuffDamage {
                    sender: 2,
                    src: 2,
                    dst: 8,
                    buff: 736,
                    value: 200,
                    barrier: 20,
                }
            ))
        );
        let dst = |x, floor| match incoming_condi(&r, &pos, ":b.2", &tick(foe(x)), clock, floor) {
            Some((_, Event::RelayBuffDamage { dst, .. })) => dst,
            other => panic!("{other:?}"),
        };
        assert_eq!(dst(1100, 0), 7);
        // Arriving late (floor 12_500) still matches where they were at 10_500.
        assert_eq!(dst(1100, 12_500), 7);
        // Nobody near, or another build: the tick still counts, with no target.
        assert_eq!(dst(2000, 0), 0);
        let other = Endpoint::Foe {
            prof: 3,
            elite: 18,
            team: 9,
            xyz: [1000, 0, 0],
        };
        assert!(matches!(
            incoming_condi(&r, &pos, ":b.2", &tick(other), clock, 0),
            Some((_, Event::RelayBuffDamage { dst: 0, .. }))
        ));
    }

    #[test]
    fn incoming_condi_only_from_the_senders_own() {
        let (r, pos) = foes();
        let clock = (10_000, 1_000_000);
        let foe = Endpoint::Foe {
            prof: 2,
            elite: 18,
            team: 9,
            xyz: [1000, 0, 0],
        };
        // :b.2 cannot report :a.1's ticks, nor :a.1 (the POV) its own.
        let theirs = WireCondi {
            src: Endpoint::Player(":a.1".into()),
            ..tick(foe.clone())
        };
        assert_eq!(incoming_condi(&r, &pos, ":b.2", &theirs, clock, 0), None);
        assert_eq!(
            incoming_condi(&r, &pos, ":a.1", &tick(foe.clone()), clock, 0),
            None
        );
        let zero = WireCondi {
            damage: 0,
            ..tick(foe)
        };
        assert_eq!(incoming_condi(&r, &pos, ":b.2", &zero, clock, 0), None);
    }

    #[test]
    fn positions_keep_a_short_history() {
        let mut pos = Positions::default();
        let at = |x: f32| Event::Motion {
            id: 1,
            kind: MotionKind::Position,
            xyz: [x.to_bits(), 0, 0],
        };
        for (t, x) in [(0, 0.0), (1_000, 1.0), (10_000, 10.0), (11_000, 11.0)] {
            pos.note(t, &at(x));
        }
        // Samples older than the window go, but the last one before it stays.
        assert_eq!(pos.at(1, 9_000).map(|p| p[0]), Some(1.0));
        assert_eq!(pos.at(1, 10_500).map(|p| p[0]), Some(10.0));
        assert_eq!(pos.at(1, 0).map(|p| p[0]), Some(1.0));
        pos.note(12_000, &Event::AgentRemove { id: 1 });
        assert_eq!(pos.at(1, 12_000), None);
    }

    #[test]
    fn pov_sender_ignored() {
        let r = reg();
        let h = wire(":b.2", ":a.1", 1_000_000);
        assert_eq!(incoming(&r, ":a.1", &h, (10_000, 1_000_000), 0), None);
    }
}
