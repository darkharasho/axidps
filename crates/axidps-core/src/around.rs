//! The "around" feed: who is in range, as anonymous rows by team and spec.
//!
//! Rows are read from the registry each time [`Around::value`] runs. Nothing here carries a
//! name, account, GUID, position or id: a row says only what kind of player is in range.

use std::collections::{BTreeMap, BTreeSet};

use axidps_protocol::v2::{AgentKind, Event};
use serde_json::{json, Value};

use crate::live;
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
    /// The `(map_id, shard)` last seen: the registry resets only when it changes.
    map: Option<(u32, u32)>,
    /// Players here who hit an enemy player, or were hit by one, in the last fight.
    fought: BTreeSet<u32>,
    /// Players who fought and left, by their last agent id, until the next fight.
    departed: BTreeMap<u32, (Row, String)>,
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
        self.map = None;
        self.fought.clear();
        self.departed.clear();
        self.generation += 1;
    }

    /// The player an event is credited to: the agent's owner if it has one, else itself.
    fn owner(reg: &Registry, id: u32) -> Option<u32> {
        let o = reg.agent(id)?.master.unwrap_or(id);
        let a = reg.agent(o)?;
        (a.kind == AgentKind::Player && reg.is_live(o)).then_some(o)
    }

    /// Whether a hit between two players on different known teams was just seen.
    fn count_hit(&mut self, reg: &Registry, src: u32, dst: u32) {
        let (Some(s), Some(d)) = (Self::owner(reg, src), Self::owner(reg, dst)) else {
            return;
        };
        let team = |id| reg.agent(id).and_then(|a| a.team);
        if let (Some(x), Some(y)) = (team(s), team(d))
            && x != y
            && self.fought.insert(s)
        {
            self.generation += 1;
        }
    }

    fn row_of(reg: &Registry, a: &crate::registry::AgentInfo) -> Row {
        Row {
            team: a.team,
            spec: some(names::spec_name(a.prof, a.elite)),
            squad: reg.is_squad(a.id),
            prof: some(names::spec_name(a.prof, 0)),
            is_self: reg.pov() == Some(a.id),
            here: true,
            fought: false,
        }
    }

    /// Call after `replay::apply(p, step)`.
    pub fn after_step(&mut self, p: &Processor, step: &Step) {
        // A new fight clears the flags before its first hit is counted.
        if let Some(s) = p.fight_start_ms()
            && self.fight_start != Some(s)
        {
            self.fight_start = Some(s);
            self.fought.clear();
            self.departed.clear();
            self.generation += 1;
        }
        let reg = p.registry();
        if let Step::Push(_, Event::Strike { src, dst, result, .. }) = *step
            && p.fight_start_ms().is_some()
            && result <= 9
        {
            self.count_hit(reg, src, dst);
        }
        if let Step::Push(_, Event::BuffApply { src, dst, buff, .. }) = *step
            && p.fight_start_ms().is_some()
            && live::CONDITIONS.contains(&buff)
        {
            self.count_hit(reg, src, dst);
        }
        if let Step::Prime(_, ev) | Step::Push(_, ev) = step {
            match *ev {
                Event::AgentAdd { id, kind, prof, .. } => {
                    // The game reuses ids: only the same profession back is the same fighter.
                    let back = kind == AgentKind::Player
                        && self
                            .departed
                            .get(&id)
                            
                            .is_some_and(|(row, _)| row.prof == some(names::spec_name(prof, 0)));
                    if back {
                        self.departed.remove(&id);
                        self.fought.insert(id);
                    }
                    self.added.insert(id, self.next_seq);
                    self.next_seq += 1;
                    self.generation += 1;
                }
                Event::Map { map_id, shard, .. } => {
                    if self.map != Some((map_id, shard)) {
                        self.map = Some((map_id, shard));
                        self.added.clear();
                        self.fought.clear();
                        self.departed.clear();
                    }
                    self.generation += 1;
                }
                // The account arrives in name chunks after the add; it decides squad and dedupe.
                Event::AgentRemove { id } => {
                    self.leave(reg, id);
                    self.generation += 1;
                }
                Event::AgentName { .. }
                | Event::Team { .. }
                | Event::Pov { .. }
                | Event::WvwTeam { .. } => self.generation += 1,
                _ => {}
            }
        }
        let start = p.fight_start_ms();
        if start != self.fight_start {
            // Only the end of a fight lands here (a start was stored above).
            self.fight_start = start;
            self.generation += 1;
        }
    }

    /// A fighter leaves: keep its row, unless the same account is still here under another id.
    fn leave(&mut self, reg: &Registry, id: u32) {
        let Some(a) = reg.agent(id) else { return };
        // Pets, minions, NPCs and clones are never rows.
        if a.kind != AgentKind::Player || !self.fought.remove(&id) {
            return;
        }
        let sibling = (!a.account.is_empty())
            .then(|| {
                reg.live_agents()
                    .find(|b| b.id != id && b.kind == AgentKind::Player && b.account == a.account)
                    .map(|b| b.id)
            })
            .flatten();
        match sibling {
            Some(b) => {
                self.fought.insert(b);
            }
            None => {
                let mut row = Self::row_of(reg, a);
                row.here = false;
                row.fought = true;
                self.departed.insert(id, (row, a.account.clone()));
            }
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
            .map(|a| {
                let mut row = Self::row_of(reg, a);
                let same = |b: &crate::registry::AgentInfo| {
                    b.id == a.id || (!a.account.is_empty() && b.account == a.account)
                };
                row.fought = reg
                    .live_agents()
                    .any(|b| same(b) && self.fought.contains(&b.id))
                    || self
                        .departed
                        .values()
                        .any(|(_, acct)| !acct.is_empty() && *acct == a.account);
                row
            })
            .collect();
        // A departed fighter whose account is back under another id is already a live row.
        rows.extend(self.departed.values().filter_map(|(row, acct)| {
            let back = !acct.is_empty() && reg.live_agents().any(|b| &b.account == acct);
            (!back).then(|| row.clone())
        }));
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

    #[test]
    fn generation_moves_when_an_account_name_completes() {
        let (mut p, mut a) = world();
        push(&mut p, &mut a, player(40, 2, 1, 0, 2));
        let g = a.generation();
        for e in name_events(40, NameField::Account, ":Alpha.0040") {
            push(&mut p, &mut a, e);
        }
        assert!(a.generation() > g);
    }

    #[test]
    fn same_map_event_keeps_the_latest_add_for_an_account() {
        let (mut p, mut a) = world();
        for (id, elite) in [(21, 62), (20, 27)] {
            push(&mut p, &mut a, player(id, 6, 1, elite, 0));
            for e in name_events(id, NameField::Account, ":Alpha.0004x") {
                push(&mut p, &mut a, e);
            }
        }
        push(&mut p, &mut a, map(38));
        let v = a.value(&p);
        let team6: Vec<_> = players(&v).iter().filter(|r| r["team"] == 6).collect();
        assert_eq!(team6.len(), 1);
        assert_eq!(team6[0]["spec"], "Dragonhunter");
    }

    fn at(p: &mut Processor, a: &mut Around, t: u64, ev: Event) {
        run(p, a, Step::Push(t, ev));
    }

    fn start_fight(p: &mut Processor, a: &mut Around, t: u64) {
        at(p, a, t, Event::Combat { id: 1, in_combat: true });
        assert!(p.fight_start_ms().is_some());
    }

    fn hit(src: u32, dst: u32, result: u8) -> Event {
        Event::Strike {
            src,
            dst,
            skill: 1,
            value: 500,
            overstack: 0,
            result,
        }
    }

    fn buff(src: u32, dst: u32, buff: u32) -> Event {
        Event::BuffApply {
            dst,
            src,
            buff,
            inst: 1,
            duration: 1000,
            level: 0,
            active: true,
        }
    }

    /// How many rows of `team` are `fought`.
    fn fought_in(v: &Value, team: u32) -> usize {
        players(v)
            .iter()
            .filter(|r| r["team"] == team && r["fought"] == true)
            .count()
    }

    fn me_fought(v: &Value) -> bool {
        players(v).iter().find(|r| r["self"] == true).unwrap()["fought"] == true
    }

    #[test]
    fn strike_on_enemy_marks_both_sides_only_when_they_hit() {
        let (mut p, mut a) = world();
        start_fight(&mut p, &mut a, 1000);
        at(&mut p, &mut a, 1100, hit(1, 6, 0));
        let v = a.value(&p);
        assert!(me_fought(&v));
        assert_eq!(fought_in(&v, 5), 0);
        at(&mut p, &mut a, 1200, hit(6, 4, 0));
        let v = a.value(&p);
        assert_eq!(fought_in(&v, 5), 1);
    }

    #[test]
    fn blocked_strike_counts() {
        let (mut p, mut a) = world();
        start_fight(&mut p, &mut a, 1000);
        let mut e = hit(7, 1, 3);
        if let Event::Strike { value, .. } = &mut e {
            *value = 0;
        }
        at(&mut p, &mut a, 1100, e);
        assert_eq!(fought_in(&a.value(&p), 5), 1);
    }

    #[test]
    fn pet_hit_credits_owner() {
        let (mut p, mut a) = world();
        start_fight(&mut p, &mut a, 1000);
        at(&mut p, &mut a, 1100, hit(9, 1, 0));
        let v = a.value(&p);
        assert_eq!(fought_in(&v, 5), 1);
        assert_eq!(players(&v).len(), 5);
    }

    #[test]
    fn condition_apply_counts_boon_does_not() {
        let (mut p, mut a) = world();
        start_fight(&mut p, &mut a, 1000);
        at(&mut p, &mut a, 1100, buff(7, 4, 736));
        assert_eq!(fought_in(&a.value(&p), 5), 1);
        at(&mut p, &mut a, 1200, buff(4, 6, 717));
        let v = a.value(&p);
        let four = players(&v)
            .iter()
            .find(|r| r["team"] == 2 && r["self"] == false && r["squad"] == true)
            .unwrap();
        assert_eq!(four["fought"], false);
    }

    #[test]
    fn same_team_and_npc_hits_do_not_count() {
        let (mut p, mut a) = world();
        start_fight(&mut p, &mut a, 1000);
        at(&mut p, &mut a, 1100, hit(1, 4, 0));
        at(&mut p, &mut a, 1200, hit(1, 8, 0));
        assert!(!me_fought(&a.value(&p)));
    }

    #[test]
    fn departed_fighter_stays_until_next_fight() {
        let (mut p, mut a) = world();
        start_fight(&mut p, &mut a, 1000);
        at(&mut p, &mut a, 1100, hit(6, 1, 0));
        at(&mut p, &mut a, 1200, Event::AgentRemove { id: 6 });
        let gone = |v: &Value| {
            players(v)
                .iter()
                .filter(|r| r["team"] == 5 && r["here"] == false && r["fought"] == true)
                .count()
        };
        assert_eq!(gone(&a.value(&p)), 1);
        run(&mut p, &mut a, Step::Tick(100_000));
        assert_eq!(a.value(&p)["fight"]["active"], false);
        assert_eq!(gone(&a.value(&p)), 1);
        at(&mut p, &mut a, 200_000, Event::Combat { id: 1, in_combat: true });
        assert_eq!(gone(&a.value(&p)), 0);
    }

    #[test]
    fn non_fighter_leaves_at_once() {
        let (mut p, mut a) = world();
        start_fight(&mut p, &mut a, 1000);
        at(&mut p, &mut a, 1100, Event::AgentRemove { id: 7 });
        let v = a.value(&p);
        assert_eq!(players(&v).iter().filter(|r| r["team"] == 5).count(), 1);
        assert!(players(&v).iter().all(|r| r["here"] == true));
    }

    #[test]
    fn return_under_same_id_counts_once() {
        let (mut p, mut a) = world();
        start_fight(&mut p, &mut a, 1000);
        at(&mut p, &mut a, 1100, hit(6, 1, 0));
        at(&mut p, &mut a, 1200, Event::AgentRemove { id: 6 });
        at(&mut p, &mut a, 1300, player(6, 5, 1, 62, 0));
        let v = a.value(&p);
        let back: Vec<_> = players(&v)
            .iter()
            .filter(|r| r["team"] == 5 && r["spec"] == "Firebrand" && r["fought"] == true)
            .collect();
        assert_eq!(back.len(), 1);
        assert_eq!(back[0]["here"], true);
        assert_eq!(players(&v).len(), 5);
    }

    #[test]
    fn fought_survives_fight_end_and_clears_on_next_start() {
        let (mut p, mut a) = world();
        start_fight(&mut p, &mut a, 1000);
        at(&mut p, &mut a, 1100, hit(1, 6, 0));
        run(&mut p, &mut a, Step::Tick(100_000));
        let v = a.value(&p);
        assert_eq!(v["fight"]["active"], false);
        assert!(me_fought(&v));
        at(&mut p, &mut a, 200_000, Event::Combat { id: 1, in_combat: true });
        assert!(!me_fought(&a.value(&p)));
        at(&mut p, &mut a, 200_100, hit(1, 6, 0));
        assert!(me_fought(&a.value(&p)));
    }

    #[test]
    fn map_change_clears_fought() {
        let (mut p, mut a) = world();
        start_fight(&mut p, &mut a, 1000);
        at(&mut p, &mut a, 1100, hit(6, 1, 0));
        at(&mut p, &mut a, 1200, Event::AgentRemove { id: 6 });
        at(&mut p, &mut a, 1300, map(99));
        push(&mut p, &mut a, player(6, 5, 1, 62, 0));
        let v = a.value(&p);
        assert_eq!(players(&v).len(), 1);
        assert_eq!(fought_in(&v, 5), 0);
    }

    #[test]
    fn no_hits_outside_a_fight() {
        // A strike does not start a fight (only a squad Combat enter does), so nothing is
        // counted while none runs.
        let (mut p, mut a) = world();
        at(&mut p, &mut a, 1000, hit(1, 6, 0));
        assert_eq!(p.fight_start_ms(), None);
        let v = a.value(&p);
        assert!(players(&v).iter().all(|r| r["fought"] == false));
    }

    /// Name agent 6 with an account, let it fight, and return the fight's next time.
    fn fought_account_six(p: &mut Processor, a: &mut Around) {
        for e in name_events(6, NameField::Account, ":Alpha.0006x") {
            push(p, a, e);
        }
        start_fight(p, a, 1000);
        at(p, a, 1100, hit(6, 1, 0));
        assert_eq!(fought_in(&a.value(p), 5), 1);
    }

    fn only_here_dragonhunter(v: &Value) {
        assert!(players(v).iter().all(|r| r["here"] == true));
        let dh: Vec<_> = players(v).iter().filter(|r| r["spec"] == "Dragonhunter").collect();
        assert_eq!(dh.len(), 1);
        assert_eq!(dh[0]["fought"], true);
    }

    #[test]
    fn fighter_removed_while_same_account_sibling_is_live() {
        let (mut p, mut a) = world();
        fought_account_six(&mut p, &mut a);
        at(&mut p, &mut a, 1150, player(20, 5, 1, 27, 0));
        for e in name_events(20, NameField::Account, ":Alpha.0006x") {
            push(&mut p, &mut a, e);
        }
        at(&mut p, &mut a, 1200, Event::AgentRemove { id: 6 });
        only_here_dragonhunter(&a.value(&p));
        assert_eq!(players(&a.value(&p)).len(), 5);
    }

    #[test]
    fn fighter_back_under_new_id_is_one_row() {
        let (mut p, mut a) = world();
        fought_account_six(&mut p, &mut a);
        at(&mut p, &mut a, 1200, Event::AgentRemove { id: 6 });
        at(&mut p, &mut a, 1300, player(30, 5, 1, 27, 0));
        for e in name_events(30, NameField::Account, ":Alpha.0006x") {
            push(&mut p, &mut a, e);
        }
        only_here_dragonhunter(&a.value(&p));
        assert_eq!(players(&a.value(&p)).len(), 5);
    }

    #[test]
    fn reused_id_by_npc_keeps_departed_and_adds_no_row() {
        let (mut p, mut a) = world();
        start_fight(&mut p, &mut a, 1000);
        at(&mut p, &mut a, 1100, hit(6, 1, 0));
        at(&mut p, &mut a, 1200, Event::AgentRemove { id: 6 });
        at(
            &mut p,
            &mut a,
            1300,
            Event::AgentAdd {
                id: 6,
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
        at(&mut p, &mut a, 1400, Event::AgentRemove { id: 6 });
        let v = a.value(&p);
        assert_eq!(players(&v).len(), 5);
        let rows: Vec<_> = players(&v).iter().filter(|r| r["here"] == false).collect();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["team"], 5);
        assert_eq!(rows[0]["fought"], true);
        assert_eq!(rows[0]["spec"], "Firebrand");
        assert!(players(&v).iter().all(|r| !r["prof"].is_null()));
    }

    #[test]
    fn reused_id_by_other_profession_is_not_credited() {
        let (mut p, mut a) = world();
        start_fight(&mut p, &mut a, 1000);
        at(&mut p, &mut a, 1100, hit(6, 1, 0));
        at(&mut p, &mut a, 1200, Event::AgentRemove { id: 6 });
        at(&mut p, &mut a, 1300, player(6, 5, 2, 0, 0));
        let v = a.value(&p);
        let gone: Vec<_> = players(&v).iter().filter(|r| r["here"] == false).collect();
        assert_eq!(gone.len(), 1);
        assert_eq!(gone[0]["fought"], true);
        assert_eq!(gone[0]["spec"], "Firebrand");
        let new: Vec<_> = players(&v)
            .iter()
            .filter(|r| r["team"] == 5 && r["here"] == true && r["spec"] != "Firebrand")
            .collect();
        assert_eq!(new.len(), 1);
        assert_eq!(new[0]["fought"], false);
    }
}
