//! Serializes a [`Log`] to the uncompressed EVTC byte layout.

use crate::model::{Agent, Event, Header, Log, Skill};

pub const HEADER_SIZE: usize = 16;
pub const AGENT_SIZE: usize = 96;
pub const SKILL_SIZE: usize = 68;
pub const EVENT_SIZE: usize = 64;

fn put_header(out: &mut Vec<u8>, h: &Header) {
    out.extend_from_slice(b"EVTC");
    out.extend_from_slice(&h.build);
    out.push(h.revision);
    out.extend_from_slice(&h.boss_id.to_le_bytes());
    out.push(h.trailing);
}

fn put_agent(out: &mut Vec<u8>, a: &Agent) {
    out.extend_from_slice(&a.addr.to_le_bytes());
    out.extend_from_slice(&a.prof.to_le_bytes());
    out.extend_from_slice(&a.is_elite.to_le_bytes());
    out.extend_from_slice(&a.toughness.to_le_bytes());
    out.extend_from_slice(&a.concentration.to_le_bytes());
    out.extend_from_slice(&a.healing.to_le_bytes());
    out.extend_from_slice(&a.hitbox_width.to_le_bytes());
    out.extend_from_slice(&a.condition.to_le_bytes());
    out.extend_from_slice(&a.hitbox_height.to_le_bytes());
    out.extend_from_slice(&a.name);
    out.extend_from_slice(&a.pad);
}

fn put_skill(out: &mut Vec<u8>, s: &Skill) {
    out.extend_from_slice(&s.id.to_le_bytes());
    out.extend_from_slice(&s.name);
}

fn put_event(out: &mut Vec<u8>, e: &Event) {
    out.extend_from_slice(&e.time.to_le_bytes());
    out.extend_from_slice(&e.src_agent.to_le_bytes());
    out.extend_from_slice(&e.dst_agent.to_le_bytes());
    out.extend_from_slice(&e.value.to_le_bytes());
    out.extend_from_slice(&e.buff_dmg.to_le_bytes());
    out.extend_from_slice(&e.overstack.to_le_bytes());
    out.extend_from_slice(&e.skillid.to_le_bytes());
    out.extend_from_slice(&e.src_instid.to_le_bytes());
    out.extend_from_slice(&e.dst_instid.to_le_bytes());
    out.extend_from_slice(&e.src_master_instid.to_le_bytes());
    out.extend_from_slice(&e.dst_master_instid.to_le_bytes());
    out.extend_from_slice(&[
        e.iff,
        e.buff,
        e.result,
        e.is_activation,
        e.is_buffremove,
        e.is_ninety,
        e.is_fifty,
        e.is_moving,
        e.is_statechange,
        e.is_flanking,
        e.is_shields,
        e.is_offcycle,
    ]);
    out.extend_from_slice(&e.pad.to_le_bytes());
}

/// Header, u32 agent count, agents, u32 skill count, skills, events.
pub fn encode(log: &Log) -> Vec<u8> {
    let mut out = Vec::with_capacity(
        HEADER_SIZE
            + 8
            + log.agents.len() * AGENT_SIZE
            + log.skills.len() * SKILL_SIZE
            + log.events.len() * EVENT_SIZE,
    );
    put_header(&mut out, &log.header);
    out.extend_from_slice(&(log.agents.len() as u32).to_le_bytes());
    log.agents.iter().for_each(|a| put_agent(&mut out, a));
    out.extend_from_slice(&(log.skills.len() as u32).to_le_bytes());
    log.skills.iter().for_each(|s| put_skill(&mut out, s));
    log.events.iter().for_each(|e| put_event(&mut out, e));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::player_name;

    fn empty() -> Log {
        Log { header: Header::wvw(), agents: vec![], skills: vec![], events: vec![] }
    }

    fn agent() -> Agent {
        Agent {
            addr: 1,
            prof: 2,
            is_elite: 3,
            toughness: 0x0A0B,
            concentration: 0,
            healing: 0,
            hitbox_width: 0,
            condition: 0,
            hitbox_height: 0,
            name: player_name("A", "B", None),
            pad: [0xE1, 0xE2, 0xE3, 0xE4],
        }
    }

    #[test]
    fn header_bytes() {
        let b = encode(&empty());
        assert!(b.starts_with(b"EVTC20261115\x01\x01\x00\x00"));
        assert_eq!(b.len(), 16 + 4 + 4);
    }

    #[test]
    fn record_sizes() {
        let mut log = empty();
        log.agents.push(agent());
        log.skills.push(Skill::named(1, "s"));
        log.events = vec![Event::default(), Event::default()];
        assert_eq!(encode(&log).len(), 16 + 4 + 96 + 4 + 68 + 128);
    }

    #[test]
    fn event_field_offsets() {
        let mut log = empty();
        log.events.push(Event {
            time: 0x0102030405060708, // public-check:ok
            value: -5,
            skillid: 0xAABBCCDD, // public-check:ok
            is_statechange: 22,
            pad: 0x11223344, // public-check:ok
            ..Default::default()
        });
        let b = encode(&log);
        let e = &b[24..];
        assert_eq!(e[0..8], 0x0102030405060708u64.to_le_bytes());
        assert_eq!(e[24..28], (-5i32).to_le_bytes());
        assert_eq!(e[36..40], 0xAABBCCDDu32.to_le_bytes());
        assert_eq!(e[56], 22);
        assert_eq!(e[60..64], 0x11223344u32.to_le_bytes());
    }

    /// `Event::from_bytes` reads back exactly what the encoder writes, every field distinct.
    #[test]
    fn event_from_bytes_round_trips() {
        let e = Event {
            time: 0x0102030405060708,      // public-check:ok
            src_agent: 0x1112131415161718, // public-check:ok
            dst_agent: 0x2122232425262728, // public-check:ok
            value: -5,
            buff_dmg: 7,
            overstack: 9,
            skillid: 0xAABBCCDD, // public-check:ok
            src_instid: 1,
            dst_instid: 2,
            src_master_instid: 3,
            dst_master_instid: 4,
            iff: 5,
            buff: 6,
            result: 7,
            is_activation: 8,
            is_buffremove: 9,
            is_ninety: 10,
            is_fifty: 11,
            is_moving: 12,
            is_statechange: 13,
            is_flanking: 14,
            is_shields: 15,
            is_offcycle: 16,
            pad: 0x11223344, // public-check:ok
        };
        let mut b = Vec::new();
        put_event(&mut b, &e);
        assert_eq!(Event::from_bytes(b.as_slice().try_into().unwrap()), e);
    }

    #[test]
    fn agent_offsets() {
        let mut log = empty();
        log.agents.push(agent());
        let b = encode(&log);
        let a = &b[20..20 + AGENT_SIZE];
        assert_eq!(a[16..18], 0x0A0Bi16.to_le_bytes());
        assert_eq!(&a[28..92], &agent().name[..]);
        assert_eq!(a[92..96], [0xE1, 0xE2, 0xE3, 0xE4]);
    }

    #[test]
    fn skill_layout() {
        let mut log = empty();
        log.skills.push(Skill::named(0x01020304, "Zap")); // public-check:ok
        let b = encode(&log);
        // No agents: skill count sits right after header + agent count.
        let s = &b[24..];
        assert_eq!(b[20..24], 1u32.to_le_bytes());
        assert_eq!(s[0..4], 0x01020304u32.to_le_bytes());
        assert_eq!(&s[4..8], b"Zap\0");
    }

    #[test]
    fn player_name_layout() {
        let n = player_name("Alice", ":Alice.1234", Some(5));
        assert!(n.starts_with(b"Alice\0:Alice.1234\x005\0"));
        assert!(n[b"Alice\0:Alice.1234\x005\0".len()..].iter().all(|&c| c == 0));

        let n = player_name(&"x".repeat(80), ":a.1", Some(12));
        assert_eq!(n.len(), 64);
        assert_eq!(n[63], 0);
        // character truncated to 55 bytes; account and subgroup intact.
        assert_eq!(&n[..55], "x".repeat(55).as_bytes());
        assert_eq!(&n[55..], b"\0:a.1\x0012\0" as &[u8]);

        let n = player_name("Foe", "", None);
        assert!(n.starts_with(b"Foe\0\0\0"));
    }
}
