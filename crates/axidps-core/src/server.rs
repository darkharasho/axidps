//! The game server's tick, extrapolated as the EVTC format expects, and the server values that ride with it
//! (EVTC sc 84 TICK, 42 INSTANCESTART, 16 SHARDID).
//!
//! The tick state lives in one object: every agent-action message stores its tick,
//! and it extrapolates from the last one at a rate
//! measured over a few hundred ms, capped at 640 ms before falling back to the nominal 40 ms per
//! tick. axidps gets the tick from `Event::ServerTick` (at most every 100 ms, with the event's
//! push time as "now") and runs the same arithmetic on ring milliseconds.

use axidps_protocol::v2::Event;

/// Nominal server tick length: 25 ticks per second.
pub const MS_PER_TICK: u32 = 40;
/// Extrapolation uses the measured rate for at most this long after the last real tick.
const MEASURED_MS: u64 = 640;
/// A sc 84 row each time the extrapolated tick passes the next multiple of this many ticks.
pub const ROW_TICKS: u32 = 25;

/// The EVTC tick state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TickClock {
    /// Last real tick.
    tick: u32,
    /// When it arrived; 0 = none since the last reset.
    at: u64,
    /// Rate baseline (newer).
    base_at: u64,
    base_tick: u32,
    /// Rate baseline (older).
    prev_at: u64,
    prev_tick: u32,
}

impl TickClock {
    /// A real tick at `now`; tick 0 resets, as at a map load.
    pub fn real(&mut self, now: u64, tick: u32) {
        if tick == 0 {
            self.reset();
            return;
        }
        self.at = now;
        self.tick = tick;
        if self.base_at.wrapping_add(200) < now {
            if self.prev_at.wrapping_add(300) < self.base_at {
                self.prev_tick = self.base_tick;
                self.prev_at = self.base_at;
            }
            self.base_at = now;
            self.base_tick = tick;
        }
    }

    /// The EVTC reset: the times and both baseline ticks go to 0; the last tick stays.
    pub fn reset(&mut self) {
        self.at = 0;
        self.base_at = 0;
        self.prev_at = 0;
        self.base_tick = 0;
        self.prev_tick = 0;
    }

    /// A real tick arrived since the last reset.
    pub fn seeded(&self) -> bool {
        self.at != 0
    }

    /// `(extrapolated tick, ticks extrapolated since the last real one)` at `now`,
    /// with the EVTC integer widths.
    pub fn extrapolate(&self, now: u64) -> (u32, u32) {
        let dticks = u64::from(self.base_tick.wrapping_sub(self.prev_tick).max(1));
        let ms_per_tick =
            u64::from(((self.base_at.wrapping_sub(self.prev_at) / dticks) as u32).max(1));
        let e = now.wrapping_sub(self.at);
        let measured = (e.min(MEASURED_MS) / ms_per_tick) as u32;
        let nominal = if e > MEASURED_MS {
            ((e - MEASURED_MS) & 0xffff_ffff) as u32 / MS_PER_TICK // public-check:ok
        } else {
            0
        };
        let since = measured.wrapping_add(nominal);
        (self.tick.wrapping_add(since), since)
    }
}

/// The latest server values, kept in and out of fights.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Server {
    pub clock: TickClock,
    /// Last ping ms (0 = unread).
    pub ping: u32,
    /// Game server IPv4 as stored in a `sockaddr_in` (0 = unread).
    pub server_ip: u32,
    /// Map-load shard ids: low 16 bits sc 16 src, high 16 bits dst (0 = unread).
    pub shard_ids: u32,
}

impl Server {
    /// Take a `ServerTick` (other events are ignored). Zero values mean unread and keep the
    /// previous one.
    pub fn apply(&mut self, time: u64, ev: &Event) {
        if let Event::ServerTick {
            tick,
            ping,
            server_ip,
            shard_ids,
        } = *ev
        {
            if tick != 0 {
                self.clock.real(time, tick);
            }
            for (v, new) in [
                (&mut self.ping, ping),
                (&mut self.server_ip, server_ip),
                (&mut self.shard_ids, shard_ids),
            ] {
                if new != 0 {
                    *v = new;
                }
            }
        }
    }

    /// A map load: EVTC logs reset the tick; the shard ids and address belong to the old map.
    pub fn map_loaded(&mut self) {
        self.clock.reset();
        self.server_ip = 0;
        self.shard_ids = 0;
    }
}

/// sc 84 rows for one log, as EVTC logs: `next` is set to the
/// extrapolated tick at log start. Each evaluation writes at most one row, when the
/// extrapolated tick is past `next`, and moves `next` on by [`ROW_TICKS`].
#[derive(Debug, Clone, Copy)]
pub struct TickRows {
    next: u32,
}

impl TickRows {
    pub fn new(start_tick: u32) -> Self {
        Self { next: start_tick }
    }

    /// `(src, dst)` of the row due at `now`, if any: the extrapolated tick and the ticks
    /// extrapolated since the last real one.
    pub fn step(&mut self, clock: &TickClock, now: u64) -> Option<(u32, u32)> {
        let (ext, since) = clock.extrapolate(now);
        (self.next < ext).then(|| {
            self.next = self.next.wrapping_add(ROW_TICKS);
            (ext, since)
        })
    }
}

/// sc 42's src: the instance start in the log's clock, `start - (u32)(tick * 40)`, wrapping
/// as in EVTC logs (negative when the instance is older than the clock's zero).
pub fn instance_start(start_ms: u64, start_tick: u32) -> u64 {
    start_ms.wrapping_sub(u64::from(start_tick.wrapping_mul(MS_PER_TICK)))
}

/// The tick at `start_ms`, back-extrapolated at the nominal rate from a real tick at `at`.
/// Used when no tick was known at log start.
pub fn tick_before(at: u64, tick: u32, start_ms: u64) -> u32 {
    let back = at.saturating_sub(start_ms) / u64::from(MS_PER_TICK);
    tick.wrapping_sub(back as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A clock whose rate baseline is exactly 40 ms per tick and whose last real tick is
    /// 505552 at 396290 (log 20261124-211816 starts at 396294 with that tick).
    fn clock_211816() -> TickClock {
        let mut c = TickClock::default();
        c.real(395_000, 505_520);
        c.real(395_400, 505_530);
        c.real(396_290, 505_552);
        c
    }

    #[test]
    fn baseline_rotates_and_measures_40_ms_per_tick() {
        let c = clock_211816();
        assert_eq!((c.prev_at, c.prev_tick), (395_400, 505_530));
        assert_eq!((c.base_at, c.base_tick), (396_290, 505_552));
        assert_eq!(c.extrapolate(396_294), (505_552, 0));
        assert_eq!(c.extrapolate(396_330), (505_553, 1));
    }

    #[test]
    fn baseline_waits_200_ms_and_rotates_only_after_300() {
        let mut c = TickClock::default();
        c.real(1000, 10);
        assert_eq!((c.base_at, c.base_tick, c.prev_at), (1000, 10, 0));
        c.real(1150, 14);
        assert_eq!((c.at, c.tick), (1150, 14));
        assert_eq!(c.base_at, 1000, "within 200 ms: no new baseline");
        c.real(1250, 16);
        // prev_at (0) + 300 < base_at (1000): rotate.
        assert_eq!((c.prev_at, c.prev_tick), (1000, 10));
        assert_eq!((c.base_at, c.base_tick), (1250, 16));
        c.real(1500, 22);
        // prev_at (1000) + 300 = 1300 is not < base_at (1250): no rotation.
        assert_eq!((c.prev_at, c.prev_tick), (1000, 10));
        assert_eq!((c.base_at, c.base_tick), (1500, 22));
    }

    /// 640 ms at the measured rate, then 40 ms per tick.
    #[test]
    fn extrapolation_caps_the_measured_rate_at_640_ms() {
        let mut c = TickClock::default();
        c.real(1000, 100);
        c.real(1400, 120); // 400 ms / 20 ticks = 20 ms per tick
        assert_eq!(c.extrapolate(1400 + 100), (125, 5));
        assert_eq!(c.extrapolate(1400 + 640), (152, 32));
        // 640/20 + 360/40 = 32 + 9.
        assert_eq!(c.extrapolate(1400 + 1000), (161, 41));
    }

    #[test]
    fn zero_rate_counts_one_ms_per_tick() {
        let mut c = TickClock::default();
        c.real(1000, 5000); // 1000 ms / 5000 ticks rounds to 0
        assert_eq!(c.extrapolate(1010), (5010, 10), "taken as 1 ms per tick");
    }

    #[test]
    fn reset_keeps_the_tick_and_clears_the_rest() {
        let mut c = clock_211816();
        c.real(400_000, 0);
        assert!(!c.seeded());
        assert_eq!(c.tick, 505_552);
        assert_eq!(
            (c.at, c.base_at, c.prev_at, c.base_tick, c.prev_tick),
            (0, 0, 0, 0, 0)
        );
    }

    /// EVTC log 20261124-211816: log start 396294 at tick 505552, sc 42 src
    /// 0xfffffffffed17b86; log 211839: start 411860 at tick 505941, src 0xfffffffffed17b8c. // public-check:ok
    #[test]
    fn instance_start_matches_evtc() {
        assert_eq!(instance_start(396_294, 505_552), 0xffff_ffff_fed1_7b86); // public-check:ok
        assert_eq!(instance_start(411_860, 505_941), 0xffff_ffff_fed1_7b8c); // public-check:ok
    }

    /// EVTC log 211816: sc 84 src 505553, 505578, 505603, 505628 (start tick 505552): the
    /// first row one tick after the start, then each time the tick passes start + 25k.
    #[test]
    fn rows_every_25_ticks_from_log_start() {
        let mut c = clock_211816();
        let mut rows = TickRows::new(c.extrapolate(396_294).0);
        assert_eq!(rows.step(&c, 396_294), None);
        assert_eq!(rows.step(&c, 396_330), Some((505_553, 1)));
        assert_eq!(rows.step(&c, 396_400), None);
        c.real(397_300, 505_576);
        assert_eq!(rows.step(&c, 397_355), None, "505577 is not past 505577");
        assert_eq!(rows.step(&c, 397_390), Some((505_578, 2)));
        c.real(398_340, 505_603);
        assert_eq!(rows.step(&c, 398_359), Some((505_603, 0)));
        c.real(399_330, 505_628);
        assert_eq!(rows.step(&c, 399_337), Some((505_628, 0)));
    }

    /// One row per evaluation: after a stall, rows catch up on the following evaluations.
    #[test]
    fn rows_catch_up_one_per_evaluation() {
        let c = clock_211816();
        let mut rows = TickRows::new(505_552);
        let late = 396_290 + 640 + 3000; // 16 + 75 ticks later
        assert_eq!(rows.step(&c, late), Some((505_643, 91)));
        assert_eq!(rows.step(&c, late), Some((505_643, 91)));
        assert_eq!(rows.step(&c, late), Some((505_643, 91)));
        assert_eq!(rows.step(&c, late), Some((505_643, 91)));
        assert_eq!(rows.step(&c, late), None, "next is 505652");
    }

    #[test]
    fn back_extrapolated_start_tick() {
        assert_eq!(tick_before(396_383, 505_554, 396_294), 505_552);
        assert_eq!(tick_before(396_200, 505_554, 396_294), 505_554);
    }

    #[test]
    fn server_keeps_last_nonzero_values_and_map_load_resets() {
        let mut s = Server::default();
        let ev = |tick, ping, server_ip, shard_ids| Event::ServerTick {
            tick,
            ping,
            server_ip,
            shard_ids,
        };
        s.apply(1000, &ev(100, 95, 0xcfdf_3164, 0x8000_0002)); // public-check:ok
        s.apply(1100, &ev(103, 0, 0, 0));
        assert_eq!(
            (s.ping, s.server_ip, s.shard_ids),
            (95, 0xcfdf_3164, 0x8000_0002) // public-check:ok
        );
        assert_eq!(s.clock.extrapolate(1100), (103, 0));
        s.apply(1200, &Event::Pov { id: 1 });
        assert_eq!(s.clock.extrapolate(1100), (103, 0));
        s.map_loaded();
        assert!(!s.clock.seeded());
        assert_eq!((s.ping, s.server_ip, s.shard_ids), (95, 0, 0));
    }
}
