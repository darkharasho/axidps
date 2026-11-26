// Scratch (not committed): Health events by agent kind/team in a recording.
use axidps_core::replay::{Player, Step};
use axidps_protocol::v2::{AgentKind, Event};
use std::collections::{BTreeMap, HashMap};
fn main() {
    let f = std::env::args().nth(1).unwrap();
    let mut r = Player::new(std::io::BufReader::new(std::fs::File::open(&f).unwrap())).unwrap();
    let mut kind: HashMap<u32, (AgentKind, u32)> = HashMap::new();
    let mut team: HashMap<u32, u32> = HashMap::new();
    let mut c: BTreeMap<String, (u32, std::collections::BTreeSet<u32>)> = BTreeMap::new();
    let mut pov = 0;
    while let Ok(Some(s)) = r.next_step() {
        let ev = match s {
            Step::Push(_, e) | Step::Prime(_, e) => e,
            _ => continue,
        };
        match ev {
            Event::AgentAdd {
                id,
                kind: k,
                team: t,
                ..
            } => {
                kind.insert(id, (k, t));
            }
            Event::Team { id, team: t } => {
                team.insert(id, t);
            }
            Event::Pov { id } => pov = id,
            Event::Health { id, max, .. } => {
                if max == 0 {
                    *c.entry("max 0".into()).or_default() =
                        (c.get("max 0").map_or(0, |e| e.0) + 1, Default::default());
                }
                let (k, t0) = kind.get(&id).copied().unwrap_or((AgentKind::Unknown, 0));
                let t = team.get(&id).copied().unwrap_or(t0);
                let pt = team.get(&pov).copied().unwrap_or(0);
                let key = format!(
                    "{k:?} {}",
                    if id == pov {
                        "pov"
                    } else if t == pt {
                        "same team"
                    } else {
                        "other team"
                    }
                );
                let e = c.entry(key).or_default();
                e.0 += 1;
                e.1.insert(id);
            }
            _ => {}
        }
    }
    for (k, (n, ids)) in c {
        println!("{k}: {n} events, {} agents", ids.len());
    }
}
