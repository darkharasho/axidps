//! The live meter's condition damage estimate for players whose ticks nobody reports: the game
//! gives exact ticks only for the local player, and squadmates running axidps relay theirs. For
//! everyone else the condition model (`crate::condi`) estimates each tick from the stacks they
//! hold on their targets and their gear, as the log's model does at the fight's end.

use axidps_protocol::v2::Event;

use crate::buffs::{Stacks, phase_source};
use crate::condi::{GameMode, Known, Model, Tick};
use crate::registry::Registry;

/// One fight's estimator: its own copy of the buff stacks and the model.
#[derive(Debug, Clone)]
pub struct Estimator {
    stacks: Stacks,
    model: Model,
    /// The game time the stacks were last advanced to.
    at: u64,
}

impl Estimator {
    /// Start from the stacks and formulas known as the fight starts (after the step that
    /// started it).
    pub fn new(stacks: &Stacks, known: &Known, pov: Option<u32>, mode: GameMode) -> Self {
        let mut stacks = stacks.clone();
        stacks.record_ticks();
        Self {
            stacks,
            model: Model::new(known.clone(), pov, mode),
            at: 0,
        }
    }

    /// Follow one event at game time `t`: the estimated ticks due before it.
    pub fn event(&mut self, reg: &Registry, t: u64, ev: &Event) -> Vec<Tick> {
        // An exact tick of the POV's sets that target's tick phase, as for the log.
        if let Event::BuffDamage { dst, buff, .. } = *ev
            && phase_source(buff)
        {
            self.stacks.sync_tick(dst, t);
            self.at = 0;
        }
        let ticks = self.advance(reg, t);
        self.model.known.apply(ev);
        match *ev {
            Event::BuffApply { .. }
            | Event::BuffRemove { .. }
            | Event::BuffRemoveAll { .. }
            | Event::BuffStack { .. } => {
                self.stacks.apply(t, ev);
            }
            Event::AgentRemove { id } => self.stacks.forget_agent(id),
            _ => {}
        }
        ticks
    }

    /// Game time moved on to `t`: the estimated ticks due by then. None on a target holding an
    /// invulnerability (its ticks do no damage).
    pub fn advance(&mut self, reg: &Registry, t: u64) -> Vec<Tick> {
        // Most events share their time with the one before: nothing new is due.
        if t == self.at {
            return Vec::new();
        }
        self.at = t;
        self.stacks.advance(t);
        let master = |id: u32| reg.agent(id).and_then(|a| a.master);
        let mut out = Vec::new();
        for fired in self.stacks.take_fired() {
            if self.stacks.holds(fired.target, |b| {
                let (invulnerable, invert) = crate::content::buff_flags(b);
                invulnerable || invert
            }) {
                continue;
            }
            let stacks = &self.stacks;
            out.extend(
                self.model
                    .tick(&fired, master, |id| stacks.active_on(id, fired.time))
                    .into_iter()
                    .filter(|t| t.damage > 0),
            );
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::condi::tests::known;
    use crate::registry::tests::add;

    fn bleed(dst: u32, src: u32, inst: u32, duration: u32) -> Event {
        Event::BuffApply {
            dst,
            src,
            buff: 736,
            inst,
            duration,
            level: 80,
            active: true,
        }
    }

    /// Bleeding from player 10 (stats known) ticks once a second while it lasts; the POV's own
    /// (1) is exact already and not estimated.
    #[test]
    fn estimates_ticks_of_known_players() {
        let mut reg = Registry::new();
        for id in [1, 10, 50, 51] {
            reg.apply(0, &add(id, 1));
        }
        let mut e = Estimator::new(&Stacks::default(), &known(), Some(1), GameMode::Wvw);
        let mut ticks = e.event(&reg, 1_000, &bleed(50, 10, 1, 3_000));
        ticks.extend(e.event(&reg, 1_000, &bleed(50, 1, 2, 3_000)));
        ticks.extend(e.advance(&reg, 10_000));
        assert_eq!(ticks.len(), 3, "{ticks:?}");
        assert!(
            ticks
                .iter()
                .all(|t| t.src == 10 && t.dst == 50 && t.damage > 0)
        );
        let first = ticks[0].damage;
        assert!(ticks.iter().all(|t| t.damage == first));
        // A stack cleansed early stops ticking.
        let mut e = Estimator::new(&Stacks::default(), &known(), Some(1), GameMode::Wvw);
        let mut ticks = e.event(&reg, 1_000, &bleed(51, 10, 3, 10_000));
        let remove = Event::BuffRemove {
            dst: 51,
            remover: 1,
            inst: 3,
        };
        ticks.extend(e.event(&reg, 2_500, &remove));
        ticks.extend(e.advance(&reg, 10_000));
        assert_eq!(ticks.len(), 1, "{ticks:?}");
    }
}
