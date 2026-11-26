//! Relay wire format (version 1) and room id.
//!
//! Messages are JSON text the server relays verbatim:
//! hello `{"v":1,"hello":{"account":..,"client":..}}`, batch
//! `{"v":1,"from":..,"heals":[[t_unix_ms, src, dst, skill, heal, barrier, flags], ..]}`.
//! Flags: bit0 buff, bit1 downed. Endpoints: player `"acc"`, minion `["owner", species]`,
//! other `[null, species]`.

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::registry::Registry;

/// Wire format version.
pub const VERSION: u64 = 1;

/// The room for a squad/party group key: lowercase hex SHA-256 of `axidps-room-v1:` + key.
/// `None` for an all-zero key (no group).
pub fn room_id(key: [u8; 16]) -> Option<String> {
    if key == [0u8; 16] {
        return None;
    }
    let mut h = Sha256::new();
    h.update(b"axidps-room-v1:");
    h.update(key);
    Some(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Endpoint {
    Player(String),
    Minion { owner: String, species: u32 },
    Other { species: u32 },
}

impl Endpoint {
    fn to_json(&self) -> Value {
        match self {
            Endpoint::Player(a) => json!(a),
            Endpoint::Minion { owner, species } => json!([owner, species]),
            Endpoint::Other { species } => json!([null, species]),
        }
    }

    fn from_json(v: &Value) -> Option<Self> {
        match v {
            Value::String(a) => Some(Endpoint::Player(a.clone())),
            Value::Array(p) if p.len() == 2 => {
                let species = u32::try_from(p[1].as_u64()?).ok()?;
                match &p[0] {
                    Value::Null => Some(Endpoint::Other { species }),
                    Value::String(o) => Some(Endpoint::Minion {
                        owner: o.clone(),
                        species,
                    }),
                    _ => None,
                }
            }
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireHeal {
    pub t_unix_ms: u64,
    pub src: Endpoint,
    pub dst: Endpoint,
    pub skill: u32,
    pub heal: u32,
    pub barrier: u32,
    pub buff: bool,
    pub downed: bool,
}

impl WireHeal {
    fn to_json(&self) -> Value {
        let flags = u32::from(self.buff) | (u32::from(self.downed) << 1);
        json!([
            self.t_unix_ms,
            self.src.to_json(),
            self.dst.to_json(),
            self.skill,
            self.heal,
            self.barrier,
            flags
        ])
    }

    fn from_json(v: &Value) -> Option<Self> {
        let a = v.as_array()?;
        if a.len() != 7 {
            return None;
        }
        let n = |i: usize| u32::try_from(a[i].as_u64()?).ok();
        let flags = n(6)?;
        Some(Self {
            t_unix_ms: a[0].as_u64()?,
            src: Endpoint::from_json(&a[1])?,
            dst: Endpoint::from_json(&a[2])?,
            skill: n(3)?,
            heal: n(4)?,
            barrier: n(5)?,
            buff: flags & 1 != 0,
            downed: flags & 2 != 0,
        })
    }
}

pub fn hello(account: &str, client: &str) -> String {
    json!({"v": VERSION, "hello": {"account": account, "client": client}}).to_string()
}

pub fn batch(from: &str, heals: &[WireHeal]) -> String {
    let heals: Vec<Value> = heals.iter().map(WireHeal::to_json).collect();
    json!({"v": VERSION, "from": from, "heals": heals}).to_string()
}

/// A batch's sender and heals. `None` for another version, a hello, or malformed text;
/// malformed single heals are skipped.
pub fn parse(text: &str) -> Option<(String, Vec<WireHeal>)> {
    let v: Value = serde_json::from_str(text).ok()?;
    if v.get("v")?.as_u64()? != VERSION {
        return None;
    }
    let from = v.get("from")?.as_str()?.to_string();
    let heals = v
        .get("heals")?
        .as_array()?
        .iter()
        .filter_map(WireHeal::from_json)
        .collect();
    Some((from, heals))
}

fn is_player(reg: &Registry, id: u32) -> Option<&str> {
    reg.agent(id)
        .map(|a| a.account.as_str())
        .filter(|a| !a.is_empty())
}

/// How the other clients name agent `id`; `None` for an unknown id.
pub fn endpoint_of(reg: &Registry, id: u32) -> Option<Endpoint> {
    let a = reg.agent(id)?;
    if !a.account.is_empty() {
        return Some(Endpoint::Player(a.account.clone()));
    }
    if let Some(owner) = a.master.and_then(|m| is_player(reg, m)) {
        return Some(Endpoint::Minion {
            owner: owner.to_string(),
            species: a.species,
        });
    }
    Some(Endpoint::Other { species: a.species })
}

/// The local agent id an endpoint names, if it resolves.
pub fn agent_of(reg: &Registry, e: &Endpoint) -> Option<u32> {
    let player = |acc: &str| {
        reg.live_agents()
            .find(|a| !a.account.is_empty() && a.account == acc)
            .map(|a| a.id)
    };
    match e {
        Endpoint::Player(acc) => player(acc),
        Endpoint::Minion { owner, species } => {
            let owner_id = player(owner)?;
            Some(
                reg.live_agents()
                    .find(|a| a.species == *species && a.master == Some(owner_id))
                    .map_or(owner_id, |a| a.id),
            )
        }
        Endpoint::Other { species } => {
            let mut it = reg.live_agents().filter(|a| a.species == *species);
            let first = it.next()?;
            it.next().is_none().then_some(first.id)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::tests::{add, name_events};
    use axidps_protocol::v2::{AgentKind, Event, NameField};

    fn sample() -> Vec<WireHeal> {
        let h = |src, dst, flags: u32| WireHeal {
            t_unix_ms: 1_700_000_000_123,
            src,
            dst,
            skill: 5,
            heal: 100,
            barrier: 7,
            buff: flags & 1 != 0,
            downed: flags & 2 != 0,
        };
        vec![
            h(
                Endpoint::Player(":a.1".into()),
                Endpoint::Player(":b.2".into()),
                0,
            ),
            h(
                Endpoint::Minion {
                    owner: ":a.1".into(),
                    species: 77,
                },
                Endpoint::Other { species: 5 },
                3,
            ),
        ]
    }

    #[test]
    fn room_id_zero_is_none() {
        assert_eq!(room_id([0; 16]), None);
    }

    #[test]
    fn room_id_known_value() {
        let mut h = Sha256::new();
        h.update(b"axidps-room-v1:");
        h.update([0x11u8; 16]);
        let want: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
        let got = room_id([0x11; 16]).unwrap();
        assert_eq!(got, want);
        assert_eq!(got.len(), 64);
    }

    #[test]
    fn batch_parse_round_trip() {
        let heals = sample();
        let text = batch(":a.1", &heals);
        assert_eq!(parse(&text), Some((":a.1".to_string(), heals)));
        assert!(text.contains(r#"[null,5]"#) && text.contains(r#"[":a.1",77]"#));
    }

    #[test]
    fn parse_rejects_other_versions() {
        let text = batch(":a.1", &sample()).replace(r#""v":1"#, r#""v":2"#);
        assert_eq!(parse(&text), None);
        assert_eq!(parse(&hello(":a.1", "axidps")), None);
        assert_eq!(parse("not json"), None);
    }

    #[test]
    fn parse_skips_bad_heals() {
        let good = sample()[0].to_json();
        let text = json!({"v": 1, "from": ":a.1", "heals": [
            [1, "x"], "junk", [1, ":a", ":b", 1, 2, 3, "f"], good.clone(),
            [1, [1, 2], ":b", 1, 2, 3, 0]
        ]})
        .to_string();
        let (from, heals) = parse(&text).unwrap();
        assert_eq!(from, ":a.1");
        assert_eq!(heals, vec![sample()[0].clone()]);
    }

    fn spawn(r: &mut Registry, id: u32, kind: AgentKind, species: u32, account: &str) {
        let Event::AgentAdd {
            prof,
            elite,
            team,
            subgroup,
            wvw_rank,
            wvw_rank_def,
            ..
        } = add(id, 1)
        else {
            unreachable!()
        };
        r.apply(
            0,
            &Event::AgentAdd {
                id,
                kind,
                prof,
                elite,
                species,
                team,
                subgroup,
                wvw_rank,
                wvw_rank_def,
            },
        );
        if !account.is_empty() {
            for e in name_events(id, NameField::Account, account) {
                r.apply(0, &e);
            }
        }
    }

    #[test]
    fn endpoint_mapping_both_ways() {
        let mut r = Registry::new();
        spawn(&mut r, 1, AgentKind::Player, 0, ":a.1");
        spawn(&mut r, 2, AgentKind::Npc, 77, "");
        r.apply(0, &Event::AgentMaster { id: 2, master: 1 });
        spawn(&mut r, 3, AgentKind::Npc, 5, "");

        assert_eq!(endpoint_of(&r, 1), Some(Endpoint::Player(":a.1".into())));
        assert_eq!(
            endpoint_of(&r, 2),
            Some(Endpoint::Minion {
                owner: ":a.1".into(),
                species: 77
            })
        );
        assert_eq!(endpoint_of(&r, 3), Some(Endpoint::Other { species: 5 }));
        assert_eq!(endpoint_of(&r, 99), None);
        for id in 1..=3 {
            let e = endpoint_of(&r, id).unwrap();
            assert_eq!(agent_of(&r, &e), Some(id));
        }

        // A second NPC with the same species makes Other ambiguous.
        spawn(&mut r, 4, AgentKind::Npc, 5, "");
        assert_eq!(agent_of(&r, &Endpoint::Other { species: 5 }), None);

        // A minion unknown locally resolves to its owner.
        let unknown = Endpoint::Minion {
            owner: ":a.1".into(),
            species: 1234,
        };
        assert_eq!(agent_of(&r, &unknown), Some(1));
        assert_eq!(agent_of(&r, &Endpoint::Player(":zz.9".into())), None);
    }
}
