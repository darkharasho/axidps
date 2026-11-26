//! Keeping text that pages and plugins write from flooding or forging the host's log: one line
//! of at most 500 characters each, and at most 20 such lines a second.

use std::time::{Duration, Instant};

/// The longest line, in characters.
pub const MAX_CHARS: usize = 500;

/// `msg` as one log line: at most [`MAX_CHARS`] characters, control characters (newlines
/// included) replaced by spaces, so it can't fake further lines.
pub fn clean(msg: &str) -> String {
    msg.chars()
        .take(MAX_CHARS)
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

/// A rate limit: `limit` lines per second; the rest are counted and reported as one line when
/// the second is over.
pub struct Gate {
    limit: u32,
    /// What the lines are, in the "dropped" line ("page" lines by default).
    what: &'static str,
    start: Option<Instant>,
    count: u32,
    dropped: u32,
}

impl Gate {
    pub fn new(limit: u32) -> Self {
        Self::named(limit, "page")
    }

    /// A gate whose "dropped" line says "… N `what` lines dropped".
    pub fn named(limit: u32, what: &'static str) -> Self {
        Gate {
            limit,
            what,
            start: None,
            count: 0,
            dropped: 0,
        }
    }

    /// Whether a line arriving at `now` may be logged; also returns the "dropped" line for the
    /// second that just ended, if it dropped any (log that first).
    pub fn pass(&mut self, now: Instant) -> (bool, Option<String>) {
        let flushed = self.flush(now);
        if self.start.is_none() {
            self.start = Some(now);
        }
        if self.count < self.limit {
            self.count += 1;
            (true, flushed)
        } else {
            self.dropped += 1;
            (false, flushed)
        }
    }

    /// The "… N page lines dropped" line once the second in which they were dropped is over
    /// (call it now and then, so the count shows up even when no more lines come).
    pub fn flush(&mut self, now: Instant) -> Option<String> {
        let start = self.start?;
        if now.duration_since(start) < Duration::from_secs(1) {
            return None;
        }
        let dropped = std::mem::take(&mut self.dropped);
        self.start = None;
        self.count = 0;
        (dropped > 0).then(|| format!("… {dropped} {} lines dropped", self.what))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_caps_and_flattens() {
        assert_eq!(clean("a\nb\r\tc\u{1b}[31m"), "a b  c [31m");
        assert_eq!(clean(&"é".repeat(600)).chars().count(), MAX_CHARS);
    }

    #[test]
    fn gate_passes_twenty_a_second_then_reports_the_rest() {
        let t0 = Instant::now();
        let mut g = Gate::new(20);
        let passed = (0..25)
            .filter(|i| g.pass(t0 + Duration::from_millis(i * 10)).0)
            .count();
        assert_eq!(passed, 20);
        assert_eq!(g.flush(t0 + Duration::from_millis(500)), None);
        assert_eq!(
            g.flush(t0 + Duration::from_millis(1000)).as_deref(),
            Some("… 5 page lines dropped")
        );
        assert_eq!(g.flush(t0 + Duration::from_millis(3000)), None);
        let (ok, note) = g.pass(t0 + Duration::from_millis(3000));
        assert!(ok);
        assert_eq!(note, None);
    }

    #[test]
    fn gate_reports_dropped_lines_with_the_next_line() {
        let t0 = Instant::now();
        let mut g = Gate::new(1);
        assert!(g.pass(t0).0);
        assert!(!g.pass(t0).0);
        let (ok, note) = g.pass(t0 + Duration::from_secs(2));
        assert!(ok);
        assert_eq!(note.as_deref(), Some("… 1 page lines dropped"));
    }
}
