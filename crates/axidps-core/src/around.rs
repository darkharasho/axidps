//! The "around" feed: who is in range, as anonymous rows by team and spec.
//!
//! Rows are read from the registry each time [`Around::value`] runs. Nothing here carries a
//! name, account, GUID, position or id: a row says only what kind of player is in range.

use std::collections::BTreeMap;

use axidps_protocol::v2::{AgentKind, Event};
use serde_json::{json, Value};

use crate::names;
use crate::processor::Processor;
use crate::registry::Registry;
use crate::replay::Step;

/// Rows past this many are dropped, and `truncated` is set.
const MAX_ROWS: usize = 400;

const COLOURS: [&str; 3] = ["red", "blue", "green"];

#[derive(Debug, Default)]
pub struct Around {
    /// Add order per agent id: a later add of the same account wins its row.
    added: BTreeMap<u32, u64>,
    next_seq: u64,
    generation: u64,
    fight_start: Option<u64>,
}

/// One presence row; the field order is the sort order.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Row {
    team: Option<u32>,
    spec: Option<&'static str>,
    squad: bool,
    prof: Option<&'static str>,
    is_self: bool,
    here: bool,
    fought: bool,
}

fn some(s: &'static str) -> Option<&'static str> {
    (!s.is_empty()).then_some(s)
}

fn team_name(team: u32, colour: Option<usize>) -> String {
    match colour {
        Some(0) => "Red".into(),
        Some(1) => "Blue".into(),
        Some(2) => "Green".into(),
        _ => format!("Team {team}"),
    }
}

impl Around {
    pub fn new() -> Self {
        Self::default()
    }

    /// Bumped whenever [`Around::value`] would differ.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// A new game run: forget everything.
    pub fn clear(&mut self) {
        self.added.clear();
        self.next_seq = 0;
        self.fight_start = None;
        self.generation += 1;
    }

    /// Call after `replay::apply(p, step)`.
    pub fn after_step(&mut self, p: &Processor, step: &Step) {
        if let Step::Prime(_, ev) | Step::Push(_, ev) = step {
            match *ev {
                Event::AgentAdd { id, .. } => {
                    self.added.insert(id, self.next_seq);
                    self.next_seq += 1;
                    self.generation += 1;
                }
                Event::Map { .. } => {
                    self.added.clear();
                    self.generation += 1;
                }
                Event::AgentRemove { .. }
                | Event::Team { .. }
                | Event::Pov { .. }
                | Event::WvwTeam { .. } => self.generation += 1,
                _ => {}
            }
        }
        let start = p.fight_start_ms();
        if start != self.fight_start {
            self.fight_start = start;
            self.generation += 1;
        }
    }

    fn rows(&self, reg: &Registry) -> Vec<Row> {
        // One row per non-empty account: the most recently added player.
        let mut by_account: BTreeMap<&str, (u64, u32)> = BTreeMap::new();
        let mut players = Vec::new();
        for a in reg.live_agents().filter(|a| a.kind == AgentKind::Player) {
            let seq = self.added.get(&a.id).copied().unwrap_or(0);
            if a.account.is_empty() {
                players.push(a.id);
            } else {
                let e = by_account.entry(&a.account).or_insert((seq, a.id));
                if seq >= e.0 {
                    *e = (seq, a.id);
                }
            }
        }
        players.extend(by_account.values().map(|&(_, id)| id));
        let mut rows: Vec<Row> = players
            .into_iter()
            .filter_map(|id| reg.agent(id))
            .map(|a| Row {
                team: a.team,
                spec: some(names::spec_name(a.prof, a.elite)),
                squad: reg.is_squad(a.id),
                prof: some(names::spec_name(a.prof, 0)),
                is_self: reg.pov() == Some(a.id),
                here: true,
                fought: false,
            })
            .collect();
        rows.sort();
        rows
    }

    /// The `around` object.
    pub fn value(&self, p: &Processor) -> Value {
        let reg = p.registry();
        let mut rows = self.rows(reg);
        let truncated = rows.len() > MAX_ROWS;
        rows.truncate(MAX_ROWS);

        let you = reg
            .pov()
            .and_then(|id| reg.agent(id))
            .and_then(|a| a.team);
        let mut ids: Vec<u32> = rows.iter().filter_map(|r| r.team).chain(you).collect();
        ids.sort_unstable();
        ids.dedup();
        let slots = p.wvw_teams().teams;
        let teams: Vec<Value> = ids
            .into_iter()
            .map(|team| {
                let colour = (team != 0)
                    .then(|| slots.iter().position(|&t| t == team))
                    .flatten();
                json!({
                    "team": team,
                    "colour": colour.map(|i| COLOURS[i]),
                    "name": team_name(team, colour),
                })
            })
            .collect();

        let players: Vec<Value> = rows
            .iter()
            .map(|r| {
                json!({
                    "team": r.team,
                    "prof": r.prof,
                    "spec": r.spec,
                    "squad": r.squad,
                    "self": r.is_self,
                    "here": r.here,
                    "fought": r.fought,
                })
            })
            .collect();

        let mut v = json!({
            "you": {"team": you},
            "teams": teams,
            "players": players,
            "fight": {"active": p.fight_start_ms().is_some()},
        });
        if truncated {
            v["truncated"] = json!(true);
        }
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::tests::name_events;
    use crate::rules::WvwRules;
    use axidps_protocol::v2::NameField;

    fn processor() -> Processor {
        Processor::new(WvwRules {
            min_squad: 1,
            min_duration_ms: 0,
            ..WvwRules::default()
        })
    }

    fn run(p: &mut Processor, a: &mut Around, s: Step) {
        crate::replay::apply(p, &s);
        a.after_step(p, &s);
    }

    fn push(p: &mut Processor, a: &mut Around, ev: Event) {
        run(p, a, Step::Push(0, ev));
    }

    fn player(id: u32, team: u32, prof: u32, elite: u32, subgroup: u8) -> Event {
        Event::AgentAdd {
            id,
            kind: AgentKind::Player,
            prof,
            elite,
            species: 0,
            team,
            subgroup,
            wvw_rank: 0,
            wvw_rank_def: 0,
        }
    }

    fn named(p: &mut Processor, a: &mut Around, id: u32, ev: Event) {
        push(p, a, ev);
        for e in name_events(id, NameField::Character, &format!("Char {id}")) {
            push(p, a, e);
        }
        for e in name_events(id, NameField::Account, &format!(":Alpha.{id:04}")) {
            push(p, a, e);
        }
    }

    fn map(map_id: u32) -> Event {
        Event::Map {
            map_id,
            map_type: 11,
            shard: 0,
            gw2_build: 1,
        }
    }

    /// Squad 1 (POV) and 4, ally 5, enemies 6 and 7, NPC 8 and minion 9.
    fn world() -> (Processor, Around) {
        let mut p = processor();
        let mut a = Around::new();
        push(&mut p, &mut a, map(38));
        named(&mut p, &mut a, 1, player(1, 2, 1, 0, 3));
        named(&mut p, &mut a, 4, player(4, 2, 1, 0, 2));
        push(&mut p, &mut a, player(5, 2, 2, 0, 0));
        push(&mut p, &mut a, player(6, 5, 1, 62, 0));
        push(&mut p, &mut a, player(7, 5, 1, 62, 0));
        push(
            &mut p,
            &mut a,
            Event::AgentAdd {
                id: 8,
                kind: AgentKind::Npc,
                prof: 0,
                elite: 0,
                species: 77,
                team: 9,
                subgroup: 0,
                wvw_rank: 0,
                wvw_rank_def: 0,
            },
        );
        push(
            &mut p,
            &mut a,
            Event::AgentAdd {
                id: 9,
                kind: AgentKind::Gadget,
                prof: 0,
                elite: 0,
                species: 0,
                team: 5,
                subgroup: 0,
                wvw_rank: 0,
                wvw_rank_def: 0,
            },
        );
        push(&mut p, &mut a, Event::AgentMaster { id: 9, master: 6 });
        push(&mut p, &mut a, Event::Pov { id: 1 });
        (p, a)
    }

    fn players(v: &Value) -> &Vec<Value> {
        v["players"].as_array().unwrap()
    }

    #[test]
    fn rows_mark_squad_ally_enemy() {
        let (p, a) = world();
        let v = a.value(&p);
        let rows = players(&v);
        assert_eq!(rows.len(), 5);
        let me = rows.iter().find(|r| r["self"] == true).unwrap();
        assert_eq!(me["squad"], true);
        assert_eq!(rows.iter().filter(|r| r["squad"] == true).count(), 2);
        let foe = rows.iter().find(|r| r["team"] == 5).unwrap();
        assert_eq!(foe["spec"], "Firebrand");
        assert_eq!(foe["prof"], "Guardian");
        assert!(rows.iter().all(|r| r["here"] == true && r["fought"] == false));
        assert_eq!(v["you"], json!({"team": 2}));
    }

    #[test]
    fn pets_and_npcs_are_never_rows() {
        let (p, a) = world();
        assert_eq!(players(&a.value(&p)).len(), 5);
    }

    #[test]
    fn remove_and_add_back() {
        let (mut p, mut a) = world();
        push(&mut p, &mut a, Event::AgentRemove { id: 6 });
        assert_eq!(players(&a.value(&p)).len(), 4);
        push(&mut p, &mut a, player(6, 5, 1, 62, 0));
        assert_eq!(players(&a.value(&p)).len(), 5);
    }

    #[test]
    fn one_account_one_row() {
        let (mut p, mut a) = world();
        for (id, elite) in [(20, 27), (21, 62)] {
            push(&mut p, &mut a, player(id, 6, 1, elite, 0));
            for e in name_events(id, NameField::Account, ":Alpha.0004x") {
                push(&mut p, &mut a, e);
            }
        }
        let v = a.value(&p);
        let team6: Vec<_> = players(&v).iter().filter(|r| r["team"] == 6).collect();
        assert_eq!(team6.len(), 1);
        assert_eq!(team6[0]["spec"], "Firebrand");
        assert_eq!(players(&v).len(), 6);
    }

    #[test]
    fn map_change_empties_rows() {
        let (mut p, mut a) = world();
        push(&mut p, &mut a, map(99));
        let v = a.value(&p);
        assert_eq!(players(&v).len(), 0);
        assert_eq!(v["teams"], json!([]));
    }

    #[test]
    fn row_cap_sets_truncated() {
        let (mut p, mut a) = world();
        assert!(a.value(&p).get("truncated").is_none());
        for id in 100..550 {
            push(&mut p, &mut a, player(id, 5, 1, 62, 0));
        }
        let v = a.value(&p);
        assert_eq!(players(&v).len(), 400);
        assert_eq!(v["truncated"], true);
    }

    #[test]
    fn team_without_colour_is_named_by_id() {
        let (mut p, mut a) = world();
        let t = |v: &Value| v["teams"].as_array().unwrap().iter().find(|t| t["team"] == 5).cloned();
        assert_eq!(
            t(&a.value(&p)).unwrap(),
            json!({"team": 5, "colour": null, "name": "Team 5"})
        );
        push(&mut p, &mut a, Event::WvwTeam { team: 5, colour: 1 });
        assert_eq!(
            t(&a.value(&p)).unwrap(),
            json!({"team": 5, "colour": "red", "name": "Red"})
        );
    }

    #[test]
    fn rows_are_anonymous() {
        let (p, a) = world();
        let v = a.value(&p);
        let s = serde_json::to_string(&v).unwrap();
        for bad in ["Char ", ":Alpha.", "\"id\"", "\"name\":\"Char", "\"account\"", "\"guid\""] {
            assert!(!s.contains(bad), "{bad} in {s}");
        }
        let allowed = ["team", "prof", "spec", "squad", "self", "here", "fought"];
        for r in players(&v) {
            assert!(r.as_object().unwrap().keys().all(|k| allowed.contains(&k.as_str())));
        }
    }

    #[test]
    fn sorted_by_team_then_spec() {
        let mut p = processor();
        let mut a = Around::new();
        push(&mut p, &mut a, player(6, 5, 8, 60, 0));
        push(&mut p, &mut a, player(7, 5, 1, 62, 0));
        let v = a.value(&p);
        let specs: Vec<_> = players(&v).iter().map(|r| r["spec"].clone()).collect();
        assert_eq!(specs, [json!("Firebrand"), json!("Scourge")]);
    }

    #[test]
    fn generation_moves_without_a_fight() {
        let (mut p, mut a) = world();
        let g = a.generation();
        push(&mut p, &mut a, player(30, 5, 1, 62, 0));
        assert!(a.generation() > g);
        assert_eq!(p.fight_start_ms(), None);
        assert_eq!(a.value(&p)["fight"]["active"], false);
    }
}
