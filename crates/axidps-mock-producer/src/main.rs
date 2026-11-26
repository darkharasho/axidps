//! `axidps-mock-producer`: writes a fake WvW v2 event stream to the ring in real time.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axidps_mock_producer::{produce, Config};
use axidps_protocol::v2::{ring_path, RingHost};
use axidps_ringbuf::v2::EventWriter;
use clap::Parser;

#[derive(Parser)]
#[command(name = "axidps-mock-producer", about = "Fake WvW events on the axidps v2 ring")]
struct Args {
    /// Ring path (default: the platform's standard ring).
    #[arg(long)]
    ring: Option<PathBuf>,
    #[arg(long, default_value_t = 1)]
    seed: u64,
    #[arg(long, default_value_t = 1)]
    fights: u32,
    #[arg(long, default_value_t = 10)]
    squad: u32,
    #[arg(long, default_value_t = 15)]
    enemies: u32,
}

fn main() -> std::io::Result<()> {
    let args = Args::parse();
    let path = args.ring.unwrap_or_else(|| ring_path(RingHost::current_build()));
    let writer = EventWriter::create_at(&path)?;
    eprintln!("writing to {}", path.display());

    let stop = Arc::new(AtomicBool::new(false));
    let s = stop.clone();
    let _ = ctrlc::set_handler(move || s.store(true, Ordering::SeqCst));

    let unix_s = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    let cfg = Config { seed: args.seed, fights: args.fights, squad: args.squad, enemies: args.enemies, unix_s };
    let started = Instant::now();
    let t0 = 1_000;
    produce(&cfg, t0, &mut |t, ev| {
        if stop.load(Ordering::SeqCst) {
            return;
        }
        let due = Duration::from_millis(t - t0);
        if let Some(wait) = due.checked_sub(started.elapsed()) {
            std::thread::sleep(wait);
        }
        writer.push(t, ev);
    });
    writer.mark_inactive();
    // Keep the ring file around briefly so a reader can drain it; the writer unlinks on drop.
    std::thread::sleep(Duration::from_secs(2));
    Ok(())
}
