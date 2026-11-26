// Scratch: print recorded non-strike events touching given instance ids (not committed).
use axidps_core::replay::{Player, Step};
use axidps_protocol::v2::Event;
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let ids: Vec<u32> = a[2..].iter().map(|s| s.parse().unwrap()).collect();
    let mut p = Player::new(std::io::BufReader::new(std::fs::File::open(&a[1]).unwrap())).unwrap();
    while let Some(s) = p.next_step().unwrap() {
        let (t, ev, tag) = match s {
            Step::Push(t, e) => (t, e, "push"),
            Step::Prime(t, e) => (t, e, "prime"),
            Step::Flush => {
                println!("FLUSH");
                continue;
            }
            _ => continue,
        };
        let hit = match ev {
            Event::AgentAdd { id, .. }
            | Event::AgentRemove { id }
            | Event::Combat { id, .. }
            | Event::Team { id, .. }
            | Event::State { id, .. } => ids.contains(&id),
            Event::LogControl { .. } => true,
            _ => false,
        };
        if hit {
            println!("{t} {tag} {ev:?}");
        }
    }
}
