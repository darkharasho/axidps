// Scratch: dump the game's buff modifiers for the buff ids given (from a recording).
use axidps_core::processor::{FightOutcome, Processor};
use axidps_core::replay::{Player, apply};
use axidps_core::rules::{Trigger, WvwRules};
fn main() {
    let mut a = std::env::args().skip(1);
    let f = a.next().unwrap();
    let ids: Vec<u32> = a.map(|s| s.parse().unwrap()).collect();
    let mut p = Processor::new(WvwRules {
        trigger: Trigger::Any,
        min_squad: 0,
        min_duration_ms: 0,
        ..WvwRules::default()
    });
    let mut r = Player::new(std::io::BufReader::new(std::fs::File::open(&f).unwrap())).unwrap();
    let mut last = None;
    while let Ok(Some(s)) = r.next_step() {
        if let Some(FightOutcome::Ended { fight, .. }) = apply(&mut p, &s) {
            last = Some(fight);
        }
    }
    if let Some(FightOutcome::Ended { fight, .. }) = p.flush() {
        last = Some(fight);
    }
    let fight = last.unwrap();
    println!("damage kind {:?}", fight.condi.damage_kind());
    for id in ids {
        println!("{id}: {:?}", fight.condi.modifiers(id));
    }
}
