//! Squad ground markers (EVTC sc 53) between fights: the last slots the poll sent, which seed
//! the next log's start rows (arcdps writes all 8 at log start while in a squad).

use axidps_protocol::v2::Event;

pub const SLOT_COUNT: usize = 8;
const LEFT_SQUAD: u8 = 0xff;

/// x, y, z f32 bits per slot; `None` outside a squad (or before any read).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SquadMarkers {
    pub slots: Option<[[u32; 3]; SLOT_COUNT]>,
}

impl SquadMarkers {
    /// Apply a `SquadMarker`; returns the slot when it changed (a row to write).
    pub fn apply(&mut self, ev: &Event) -> Option<(u8, [u32; 3])> {
        let Event::SquadMarker { slot, x, y, z } = *ev else {
            return None;
        };
        if slot == LEFT_SQUAD {
            self.slots = None;
            return None;
        }
        // Slots not sent yet stay +inf (empty) until their event arrives.
        let slots = self
            .slots
            .get_or_insert([[f32::INFINITY.to_bits(); 3]; SLOT_COUNT]);
        let s = slots.get_mut(usize::from(slot))?;
        let new = [x, y, z];
        (*s != new).then(|| {
            *s = new;
            (slot, new)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(slot: u8, v: u32) -> Event {
        Event::SquadMarker {
            slot,
            x: v,
            y: v,
            z: v,
        }
    }

    #[test]
    fn keeps_slots_and_reports_changes() {
        let mut m = SquadMarkers::default();
        assert_eq!(m.apply(&ev(4, 7)), Some((4, [7; 3])));
        assert_eq!(m.apply(&ev(4, 7)), None, "unchanged");
        let inf = f32::INFINITY.to_bits();
        assert_eq!(m.apply(&ev(0, inf)), None, "already empty");
        assert_eq!(m.slots.unwrap()[4], [7; 3]);
        assert_eq!(m.apply(&ev(9, 1)), None, "out of range");
        assert_eq!(m.apply(&ev(LEFT_SQUAD, 0)), None);
        assert_eq!(m.slots, None);
    }
}
