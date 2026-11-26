// Scratch (not committed): P3, level byte of condition applies by source kind.
use axidps_core::replay::{Player, Step};
use axidps_protocol::v2::{AgentKind, Event};
use std::collections::{BTreeMap, HashMap};
fn main() {
    let mut kinds: HashMap<u32, AgentKind> = HashMap::new();
    let mut pov = 0u32;
    let mut hist: BTreeMap<(String, u32, u8), u32> = BTreeMap::new();
    for f in std::env::args().skip(1) {
        let mut p = Player::new(std::io::BufReader::new(std::fs::File::open(&f).unwrap())).unwrap();
        loop {
            let s = match p.next_step() {
                Ok(Some(s)) => s,
                Ok(None) => break,
                Err(e) => {
                    eprintln!("err {e}");
                    break;
                }
            };
            let ev = match s {
                Step::Push(_, e) | Step::Prime(_, e) => e,
                _ => continue,
            };
            match ev {
                Event::AgentAdd { id, kind, .. } => {
                    kinds.insert(id, kind);
                }
                Event::Pov { id } => pov = id,
                Event::BuffApply {
                    src,
                    buff,
                    level,
                    active,
                    ..
                } if true => {
                    let who = if src == pov {
                        "pov".into()
                    } else {
                        format!("{:?}", kinds.get(&src))
                    };
                    let _ = active;
                    *hist.entry((who, buff, level)).or_default() += 1;
                }
                _ => {}
            }
        }
    }
    for (k, n) in hist {
        println!("{k:?} {n}");
    }
}
