// Scratch (not committed): count recorded event kinds.
use axidps_core::replay::{Player, Step};
use std::collections::BTreeMap;
fn main() {
    let mut h: BTreeMap<String, u32> = BTreeMap::new();
    for f in std::env::args().skip(1) {
        let mut p = Player::new(std::io::BufReader::new(std::fs::File::open(&f).unwrap())).unwrap();
        while let Ok(Some(s)) = p.next_step() {
            let k = match s {
                Step::Push(_, e) | Step::Prime(_, e) => format!("{e:?}")
                    .split([' ', '{', '('])
                    .next()
                    .unwrap()
                    .to_string(),
                o => format!("{o:?}").chars().take(6).collect(),
            };
            *h.entry(k).or_default() += 1;
        }
    }
    for (k, n) in h {
        println!("{k} {n}");
    }
}
