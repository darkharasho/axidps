// End to end against the deployed relay, no game: two clients join one random room, each sends one
// batch, each must receive the other's within 5 s. Prints `ok` and exits 0, else a message and 1.
use axidps_core::relay::client::{Cmd, Relay, WireHeal};
use axidps_core::relay::wire::{room_id, Endpoint};
use std::time::{Duration, Instant};

use axidps_core::relay::DEFAULT_RELAY_URL as URL;

fn fail(msg: &str) -> ! {
    eprintln!("relay_pair failed: {msg}");
    std::process::exit(1);
}

fn random_key() -> [u8; 16] {
    use std::hash::{BuildHasher, Hasher};
    let mut key = [0u8; 16];
    for half in key.chunks_mut(8) {
        // RandomState is seeded from the OS per instance.
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos()),
        );
        half.copy_from_slice(&h.finish().to_le_bytes());
    }
    key
}

fn heal(me: &str, skill: u32) -> WireHeal {
    WireHeal {
        t_unix_ms: 1_700_000_000_000,
        src: Endpoint::Player(me.into()),
        dst: Endpoint::Player(me.into()),
        skill,
        heal: 1000 + skill,
        barrier: 0,
        buff: false,
        downed: false,
    }
}

fn main() {
    let room = room_id(random_key()).unwrap_or_else(|| fail("zero room key"));
    let accounts = [":e2e.1", ":e2e.2"];
    let relays: Vec<Relay> = accounts.iter().map(|_| Relay::start(URL.into())).collect();
    for (i, r) in relays.iter().enumerate() {
        r.send(Cmd::Account(accounts[i].into()));
        r.send(Cmd::Room(Some(room.clone())));
        r.send(Cmd::Active(true));
    }
    // Let both sockets join before the single batch goes out, so each batch finds its peer.
    std::thread::sleep(Duration::from_secs(2));
    for (i, r) in relays.iter().enumerate() {
        r.send(Cmd::Heal(heal(accounts[i], 100 + i as u32)));
    }
    let start = Instant::now();
    let mut got = [false; 2];
    while start.elapsed() < Duration::from_secs(5) && !got.iter().all(|g| *g) {
        for (i, r) in relays.iter().enumerate() {
            let peer = accounts[1 - i];
            for b in r.try_recv() {
                if b.from == peer && b.heals.iter().any(|h| h.skill == 100 + (1 - i) as u32) {
                    got[i] = true;
                }
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    for (i, g) in got.iter().enumerate() {
        if !g {
            fail(&format!("client {} got nothing from its peer within 5 s", i + 1));
        }
    }
    println!("ok");
}
