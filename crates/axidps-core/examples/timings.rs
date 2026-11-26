// Scratch: print `skill start_value full_ms` for every table skill with timing (not committed).
fn main() {
    for s in axidps_core::content::skills() {
        if let Some(t) = axidps_core::content::timing(s) {
            println!(
                "{s} {} {} {}",
                t.start_value(),
                t.full_ms,
                u8::from(t.speed_immune)
            );
        }
    }
}
