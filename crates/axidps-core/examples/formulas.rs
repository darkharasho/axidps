// Scratch (not committed): modifiers of the buff ids given after `--`, from recordings.
use axidps_core::replay::{Player, Step};
use axidps_protocol::v2::Event;
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let split = args.iter().position(|a| a == "--").unwrap_or(args.len());
    let ids: Vec<u32> = args
        .get(split + 1..)
        .unwrap_or(&[])
        .iter()
        .map(|s| s.parse().unwrap())
        .collect();
    let mut seen = std::collections::BTreeSet::new();
    for f in &args[..split] {
        let mut r = Player::new(std::io::BufReader::new(std::fs::File::open(f).unwrap())).unwrap();
        while let Ok(Some(s)) = r.next_step() {
            let ev = match s {
                Step::Push(_, e) | Step::Prime(_, e) => e,
                _ => continue,
            };
            if let Event::BuffFormula {
                buff,
                index,
                count,
                category,
                ftype,
                target,
                base,
                c1,
                c2,
                mode,
                prereq,
            } = ev
                && (ids.is_empty() || ids.contains(&buff))
                && seen.insert((buff, index))
            {
                println!(
                    "{buff} cat={category} {index}/{count} ftype={ftype} target={target:#x} base={} c1={} c2={} mode={mode} prereq={prereq:#x}",
                    f32::from_bits(base),
                    f32::from_bits(c1),
                    f32::from_bits(c2)
                );
            }
        }
    }
}
