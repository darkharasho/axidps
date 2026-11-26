//! Fake WvW v2 event streams, for exercising `axidps-core` without the game.

use axidps_protocol::v2::{
    name_events, AgentKind, AgentState, Event, LogAction, NameField, MAX_GROUPS, MOCK_DLL_VERSION,
};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

/// Fight length, and the quiet gap after each fight (longer than core's 20 s inactivity end).
pub const FIGHT_MS: u64 = 20_000;
pub const GAP_MS: u64 = 30_000;
/// Quiet time before the first fight. A reader that attaches after a fight's combat-enter
/// events only replays them for state, so the first fight must start after it attaches.
pub const LEAD_MS: u64 = 5_000;
const TICK_MS: u64 = 100;
const TEAM_SQUAD: u32 = 433;
const TEAM_ENEMY: u32 = 2739;
/// First enemy instance id; squad members are 1..=squad.
const ENEMY_BASE: u32 = 1000;
const MAX_HEALTH: u32 = 20_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Config {
    pub seed: u64,
    pub fights: u32,
    pub squad: u32,
    pub enemies: u32,
    /// Unix seconds at `t0`, written in the `Status` event.
    pub unix_s: i64,
}

impl Default for Config {
    fn default() -> Self {
        Self { seed: 1, fights: 1, squad: 10, enemies: 15, unix_s: 0 }
    }
}

/// Emit the whole stream, timestamps starting at `t0` (ring milliseconds).
///
/// Order: `Status` (core live), `Map`, `Pov`, each agent's `AgentAdd`, names and full
/// `Health`; then per fight: everyone enters combat, strikes every 100 ms per agent with
/// results 0–9 and one breakbar hit (10), a `Health` per second, one enemy goes Down then
/// Dead (and stops striking and being struck), everyone leaves combat and the enemy is Up
/// again; finally `LogControl End`. `Status` carries [`MOCK_DLL_VERSION`].
pub fn produce(cfg: &Config, t0: u64, emit: &mut impl FnMut(u64, &Event)) {
    let mut rng = StdRng::seed_from_u64(cfg.seed);
    let squad: Vec<u32> = (1..=cfg.squad).collect();
    let enemies: Vec<u32> = (0..cfg.enemies).map(|i| ENEMY_BASE + i).collect();

    emit(t0, &Event::Status { unix_s: cfg.unix_s, dll_version: MOCK_DLL_VERSION, group_state: [0; MAX_GROUPS] });
    emit(t0, &Event::Map { map_id: 38, map_type: 11, shard: 0, gw2_build: 0 });
    if let Some(&pov) = squad.first() {
        emit(t0, &Event::Pov { id: pov });
    }
    for (i, &id) in squad.iter().enumerate() {
        add_player(emit, t0, &mut rng, id, TEAM_SQUAD, 1 + (i % 5) as u8);
        names(emit, t0, id, &format!("Mock {id}"), Some(&format!(":Mock{id}.1234")));
    }
    for &id in &enemies {
        add_player(emit, t0, &mut rng, id, TEAM_ENEMY, 0);
        names(emit, t0, id, &format!("Enemy {id}"), None);
    }
    for &id in squad.iter().chain(&enemies) {
        emit(t0, &Event::Health { id, pct_x100: 10_000, max: MAX_HEALTH });
    }

    let mut end = t0;
    for f in 0..u64::from(cfg.fights) {
        let start = t0 + LEAD_MS + f * (FIGHT_MS + GAP_MS);
        end = start + FIGHT_MS;
        fight(emit, &mut rng, start, &squad, &enemies, f);
    }
    emit(end + 1000, &Event::LogControl { action: LogAction::End, reason: 0 });
}

fn add_player(emit: &mut impl FnMut(u64, &Event), t: u64, rng: &mut StdRng, id: u32, team: u32, subgroup: u8) {
    let ev = Event::AgentAdd {
        id,
        kind: AgentKind::Player,
        prof: rng.gen_range(1..=9),
        elite: 0,
        species: 0,
        team,
        subgroup,
        // A spread of WvW ranks, without drawing from the rng (keeps its sequence).
        wvw_rank: 1 + id.wrapping_mul(97) % 9999,
        wvw_rank_def: 0,
    };
    emit(t, &ev);
}

fn names(emit: &mut impl FnMut(u64, &Event), t: u64, id: u32, character: &str, account: Option<&str>) {
    name_events(id, NameField::Character, character).for_each(|e| emit(t, &e));
    if let Some(a) = account {
        name_events(id, NameField::Account, a).for_each(|e| emit(t, &e));
    }
}

fn fight(
    emit: &mut impl FnMut(u64, &Event),
    rng: &mut StdRng,
    start: u64,
    squad: &[u32],
    enemies: &[u32],
    index: u64,
) {
    if squad.is_empty() || enemies.is_empty() {
        return;
    }
    let everyone: Vec<u32> = squad.iter().chain(enemies).copied().collect();
    for &id in &everyone {
        emit(start, &Event::Combat { id, in_combat: true });
    }
    let victim = enemies[index as usize % enemies.len()];
    let (downed_at, dead_at) = (start + FIGHT_MS / 2, start + FIGHT_MS / 2 + 2000);
    let mut t = start;
    while t < start + FIGHT_MS {
        if t == downed_at {
            emit(t, &Event::Strike { src: squad[0], dst: victim, skill: 1010, value: 125, overstack: 0, result: 10 });
            emit(t, &Event::State { id: victim, state: AgentState::Down });
        }
        if t == dead_at {
            emit(t, &Event::State { id: victim, state: AgentState::Dead });
        }
        let dead = t >= dead_at;
        for &src in &everyone {
            if dead && src == victim {
                continue;
            }
            let foes: Vec<u32> = (if src < ENEMY_BASE { enemies } else { squad })
                .iter()
                .copied()
                .filter(|&id| !(dead && id == victim))
                .collect();
            let dst = foes[rng.gen_range(0..foes.len())];
            let result: u8 = rng.gen_range(0..=9);
            let value = if result <= 2 { rng.gen_range(100..2000) } else { 0 };
            let skill = rng.gen_range(1000..1010);
            emit(t, &Event::Strike { src, dst, skill, value, overstack: 0, result });
        }
        if (t - start).is_multiple_of(1000) {
            let id = enemies[rng.gen_range(0..enemies.len())];
            if !(dead && id == victim) {
                emit(t, &Event::Health { id, pct_x100: rng.gen_range(1000..10_000), max: MAX_HEALTH });
            }
        }
        t += TICK_MS;
    }
    for &id in &everyone {
        emit(start + FIGHT_MS, &Event::Combat { id, in_combat: false });
    }
    // The victim respawns before the next fight.
    emit(start + FIGHT_MS, &Event::State { id: victim, state: AgentState::Up });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_fight_waits_for_a_late_reader() {
        let mut first_enter = None;
        produce(&Config::default(), 1_000, &mut |t, ev| {
            if matches!(ev, Event::Combat { in_combat: true, .. }) {
                first_enter.get_or_insert(t);
            }
        });
        assert_eq!(first_enter, Some(1_000 + LEAD_MS));
    }

    fn stream(cfg: &Config) -> Vec<(u64, Event)> {
        let mut out = Vec::new();
        produce(cfg, 1_000, &mut |t, ev| out.push((t, *ev)));
        out
    }

    #[test]
    fn events_are_in_time_order() {
        let s = stream(&Config { fights: 2, ..Config::default() });
        assert!(s.windows(2).all(|w| w[0].0 <= w[1].0), "timestamps go backwards");
    }

    #[test]
    fn dead_agent_stops_striking_and_being_struck() {
        let s = stream(&Config { fights: 2, ..Config::default() });
        let mut dead = std::collections::BTreeSet::new();
        let mut saw_dead = false;
        for (_, ev) in &s {
            match *ev {
                Event::State { id, state: AgentState::Dead } => {
                    dead.insert(id);
                    saw_dead = true;
                }
                Event::State { id, state: AgentState::Up } => {
                    dead.remove(&id);
                }
                Event::Strike { src, dst, .. } => {
                    assert!(!dead.contains(&src) && !dead.contains(&dst), "strike involving a dead agent: {ev:?}");
                }
                _ => {}
            }
        }
        assert!(saw_dead);
    }

    #[test]
    fn status_carries_the_mock_marker() {
        let s = stream(&Config::default());
        let Event::Status { dll_version, .. } = s[0].1 else { panic!("first event is {:?}", s[0].1) };
        assert_eq!(dll_version, MOCK_DLL_VERSION);
    }
}
