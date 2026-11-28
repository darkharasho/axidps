//! WvW objective status (EVTC sc 75): the EVTC per-objective cache.
//! Keyed by the claimable's GUID and never
//! cleared (entries outlive map changes; the map test filters them).

use std::collections::BTreeMap;

use axidps_protocol::v2::Event;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Objective {
    pub owner: u32,
    /// 0 until a progress message gave it one.
    pub map: u32,
    pub id: u32,
    pub kind: u8,
    pub progress: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Objectives {
    entries: BTreeMap<[u8; 16], Objective>,
}

impl Objectives {
    /// Apply a `WvwObjective` on map `current`; returns the entry when EVTC logs carry a row: a
    /// progress message whose map is the current one, or an owner message for an entry a
    /// progress message already put on the current map.
    pub fn apply(&mut self, ev: &Event, current: Option<u32>) -> Option<Objective> {
        let Event::WvwObjective {
            op,
            guid,
            map_id,
            objective_id,
            objective_type,
            value,
        } = *ev
        else {
            return None;
        };
        let e = self.entries.entry(guid).or_default();
        match op {
            0 => {
                if objective_id != 0 {
                    e.id = objective_id;
                    e.kind = objective_type;
                }
                e.map = map_id;
                e.progress = value;
            }
            1 => e.owner = value,
            _ => return None,
        }
        (e.map != 0 && Some(e.map) == current).then_some(*e)
    }

    /// The entries EVTC logs carry again at a WvW log start: those on map `current`.
    pub fn on_map(&self, current: u32) -> impl Iterator<Item = Objective> + '_ {
        self.entries
            .values()
            .filter(move |e| e.map != 0 && e.map == current)
            .copied()
    }
}

/// Team id and world id per colour (red, blue, green), last seen wins and never reset, like
/// The EVTC globals behind sc 74. A colour never seen stays 0, as EVTC logs carry it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Teams {
    pub teams: [u32; 3],
    pub worlds: [u32; 3],
}

impl Teams {
    pub fn apply(&mut self, ev: &Event) {
        let (slots, colour, v) = match *ev {
            Event::WvwTeam { team, colour } => (&mut self.teams, colour, team),
            Event::WvwWorld { colour, world } => (&mut self.worlds, colour, world),
            _ => return,
        };
        if let Some(slot) = usize::from(colour)
            .checked_sub(1)
            .and_then(|i| slots.get_mut(i))
        {
            *slot = v;
        }
    }

    /// The EVTC sc 74 fields `(src, dst, value, buff_dmg)`: red | blue world << 32,
    /// green world | red team << 32, blue team, green team.
    pub fn row(&self) -> Option<(u64, u64, i32, i32)> {
        let [r, b, g] = self.teams;
        let [wr, wb, wg] = self.worlds;
        (*self != Self::default()).then_some((
            u64::from(wr) | u64::from(wb) << 32,
            u64::from(wg) | u64::from(r) << 32,
            b as i32,
            g as i32,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn teams_by_colour() {
        let mut t = Teams::default();
        assert_eq!(t.row(), None);
        t.apply(&Event::WvwTeam {
            team: 708,
            colour: 1,
        });
        t.apply(&Event::WvwTeam {
            team: 2780,
            colour: 3,
        });
        t.apply(&Event::WvwTeam { team: 9, colour: 0 });
        assert_eq!(t.row(), Some((0, 708 << 32, 0, 2780)));
        t.apply(&Event::WvwWorld {
            colour: 1,
            world: 11005,
        });
        t.apply(&Event::WvwWorld {
            colour: 2,
            world: 11001,
        });
        t.apply(&Event::WvwWorld {
            colour: 3,
            world: 11006,
        });
        t.apply(&Event::WvwWorld {
            colour: 4,
            world: 1,
        });
        assert_eq!(
            t.row(),
            Some((11005 | 11001 << 32, 11006 | 708 << 32, 0, 2780)),
            "the 20261124 logs' row, blue team aside"
        );
    }

    fn ev(op: u8, g: u8, map_id: u32, objective_id: u32, value: u32) -> Event {
        Event::WvwObjective {
            op,
            guid: [g; 16],
            map_id,
            objective_id,
            objective_type: 4,
            value,
        }
    }

    #[test]
    fn rows_follow_evtc_layout() {
        let mut o = Objectives::default();
        assert_eq!(
            o.apply(&ev(1, 1, 0, 0, 708), Some(38)),
            None,
            "owner before any map"
        );
        let r = o.apply(&ev(0, 1, 38, 16, 70), Some(38)).unwrap();
        assert_eq!(
            (r.owner, r.map, r.id, r.kind, r.progress),
            (708, 38, 16, 4, 70)
        );
        assert_eq!(
            o.apply(&ev(1, 1, 0, 0, 2780), Some(38)).unwrap().owner,
            2780
        );
        assert_eq!(o.apply(&ev(0, 2, 95, 33, 5), Some(38)), None, "another map");
        assert_eq!(
            o.apply(&ev(0, 1, 38, 0, 71), Some(38)).unwrap().id,
            16,
            "id kept"
        );
        assert_eq!(o.on_map(38).count(), 1);
        assert_eq!(o.on_map(95).map(|e| e.id).collect::<Vec<_>>(), vec![33]);
    }
}
