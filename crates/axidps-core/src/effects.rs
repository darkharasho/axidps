//! Effect and missile rows (EVTC statechanges 57-63 and 79), as arcdps
//! writes them.
//!
//! Positions and angles arrive quantized (i16, as arcdps writes them); here they are only packed
//! into the 64-byte event the way arcdps lays them out. Per missile id this keeps the owner and
//! skill of the create (a fallback when the DLL could not read the game object) and the launch
//! target arcdps records, which outlives the missile like arcdps's side table (a create resets
//! only its hit counters, which are not reproduced: 59's value and is_flanking are written 0).

use std::collections::BTreeMap;

use axidps_protocol::v2::{Event, MISSILE_OBJECT, MISSILE_STILL};

pub const MISSILE_CREATE: u8 = 57;
pub const MISSILE_LAUNCH: u8 = 58;
pub const MISSILE_REMOVE: u8 = 59;
pub const EFFECT_GROUND_CREATE: u8 = 60;
pub const EFFECT_GROUND_REMOVE: u8 = 61;
pub const EFFECT_AGENT_CREATE: u8 = 62;
pub const EFFECT_AGENT_REMOVE: u8 = 63;
pub const MISSILE_EFFECT: u8 = 79;

/// One row. Agents are instance ids (0 = none); the assembler writes their addrs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Row {
    pub statechange: u8,
    pub src: u32,
    pub dst: u32,
    /// A raw `dst_agent` (ground effects pack position and orientation there); `dst` is 0.
    pub dst_raw: Option<u64>,
    pub value: i32,
    pub buff_dmg: i32,
    pub overstack: u32,
    pub skill: u32,
    /// Bytes 0x30..0x3c of the event (iff .. is_offcycle); byte 8 (is_statechange) is unused.
    pub tail: [u8; 12],
    /// Write iff from the agents' teams instead of `tail[0]` (missile remove).
    pub team_iff: bool,
    pub pad: u32,
    /// A skill id for the log's skill table (missile rows).
    pub table_skill: Option<u32>,
}

impl Row {
    fn new(statechange: u8) -> Row {
        Row {
            statechange,
            src: 0,
            dst: 0,
            dst_raw: None,
            value: 0,
            buff_dmg: 0,
            overstack: 0,
            skill: 0,
            tail: [0; 12],
            team_iff: false,
            pad: 0,
            table_skill: None,
        }
    }

    fn tail_u32(&mut self, at: usize, v: u32) {
        if let Some(t) = self.tail.get_mut(at..at + 4) {
            t.copy_from_slice(&v.to_le_bytes());
        }
    }

    fn tail_i16(&mut self, at: usize, v: i16) {
        if let Some(t) = self.tail.get_mut(at..at + 2) {
            t.copy_from_slice(&v.to_le_bytes());
        }
    }
}

/// Two i16 as one 32-bit word, low first (arcdps writes consecutive i16s).
fn pair(lo: i16, hi: i16) -> u32 {
    u32::from(lo as u16) | (u32::from(hi as u16) << 16)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Missile {
    owner: u32,
    skill: u32,
    /// The target arcdps recorded at the last qualifying launch (0 = none).
    target: u32,
}

/// The missile table, by missile id.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Effects {
    missiles: BTreeMap<u32, Missile>,
}

impl Effects {
    /// The rows for `ev` (none for other events), updating the missile table.
    pub fn apply(&mut self, ev: &Event) -> Vec<Row> {
        let row = match *ev {
            Event::EffectGround {
                effect,
                owner,
                duration,
                tracking,
                pos,
                orient,
                scale,
                shields,
                flags,
                flanking,
            } => {
                let mut r = Row::new(EFFECT_GROUND_CREATE);
                r.src = owner;
                r.dst_raw = Some(
                    u64::from(pair(pos[0], pos[1])) | u64::from(pair(pos[2], orient[0])) << 32,
                );
                r.value = pair(orient[1], orient[2]) as i32;
                r.skill = effect;
                r.tail_u32(0, duration);
                r.tail[4] = flags;
                r.tail_i16(6, scale);
                r.tail[9] = flanking;
                r.tail_i16(10, shields);
                r.pad = tracking;
                r
            }
            Event::EffectGroundRemove { tracking } => {
                let mut r = Row::new(EFFECT_GROUND_REMOVE);
                r.pad = tracking;
                r
            }
            Event::EffectAgent {
                host,
                src,
                effect,
                duration,
                tracking,
            } => {
                let mut r = Row::new(EFFECT_AGENT_CREATE);
                r.src = src;
                r.dst = host;
                r.skill = effect;
                r.tail_u32(0, duration);
                r.pad = tracking;
                r
            }
            Event::EffectAgentRemove { host, tracking } => {
                let mut r = Row::new(EFFECT_AGENT_REMOVE);
                r.src = host;
                r.pad = tracking;
                r
            }
            Event::MissileCreate {
                id,
                owner,
                skill,
                skin,
                word_1e,
                pos,
                byte_22,
                byte_23,
            } => {
                let m = self.missiles.entry(id).or_default();
                m.owner = owner;
                m.skill = skill;
                let mut r = Row::new(MISSILE_CREATE);
                r.src = owner;
                r.value = pair(pos[0], pos[1]) as i32;
                r.buff_dmg = i32::from(pos[2] as u16);
                r.overstack = skin;
                r.skill = skill;
                r.tail_u32(0, word_1e);
                r.tail[10] = byte_22;
                r.tail[11] = byte_23;
                r.pad = id;
                r.table_skill = Some(skill);
                r
            }
            Event::MissileLaunch {
                id,
                owner,
                skill,
                target,
                target_pos,
                pos,
                radius,
                speed,
                flags,
                motion,
                bits,
            } => {
                let (owner, skill) = self.owner_skill(id, owner, skill, bits);
                let still = bits & MISSILE_OBJECT != 0 && bits & MISSILE_STILL != 0;
                // arcdps records the target of a launch from rest, if it knows the agent.
                if still && target != 0 {
                    self.missiles.entry(id).or_default().target = target;
                }
                let mut r = Row::new(MISSILE_LAUNCH);
                r.src = owner;
                r.dst = target;
                r.value = pair(target_pos[0], target_pos[1]) as i32;
                r.buff_dmg = pair(target_pos[2], pos[0]) as i32;
                r.overstack = pair(pos[1], pos[2]);
                r.skill = skill;
                r.tail[0] = motion;
                r.tail_i16(2, radius);
                r.tail_u32(4, flags);
                r.tail[9] = u8::from(still);
                r.tail_i16(10, speed);
                r.pad = id;
                r.table_skill = Some(skill);
                r
            }
            Event::MissileRemove {
                id,
                owner,
                skill,
                other,
                pos,
                bits,
            } => {
                let (owner, skill) = self.owner_skill(id, owner, skill, bits);
                let recorded = self.missiles.get(&id).map_or(0, |m| m.target);
                let mut r = Row::new(MISSILE_REMOVE);
                r.src = owner;
                r.dst = recorded;
                r.buff_dmg = pair(pos[0], pos[1]) as i32;
                r.overstack = u32::from(pos[2] as u16);
                r.skill = skill;
                r.tail[10] = u8::from(other != 0 && other == recorded);
                r.team_iff = true;
                r.pad = id;
                r.table_skill = Some(skill);
                r
            }
            Event::MissileEffect {
                id,
                owner,
                effect,
                value,
                bits,
            } => {
                let (owner, _) = self.owner_skill(id, owner, 0, bits);
                let mut r = Row::new(MISSILE_EFFECT);
                r.dst = owner;
                r.value = value as i32;
                r.skill = effect;
                r.pad = id;
                r
            }
            _ => return Vec::new(),
        };
        vec![row]
    }

    /// The object's owner and skill when the DLL read it (and they update the table, as a reflect
    /// changes the owner); else the create's.
    fn owner_skill(&mut self, id: u32, owner: u32, skill: u32, bits: u8) -> (u32, u32) {
        if bits & MISSILE_OBJECT != 0 {
            let m = self.missiles.entry(id).or_default();
            m.owner = owner;
            if skill != 0 {
                m.skill = skill;
            }
            (owner, skill)
        } else {
            self.missiles
                .get(&id)
                .map_or((0, 0), |m| (m.owner, m.skill))
        }
    }

    /// A new map: missile ids are per map.
    pub fn clear(&mut self) {
        self.missiles.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(e: &mut Effects, ev: Event) -> Row {
        let rows = e.apply(&ev);
        assert_eq!(rows.len(), 1, "{ev:?}");
        rows[0]
    }

    fn create(id: u32, owner: u32, skill: u32) -> Event {
        Event::MissileCreate {
            id,
            owner,
            skill,
            skin: 88,
            word_1e: 0xC000_0021, // public-check:ok
            pos: [1, -2, 3],
            byte_22: 5,
            byte_23: 6,
        }
    }

    fn launch(id: u32, owner: u32, skill: u32, target: u32, bits: u8) -> Event {
        Event::MissileLaunch {
            id,
            owner,
            skill,
            target,
            target_pos: [10, 20, -30],
            pos: [40, 50, 60],
            radius: 120,
            speed: 2000,
            flags: 0x30,
            motion: 4,
            bits,
        }
    }

    fn remove(id: u32, owner: u32, skill: u32, other: u32, bits: u8) -> Event {
        Event::MissileRemove {
            id,
            owner,
            skill,
            other,
            pos: [7, 8, -9],
            bits,
        }
    }

    #[test]
    fn ground_create_layout() {
        let mut e = Effects::default();
        let r = one(
            &mut e,
            Event::EffectGround {
                effect: 17_000,
                owner: 4,
                duration: 4000,
                tracking: 5,
                pos: [100, -200, 31],
                orient: [500, -250, 3000],
                scale: 1500,
                shields: 250,
                flags: 12,
                flanking: 1,
            },
        );
        assert_eq!(r.statechange, EFFECT_GROUND_CREATE);
        assert_eq!((r.src, r.dst, r.skill, r.pad), (4, 0, 17_000, 5));
        // dst_agent: x, y, z, orientation x; value: orientation y, z.
        assert_eq!(
            r.dst_raw,
            Some(100u64 | (0xff38u64 << 16) | (31u64 << 32) | (500u64 << 48))
        );
        assert_eq!(r.value as u32, pair(-250, 3000));
        assert_eq!(&r.tail[0..4], &4000u32.to_le_bytes()); // duration
        assert_eq!(r.tail[4], 12); // is_buffremove = flags
        assert_eq!(&r.tail[6..8], &1500i16.to_le_bytes()); // scale
        assert_eq!(r.tail[9], 1); // is_flanking
        assert_eq!(&r.tail[10..12], &250i16.to_le_bytes());
        assert_eq!(r.table_skill, None, "effect ids are not skills");
    }

    #[test]
    fn ground_and_agent_effect_rows() {
        let mut e = Effects::default();
        let r = one(&mut e, Event::EffectGroundRemove { tracking: 77 });
        assert_eq!((r.statechange, r.src, r.dst, r.pad), (61, 0, 0, 77));
        assert_eq!(r.tail, [0; 12]);

        let r = one(
            &mut e,
            Event::EffectAgent {
                host: 6,
                src: 0x55,
                effect: 21_000,
                duration: 2500,
                tracking: 99,
            },
        );
        assert_eq!(
            (r.statechange, r.src, r.dst, r.skill, r.pad),
            (62, 0x55, 6, 21_000, 99)
        );
        assert_eq!(&r.tail[0..4], &2500u32.to_le_bytes());

        let r = one(
            &mut e,
            Event::EffectAgentRemove {
                host: 6,
                tracking: 99,
            },
        );
        assert_eq!((r.statechange, r.src, r.dst, r.pad), (63, 6, 0, 99));
    }

    #[test]
    fn missile_create_layout() {
        let mut e = Effects::default();
        let r = one(&mut e, create(9, 3, 5491));
        assert_eq!(r.statechange, MISSILE_CREATE);
        assert_eq!((r.src, r.dst, r.skill, r.pad), (3, 0, 5491, 9));
        assert_eq!(r.value as u32, pair(1, -2));
        assert_eq!(r.buff_dmg, 3); // z in the low half
        assert_eq!(r.overstack, 88);
        assert_eq!(&r.tail[0..4], &0xC000_0021u32.to_le_bytes());
        assert_eq!((r.tail[10], r.tail[11]), (5, 6));
        assert_eq!(r.table_skill, Some(5491));
    }

    #[test]
    fn missile_launch_layout_and_recorded_target() {
        let mut e = Effects::default();
        e.apply(&create(9, 3, 5491));
        let r = one(
            &mut e,
            launch(9, 2, 5492, 7, MISSILE_OBJECT | MISSILE_STILL),
        );
        assert_eq!(r.statechange, MISSILE_LAUNCH);
        // The object's owner and skill win over the create's (reflects).
        assert_eq!((r.src, r.dst, r.skill, r.pad), (2, 7, 5492, 9));
        assert_eq!(r.value as u32, pair(10, 20));
        assert_eq!(r.buff_dmg as u32, pair(-30, 40));
        assert_eq!(r.overstack, pair(50, 60));
        assert_eq!(r.tail[0], 4); // iff byte = motion
        assert_eq!(&r.tail[2..4], &120i16.to_le_bytes()); // radius
        assert_eq!(&r.tail[4..8], &0x30u32.to_le_bytes()); // flags
        assert_eq!(r.tail[9], 1); // still
        assert_eq!(&r.tail[10..12], &2000i16.to_le_bytes()); // speed
        assert!(!r.team_iff);

        // The remove's dst is the recorded target; is_shields when the other agent is it.
        let r = one(&mut e, remove(9, 2, 5492, 7, MISSILE_OBJECT));
        assert_eq!(r.statechange, MISSILE_REMOVE);
        assert_eq!((r.src, r.dst, r.skill, r.pad), (2, 7, 5492, 9));
        assert_eq!(r.value, 0);
        assert_eq!(r.buff_dmg as u32, pair(7, 8));
        assert_eq!(r.overstack, u32::from(-9i16 as u16));
        assert_eq!((r.tail[9], r.tail[10]), (0, 1));
        assert!(r.team_iff);
        let r = one(&mut e, remove(9, 2, 5492, 8, MISSILE_OBJECT));
        assert_eq!(r.tail[10], 0);
    }

    #[test]
    fn launch_records_only_still_known_targets() {
        let mut e = Effects::default();
        e.apply(&create(9, 3, 5491));
        e.apply(&launch(9, 3, 5491, 7, MISSILE_OBJECT)); // moving: not recorded
        assert_eq!(one(&mut e, remove(9, 3, 5491, 0, MISSILE_OBJECT)).dst, 0);
        e.apply(&launch(9, 3, 5491, 0, MISSILE_OBJECT | MISSILE_STILL)); // no target
        assert_eq!(one(&mut e, remove(9, 3, 5491, 0, MISSILE_OBJECT)).dst, 0);
        e.apply(&launch(9, 3, 5491, 7, MISSILE_OBJECT | MISSILE_STILL));
        // A new missile with the same id keeps the recorded target, as arcdps's side table.
        e.apply(&create(9, 4, 6000));
        assert_eq!(one(&mut e, remove(9, 4, 6000, 0, MISSILE_OBJECT)).dst, 7);
    }

    #[test]
    fn unread_object_falls_back_to_the_create() {
        let mut e = Effects::default();
        e.apply(&create(9, 3, 5491));
        let r = one(&mut e, launch(9, 0, 0, 7, 0));
        assert_eq!((r.src, r.skill), (3, 5491));
        assert_eq!(r.tail[9], 0);
        let r = one(&mut e, remove(9, 0, 0, 0, 0));
        assert_eq!((r.src, r.skill), (3, 5491));
        let r = one(
            &mut e,
            Event::MissileEffect {
                id: 9,
                owner: 0,
                effect: 0x1234,
                value: u32::MAX,
                bits: 0,
            },
        );
        assert_eq!(r.statechange, MISSILE_EFFECT);
        assert_eq!((r.src, r.dst, r.skill, r.pad), (0, 3, 0x1234, 9));
        assert_eq!(r.value, -1);
        assert_eq!(r.table_skill, None);
        // Unknown missile and no object: owner and skill 0.
        let r = one(&mut e, remove(10, 0, 0, 0, 0));
        assert_eq!((r.src, r.skill), (0, 0));
    }

    #[test]
    fn missile_effect_uses_the_object_owner() {
        let mut e = Effects::default();
        e.apply(&create(9, 3, 5491));
        let r = one(
            &mut e,
            Event::MissileEffect {
                id: 9,
                owner: 5,
                effect: 7,
                value: 1,
                bits: MISSILE_OBJECT,
            },
        );
        assert_eq!((r.src, r.dst), (0, 5));
    }

    #[test]
    fn other_events_make_no_rows_and_clear_forgets() {
        let mut e = Effects::default();
        assert!(e.apply(&Event::AgentRemove { id: 3 }).is_empty());
        e.apply(&create(9, 3, 5491));
        e.clear();
        assert_eq!(one(&mut e, remove(9, 0, 0, 0, 0)).src, 0);
    }
}
