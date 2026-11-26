// Scratch: buffs whose BUFFINFO (sc 30) row has is_flanking / is_shields set (not committed).
fn main() {
    for s in axidps_core::content::skills() {
        for e in axidps_core::content::rows(s) {
            if e.is_statechange == 30 && (e.is_flanking != 0 || e.is_shields != 0) {
                println!(
                    "{s} flank {} shields {} cat {}",
                    e.is_flanking, e.is_shields, e.is_offcycle
                );
            }
        }
    }
}
