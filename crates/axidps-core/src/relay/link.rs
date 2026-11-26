//! Core's side of the relay: local heals to wire heals, and peer wire heals to local events.

use axidps_protocol::v2::{AgentState, Event};

use super::wire::{Endpoint, WireHeal, agent_of, endpoint_of};
use crate::registry::Registry;

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
    let mapped = i128::from(clock.0) + i128::from(h.t_unix_ms) - i128::from(clock.1);
    let t = u64::try_from(mapped.max(0)).unwrap_or(u64::MAX).max(floor);
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

    #[test]
    fn pov_sender_ignored() {
        let r = reg();
        let h = wire(":b.2", ":a.1", 1_000_000);
        assert_eq!(incoming(&r, ":a.1", &h, (10_000, 1_000_000), 0), None);
    }
}
