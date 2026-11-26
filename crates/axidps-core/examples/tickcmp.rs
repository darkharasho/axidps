// Scratch (not committed): model tick times vs exact POV Bleeding rows, per target.
use axidps_core::processor::{FightOutcome, Processor};
use axidps_core::replay::{Player, apply};
use axidps_core::rules::{Trigger, WvwRules};
use axidps_protocol::v2::Event;
use std::collections::BTreeMap;
fn main() {
    let f = std::env::args().nth(1).unwrap();
    let mut p = Processor::new(WvwRules {
        trigger: Trigger::Any,
        min_squad: 0,
        min_duration_ms: 0,
        ..WvwRules::default()
    });
    let mut r = Player::new(std::io::BufReader::new(std::fs::File::open(&f).unwrap())).unwrap();
    let mut fights = vec![];
    while let Ok(Some(s)) = r.next_step() {
        if let Some(FightOutcome::Ended { fight, .. }) = apply(&mut p, &s) {
            fights.push(fight);
        }
    }
    let fight = fights.into_iter().max_by_key(|f| f.events.len()).unwrap();
    let pov = fight.pov.unwrap();
    let mut stacks = fight.buffs.clone();
    stacks.record_ticks();
    let mut model: BTreeMap<u32, Vec<u64>> = BTreeMap::new();
    let mut exact: BTreeMap<u32, Vec<u64>> = BTreeMap::new();
    let mut evs = fight.events.clone();
    evs.sort_by_key(|e| e.0);
    let sweep = |stacks: &mut axidps_core::buffs::Stacks,
                     model: &mut BTreeMap<u32, Vec<u64>>| {
        for fi in stacks.take_fired() {
            if fi.time >= fight.start_ms && fi.stacks.iter().any(|&(s, b, _)| s == pov && b == 736)
            {
                model.entry(fi.target).or_default().push(fi.time);
            }
        }
    };
    for &(t, ev) in &evs {
        stacks.advance(t);
        sweep(&mut stacks, &mut model);
        stacks.apply(t, &ev);
        if let Event::BuffDamage {
            src,
            dst,
            buff: 736,
            ..
        } = ev
        {
            if src == pov {
                exact.entry(dst).or_default().push(t);
            }
        }
    }
    stacks.advance(fight.end_ms);
    sweep(&mut stacks, &mut model);
    for (d, ex) in &exact {
        let m = model.get(d).cloned().unwrap_or_default();
        let fmt = |v: &[u64]| {
            v.iter()
                .map(|t| format!("{}", t - fight.start_ms))
                .collect::<Vec<_>>()
                .join(" ")
        };
        if m.len() != ex.len() {
            println!(
                "dst {d}: model {} exact {}\n  M {}\n  E {}",
                m.len(),
                ex.len(),
                fmt(&m),
                fmt(ex)
            );
        }
    }
}
