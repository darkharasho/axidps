//! Src/dst master instids for the assembler, as arcdps's event finalizer writes them: every row
//! with a src (dst) agent gets that agent's current master, while the master is a tracked agent.

use std::collections::{BTreeMap, BTreeSet};

use axidps_evtc::Event as EvtcEvent;
use axidps_protocol::v2::Event;

use crate::registry::AgentInfo;

/// Masters as of the row being written, and every master a row named.
pub(crate) struct Masters {
    of: BTreeMap<u32, u32>,
    named: BTreeSet<u32>,
}

impl Masters {
    /// The masters known at the fight's start.
    pub(crate) fn new(snapshot: &[AgentInfo]) -> Self {
        Self {
            of: snapshot
                .iter()
                .filter_map(|a| a.master.map(|m| (a.id, m)))
                .collect(),
            named: BTreeSet::new(),
        }
    }

    fn master(&mut self, instid: u16, present: &BTreeSet<u32>) -> u16 {
        let Some(&m) = self.of.get(&u32::from(instid)).filter(|_| instid != 0) else {
            return 0;
        };
        let Ok(m16) = u16::try_from(m) else { return 0 };
        if !present.contains(&m) {
            return 0;
        }
        self.named.insert(m);
        m16
    }

    /// Write the master instids of `rows` (rows a producer already gave one are left alone).
    /// `present`: the agents tracked now; a master that is not gets no row.
    pub(crate) fn fill(&mut self, rows: &mut [EvtcEvent], present: &BTreeSet<u32>) {
        for e in rows {
            if e.src_master_instid == 0 {
                e.src_master_instid = self.master(e.src_instid, present);
            }
            if e.dst_master_instid == 0 {
                e.dst_master_instid = self.master(e.dst_instid, present);
            }
        }
    }

    /// Follow `ev`, after its rows were filled: a despawn's rows keep the master.
    pub(crate) fn apply(&mut self, ev: &Event) {
        match *ev {
            Event::AgentMaster { id, master: 0 } | Event::AgentRemove { id } => {
                self.of.remove(&id);
            }
            Event::AgentMaster { id, master } | Event::AttackTarget { id, parent: master } => {
                self.of.insert(id, master);
            }
            _ => {}
        }
    }

    /// Every master a row named: the agent table lists them.
    pub(crate) fn named(&self) -> impl Iterator<Item = u32> + '_ {
        self.named.iter().copied()
    }
}
