//! When a launcher that runs with the game quits: once Guild Wars 2 has run and then stayed
//! closed for [`CLOSED_FOR`], or when it was started without the game and the game has not
//! appeared within [`FIRST_WAIT`] (a manual start, or the first one after install, which still
//! does its install work).

use std::time::{Duration, Instant};

use crate::autostart::Lifetime;

/// How long the game must stay closed before the launcher quits.
pub const CLOSED_FOR: Duration = Duration::from_secs(10);
/// How long a launcher started without the game waits for it.
pub const FIRST_WAIT: Duration = Duration::from_secs(120);
/// How often the run loop asks whether the game runs.
pub const POLL: Duration = Duration::from_secs(2);

/// What the run loop should do after a look at the game.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Stay,
    /// The game has closed: finish up and quit.
    GameClosed,
    /// The game never started.
    NeverStarted,
}

/// Watches the game for a launcher; see the module docs.
pub struct GameWatch {
    lifetime: Lifetime,
    started: Instant,
    seen: bool,
    closed_since: Option<Instant>,
}

impl GameWatch {
    pub fn new(lifetime: Lifetime, now: Instant) -> GameWatch {
        GameWatch {
            lifetime,
            started: now,
            seen: false,
            closed_since: None,
        }
    }

    /// Takes one look at the game (`running`) at `now`.
    pub fn observe(&mut self, now: Instant, running: bool) -> Verdict {
        if self.lifetime == Lifetime::Always {
            return Verdict::Stay;
        }
        if running {
            self.seen = true;
            self.closed_since = None;
            return Verdict::Stay;
        }
        if !self.seen {
            return if now.duration_since(self.started) >= FIRST_WAIT {
                Verdict::NeverStarted
            } else {
                Verdict::Stay
            };
        }
        let since = *self.closed_since.get_or_insert(now);
        if now.duration_since(since) >= CLOSED_FOR {
            Verdict::GameClosed
        } else {
            Verdict::Stay
        }
    }

    /// The game was found running again while quitting: start over as if it never closed.
    pub fn reopened(&mut self, now: Instant) {
        self.observe(now, true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(secs: u64) -> Duration {
        Duration::from_secs(secs)
    }

    #[test]
    fn quits_once_the_game_stays_closed() {
        let t0 = Instant::now();
        let mut w = GameWatch::new(Lifetime::WithGame, t0);
        assert_eq!(w.observe(t0, true), Verdict::Stay);
        assert_eq!(w.observe(t0 + s(600), true), Verdict::Stay);
        assert_eq!(w.observe(t0 + s(602), false), Verdict::Stay);
        assert_eq!(w.observe(t0 + s(611), false), Verdict::Stay);
        assert_eq!(w.observe(t0 + s(612), false), Verdict::GameClosed);
    }

    #[test]
    fn a_short_gap_does_not_quit() {
        // The game restarting (or the process list missing it once) resets the countdown.
        let t0 = Instant::now();
        let mut w = GameWatch::new(Lifetime::WithGame, t0);
        w.observe(t0, true);
        assert_eq!(w.observe(t0 + s(2), false), Verdict::Stay);
        assert_eq!(w.observe(t0 + s(10), true), Verdict::Stay);
        assert_eq!(w.observe(t0 + s(14), false), Verdict::Stay);
        assert_eq!(w.observe(t0 + s(22), false), Verdict::Stay);
        assert_eq!(w.observe(t0 + s(24), false), Verdict::GameClosed);
    }

    #[test]
    fn started_without_the_game_waits_two_minutes() {
        let t0 = Instant::now();
        let mut w = GameWatch::new(Lifetime::WithGame, t0);
        assert_eq!(w.observe(t0 + s(30), false), Verdict::Stay);
        assert_eq!(w.observe(t0 + s(119), false), Verdict::Stay);
        assert_eq!(w.observe(t0 + s(120), false), Verdict::NeverStarted);
    }

    #[test]
    fn a_game_started_in_the_first_wait_is_watched_as_usual() {
        let t0 = Instant::now();
        let mut w = GameWatch::new(Lifetime::WithGame, t0);
        assert_eq!(w.observe(t0 + s(100), true), Verdict::Stay);
        // Past the first wait, but the game is up: the launcher stays until it closes.
        assert_eq!(w.observe(t0 + s(500), true), Verdict::Stay);
        assert_eq!(w.observe(t0 + s(502), false), Verdict::Stay);
        assert_eq!(w.observe(t0 + s(512), false), Verdict::GameClosed);
    }

    #[test]
    fn reopened_starts_the_countdown_over() {
        let t0 = Instant::now();
        let mut w = GameWatch::new(Lifetime::WithGame, t0);
        w.observe(t0, true);
        w.observe(t0 + s(2), false);
        assert_eq!(w.observe(t0 + s(12), false), Verdict::GameClosed);
        w.reopened(t0 + s(13));
        assert_eq!(w.observe(t0 + s(15), false), Verdict::Stay);
        assert_eq!(w.observe(t0 + s(25), false), Verdict::GameClosed);
    }

    #[test]
    fn a_login_started_launcher_never_quits() {
        let t0 = Instant::now();
        let mut w = GameWatch::new(Lifetime::Always, t0);
        assert_eq!(w.observe(t0 + s(1000), false), Verdict::Stay);
        w.observe(t0 + s(1001), true);
        assert_eq!(w.observe(t0 + s(2000), false), Verdict::Stay);
    }
}
