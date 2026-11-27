// Scratch (not committed): POV stats, formulas, and model vs exact per tick for 736/723.
use axidps_core::buffs::Stacks;
use axidps_core::condi::{Known, eval};
use axidps_core::replay::{Player, Step};
use axidps_protocol::v2::Event;
fn main() {
    let f = std::env::args().nth(1).unwrap();
    let mut r = Player::new(std::io::BufReader::new(std::fs::File::open(&f).unwrap())).unwrap();
    let mut known = Known::default();
    let mut pov = 0u32;
    let mut stacks = Stacks::default();
    stacks.record_ticks();
    let mut shown = 0;
    let mut last_exact: Vec<(u64, u32, u32, i32)> = vec![];
    loop {
        let s = match r.next_step() {
            Ok(Some(s)) => s,
            _ => break,
        };
        let (t, ev) = match s {
            Step::Push(t, e) | Step::Prime(t, e) => (t, e),
            _ => continue,
        };
        stacks.advance(t);
        for fi in stacks.take_fired() {
            let mine: Vec<_> = fi
                .stacks
                .iter()
                .filter(|&&(s, b, _, _)| s == pov && (b == 736 || b == 723))
                .collect();
            if mine.is_empty() || shown > 60 {
                continue;
            }
            shown += 1;
            let kind = known.damage_kind();
            let st = known.stats(pov).copied();
            let mut sum = 0.0;
            for &&(_, b, lvl, _) in &mine {
                if let (Some(k), Some(mods), Some(st)) = (kind, known.modifiers(b), st) {
                    for m in mods.iter().filter(|m| m.target == k) {
                        sum += eval(m, lvl, &st).unwrap_or(f32::NAN);
                    }
                }
            }
            println!(
                "tick t={} dst={} stacks={:?} model={sum:.1} recent_exact={:?}",
                fi.time,
                fi.target,
                mine,
                last_exact
                    .iter()
                    .filter(|e| e.2 == fi.target)
                    .collect::<Vec<_>>()
            );
            last_exact.clear();
        }
        stacks.apply(t, &ev);
        known.apply(&ev);
        match ev {
            Event::Pov { id } => pov = id,
            Event::CoreStats { id, level, attrs } if id == pov => {
                println!("stats t={t} lvl={level} attrs={attrs:?}")
            }
            Event::BuffFormula {
                buff: b @ (736 | 723),
                index,
                count,
                ftype,
                target,
                base,
                c1,
                c2,
                mode,
                ..
            } => println!(
                "formula {b} {index}/{count} ftype={ftype} target={target:#x} base={} c1={} c2={} mode={mode}",
                f32::from_bits(base),
                f32::from_bits(c1),
                f32::from_bits(c2)
            ),
            Event::BuffDamage {
                src,
                dst,
                buff: b @ (736 | 723),
                value,
                ..
            } if src == pov => last_exact.push((t, b, dst, value)),
            Event::BuffApply {
                src,
                buff: 736 | 723,
                ..
            } if src == pov && shown < 60 => println!("apply t={t} {ev:?}"),
            _ => {}
        }
    }
    println!("kind={:?}", known.damage_kind());
}
