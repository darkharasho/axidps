// Scratch (not committed): POV Bleeding/Burning model ticks matched to exact rows, per fight.
use axidps_core::condi::{GameMode, Modifier, PREREQ_READ, eval, mode_applies};
use axidps_core::processor::{FightOutcome, Processor};
use axidps_core::replay::{Player, apply};
use axidps_core::rules::{Trigger, WvwRules};
use axidps_protocol::v2::Event;
fn main() {
    let f = std::env::args().nth(1).unwrap();
    let buff: u32 = std::env::args().nth(2).map_or(736, |s| s.parse().unwrap());
    let verbose = std::env::args().nth(3).is_some();
    let mut p = Processor::new(WvwRules {
        trigger: Trigger::Any,
        min_squad: 0,
        min_duration_ms: 0,
        ..WvwRules::default()
    });
    let mut r = Player::new(std::io::BufReader::new(std::fs::File::open(&f).unwrap())).unwrap();
    let mut fights = vec![];
    let mut n = 0u64;
    loop {
        match r.next_step() {
            Ok(Some(s)) => {
                n += 1;
                if let Some(FightOutcome::Ended { fight, .. }) = apply(&mut p, &s) {
                    fights.push(fight);
                }
            }
            Ok(None) => {
                eprintln!("end after {n} steps");
                break;
            }
            Err(e) => {
                eprintln!("error after {n} steps: {e}");
                break;
            }
        }
    }
    if let Some(FightOutcome::Ended { fight, .. }) = p.flush() {
        fights.push(fight);
    }
    eprintln!("{} fights", fights.len());
    for fight in fights {
        let Some(pov) = fight.pov else { continue };
        let known = &fight.condi;
        let kind = known.damage_kind();
        let mode = GameMode::from_map_type(match fight.map {
            Some(Event::Map { map_type, .. }) => Some(map_type),
            _ => None,
        });
        let mods: Vec<Modifier> = known
            .modifiers(buff)
            .unwrap_or_default()
            .into_iter()
            .filter(|m| {
                Some(m.target) == kind && m.prereq == PREREQ_READ && mode_applies(m.mode, mode)
            })
            .collect();
        let st = known.stats(pov).copied();
        let per = st.map(|s| {
            mods.iter()
                .map(|m| eval(m, 80, &s).unwrap_or(0.0))
                .sum::<f32>()
        });
        let mut stacks = fight.buffs.clone();
        stacks.record_ticks();
        let mut model: Vec<(u64, u32, usize)> = vec![];
        let mut exact: Vec<(u64, u32, i32, i32)> = vec![];
        let mut tl: std::collections::BTreeMap<u32, Vec<(u64, String)>> = Default::default();
        let mut evs = fight.events.clone();
        evs.sort_by_key(|e| e.0);
        let sweep = |stacks: &mut axidps_core::buffs::Stacks,
                     model: &mut Vec<(u64, u32, usize)>| {
            for fi in stacks.take_fired() {
                let n = fi
                    .stacks
                    .iter()
                    .filter(|&&(s, b, _, _)| s == pov && b == buff)
                    .count();
                if fi.time >= fight.start_ms && n > 0 {
                    model.push((fi.time, fi.target, n));
                }
            }
        };
        for &(t, ev) in &evs {
            stacks.advance(t);
            sweep(&mut stacks, &mut model);
            stacks.apply(t, &ev);
            match ev {
                Event::State { id, state } => tl
                    .entry(id)
                    .or_default()
                    .push((t, format!("state {state:?}"))),
                Event::AgentRemove { id } => tl.entry(id).or_default().push((t, "removed".into())),
                Event::AgentAdd { id, .. } => tl.entry(id).or_default().push((t, "added".into())),
                Event::BuffApply {
                    dst,
                    src,
                    buff: b,
                    duration,
                    ..
                } if src == pov && b == buff => tl
                    .entry(dst)
                    .or_default()
                    .push((t, format!("apply dur {duration}"))),
                Event::BuffRemove { dst, .. } => {
                    tl.entry(dst).or_default().push((t, "remove-inst".into()))
                }
                Event::BuffRemoveAll { dst, buff: b, .. } if b == buff => {
                    tl.entry(dst).or_default().push((t, "remove-all".into()))
                }
                _ => {}
            }
            if let Event::BuffDamage {
                src,
                dst,
                buff: b,
                value,
                barrier,
                ..
            } = ev
                && src == pov
                && b == buff
            {
                exact.push((t, dst, value, barrier as i32));
            }
        }
        stacks.advance(fight.end_ms);
        sweep(&mut stacks, &mut model);
        eprintln!(
            "fight pov {pov} exact {} model {}",
            exact.len(),
            model.len()
        );
        if exact.is_empty() {
            continue;
        }
        // match each exact row to the nearest unused model tick on the same target within 1 s
        let mut used = vec![false; model.len()];
        let (mut matched, mut ex_un) = (0, 0);
        let (mut sv, mut sb, mut ms) = (0i64, 0i64, 0usize);
        let mut lines = vec![];
        for &(t, d, v, b) in &exact {
            let best = model
                .iter()
                .enumerate()
                .filter(|(i, m)| !used[*i] && m.1 == d && m.0.abs_diff(t) <= 1000)
                .min_by_key(|(_, m)| m.0.abs_diff(t))
                .map(|(i, _)| i);
            match best {
                Some(i) => {
                    used[i] = true;
                    matched += 1;
                    sv += v as i64;
                    sb += b as i64;
                    ms += model[i].2;
                    lines.push(format!(
                        "  dst {d} exact t+{} v={v} b={b} | model dt={} stacks={}",
                        t - fight.start_ms,
                        model[i].0 as i64 - t as i64,
                        model[i].2
                    ));
                }
                None => {
                    ex_un += 1;
                    lines.push(format!(
                        "  dst {d} exact t+{} v={v} b={b} | NO MODEL",
                        t - fight.start_ms
                    ));
                }
            }
        }
        let mod_un: usize = used.iter().filter(|u| !**u).count();
        println!(
            "fight {}s map mode {:?} pov {pov} stats {:?} per-stack {:?} mods {}",
            (fight.end_ms - fight.start_ms) / 1000,
            mode,
            st.map(|s| s.attrs),
            per,
            mods.len()
        );
        println!(
            "  exact rows {} matched {matched} exact-unmatched {ex_un} model-unmatched {mod_un}; matched: value {sv} barrier {sb} model stacks {ms} -> per stack exact {:.1}",
            exact.len(),
            (sv + sb) as f64 / ms.max(1) as f64
        );
        if verbose {
            for (i, m) in model.iter().enumerate() {
                if used[i] {
                    continue;
                }
                let near: Vec<String> = tl
                    .get(&m.1)
                    .map(|v| {
                        v.iter()
                            .filter(|e| e.0 + 3000 >= m.0 && e.0 <= m.0 + 1000)
                            .map(|e| format!("{:+} {}", e.0 as i64 - m.0 as i64, e.1))
                            .collect()
                    })
                    .unwrap_or_default();
                let last_ex = exact
                    .iter()
                    .filter(|e| e.1 == m.1 && e.0 <= m.0 + 1000)
                    .map(|e| e.0)
                    .max();
                println!(
                    "  UNMATCHED dst {} t+{} stacks {} last-exact {:?} | {}",
                    m.1,
                    m.0 - fight.start_ms,
                    m.2,
                    last_ex.map(|x| m.0 as i64 - x as i64),
                    near.join(", ")
                );
            }
        }
    }
}
