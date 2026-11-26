//! Live stats feed for the in-game meter overlay: per-player squad numbers for the fight recording
//! (or the last one), as schema v1 JSON, plus `timeline`: the squad's damage dealt (`out`) and
//! taken (`in`) per `bucket_ms` of the fight, for the overlay's chart.

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use axidps_protocol::v2::{AgentKind, AgentState, Event};
use serde_json::{Value, json};

use crate::boons::{self, BoonTracker, SQUAD_BOONS};
use crate::buffs::{
    self, BUFF_APPLY, BUFF_DURATION, BUFF_INITIAL, BUFF_REMOVE_ALL, BUFF_REMOVE_ONE, STACK_ACTIVE,
    STACK_RESET,
};
use crate::processor::{FightOutcome, Processor};
use crate::registry::Registry;
use crate::replay::Step;

/// Minimum time between two writes of the feed.
pub const MIN_INTERVAL: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveState {
    Idle,
    Fight,
    Last,
}

/// Strike results that carry damage in `value`: normal, crit, glance, block, evade, interrupt,
/// absorb, blind. Breakbar (10), CC (12) and the like use `value` for something else.
const DAMAGE_RESULTS: std::ops::RangeInclusive<u8> = 0..=7;
/// Strike result: killing blow (`src` killed `dst`).
const STRIKE_KILL: u8 = 8;
/// Strike result: downing hit (`src` put `dst` into downstate).
const STRIKE_DOWN: u8 = 9;

/// Where the tracker's per-account ids start: above any agent id.
const ACCOUNT_IDS: u32 = 1 << 31;

/// MumbleLink map type of the Armistice Bastion WvW lounge.
const WVW_LOUNGE: u32 = 18;

/// Whether the WvW counting settings apply on a map type: the WvW maps and the Armistice
/// Bastion lounge (its training golems are where players try the settings out).
fn counts_as_wvw(map_type: u32) -> bool {
    crate::names::is_wvw_map(map_type) || map_type == WVW_LOUNGE
}

/// What the meter counts: the hub's Settings, saved by the overlay as `core` in its ui.json.
/// Saved logs always keep everything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Counting {
    /// In WvW (and Armistice Bastion), damage to gadgets (siege, gates, walls).
    pub wvw_gadgets: bool,
    /// In WvW, damage to NPCs that are no player's pet or clone (guards, lords, supervisors).
    pub wvw_npcs: bool,
}

impl Default for Counting {
    fn default() -> Self {
        Counting {
            wvw_gadgets: true,
            wvw_npcs: true,
        }
    }
}

impl Counting {
    /// From the overlay's ui.json (`{"core": {"wvw_gadgets": false, ...}}`); missing keys keep
    /// their default.
    pub fn from_ui_json(v: &Value) -> Self {
        let d = Counting::default();
        let c = &v["core"];
        Counting {
            wvw_gadgets: c["wvw_gadgets"].as_bool().unwrap_or(d.wvw_gadgets),
            wvw_npcs: c["wvw_npcs"].as_bool().unwrap_or(d.wvw_npcs),
        }
    }
}

/// One agent's numbers in the current fight.
#[derive(Debug, Clone, Copy, Default)]
struct Acc {
    power: u64,
    condi: u64,
    downs: u64,
    kills: u64,
    deaths: u64,
    downed: u64,
    taken: u64,
    strips: u64,
    cleanses: u64,
    /// Damage that led to enemy players' downs (axilog's arcdps-methodology down contribution).
    down_contrib: u64,
}

/// Healing and barrier one way between the POV and one other agent (or itself).
#[derive(Debug, Clone, Copy, Default)]
struct HealSum {
    healing: u64,
    barrier: u64,
}

/// One player's healing one way (given or received) as copies by who reported them: `own` is
/// the player's own copies, `by_sender` everyone else's. Every heal reaches the meter once per
/// sharing squadmate, so the copies are alternatives, not additions (Elite Insights' rule).
#[derive(Debug, Clone, Default)]
struct CopySums {
    own: Option<HealSum>,
    by_sender: BTreeMap<u32, HealSum>,
}

impl CopySums {
    fn add(&mut self, own: bool, sender: u32, h: HealSum) {
        let e = if own {
            self.own.get_or_insert_with(HealSum::default)
        } else {
            self.by_sender.entry(sender).or_default()
        };
        e.healing = e.healing.saturating_add(h.healing);
        e.barrier = e.barrier.saturating_add(h.barrier);
    }

    /// The player's own copies if any, else the largest sender's.
    fn shown(&self) -> HealSum {
        self.own.unwrap_or_else(|| {
            self.by_sender
                .values()
                .copied()
                .max_by_key(|h| h.healing.saturating_add(h.barrier))
                .unwrap_or_default()
        })
    }
}

/// One player's healing the way the arcdps healing addon counts it (its own window and its
/// "peers outgoing"): everything the player gives and gets from its own combat entry until its
/// combat exit, kept until it next enters combat, whatever fights core splits that time into.
/// Counted from the player's own copies only (the POV's events, a sharer's relayed ones).
#[derive(Debug, Clone, Default)]
struct OwnCombat {
    /// Game time the player last entered combat (None until it first does).
    start: Option<u64>,
    /// Game time it left that combat (None while still in it).
    end: Option<u64>,
    /// A copy of its own heals has arrived (the POV, or a player sharing its heals).
    shared: bool,
    /// Given by target and received by source, by owner.
    out: BTreeMap<u32, HealSum>,
    inc: BTreeMap<u32, HealSum>,
}

impl OwnCombat {
    fn in_combat(&self) -> bool {
        self.start.is_some() && self.end.is_none()
    }

    fn total(m: &BTreeMap<u32, HealSum>) -> HealSum {
        m.values().fold(HealSum::default(), |a, h| HealSum {
            healing: a.healing.saturating_add(h.healing),
            barrier: a.barrier.saturating_add(h.barrier),
        })
    }

    /// Combat time up to game time `now`, in ms.
    fn duration_ms(&self, now: u64) -> u64 {
        self.start
            .map_or(0, |s| self.end.unwrap_or(now).saturating_sub(s))
    }
}

/// Boons (Elite Insights' list): stripping one from someone else is a strip.
const BOONS: &[u32] = &[
    717, 718, 719, 725, 726, 740, 743, 873, 1122, 1187, 26980, 30328,
];
/// Conditions: removing one is a cleanse.
const CONDITIONS: &[u32] = &[
    720, 721, 722, 723, 727, 736, 737, 738, 742, 791, 861, 19426, 26766, 27705,
];

/// Down contribution (axilog's arcdps methodology): a down's window opens this long before the
/// target was last at [`CONTRIB_FULL_X100`] health or more...
const CONTRIB_LEAD_IN_MS: u64 = 2000;
const CONTRIB_FULL_X100: u16 = 9900;
/// ...and no earlier than this long after its previous down (the downstate invulnerability).
const CONTRIB_RESET_MS: u64 = 2100;
/// Hits kept per target between full health and a down (a bound, far above a real life).
const CONTRIB_MAX_HITS: usize = 20_000;

/// Timeline bucket width.
pub const BUCKET_MS: u64 = 1000;
/// Longest timeline kept (buckets): an hour.
const MAX_BUCKETS: usize = 3600;

/// One agent's damage dealt (`out`) and taken (`inc`) per bucket.
#[derive(Debug, Clone, Default)]
struct Series {
    out: Vec<u64>,
    inc: Vec<u64>,
}

fn add_heal(m: &mut BTreeMap<u32, HealSum>, id: u32, h: HealSum) {
    let e = m.entry(id).or_default();
    e.healing = e.healing.saturating_add(h.healing);
    e.barrier = e.barrier.saturating_add(h.barrier);
}

fn add_at(v: &mut Vec<u64>, i: usize, n: u64) {
    if i >= MAX_BUCKETS {
        return;
    }
    if v.len() <= i {
        v.resize(i + 1, 0);
    }
    v[i] = v[i].saturating_add(n);
}

/// What a row shows about who a squad member is, kept so a fight's rows survive the registry
/// forgetting them (a map change ends the fight and clears the registry).
#[derive(Debug, Clone)]
struct Ident {
    name: String,
    account: String,
    prof: u32,
    elite: u32,
    group: u8,
    me: bool,
}

/// Per-player squad numbers for the fight recording (state "fight"), or the last one ("last").
pub struct LiveStats {
    state: LiveState,
    start_ms: u64,
    /// Latest game time seen in the fight; its end once it ended.
    now_ms: u64,
    map: u32,
    acc: BTreeMap<u32, Acc>,
    /// Latest up/down/dead per agent this fight, so a repeated state is not counted twice.
    states: BTreeMap<u32, AgentState>,
    /// Pet or minion id -> master id.
    masters: BTreeMap<u32, u32>,
    /// Down contribution: per target, (time, attacker owner, damage) since its window could
    /// start, the last time its health was at least 99%, and the reset floor after its last down.
    recent: BTreeMap<u32, Vec<(u64, u32, u64)>>,
    full_at: BTreeMap<u32, u64>,
    reset: BTreeMap<u32, u64>,
    /// The POV's healing by target (`heal_out`) and received by source (`heal_in`), by owner.
    heal_out: BTreeMap<u32, HealSum>,
    heal_in: BTreeMap<u32, HealSum>,
    /// Healing given and received per player owner this fight, from every copy reported (the
    /// POV's own events and the relayed ones).
    given: BTreeMap<u32, CopySums>,
    received: BTreeMap<u32, CopySums>,
    /// The ended fight's healing breakdown, kept with its rows.
    last_healing: Value,
    /// Each player's healing over its own combat (the healing addon's numbers), by player;
    /// shown instead of the per-fight numbers for the POV and sharers once their combat state
    /// is known.
    combats: BTreeMap<u32, OwnCombat>,
    /// Latest game time seen by [`LiveStats::event`].
    clock: u64,
    /// Live buff instances (tracked in and out of fights): instance -> (target, buff).
    stacks: BTreeMap<u32, (u32, u32)>,
    /// Squad members as last seen in the registry this fight.
    idents: BTreeMap<u32, Ident>,
    /// Damage dealt and taken per agent per [`BUCKET_MS`] of the fight (dealt by owner).
    series: BTreeMap<u32, Series>,
    /// The ended fight's rows and timeline, kept until the next fight starts.
    last_rows: Vec<Value>,
    last_timeline: Value,
    /// Bumped on every change to what [`LiveStats::snapshot`] returns (apart from `t`).
    generation: u64,
    /// What is counted (the hub's Settings).
    counting: Counting,
    /// The current map is WvW.
    wvw: bool,
    /// The local player, as the registry last knew it.
    pov: Option<u32>,
    /// Boon uptime and generation this fight, fed from `buff_stacks`' rows.
    boons: BoonTracker,
    /// Our own copy of the buff stacks (the processor's rows are not kept), in and out of fights.
    buff_stacks: buffs::Stacks,
    /// The map the stacks and roster belong to (ids are reused per map).
    boon_map: Option<(u32, u32)>,
    /// Agent -> (is a player, team) from the poll, to leave enemy holders out of the tracker.
    roster: BTreeMap<u32, (bool, u32)>,
    roster_pov: Option<u32>,
    /// One id per account for the tracker: agent -> first id seen with its account, and back to
    /// the account's current id.
    by_account: BTreeMap<String, u32>,
    canon_of: BTreeMap<u32, u32>,
    live_of: BTreeMap<u32, u32>,
    /// Ids the tracker holds rows under before their account was known.
    uncanon: BTreeSet<u32>,
    /// Per account id: whether it was squad, and its subgroup, when last seen this fight.
    boon_seen: BTreeMap<u32, (bool, Option<u8>)>,
    /// Game time of the tracker's last tick.
    boon_tick: Option<u64>,
}

impl Default for LiveStats {
    fn default() -> Self {
        Self::new()
    }
}

impl LiveStats {
    pub fn new() -> Self {
        Self {
            state: LiveState::Idle,
            start_ms: 0,
            now_ms: 0,
            map: 0,
            acc: BTreeMap::new(),
            states: BTreeMap::new(),
            masters: BTreeMap::new(),
            recent: BTreeMap::new(),
            full_at: BTreeMap::new(),
            reset: BTreeMap::new(),
            heal_out: BTreeMap::new(),
            heal_in: BTreeMap::new(),
            given: BTreeMap::new(),
            received: BTreeMap::new(),
            last_healing: Value::Null,
            combats: BTreeMap::new(),
            clock: 0,
            stacks: BTreeMap::new(),
            idents: BTreeMap::new(),
            series: BTreeMap::new(),
            last_rows: Vec::new(),
            last_timeline: Value::Null,
            generation: 0,
            counting: Counting::default(),
            wvw: false,
            pov: None,
            boons: BoonTracker::new(),
            buff_stacks: buffs::Stacks::default(),
            boon_map: None,
            roster: BTreeMap::new(),
            roster_pov: None,
            by_account: BTreeMap::new(),
            canon_of: BTreeMap::new(),
            live_of: BTreeMap::new(),
            uncanon: BTreeSet::new(),
            boon_seen: BTreeMap::new(),
            boon_tick: None,
        }
    }

    /// Change what is counted from now on (numbers already counted stay).
    pub fn set_counting(&mut self, c: Counting) {
        self.counting = c;
    }

    /// The counting settings leave `ev` out: a strike or condition tick on a WvW gadget or
    /// NPC that is switched off.
    fn skips(&self, reg: &Registry, ev: &Event) -> bool {
        let c = self.counting;
        if !self.wvw || (c.wvw_gadgets && c.wvw_npcs) {
            return false;
        }
        let dst = match *ev {
            Event::Strike { dst, .. } | Event::BuffDamage { dst, .. } => dst,
            _ => return false,
        };
        if self.owner(dst) != dst {
            return false; // a player's pet or clone
        }
        match reg.agent(dst).map(|a| a.kind) {
            Some(AgentKind::Gadget) => !c.wvw_gadgets,
            Some(AgentKind::Npc) => !c.wvw_npcs,
            _ => false,
        }
    }

    pub fn state(&self) -> LiveState {
        self.state
    }

    /// Changes whenever the snapshot (apart from its timestamp) may have changed.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The local player is `id` (live core follows the registry's POV in [`LiveStats::after_step`]).
    pub fn set_pov(&mut self, id: u32) {
        self.pov = Some(id);
    }

    /// The POV's healing breakdown: given by target and received by source, largest first.
    fn healing(&self, reg: &Registry) -> Value {
        json!({"out": self.heal_list(&self.heal_out, reg), "in": self.heal_list(&self.heal_in, reg)})
    }

    /// Who healed or was healed, largest first.
    fn heal_list(&self, m: &BTreeMap<u32, HealSum>, reg: &Registry) -> Vec<Value> {
        let mut v: Vec<(u64, Value)> = m
            .iter()
            .map(|(&id, h)| {
                let (name, prof, elite) = match (self.idents.get(&id), reg.agent(id)) {
                    (Some(i), _) => (i.name.clone(), i.prof, i.elite),
                    (None, Some(a)) => (a.character.clone(), a.prof, a.elite),
                    (None, None) => (String::new(), 0, 0),
                };
                let total = h.healing.saturating_add(h.barrier);
                (
                    total,
                    json!({
                        "name": name,
                        "prof": prof,
                        "elite": elite,
                        "me": self.pov == Some(id),
                        "healing": h.healing,
                        "barrier": h.barrier,
                    }),
                )
            })
            .collect();
        v.sort_by_key(|r| std::cmp::Reverse(r.0));
        v.into_iter().map(|(_, v)| v).collect()
    }

    /// Put each player's own-combat healing into its row (the POV and sharers), and the POV's
    /// into the healing breakdown.
    fn apply_own(&self, players: &mut [Value], healing: &mut Value, reg: &Registry) {
        for (&id, c) in &self.combats {
            let me = self.pov == Some(id);
            if c.start.is_none() || !(me || c.shared) {
                continue;
            }
            let (out, inc) = (OwnCombat::total(&c.out), OwnCombat::total(&c.inc));
            let ms = c.duration_ms(self.clock);
            let hps = if ms == 0 {
                0.0
            } else {
                (out.healing.saturating_add(out.barrier) as f64 * 10_000.0 / ms as f64).round()
                    / 10.0
            };
            let who = self.idents.get(&id);
            let row = players.iter_mut().find(|p| {
                if me {
                    return p["me"] == true;
                }
                who.is_some_and(|w| {
                    if w.account.is_empty() {
                        p["name"] == w.name.as_str()
                    } else {
                        p["account"] == w.account.as_str()
                    }
                })
            });
            if let Some(p) = row {
                p["healing"] = json!(out.healing);
                p["barrier"] = json!(out.barrier);
                p["hps"] = json!(hps);
                p["healing_in"] = json!(inc.healing);
                p["barrier_in"] = json!(inc.barrier);
                p["heal_full"] = json!(true);
            }
            if me {
                *healing = json!({
                    "out": self.heal_list(&c.out, reg),
                    "in": self.heal_list(&c.inc, reg),
                });
            }
        }
    }

    /// Count `minion`'s strikes toward `master`. Live core sets it from the poll's
    /// `AgentMaster` events ([`LiveStats::event`]).
    pub fn set_master(&mut self, minion: u32, master: u32) {
        self.masters.insert(minion, master);
    }

    /// Keep the master map current, in and out of fights: `AgentMaster` sets (0 clears) it, a
    /// despawn clears it.
    fn track_masters(&mut self, ev: &Event) {
        match *ev {
            Event::AgentMaster { id, master: 0 } | Event::AgentRemove { id } => {
                self.masters.remove(&id);
            }
            Event::AgentMaster { id, master } => self.set_master(id, master),
            _ => {}
        }
    }

    fn owner(&self, id: u32) -> u32 {
        self.masters.get(&id).copied().unwrap_or(id)
    }

    /// One copy of a heal, reported by `sender`: given by `by` (when it has a source) and
    /// received by `to`.
    fn count_heal(&mut self, sender: u32, has_src: bool, by: u32, to: u32, sum: HealSum) {
        if has_src {
            self.given
                .entry(by)
                .or_default()
                .add(sender == by, sender, sum);
        }
        self.received
            .entry(to)
            .or_default()
            .add(sender == to, sender, sum);
    }

    /// A fight started at `start_ms` on `map` (0 = unknown): numbers start from zero.
    pub fn begin(&mut self, start_ms: u64, map: u32) {
        self.state = LiveState::Fight;
        self.start_ms = start_ms;
        self.now_ms = start_ms;
        self.map = map;
        self.acc.clear();
        self.states.clear();
        self.clear_contrib();
        self.idents.clear();
        self.series.clear();
        self.heal_out.clear();
        self.heal_in.clear();
        self.given.clear();
        self.received.clear();
        self.last_rows.clear();
        self.last_timeline = Value::Null;
        self.last_healing = Value::Null;
        self.boons.clear();
        self.uncanon.clear();
        self.boon_seen.clear();
        self.boon_tick = None;
        // The stacks held at the start are the fight's first rows.
        let initial = self.buff_stacks.initial(start_ms);
        self.feed_boons(start_ms, initial);
        self.generation += 1;
    }

    /// Forget everything: a new game run (its agent ids mean nothing to the old numbers).
    pub fn clear(&mut self) {
        let (generation, counting) = (self.generation + 1, self.counting);
        *self = Self::new();
        self.generation = generation;
        self.counting = counting;
    }

    /// The fight ended at `end_ms`; `reg` is the registry as of the end. Its rows are kept.
    pub fn end(&mut self, end_ms: u64, reg: &Registry) {
        if self.state != LiveState::Fight {
            return;
        }
        self.now_ms = end_ms.max(self.start_ms);
        self.tick_boons(reg, true);
        self.last_rows = self.rows(reg);
        self.boons.clear();
        self.last_timeline = self.timeline();
        self.last_healing = self.healing(reg);
        self.heal_out.clear();
        self.heal_in.clear();
        self.given.clear();
        self.received.clear();
        self.state = LiveState::Last;
        self.series.clear();
        self.acc.clear();
        self.states.clear();
        self.clear_contrib();
        self.generation += 1;
    }

    /// Game time moved on to `now_ms` during the fight.
    pub fn advance(&mut self, now_ms: u64) {
        if self.state == LiveState::Fight && now_ms > self.now_ms {
            self.now_ms = now_ms;
            self.generation += 1;
        }
    }

    /// Follow every player's own combat and count its own heals in it (in and out of fights).
    fn own_event(&mut self, t: u64, ev: &Event) {
        self.clock = self.clock.max(t);
        match *ev {
            Event::Combat { id, in_combat } => {
                let c = self.combats.entry(id).or_default();
                if in_combat && !c.in_combat() {
                    // A new combat: the addon starts over.
                    c.start = Some(t);
                    c.end = None;
                    c.out.clear();
                    c.inc.clear();
                    self.generation += 1;
                } else if !in_combat && c.in_combat() {
                    c.end = Some(t);
                    self.generation += 1;
                }
                // Enemies and players who share nothing are not kept once out of combat.
                if !in_combat && !c.shared && self.pov != Some(id) {
                    self.combats.remove(&id);
                }
            }
            Event::AgentRemove { id }
                if self.pov != Some(id) && self.combats.get(&id).is_some_and(|c| !c.shared) =>
            {
                self.combats.remove(&id);
            }
            Event::Heal {
                src,
                dst,
                value,
                barrier,
                ..
            } => {
                if let Some(pov) = self.pov {
                    self.own_heal(pov, src, dst, value, barrier);
                }
            }
            Event::RelayHeal {
                sender,
                src,
                dst,
                value,
                barrier,
                ..
            } if self.pov != Some(sender) => self.own_heal(sender, src, dst, value, barrier),
            _ => {}
        }
    }

    /// One heal as reported by `sender`: it counts for the giver if `sender` gave it and for
    /// the receiver if `sender` got it, each in its own combat.
    fn own_heal(&mut self, sender: u32, src: u32, dst: u32, value: i32, barrier: u32) {
        if src == 0 || value <= 0 {
            return;
        }
        let total = u64::from(value.unsigned_abs());
        let bar = u64::from(barrier).min(total);
        let sum = HealSum {
            healing: total - bar,
            barrier: bar,
        };
        let (by, to) = (self.owner(src), self.owner(dst));
        if sender == by
            && let Some(c) = self.combats.get_mut(&by)
            && c.in_combat()
        {
            add_heal(&mut c.out, to, sum);
            c.shared = true;
            self.generation += 1;
        }
        if sender == to
            && let Some(c) = self.combats.get_mut(&to)
            && c.in_combat()
        {
            add_heal(&mut c.inc, by, sum);
            c.shared = true;
            self.generation += 1;
        }
    }

    /// Count one live event at game time `t`. Ignored outside a fight.
    pub fn event(&mut self, t: u64, ev: &Event) {
        self.track_masters(ev);
        let removed = self.track_stacks(t, ev);
        if self.state == LiveState::Idle
            && matches!(
                ev,
                Event::AgentAdd { .. } | Event::AgentName { .. } | Event::AgentRemove { .. }
            )
        {
            // Out of combat the feed lists the squad: a join, leave or name is a change.
            self.generation += 1;
        }
        self.own_event(t, ev);
        if self.state != LiveState::Fight {
            return;
        }
        if let Some((remover, dst, buff, n)) = removed
            && remover != 0
            && n > 0
        {
            let by = self.owner(remover);
            let a = self.acc.entry(by).or_default();
            if BOONS.contains(&buff) && by != dst {
                a.strips += n;
            } else if CONDITIONS.contains(&buff) {
                a.cleanses += n;
            } else {
                return;
            }
            self.generation += 1;
            return;
        }
        self.advance(t);
        match *ev {
            Event::Strike {
                src,
                dst,
                value,
                result,
                ..
            } => {
                let by = self.owner(src);
                if DAMAGE_RESULTS.contains(&result) && value > 0 {
                    if by == self.owner(dst) {
                        return;
                    }
                    let dmg = u64::from(value.unsigned_abs());
                    let a = self.acc.entry(by).or_default();
                    a.power = a.power.saturating_add(dmg);
                    let d = self.acc.entry(dst).or_default();
                    d.taken = d.taken.saturating_add(dmg);
                    self.add_series(t, by, dst, dmg);
                } else if (result == STRIKE_DOWN || result == STRIKE_KILL) && src != 0 && src != dst
                {
                    let a = self.acc.entry(by).or_default();
                    if result == STRIKE_DOWN {
                        a.downs += 1;
                    } else {
                        a.kills += 1;
                    }
                } else {
                    return;
                }
                self.generation += 1;
            }
            // The local player's condition ticks, dealt and taken (kind 42).
            Event::BuffDamage {
                src, dst, value, ..
            } => {
                let by = self.owner(src);
                if value <= 0 || by == self.owner(dst) {
                    return;
                }
                let dmg = u64::from(value.unsigned_abs());
                if src != 0 {
                    let a = self.acc.entry(by).or_default();
                    a.condi = a.condi.saturating_add(dmg);
                }
                let d = self.acc.entry(dst).or_default();
                d.taken = d.taken.saturating_add(dmg);
                self.add_series(t, if src != 0 { by } else { 0 }, dst, dmg);
                self.generation += 1;
            }
            // The local player's heals and barrier, given and received (kind 43).
            Event::Heal {
                src,
                dst,
                value,
                barrier,
                ..
            } => {
                if value <= 0 {
                    return;
                }
                let total = u64::from(value.unsigned_abs());
                let bar = u64::from(barrier).min(total);
                let sum = HealSum {
                    healing: total - bar,
                    barrier: bar,
                };
                let (by, to) = (self.owner(src), self.owner(dst));
                // A local heal is a copy sent by the POV.
                let sender = self.pov.unwrap_or(0);
                self.count_heal(sender, src != 0, by, to, sum);
                // Which side is the POV: the game reports only heals it gives or gets.
                let pov = self.pov;
                if src != 0 && pov.is_some_and(|p| p == by) {
                    add_heal(&mut self.heal_out, to, sum);
                }
                if pov.is_some_and(|p| p == to) && src != 0 {
                    add_heal(&mut self.heal_in, by, sum);
                }
                self.generation += 1;
            }
            // A squadmate's copy of a heal, relayed (kind 61). The POV's own heals come from
            // its local stream, so a copy "from" the POV is ignored.
            Event::RelayHeal {
                sender,
                src,
                dst,
                value,
                barrier,
                ..
            } => {
                if value <= 0 || self.pov == Some(sender) {
                    return;
                }
                let total = u64::from(value.unsigned_abs());
                let bar = u64::from(barrier).min(total);
                let sum = HealSum {
                    healing: total - bar,
                    barrier: bar,
                };
                let (by, to) = (self.owner(src), self.owner(dst));
                self.count_heal(sender, src != 0, by, to, sum);
                self.generation += 1;
            }
            Event::State { id, state } => {
                if self.states.insert(id, state) == Some(state) {
                    return;
                }
                let a = self.acc.entry(id).or_default();
                match state {
                    AgentState::Down => a.downed += 1,
                    AgentState::Dead => a.deaths += 1,
                    AgentState::Up => return,
                }
                self.generation += 1;
            }
            _ => {}
        }
    }

    /// Keep our stack copy, the roster and the map current (in and out of fights), and feed the
    /// rows a stack event wrote to the boon tracker while a fight records.
    fn track_boons(&mut self, t: u64, ev: &Event) {
        match *ev {
            Event::Map { map_id, shard, .. } => {
                if self.boon_map.is_some_and(|m| m != (map_id, shard)) {
                    // Instance ids are reused per map.
                    self.buff_stacks.clear();
                    self.roster.clear();
                    self.roster_pov = None;
                    self.by_account.clear();
                    self.canon_of.clear();
                    self.live_of.clear();
                    self.uncanon.clear();
                }
                self.boon_map = Some((map_id, shard));
            }
            Event::AgentAdd { id, kind, team, .. } => {
                self.roster.insert(id, (kind == AgentKind::Player, team));
            }
            Event::Pov { id } => self.roster_pov = Some(id),
            Event::AgentRemove { id } => {
                self.roster.remove(&id);
                self.retire_account(id);
                self.buff_stacks.forget_agent(id);
            }
            Event::BuffApply { .. }
            | Event::BuffRemove { .. }
            | Event::BuffRemoveAll { .. }
            | Event::BuffStack { .. } => {
                let rows = self.buff_stacks.apply(t, ev);
                if self.state == LiveState::Fight {
                    self.feed_boons(t, rows);
                }
            }
            _ => {}
        }
    }

    /// Whether `id` can not be a squad player: a pet, an NPC, or a player on another team than the
    /// POV. An agent not seen yet, or a player not named yet, may still turn out to be squad.
    fn foreign(&self, id: u32) -> bool {
        match self.roster.get(&id) {
            None => false,
            Some(&(player, team)) => {
                !player
                    || self
                        .roster_pov
                        .and_then(|p| self.roster.get(&p))
                        .is_some_and(|&(_, pt)| pt != team)
            }
        }
    }

    /// One id per account, so a rejoining player keeps its boons.
    fn canon(&self, id: u32) -> u32 {
        self.canon_of.get(&id).copied().unwrap_or(id)
    }

    /// The tracker id a row's boons are under: its account's, unless another live id stands for
    /// the account now (that row shows them).
    fn boon_id(&self, id: u32, account: &str) -> u32 {
        match self.by_account.get(account) {
            Some(&c) if self.live_of.get(&c).is_none_or(|&l| l == id) => c,
            _ => self.canon(id),
        }
    }

    /// Remember which account `id` is, once the registry knows it. An account's id for the
    /// tracker is its own number (above every live id), so a reused live id never meets it.
    fn note_account(&mut self, reg: &Registry, id: u32) {
        let Some(a) = reg.agent(id) else {
            return;
        };
        if a.kind != AgentKind::Player || a.account.is_empty() {
            return;
        }
        let next = ACCOUNT_IDS + self.by_account.len() as u32;
        let c = *self.by_account.entry(a.account.clone()).or_insert(next);
        if self.canon_of.get(&id).is_some_and(|&old| old != c) {
            self.retire_account(id);
        }
        if self.uncanon.remove(&id) {
            // Rows fed before the account was known went in under the raw id.
            self.boons.rename(id, c);
        }
        self.canon_of.insert(id, c);
        self.live_of.insert(c, id);
    }

    /// `id` is gone or was given to someone else: it no longer stands for its account.
    fn retire_account(&mut self, id: u32) {
        self.canon_of.remove(&id);
        self.live_of.retain(|_, &mut live| live != id);
    }

    /// Give the tracker `rows` written at `t`: sources resolved to their master and to one id per
    /// account, holders that can not be squad left out (the tracker keeps every row it gets, and
    /// a big fight has hundreds of enemy holders).
    fn feed_boons(&mut self, t: u64, rows: Vec<buffs::Row>) {
        let mut keep = Vec::new();
        for mut r in rows {
            let squad_boon = SQUAD_BOONS.contains(&r.skill);
            let holder = boons::owner(&r);
            if !squad_boon || self.foreign(holder) {
                continue;
            }
            let source = |s: &Self, id: u32| if id == 0 { 0 } else { s.canon(s.owner(id)) };
            match r.statechange {
                BUFF_INITIAL | BUFF_APPLY => {
                    r.src = source(self, r.src);
                    r.dst = self.canon(r.dst);
                }
                BUFF_DURATION => r.dst = self.canon(r.dst),
                BUFF_REMOVE_ONE | BUFF_REMOVE_ALL => {
                    r.src = self.canon(r.src);
                    r.dst = source(self, r.dst);
                }
                STACK_ACTIVE | STACK_RESET => r.src = self.canon(r.src),
                _ => continue,
            }
            for id in [r.src, r.dst] {
                if id != 0 && id < ACCOUNT_IDS {
                    self.uncanon.insert(id);
                }
            }
            keep.push(r);
        }
        self.boons.rows(t, &keep);
    }

    /// Bring the boon totals to the latest game time: at most once a second, or `force`d.
    fn tick_boons(&mut self, reg: &Registry, force: bool) {
        let now = self.now_ms;
        match self.boon_tick {
            Some(last) if last == now || (!force && now < last + 1000) => return,
            _ => {}
        }
        self.boon_tick = Some(now);
        for (&c, &id) in &self.live_of {
            if let Some(a) = reg.agent(id) {
                self.boon_seen.insert(c, (reg.is_squad(id), a.subgroup));
            }
        }
        let live_of = &self.live_of;
        let seen = &self.boon_seen;
        // A player who left keeps the squad and subgroup it had (axilog keeps every player of
        // the log as a holder, source and target).
        let here = |c: u32| {
            live_of
                .get(&c)
                .copied()
                .filter(|&id| reg.agent(id).is_some())
        };
        let groups = |c: u32| match here(c) {
            Some(id) => reg.agent(id).and_then(|a| a.subgroup),
            None => seen
                .get(&c)
                .map_or_else(|| reg.agent(c).and_then(|a| a.subgroup), |s| s.1),
        };
        let squad = |c: u32| match here(c) {
            Some(id) => reg.is_squad(id),
            None => seen.get(&c).map_or_else(|| reg.is_squad(c), |s| s.0),
        };
        // Regeneration's stack order reads the agent table's healing field, which arcdps and our
        // writer leave at 0 for every player: 0 is what axilog sees on both logs.
        self.boons.tick(now, &groups, &squad, &|_| 0);
    }

    /// Keep the instance map current; a removal returns (remover, target, buff, stacks removed).
    fn track_stacks(&mut self, t: u64, ev: &Event) -> Option<(u32, u32, u32, u64)> {
        self.track_boons(t, ev);
        match *ev {
            Event::BuffApply {
                dst, buff, inst, ..
            } => {
                self.stacks.insert(inst, (dst, buff));
                None
            }
            Event::BuffRemove { dst, remover, inst } => {
                let (_, buff) = self.stacks.remove(&inst)?;
                Some((remover, dst, buff, 1))
            }
            Event::BuffRemoveAll { dst, remover, buff } => {
                let before = self.stacks.len();
                self.stacks.retain(|_, &mut (d, b)| (d, b) != (dst, buff));
                Some((remover, dst, buff, (before - self.stacks.len()) as u64))
            }
            Event::AgentRemove { id } => {
                self.stacks.retain(|_, &mut (d, _)| d != id);
                None
            }
            _ => None,
        }
    }

    /// Follow the processor after `step` was applied to it (`outcome` is what that returned):
    /// fight start and end, live events, and game time.
    pub fn after_step(&mut self, p: &Processor, step: &Step, outcome: Option<&FightOutcome>) {
        let (time, ev) = match *step {
            Step::Push(t, ev) => (Some(t), Some(ev)),
            Step::Tick(t) => (Some(t), None),
            _ => (None, None),
        };
        let current = p.fight_start_ms();
        // One push can end a fight (inactivity) and start the next one.
        if self.state == LiveState::Fight && (outcome.is_some() || current != Some(self.start_ms)) {
            match outcome {
                Some(FightOutcome::Ended { fight, registry }) => self.end(fight.end_ms, registry),
                Some(FightOutcome::Discarded { duration_ms, .. }) => {
                    self.end(self.start_ms.saturating_add(*duration_ms), p.registry())
                }
                None => self.end(self.now_ms, p.registry()),
            }
        }
        if let Some(start) = current {
            if self.state != LiveState::Fight {
                // The poll may have named agents before this core was watching.
                let reg = p.registry();
                for a in reg.live_agents() {
                    if let Some(team) = a.team {
                        self.roster
                            .entry(a.id)
                            .or_insert((a.kind == AgentKind::Player, team));
                    }
                    self.note_account(reg, a.id);
                }
                if let Some(id) = reg.pov() {
                    self.roster_pov = Some(id);
                }
                self.begin(start, p.map_id().unwrap_or(0));
            } else if self.map == 0
                && let Some(m) = p.map_id()
            {
                self.map = m;
                self.generation += 1;
            }
        }
        self.wvw = p.map_type().is_some_and(counts_as_wvw);
        if let Some(id) = p.registry().pov() {
            self.pov = Some(id);
        }
        if let Some(Event::AgentAdd { id, .. }) = ev {
            // A new life of the id (the registry has forgotten the old account, if any).
            self.retire_account(id);
        }
        if let Some(Event::AgentAdd { id, .. } | Event::AgentName { id, .. }) = ev {
            self.note_account(p.registry(), id);
        }
        if let Some(ev) = ev
            && let Some(t) = time
        {
            if self.skips(p.registry(), &ev) {
                self.track_masters(&ev);
                self.track_stacks(t, &ev);
            } else {
                self.event(t, &ev);
            }
            self.contrib_event(p.registry(), t, &ev);
        }
        if let Some(t) = time {
            self.advance(t);
        }
    }

    fn clear_contrib(&mut self) {
        self.recent.clear();
        self.full_at.clear();
        self.reset.clear();
    }

    /// Where `target`'s down-contribution window starts: 2 s before it was last at 99% health
    /// or more (the fight start if not seen), no earlier than its reset after its last down.
    fn window_start(&self, target: u32) -> u64 {
        let anchor = self.full_at.get(&target).copied().unwrap_or(self.start_ms);
        let lo = anchor.saturating_sub(CONTRIB_LEAD_IN_MS).max(self.start_ms);
        lo.max(self.reset.get(&target).copied().unwrap_or(0))
    }

    /// `id`'s health is `pct_x100` at `t`; at 99% or more its window restarts.
    fn health(&mut self, t: u64, id: u32, pct_x100: u16) {
        if pct_x100 < CONTRIB_FULL_X100 {
            return;
        }
        self.full_at.insert(id, t);
        let lo = self.window_start(id);
        if let Some(v) = self.recent.get_mut(&id) {
            v.retain(|&(at, _, _)| at >= lo);
        }
    }

    /// Enemy player `target` went down at `t` (a downing strike): everyone not on its side who
    /// damaged it in its window `[window_start, t]` gets that damage as down contribution
    /// (axilog's arcdps methodology). A down within 2.1 s of the last one gets nothing more.
    pub fn enemy_down(&mut self, t: u64, target: u32) {
        if self.state != LiveState::Fight {
            return;
        }
        let lo = self.window_start(target);
        self.reset
            .insert(target, t.saturating_add(CONTRIB_RESET_MS));
        let Some(hits) = self.recent.remove(&target) else {
            return;
        };
        let side = self.owner(target);
        for (at, by, dmg) in hits {
            if at < lo || at > t || by == 0 || by == side {
                continue;
            }
            let a = self.acc.entry(by).or_default();
            a.down_contrib = a.down_contrib.saturating_add(dmg);
        }
        self.generation += 1;
    }

    /// Health and downing strikes for down contribution, after `ev` was counted.
    fn contrib_event(&mut self, reg: &Registry, t: u64, ev: &Event) {
        match *ev {
            Event::Health { id, pct_x100, .. } => self.health(t, id, pct_x100),
            Event::Strike { .. } | Event::BuffDamage { .. } if self.state == LiveState::Fight => {
                let (src, dst, value, down) = match *ev {
                    Event::Strike {
                        src,
                        dst,
                        value,
                        result,
                        ..
                    } => (
                        src,
                        dst,
                        if DAMAGE_RESULTS.contains(&result) {
                            value
                        } else {
                            0
                        },
                        result == STRIKE_DOWN,
                    ),
                    Event::BuffDamage {
                        src, dst, value, ..
                    } => (src, dst, value, false),
                    _ => return,
                };
                // Only enemy players' downs are counted, so only their damage is kept.
                if !reg.agent(dst).is_some_and(|a| a.kind == AgentKind::Player) || reg.is_squad(dst)
                {
                    return;
                }
                if value > 0 && src != 0 {
                    let by = self.owner(src);
                    let v = self.recent.entry(dst).or_default();
                    if v.len() < CONTRIB_MAX_HITS {
                        v.push((t, by, u64::from(value.unsigned_abs())));
                    }
                }
                if down {
                    self.enemy_down(t, dst);
                }
            }
            _ => {}
        }
    }

    /// `dmg` at game time `t`, dealt by `by` (0: no one to credit) to `dst`.
    fn add_series(&mut self, t: u64, by: u32, dst: u32, dmg: u64) {
        let i = (t.saturating_sub(self.start_ms) / BUCKET_MS) as usize;
        if by != 0 {
            add_at(&mut self.series.entry(by).or_default().out, i, dmg);
        }
        add_at(&mut self.series.entry(dst).or_default().inc, i, dmg);
    }

    /// The squad's damage dealt and taken per bucket, through the fight's current duration.
    /// Squad = the members [`LiveStats::rows`] last listed.
    fn timeline(&self) -> Value {
        let n = (self.duration_ms().div_ceil(BUCKET_MS) as usize).min(MAX_BUCKETS);
        let (mut out, mut inc) = (vec![0u64; n], vec![0u64; n]);
        for (id, s) in &self.series {
            if !self.idents.contains_key(id) {
                continue;
            }
            for (o, v) in out.iter_mut().zip(&s.out) {
                *o += v;
            }
            for (o, v) in inc.iter_mut().zip(&s.inc) {
                *o += v;
            }
        }
        json!({"bucket_ms": BUCKET_MS, "out": out, "in": inc})
    }

    fn duration_ms(&self) -> u64 {
        match self.state {
            LiveState::Idle => 0,
            LiveState::Fight | LiveState::Last => self.now_ms.saturating_sub(self.start_ms),
        }
    }

    /// Rows for the squad members in `reg` and everyone with numbers who was last seen as one,
    /// highest damage first.
    fn rows(&mut self, reg: &Registry) -> Vec<Value> {
        if self.state == LiveState::Fight {
            self.tick_boons(reg, false);
        }
        let mut ids: std::collections::BTreeSet<u32> = self.acc.keys().copied().collect();
        ids.extend(
            reg.live_agents()
                .map(|a| a.id)
                .filter(|&id| reg.is_squad(id)),
        );
        let duration_ms = self.duration_ms();
        let mut rows: Vec<(u64, Value)> = Vec::new();
        for id in ids {
            if let Some(a) = reg.agent(id) {
                if reg.is_squad(id) {
                    self.idents.insert(
                        id,
                        Ident {
                            name: a.character.clone(),
                            account: a.account.clone(),
                            prof: a.prof,
                            elite: a.elite,
                            group: a.subgroup.unwrap_or(0),
                            me: reg.pov() == Some(id),
                        },
                    );
                } else {
                    self.idents.remove(&id);
                }
            }
            let Some(who) = self.idents.get(&id) else {
                continue;
            };
            let n = self.acc.get(&id).copied().unwrap_or_default();
            let damage = n.power.saturating_add(n.condi);
            let given = self.given.get(&id).map(CopySums::shown).unwrap_or_default();
            let received = self
                .received
                .get(&id)
                .map(CopySums::shown)
                .unwrap_or_default();
            let heal_full = who.me || self.given.get(&id).is_some_and(|c| c.own.is_some());
            let per_s = |n: u64| {
                if duration_ms == 0 {
                    0.0
                } else {
                    (n as f64 * 10_000.0 / duration_ms as f64).round() / 10.0
                }
            };
            let dps = per_s(damage);
            let boons: serde_json::Map<String, Value> = self
                .boons
                .totals(self.boon_id(id, &who.account))
                .into_iter()
                .flatten()
                .filter(|(_, v)| v.iter().any(|&n| n > 0))
                .map(|(b, v)| (b.to_string(), json!(v)))
                .collect();
            let mut row = json!({
                    "name": who.name,
                    "account": who.account,
                    "prof": who.prof,
                    "elite": who.elite,
                    "spec": crate::names::spec_name(who.prof, who.elite),
                    "group": who.group,
                    "me": who.me,
                    "damage": damage,
                    "power": n.power,
                    "condi": n.condi,
                    "dps": dps,
                    "downs": n.downs,
                    "kills": n.kills,
                    "deaths": n.deaths,
                    "downed": n.downed,
                    "taken": n.taken,
                    "strips": n.strips,
                    "cleanses": n.cleanses,
                    "down_contrib": n.down_contrib,
                    "healing": given.healing,
                    "barrier": given.barrier,
                    "hps": per_s(given.healing.saturating_add(given.barrier)),
                    "healing_in": received.healing,
                    "barrier_in": received.barrier,
                    // Complete for the POV and for players sharing their heals; for anyone
                    // else it is what the best-informed sharer saw.
                    "heal_full": heal_full,
            });
            if !boons.is_empty() {
                row["boons"] = Value::Object(boons);
            }
            rows.push((damage, row));
        }
        rows.sort_by_key(|r| std::cmp::Reverse(r.0));
        rows.into_iter().map(|(_, v)| v).collect()
    }

    /// The schema v1 value at wall time `unix_ms`. `reg` is the registry now (used while a
    /// fight is recording; an ended fight shows its players as of its end).
    pub fn snapshot(&mut self, reg: &Registry, unix_ms: u64) -> Value {
        let (state, mut players, timeline, mut healing) = match self.state {
            // Before the first fight: the squad (or party) as it is, every number zero.
            LiveState::Idle => ("idle", self.rows(reg), Value::Null, Value::Null),
            LiveState::Fight => {
                let rows = self.rows(reg);
                ("fight", rows, self.timeline(), self.healing(reg))
            }
            LiveState::Last => (
                "last",
                self.last_rows.clone(),
                self.last_timeline.clone(),
                self.last_healing.clone(),
            ),
        };
        self.apply_own(&mut players, &mut healing, reg);
        let map = if self.state == LiveState::Idle {
            0
        } else {
            self.map
        };
        json!({
            "v": 1,
            "t": unix_ms,
            "state": state,
            "fight": {"duration_ms": self.duration_ms(), "map": map},
            "players": players,
            "timeline": timeline,
            "healing": healing,
        })
    }
}

/// Writes the feed to a file, atomically, at most every [`MIN_INTERVAL`] and only on change.
pub struct LiveWriter {
    path: PathBuf,
    /// When the last write was tried and the generation it held.
    last: Option<(Instant, u64)>,
}

impl LiveWriter {
    pub fn new(path: PathBuf) -> Self {
        Self { path, last: None }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// A write of `generation` is due at `now`: the first one, or a changed one at least
    /// [`MIN_INTERVAL`] after the last.
    pub fn due(&self, now: Instant, generation: u64) -> bool {
        match self.last {
            None => true,
            Some((at, g)) => g != generation && now.duration_since(at) >= MIN_INTERVAL,
        }
    }

    /// `generation` was never written (ignoring the interval; for a final write on exit).
    pub fn changed(&self, generation: u64) -> bool {
        self.last.is_none_or(|(_, g)| g != generation)
    }

    /// Write `json` (of `generation`). A failed write is not retried until the next change.
    pub fn write(&mut self, now: Instant, generation: u64, json: &Value) -> io::Result<()> {
        self.last = Some((now, generation));
        let bytes = serde_json::to_vec(json).map_err(io::Error::other)?;
        write_atomic(&self.path, &bytes)
    }
}

/// Write `bytes` to `path` through `<path>.tmp` and a rename, so readers never see a partial
/// file.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::tests::{add, name_events};
    use crate::rules::WvwRules;
    use axidps_protocol::v2::{AgentKind, NameField};

    const NPC: u32 = 3;
    const ENEMY: u32 = 2;
    const PET: u32 = 50;

    fn strike(src: u32, dst: u32, value: i32, result: u8) -> Event {
        Event::Strike {
            src,
            dst,
            skill: 1,
            value,
            overstack: 0,
            result,
        }
    }

    fn squad_events(id: u32, group: u8) -> Vec<Event> {
        let mut v = vec![add(id, group)];
        v.extend(name_events(id, NameField::Character, &format!("Char {id}")));
        v.extend(name_events(
            id,
            NameField::Account,
            &format!(":Alpha.{id:04}"),
        ));
        v
    }

    fn world_events() -> Vec<Event> {
        let mut v = squad_events(1, 3);
        v.extend(squad_events(4, 2));
        v.push(Event::AgentAdd {
            id: ENEMY,
            kind: AgentKind::Player,
            prof: 2,
            elite: 0,
            species: 0,
            team: 2,
            subgroup: 0,
            wvw_rank: 0,
            wvw_rank_def: 0,
        });
        v.push(Event::AgentAdd {
            id: NPC,
            kind: AgentKind::Npc,
            prof: 0,
            elite: 0,
            species: 77,
            team: 9,
            subgroup: 0,
            wvw_rank: 0,
            wvw_rank_def: 0,
        });
        v.push(Event::Pov { id: 1 });
        v
    }

    /// Squad players 1 (POV, group 3) and 4 (group 2), enemy player 2, NPC 3.
    fn world() -> Registry {
        let mut r = Registry::new();
        for e in world_events() {
            r.apply(0, &e);
        }
        r
    }

    fn feed(l: &mut LiveStats, _r: &Registry, evs: &[(u64, Event)]) {
        for (t, e) in evs {
            l.event(*t, e);
        }
    }

    fn player<'a>(v: &'a Value, name: &str) -> Option<&'a Value> {
        v["players"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == name)
    }

    /// Down contribution follows axilog: damage to an enemy player from 2 s before it was last at
    /// 99%+ health through its down counts; a down within 2.1 s of the last adds nothing older.
    #[test]
    fn down_contribution_uses_the_health_anchored_window() {
        let r = world();
        let mut l = LiveStats::new();
        l.begin(0, 38);
        let hp = |pct_x100| Event::Health {
            id: ENEMY,
            pct_x100,
            max: 1000,
        };
        let steps: Vec<(u64, Event)> = vec![
            (1_000, strike(1, ENEMY, 100, 0)), // before the window: dropped
            (5_000, hp(10_000)),               // full health: window opens at 3 000
            (4_000, strike(4, ENEMY, 40, 0)),  // late row inside the lead-in
            (6_000, hp(5_000)),
            (7_000, strike(1, ENEMY, 300, 0)),
            (7_500, strike(4, ENEMY, 7, 3)), // condi-free crit (result 3): damage
            (7_600, strike(1, NPC, 999, 0)), // NPC damage: not an enemy player
            (8_000, strike(1, ENEMY, 0, STRIKE_DOWN)),
            (9_000, strike(1, ENEMY, 50, 0)), // downed: inside the reset, next down
            (9_500, strike(4, ENEMY, 0, STRIKE_DOWN)),
            (12_000, strike(4, ENEMY, 20, 0)),
            (12_500, strike(4, ENEMY, 0, STRIKE_DOWN)),
        ];
        for (t, e) in &steps {
            l.event(*t, e);
            l.contrib_event(&r, *t, e);
        }
        let v = l.snapshot(&r, 0);
        assert_eq!(player(&v, "Char 1").unwrap()["down_contrib"], 300);
        assert_eq!(player(&v, "Char 4").unwrap()["down_contrib"], 40 + 7 + 20);
    }

    /// The POV's heals and barrier count to its row and its breakdown, its pet's to it; others
    /// show only what passed between them and the POV.
    #[test]
    fn heals_count_given_and_received_with_the_povs_breakdown() {
        let r = world();
        let mut l = LiveStats::new();
        l.set_pov(1);
        l.set_master(PET, 1);
        l.begin(0, 38);
        let heal = |src, dst, value, barrier| Event::Heal {
            src,
            dst,
            skill: 9,
            value,
            barrier,
            buff: false,
        };
        feed(
            &mut l,
            &r,
            &[
                (1_000, heal(1, 1, 500, 0)),     // self-heal
                (2_000, heal(PET, 4, 300, 100)), // pet: 200 healing + 100 barrier to Char 4
                (3_000, heal(4, 1, 250, 50)),    // Char 4 heals the POV
                (4_000, heal(1, 4, 0, 0)),       // nothing: ignored
            ],
        );
        let v = l.snapshot(&r, 0);
        let me = player(&v, "Char 1").unwrap();
        assert_eq!(me["healing"], 700);
        assert_eq!(me["barrier"], 100);
        assert_eq!(me["healing_in"], 700);
        assert_eq!(me["barrier_in"], 50);
        assert_eq!(me["heal_full"], true);
        let other = player(&v, "Char 4").unwrap();
        assert_eq!(other["healing"], 200);
        assert_eq!(other["barrier"], 50);
        assert_eq!(other["healing_in"], 200);
        assert_eq!(other["heal_full"], false);
        let out = v["healing"]["out"].as_array().unwrap();
        assert_eq!(out[0]["name"], "Char 1");
        assert_eq!(out[0]["healing"], 500);
        assert_eq!(out[1]["name"], "Char 4");
        assert_eq!(out[1]["barrier"], 100);
        let inc = v["healing"]["in"].as_array().unwrap();
        assert_eq!(inc.len(), 2);
        assert_eq!(inc[0]["name"], "Char 1");
        assert_eq!(inc[1]["healing"], 200);
        l.end(5_000, &r);
        assert_eq!(l.snapshot(&r, 0)["healing"]["out"][0]["healing"], 500);
    }

    /// The POV's healing follows its own combat like the healing addon: heals before and
    /// after core's fight count, heals out of combat don't, a new combat starts from zero.
    #[test]
    fn pov_healing_follows_own_combat() {
        let r = world();
        let mut l = LiveStats::new();
        l.set_pov(1);
        let heal = |src, dst, value, barrier| Event::Heal {
            src,
            dst,
            skill: 9,
            value,
            barrier,
            buff: false,
        };
        let combat = |in_combat| Event::Combat { id: 1, in_combat };
        feed(
            &mut l,
            &r,
            &[
                (100, heal(1, 4, 999, 0)), // out of combat: not counted
                (500, combat(true)),
                (600, heal(1, 4, 300, 0)), // before core's fight
                (700, heal(4, 1, 80, 0)),  // received
            ],
        );
        l.begin(1_000, 38);
        feed(&mut l, &r, &[(1_500, heal(1, 1, 200, 50))]);
        l.end(2_000, &r);
        feed(
            &mut l,
            &r,
            &[
                (2_500, heal(1, 4, 100, 0)), // after the fight, still in combat
                (3_000, combat(false)),
                (3_500, heal(1, 4, 777, 0)), // out of combat again
            ],
        );
        let v = l.snapshot(&r, 0);
        let me = player(&v, "Char 1").unwrap();
        assert_eq!(me["healing"], 550);
        assert_eq!(me["barrier"], 50);
        assert_eq!(me["healing_in"], 230);
        // 600 over 2.5 s of combat.
        assert_eq!(me["hps"], 240.0);
        let out = v["healing"]["out"].as_array().unwrap();
        assert_eq!(out[0]["name"], "Char 4");
        assert_eq!(out[0]["healing"], 400);
        // A new combat starts over.
        feed(&mut l, &r, &[(4_000, combat(true))]);
        let v = l.snapshot(&r, 0);
        assert_eq!(player(&v, "Char 1").unwrap()["healing"], 0);
    }

    /// A sharer's healing follows its own combat (the addon's "peers outgoing"): counted from
    /// its own copies between its combat entry and exit, across core's fight boundaries.
    #[test]
    fn sharer_healing_follows_its_own_combat() {
        let r = world();
        let mut l = LiveStats::new();
        l.set_pov(1);
        let combat = |id, in_combat| Event::Combat { id, in_combat };
        feed(
            &mut l,
            &r,
            &[
                (100, relay(4, 4, 1, 999)), // out of combat: not counted
                (500, combat(4, true)),
                (600, relay(4, 4, 1, 300)), // before core's fight
                (650, relay(6, 4, 6, 50)),  // Char 6's copy: not Char 4's own
            ],
        );
        l.begin(1_000, 38);
        feed(&mut l, &r, &[(1_500, relay(4, 4, 6, 200))]);
        l.end(2_000, &r);
        feed(
            &mut l,
            &r,
            &[
                (3_000, combat(4, false)),
                (3_500, relay(4, 4, 6, 777)), // out of combat again
            ],
        );
        let v = l.snapshot(&r, 0);
        let p4 = player(&v, "Char 4").unwrap();
        assert_eq!(p4["healing"], 500);
        assert_eq!(p4["heal_full"], true);
        // 500 over 2.5 s of its combat.
        assert_eq!(p4["hps"], 200.0);
    }

    fn relay(sender: u32, src: u32, dst: u32, value: i32) -> Event {
        Event::RelayHeal {
            sender,
            src,
            dst,
            skill: 9,
            value,
            barrier: 0,
            buff: false,
            downed: false,
        }
    }

    fn local_heal(src: u32, dst: u32, value: i32) -> Event {
        Event::Heal {
            src,
            dst,
            skill: 9,
            value,
            barrier: 0,
            buff: false,
        }
    }

    /// The world plus a second sharing squadmate, Char 6.
    fn relay_world() -> Registry {
        let mut r = world();
        for e in squad_events(6, 3) {
            r.apply(0, &e);
        }
        r
    }

    #[test]
    fn one_heal_three_copies_counts_once() {
        let r = relay_world();
        let mut l = LiveStats::new();
        l.set_pov(1);
        l.begin(0, 38);
        feed(
            &mut l,
            &r,
            &[
                (1_000, local_heal(4, 1, 300)),
                (1_000, relay(4, 4, 1, 300)),
                (1_000, relay(6, 4, 1, 300)),
            ],
        );
        let v = l.snapshot(&r, 0);
        assert_eq!(player(&v, "Char 4").unwrap()["healing"], 300);
        assert_eq!(player(&v, "Char 1").unwrap()["healing_in"], 300);
    }

    #[test]
    fn pov_copy_from_relay_is_ignored() {
        let r = relay_world();
        let mut l = LiveStats::new();
        l.set_pov(1);
        l.begin(0, 38);
        feed(&mut l, &r, &[(1_000, relay(1, 4, 6, 300))]);
        let v = l.snapshot(&r, 0);
        assert_eq!(player(&v, "Char 4").unwrap()["healing"], 0);
        assert_eq!(player(&v, "Char 6").unwrap()["healing_in"], 0);
    }

    #[test]
    fn non_sharing_giver_counts_from_one_sender() {
        let r = relay_world();
        let mut l = LiveStats::new();
        l.set_pov(1);
        l.begin(0, 38);
        // Char 6 does not share; Char 4 and the POV both saw some of its heals.
        feed(
            &mut l,
            &r,
            &[
                (1_000, relay(4, 6, 4, 100)),
                (2_000, relay(4, 6, 1, 200)),
                (3_000, local_heal(6, 1, 200)),
            ],
        );
        let v = l.snapshot(&r, 0);
        // Char 4 saw 300, the POV 200: the larger, not 500.
        assert_eq!(player(&v, "Char 6").unwrap()["healing"], 300);
        assert_eq!(player(&v, "Char 6").unwrap()["heal_full"], false);
    }

    #[test]
    fn heal_full_for_sharing_players() {
        let r = relay_world();
        let mut l = LiveStats::new();
        l.set_pov(1);
        l.begin(0, 38);
        feed(
            &mut l,
            &r,
            &[(1_000, relay(4, 4, 6, 100)), (1_000, relay(6, 4, 6, 100))],
        );
        let v = l.snapshot(&r, 0);
        assert_eq!(player(&v, "Char 4").unwrap()["heal_full"], true);
        assert_eq!(player(&v, "Char 6").unwrap()["heal_full"], false);
        assert_eq!(player(&v, "Char 1").unwrap()["heal_full"], true);
        // Char 6's own received copy is absent, so the one sender counts once.
        assert_eq!(player(&v, "Char 6").unwrap()["healing_in"], 100);
    }

    #[test]
    fn counting_reads_the_overlay_ui_json() {
        let v: Value =
            serde_json::from_str(r#"{"theme":"glass","core":{"wvw_npcs":false}}"#).unwrap();
        let c = Counting::from_ui_json(&v);
        assert!(c.wvw_gadgets && !c.wvw_npcs);
        assert_eq!(Counting::from_ui_json(&Value::Null), Counting::default());
    }

    #[test]
    fn wvw_counting_applies_in_wvw_and_armistice_bastion_only() {
        assert!(counts_as_wvw(9) && counts_as_wvw(15) && counts_as_wvw(WVW_LOUNGE));
        assert!(!counts_as_wvw(5) && !counts_as_wvw(4) && !counts_as_wvw(2));
    }

    #[test]
    fn wvw_counting_leaves_out_npc_targets_but_not_players_or_pets() {
        let mut r = world();
        r.apply(
            0,
            &Event::AgentAdd {
                id: 60,
                kind: AgentKind::Gadget,
                prof: 0,
                elite: 0,
                species: 0,
                team: 9,
                subgroup: 0,
                wvw_rank: 0,
                wvw_rank_def: 0,
            },
        );
        let mut l = LiveStats::new();
        l.set_counting(Counting {
            wvw_gadgets: false,
            wvw_npcs: false,
        });
        assert!(!l.skips(&r, &strike(1, NPC, 500, 0)), "only in WvW");
        l.wvw = true;
        assert!(l.skips(&r, &strike(1, NPC, 500, 0)));
        assert!(l.skips(&r, &strike(1, 60, 500, 0)));
        assert!(!l.skips(&r, &strike(1, ENEMY, 500, 0)));
        l.set_master(NPC, ENEMY); // the NPC is the enemy player's pet
        assert!(!l.skips(&r, &strike(1, NPC, 500, 0)));
        l.set_counting(Counting {
            wvw_gadgets: true,
            wvw_npcs: false,
        });
        assert!(!l.skips(&r, &strike(1, 60, 500, 0)));
    }

    #[test]
    fn damage_and_dps_from_strikes() {
        let r = world();
        let mut l = LiveStats::new();
        l.begin(1000, 38);
        feed(
            &mut l,
            &r,
            &[
                (1100, strike(1, NPC, 500, 0)),
                (1200, strike(1, NPC, 300, 1)),
                (1300, strike(1, NPC, 100, 2)),
                // Breakbar and CC pseudo-hits are not damage.
                (1400, strike(1, NPC, 125, 10)),
                (1500, strike(1, NPC, 900, 12)),
                // A block carries no damage.
                (1600, strike(1, NPC, 0, 3)),
            ],
        );
        l.advance(3000);
        let v = l.snapshot(&r, 42);
        assert_eq!(v["state"], "fight");
        assert_eq!(v["fight"]["duration_ms"], 2000);
        assert_eq!(v["fight"]["map"], 38);
        let p = player(&v, "Char 1").unwrap();
        assert_eq!(p["damage"], 900);
        assert_eq!(p["power"], 900);
        assert_eq!(p["condi"], 0);
        assert_eq!(p["dps"], 450.0);
        assert_eq!(p["account"], ":Alpha.0001");
        assert_eq!(p["group"], 3);
        assert_eq!(p["prof"], 1);
        assert_eq!(p["me"], true);
        let q = player(&v, "Char 4").unwrap();
        assert_eq!(q["me"], false);
        assert_eq!(q["damage"], 0);
        assert_eq!(q["dps"], 0.0);
        // Sorted by damage, highest first.
        assert_eq!(v["players"][0]["name"], "Char 1");
    }

    #[test]
    fn timeline_sums_squad_damage_per_second() {
        let r = world();
        let mut l = LiveStats::new();
        l.begin(1000, 38);
        feed(
            &mut l,
            &r,
            &[
                (1100, strike(1, NPC, 500, 0)),
                (1900, strike(1, NPC, 300, 1)),
                (2100, strike(NPC, 1, 200, 0)),
                (3500, strike(1, NPC, 50, 0)),
                // Not damage: no bucket.
                (3600, strike(1, NPC, 125, 10)),
            ],
        );
        l.advance(4000);
        let v = l.snapshot(&r, 42);
        assert_eq!(v["timeline"]["bucket_ms"], 1000);
        assert_eq!(v["timeline"]["out"], json!([800, 0, 50]));
        assert_eq!(v["timeline"]["in"], json!([0, 200, 0]));
        // An ended fight keeps its timeline; a new one starts empty.
        l.end(4000, &r);
        assert_eq!(l.snapshot(&r, 43)["timeline"]["out"], json!([800, 0, 50]));
        l.begin(9000, 38);
        assert_eq!(l.snapshot(&r, 44)["timeline"]["out"], json!([]));
        l.clear();
        assert!(l.snapshot(&r, 45)["timeline"].is_null());
    }

    fn apply(dst: u32, src: u32, buff: u32, inst: u32) -> Event {
        Event::BuffApply {
            dst,
            src,
            buff,
            inst,
            duration: 5000,
            level: 0,
            active: true,
        }
    }

    /// Strips: boon stacks a player removes from someone else; cleanses: condition stacks a
    /// player removes (from anyone). Stacks applied before the fight are known.
    #[test]
    fn strips_and_cleanses() {
        const MIGHT: u32 = 740;
        const STABILITY: u32 = 1122;
        const BLEEDING: u32 = 736;
        const POISON: u32 = 723;
        const OTHER: u32 = 12345;
        let r = world();
        let mut l = LiveStats::new();
        feed(&mut l, &r, &[(900, apply(ENEMY, ENEMY, STABILITY, 70))]);
        l.begin(1000, 38);
        feed(
            &mut l,
            &r,
            &[
                (1100, apply(ENEMY, ENEMY, MIGHT, 71)),
                (1101, apply(ENEMY, ENEMY, MIGHT, 72)),
                (1102, apply(ENEMY, ENEMY, MIGHT, 73)),
                (1103, apply(4, ENEMY, BLEEDING, 80)),
                (1104, apply(4, ENEMY, POISON, 81)),
                (1105, apply(ENEMY, ENEMY, OTHER, 90)),
                // Player 1 strips one stability (applied before the fight) and all 3 might.
                (
                    1200,
                    Event::BuffRemove {
                        dst: ENEMY,
                        remover: 1,
                        inst: 70,
                    },
                ),
                (
                    1201,
                    Event::BuffRemoveAll {
                        dst: ENEMY,
                        remover: 1,
                        buff: MIGHT,
                    },
                ),
                // Removing a non-boon is not a strip; an unknown instance is not counted.
                (
                    1202,
                    Event::BuffRemove {
                        dst: ENEMY,
                        remover: 1,
                        inst: 90,
                    },
                ),
                (
                    1203,
                    Event::BuffRemove {
                        dst: ENEMY,
                        remover: 1,
                        inst: 999,
                    },
                ),
                // Player 4 cleanses their own bleeding; player 1 cleanses 4's poison.
                (
                    1300,
                    Event::BuffRemove {
                        dst: 4,
                        remover: 4,
                        inst: 80,
                    },
                ),
                (
                    1301,
                    Event::BuffRemoveAll {
                        dst: 4,
                        remover: 1,
                        buff: POISON,
                    },
                ),
                // Natural expiry (no remover) counts for no one.
                (1302, apply(4, ENEMY, BLEEDING, 82)),
                (
                    1303,
                    Event::BuffRemove {
                        dst: 4,
                        remover: 0,
                        inst: 82,
                    },
                ),
            ],
        );
        let v = l.snapshot(&r, 42);
        let p1 = player(&v, "Char 1").unwrap();
        assert_eq!(
            (p1["strips"].as_u64(), p1["cleanses"].as_u64()),
            (Some(4), Some(1))
        );
        let p4 = player(&v, "Char 4").unwrap();
        assert_eq!(
            (p4["strips"].as_u64(), p4["cleanses"].as_u64()),
            (Some(0), Some(1))
        );
    }

    #[test]
    fn dps_is_zero_for_zero_duration() {
        let r = world();
        let mut l = LiveStats::new();
        l.begin(1000, 38);
        l.event(1000, &strike(1, NPC, 500, 0));
        let v = l.snapshot(&r, 0);
        assert_eq!(v["fight"]["duration_ms"], 0);
        assert_eq!(player(&v, "Char 1").unwrap()["dps"], 0.0);
    }

    #[test]
    fn pets_count_toward_their_master() {
        let r = world();
        let mut l = LiveStats::new();
        l.set_master(PET, 4);
        l.begin(0, 38);
        feed(
            &mut l,
            &r,
            &[
                (100, strike(PET, NPC, 250, 0)),
                (200, strike(4, NPC, 50, 0)),
                (300, strike(PET, ENEMY, 0, 9)),
                (400, strike(PET, ENEMY, 0, 8)),
            ],
        );
        l.advance(1000);
        let v = l.snapshot(&r, 0);
        let p = player(&v, "Char 4").unwrap();
        assert_eq!(p["damage"], 300);
        assert_eq!(p["downs"], 1);
        assert_eq!(p["kills"], 1);
        assert_eq!(v["players"].as_array().unwrap().len(), 2);
    }

    /// The poll's `AgentMaster` events (sent in or out of a fight) set and clear the master;
    /// a despawn clears it too.
    #[test]
    fn master_events_credit_pets() {
        let r = world();
        let mut l = LiveStats::new();
        l.event(0, &Event::AgentMaster { id: PET, master: 4 });
        l.begin(0, 38);
        feed(
            &mut l,
            &r,
            &[
                (100, strike(PET, NPC, 250, 0)),
                (200, Event::AgentMaster { id: PET, master: 0 }),
                (300, strike(PET, NPC, 1000, 0)),
                (400, Event::AgentMaster { id: PET, master: 4 }),
                (500, strike(PET, NPC, 7, 0)),
                (600, Event::AgentRemove { id: PET }),
                (700, strike(PET, NPC, 2000, 0)),
            ],
        );
        l.advance(1000);
        let v = l.snapshot(&r, 0);
        assert_eq!(player(&v, "Char 4").unwrap()["damage"], 257);
    }

    fn tick(src: u32, dst: u32, value: i32) -> Event {
        Event::BuffDamage {
            src,
            dst,
            buff: 736,
            value,
            barrier: 0,
            stacks: 1,
        }
    }

    /// The local player's own condition ticks (kind 42) fill `condi`, count in `damage` and in
    /// the target's `taken`; pets credit their master, self-damage and zero ticks do not count.
    #[test]
    fn condi_from_buff_damage() {
        let r = world();
        let mut l = LiveStats::new();
        l.set_master(PET, 1);
        l.begin(0, 38);
        feed(
            &mut l,
            &r,
            &[
                (100, strike(1, NPC, 500, 0)),
                (200, tick(1, NPC, 34)),
                (300, tick(PET, ENEMY, 66)),
                (400, tick(1, 1, 99)),
                (500, tick(1, NPC, 0)),
                (600, tick(ENEMY, 1, 120)),
                (700, tick(0, 1, 5)),
            ],
        );
        l.advance(1000);
        let v = l.snapshot(&r, 0);
        let p = player(&v, "Char 1").unwrap();
        assert_eq!(p["power"], 500);
        assert_eq!(p["condi"], 100);
        assert_eq!(p["damage"], 600);
        assert_eq!(p["taken"], 125);
    }

    #[test]
    fn only_squad_members_are_listed() {
        let r = world();
        let mut l = LiveStats::new();
        l.begin(0, 38);
        feed(
            &mut l,
            &r,
            &[
                (100, strike(ENEMY, 1, 700, 0)),
                (200, strike(NPC, 1, 10, 0)),
            ],
        );
        let v = l.snapshot(&r, 0);
        let names: Vec<&str> = v["players"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["name"].as_str().unwrap())
            .collect();
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"Char 1") && names.contains(&"Char 4"));
    }

    #[test]
    fn downs_kills_deaths_downed_and_taken() {
        let r = world();
        let mut l = LiveStats::new();
        l.begin(0, 38);
        feed(
            &mut l,
            &r,
            &[
                (100, strike(1, ENEMY, 0, 9)),
                (200, strike(1, ENEMY, 0, 8)),
                // A death with no killer names the target itself: no credit.
                (250, strike(ENEMY, ENEMY, 0, 8)),
                (300, strike(ENEMY, 1, 200, 0)),
                (310, strike(NPC, 1, 50, 1)),
                (
                    400,
                    Event::State {
                        id: 1,
                        state: AgentState::Down,
                    },
                ),
                // A repeated state is not a second down.
                (
                    410,
                    Event::State {
                        id: 1,
                        state: AgentState::Down,
                    },
                ),
                (
                    500,
                    Event::State {
                        id: 1,
                        state: AgentState::Dead,
                    },
                ),
                (
                    600,
                    Event::State {
                        id: 1,
                        state: AgentState::Up,
                    },
                ),
                (
                    700,
                    Event::State {
                        id: 1,
                        state: AgentState::Down,
                    },
                ),
                (
                    800,
                    Event::State {
                        id: ENEMY,
                        state: AgentState::Dead,
                    },
                ),
            ],
        );
        let v = l.snapshot(&r, 0);
        let p = player(&v, "Char 1").unwrap();
        assert_eq!(p["downs"], 1);
        assert_eq!(p["kills"], 1);
        assert_eq!(p["taken"], 250);
        assert_eq!(p["downed"], 2);
        assert_eq!(p["deaths"], 1);
        assert_eq!(p["damage"], 0);
    }

    #[test]
    fn events_outside_a_fight_do_not_count() {
        let r = world();
        let mut l = LiveStats::new();
        l.event(10, &strike(1, NPC, 500, 0));
        let v = l.snapshot(&r, 0);
        assert_eq!(v["state"], "idle");
        // Out of combat the squad is listed, with nothing counted.
        let ps = v["players"].as_array().unwrap();
        assert!(!ps.is_empty());
        assert!(ps.iter().all(|p| p["damage"] == 0 && p["dps"] == 0.0));
        assert_eq!(v["fight"]["duration_ms"], 0);
    }

    #[test]
    fn json_field_names_match_schema_v1() {
        let r = world();
        let mut l = LiveStats::new();
        l.begin(0, 38);
        l.event(10, &strike(1, NPC, 5, 0));
        let v = l.snapshot(&r, 1_700_000_000_123);
        let keys = |o: &Value| -> Vec<String> {
            let mut k: Vec<String> = o.as_object().unwrap().keys().cloned().collect();
            k.sort();
            k
        };
        assert_eq!(
            keys(&v),
            ["fight", "healing", "players", "state", "t", "timeline", "v"]
        );
        assert_eq!(v["v"], 1);
        assert_eq!(v["t"], 1_700_000_000_123u64);
        assert_eq!(keys(&v["fight"]), ["duration_ms", "map"]);
        let mut want = vec![
            "name",
            "account",
            "prof",
            "elite",
            "spec",
            "group",
            "me",
            "damage",
            "power",
            "condi",
            "dps",
            "downs",
            "kills",
            "deaths",
            "downed",
            "taken",
            "strips",
            "cleanses",
            "down_contrib",
            "healing",
            "barrier",
            "hps",
            "healing_in",
            "barrier_in",
            "heal_full",
        ];
        want.sort();
        assert_eq!(keys(&v["players"][0]), want);
        assert!(v["players"][0]["dps"].is_f64());
        assert!(serde_json::to_string(&v).unwrap().contains("\"dps\":"));
    }

    /// The custom window catalog (docs/custom-windows-stats.md and STATS in the overlay page) is
    /// exactly the per-player stat keys. A new feed key fails here until both are updated.
    #[test]
    fn feed_player_keys_are_the_catalog() {
        let r = world();
        let mut l = LiveStats::new();
        l.begin(0, 38);
        l.event(10, &strike(1, NPC, 5, 0));
        let v = l.snapshot(&r, 1_700_000_000_123);
        let mut got: Vec<String> = v["players"][0]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        got.sort();
        let identity = [
            "name",
            "account",
            "prof",
            "elite",
            "spec",
            "group",
            "me",
            "heal_full",
        ];
        let catalog = [
            "damage",
            "dps",
            "power",
            "condi",
            "down_contrib",
            "downs",
            "kills",
            "deaths",
            "downed",
            "taken",
            "cleanses",
            "strips",
            "healing",
            "barrier",
            "hps",
            "healing_in",
            "barrier_in",
        ];
        assert_eq!(catalog.len(), 17);
        let mut want: Vec<String> = identity
            .iter()
            .chain(catalog.iter())
            .map(|s| s.to_string())
            .collect();
        want.sort();
        // `boons` is optional (only players with a boon have it), keyed by squad boon id.
        got.retain(|k| k != "boons");
        assert_eq!(got, want);
        let mut l = LiveStats::new();
        l.begin(0, 38);
        l.event(1000, &boon_apply(1, 4, PROTECTION, 900, 2000));
        l.advance(5000);
        let v = l.snapshot(&r, 1);
        let boons = v["players"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|p| p["boons"].as_object())
            .flatten()
            .map(|(k, _)| k.parse::<u32>().unwrap())
            .collect::<Vec<_>>();
        assert!(!boons.is_empty());
        assert!(boons.iter().all(|b| SQUAD_BOONS.contains(b)));
    }

    fn boon_apply(dst: u32, src: u32, buff: u32, inst: u32, duration: u32) -> Event {
        Event::BuffApply {
            dst,
            src,
            buff,
            inst,
            duration,
            level: 0,
            active: true,
        }
    }

    const PROTECTION: u32 = 717;

    /// Boons are cumulative ms per player: [up, self, group, squad]. Player 4 (group 2) protects
    /// player 1 (group 3) for 2 s: up for 1, squad-wide generation for 4.
    #[test]
    fn feed_carries_cumulative_boons() {
        let r = world();
        let mut l = LiveStats::new();
        l.begin(0, 38);
        l.event(1000, &boon_apply(1, 4, PROTECTION, 900, 2000));
        l.advance(5000);
        let v = l.snapshot(&r, 1);
        assert_eq!(
            player(&v, "Char 1").unwrap()["boons"],
            json!({"717": [2000, 0, 0, 0]})
        );
        assert_eq!(
            player(&v, "Char 4").unwrap()["boons"],
            json!({"717": [0, 0, 0, 2000]})
        );
    }

    #[test]
    fn minion_boons_credit_the_master() {
        let r = world();
        let mut l = LiveStats::new();
        l.set_master(PET, 4);
        l.begin(0, 38);
        l.event(1000, &boon_apply(1, PET, PROTECTION, 900, 2000));
        l.advance(5000);
        let v = l.snapshot(&r, 1);
        assert_eq!(
            player(&v, "Char 4").unwrap()["boons"],
            json!({"717": [0, 0, 0, 2000]})
        );
    }

    #[test]
    fn feed_omits_empty_boons() {
        let r = world();
        let mut l = LiveStats::new();
        l.begin(0, 38);
        // An enemy holds it and a non-squad boon on a squad player does not count.
        l.event(1000, &boon_apply(ENEMY, 4, PROTECTION, 900, 2000));
        l.event(1000, &boon_apply(1, 4, 736, 901, 2000));
        l.advance(5000);
        let v = l.snapshot(&r, 1);
        assert!(player(&v, "Char 1").unwrap().get("boons").is_none());
        assert!(player(&v, "Char 4").unwrap().get("boons").is_none());
    }

    /// Rows of holders known to be enemies or pets never reach the tracker.
    #[test]
    fn foreign_holders_are_not_tracked() {
        let mut l = LiveStats::new();
        for e in world_events() {
            l.event(0, &e);
        }
        l.begin(0, 38);
        l.event(1000, &boon_apply(ENEMY, 4, PROTECTION, 900, 2000));
        l.event(1000, &boon_apply(NPC, 4, PROTECTION, 901, 2000));
        l.event(1000, &boon_apply(1, 4, PROTECTION, 902, 2000));
        assert_eq!(l.boons.rows_held(), 1);
    }

    /// 50 players holding the 13 squad boons add at most 40 KB to the feed.
    #[test]
    fn boons_feed_size_is_bounded() {
        let mut r = Registry::new();
        let mut l = LiveStats::new();
        for id in 10..60u32 {
            for e in squad_events(id, (id % 5 + 1) as u8) {
                r.apply(0, &e);
            }
        }
        r.apply(0, &Event::Pov { id: 10 });
        l.begin(0, 38);
        let mut inst = 1000;
        for id in 10..60u32 {
            for &b in &SQUAD_BOONS {
                inst += 1;
                let src = 10 + (id - 9) % 50;
                l.event(1000, &boon_apply(id, src, b, inst, 600_000));
            }
        }
        l.advance(600_000);
        let v = l.snapshot(&r, 1);
        let with: usize = v["players"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| serde_json::to_string(&p["boons"]).unwrap().len())
            .sum();
        assert!(with > 0, "boons are present");
        assert!(with <= 40_000, "boons take {with} bytes");
        println!("boons bytes: {with}");
    }

    /// A zero-length boon next to a real one adds no entry.
    #[test]
    fn zero_boons_are_left_out_of_a_non_empty_map() {
        let r = world();
        let mut l = LiveStats::new();
        l.begin(0, 38);
        l.event(1000, &boon_apply(1, 4, PROTECTION, 900, 2000));
        l.event(1000, &boon_apply(1, 4, 719, 901, 0));
        l.advance(5000);
        let v = l.snapshot(&r, 1);
        assert_eq!(
            player(&v, "Char 1").unwrap()["boons"],
            json!({"717": [2000, 0, 0, 0]})
        );
    }

    fn map_event() -> Event {
        Event::Map {
            map_id: 38,
            map_type: 11,
            shard: 0,
            gw2_build: 1,
        }
    }

    /// Through the processor: a boon up before the fight counts from its start (the initial
    /// rows), the fight's feed carries it, and the ended fight's feed keeps it.
    #[test]
    fn processor_path_counts_initial_boons_and_keeps_them_after_the_fight() {
        let mut p = processor();
        let mut l = LiveStats::new();
        for e in world_events() {
            run(&mut p, &mut l, Step::Push(0, e));
        }
        run(&mut p, &mut l, Step::Push(0, map_event()));
        run(
            &mut p,
            &mut l,
            Step::Push(500, boon_apply(1, 4, PROTECTION, 900, 600_000)),
        );
        let enter = Event::Combat {
            id: 1,
            in_combat: true,
        };
        run(&mut p, &mut l, Step::Push(1000, enter));
        run(&mut p, &mut l, Step::Push(2000, strike(1, NPC, 400, 0)));
        run(&mut p, &mut l, Step::Tick(3000));
        let v = l.snapshot(p.registry(), 0);
        assert_eq!(v["state"], "fight");
        assert_eq!(
            player(&v, "Char 1").unwrap()["boons"],
            json!({"717": [2000, 0, 0, 0]})
        );
        assert_eq!(
            player(&v, "Char 4").unwrap()["boons"],
            json!({"717": [0, 0, 0, 2000]})
        );
        // The fight ends at its last activity (2000): one second of it.
        run(&mut p, &mut l, Step::Tick(30_000));
        assert_eq!(l.state(), LiveState::Last);
        let v = l.snapshot(p.registry(), 0);
        assert_eq!(v["state"], "last");
        assert_eq!(
            player(&v, "Char 1").unwrap()["boons"],
            json!({"717": [1000, 0, 0, 0]})
        );
        assert_eq!(
            player(&v, "Char 4").unwrap()["boons"],
            json!({"717": [0, 0, 0, 1000]})
        );
        // The next fight starts from zero.
        run(&mut p, &mut l, Step::Push(50_000, enter));
        let v = l.snapshot(p.registry(), 0);
        assert!(player(&v, "Char 1").unwrap().get("boons").is_none());
    }

    fn named(id: u32, group: u8, character: &str, account: &str) -> Vec<Event> {
        let mut v = vec![add(id, group)];
        v.extend(name_events(id, NameField::Character, character));
        v.extend(name_events(id, NameField::Account, account));
        v
    }

    /// Account X is id 7, leaves, rejoins as id 9; then the game gives id 7 to account Y.
    /// They stay two players, and X keeps its boons across the rejoin.
    #[test]
    fn rejoined_and_reused_ids_do_not_merge_players() {
        let mut p = processor();
        let mut l = LiveStats::new();
        for e in world_events() {
            run(&mut p, &mut l, Step::Push(0, e));
        }
        for e in named(7, 2, "Xena", ":X.0001") {
            run(&mut p, &mut l, Step::Push(0, e));
        }
        run(&mut p, &mut l, Step::Push(0, map_event()));
        let enter = Event::Combat {
            id: 1,
            in_combat: true,
        };
        run(&mut p, &mut l, Step::Push(1000, enter));
        run(
            &mut p,
            &mut l,
            Step::Push(1500, boon_apply(7, 4, PROTECTION, 900, 600_000)),
        );
        run(
            &mut p,
            &mut l,
            Step::Push(2000, Event::AgentRemove { id: 7 }),
        );
        for e in named(9, 2, "Xena", ":X.0001") {
            run(&mut p, &mut l, Step::Push(2100, e));
        }
        run(
            &mut p,
            &mut l,
            Step::Push(2200, boon_apply(9, 4, PROTECTION, 901, 600_000)),
        );
        run(&mut p, &mut l, Step::Push(2500, strike(1, NPC, 400, 0)));
        for e in named(7, 2, "Yara", ":Y.0002") {
            run(&mut p, &mut l, Step::Push(3000, e));
        }
        run(
            &mut p,
            &mut l,
            Step::Push(3000, boon_apply(7, 4, 1187, 902, 2000)),
        );
        run(&mut p, &mut l, Step::Push(4000, strike(1, NPC, 400, 0)));
        run(&mut p, &mut l, Step::Tick(5000));
        let v = l.snapshot(p.registry(), 0);
        assert_eq!(v["state"], "fight");
        let keys = |name: &str| -> Vec<String> {
            player(&v, name).unwrap()["boons"]
                .as_object()
                .map(|o| o.keys().cloned().collect())
                .unwrap_or_default()
        };
        assert_eq!(keys("Xena"), ["717"]);
        assert_eq!(keys("Yara"), ["1187"]);
        assert_eq!(player(&v, "Yara").unwrap()["boons"]["1187"][0], 2000);
        assert!(
            player(&v, "Xena").unwrap()["boons"]["717"][0]
                .as_u64()
                .unwrap()
                >= 2800
        );
    }

    /// Player 7's boons start before its account name arrives: they count for the account, as
    /// holder and as source, once it does.
    #[test]
    fn boons_before_the_account_is_known_count_for_it() {
        let mut p = processor();
        let mut l = LiveStats::new();
        for e in world_events() {
            run(&mut p, &mut l, Step::Push(0, e));
        }
        run(&mut p, &mut l, Step::Push(0, add(7, 2)));
        run(&mut p, &mut l, Step::Push(0, map_event()));
        let enter = Event::Combat {
            id: 1,
            in_combat: true,
        };
        run(&mut p, &mut l, Step::Push(1000, enter));
        run(
            &mut p,
            &mut l,
            Step::Push(1500, boon_apply(7, 4, PROTECTION, 900, 600_000)),
        );
        run(
            &mut p,
            &mut l,
            Step::Push(1500, boon_apply(1, 7, 1187, 901, 600_000)),
        );
        run(&mut p, &mut l, Step::Push(2000, strike(1, NPC, 400, 0)));
        run(&mut p, &mut l, Step::Tick(2500));
        let _ = l.snapshot(p.registry(), 0);
        for e in name_events(7, NameField::Character, "Xena")
            .into_iter()
            .chain(name_events(7, NameField::Account, ":X.0001"))
        {
            run(&mut p, &mut l, Step::Push(3000, e));
        }
        run(&mut p, &mut l, Step::Push(4000, strike(1, NPC, 400, 0)));
        run(&mut p, &mut l, Step::Tick(5000));
        let v = l.snapshot(p.registry(), 0);
        assert_eq!(v["state"], "fight");
        let x = &player(&v, "Xena").unwrap()["boons"];
        assert_eq!(x["717"][0], 3500);
        assert_eq!(x["1187"][3], 3500);
    }

    /// Player 7 leaves mid-fight: its boons, the credit it gave and the credit it got stay, as
    /// axilog counts every player of the log.
    #[test]
    fn departed_players_keep_their_boons() {
        let mut p = processor();
        let mut l = LiveStats::new();
        for e in world_events() {
            run(&mut p, &mut l, Step::Push(0, e));
        }
        for e in named(7, 2, "Xena", ":X.0001") {
            run(&mut p, &mut l, Step::Push(0, e));
        }
        run(&mut p, &mut l, Step::Push(0, map_event()));
        let enter = Event::Combat {
            id: 1,
            in_combat: true,
        };
        run(&mut p, &mut l, Step::Push(1000, enter));
        run(
            &mut p,
            &mut l,
            Step::Push(1000, boon_apply(7, 4, PROTECTION, 900, 2000)),
        );
        run(
            &mut p,
            &mut l,
            Step::Push(1000, boon_apply(1, 7, 1187, 901, 2000)),
        );
        run(&mut p, &mut l, Step::Push(1500, strike(7, NPC, 400, 0)));
        run(&mut p, &mut l, Step::Tick(2000));
        let _ = l.snapshot(p.registry(), 0);
        run(
            &mut p,
            &mut l,
            Step::Push(2500, Event::AgentRemove { id: 7 }),
        );
        run(&mut p, &mut l, Step::Push(4000, strike(1, NPC, 400, 0)));
        run(&mut p, &mut l, Step::Tick(5000));
        let v = l.snapshot(p.registry(), 0);
        assert_eq!(v["state"], "fight");
        assert_eq!(
            player(&v, "Xena").unwrap()["boons"],
            json!({"717": [2000, 0, 0, 0], "1187": [0, 0, 0, 2000]})
        );
        assert_eq!(player(&v, "Char 4").unwrap()["boons"]["717"][3], 2000);
        assert_eq!(player(&v, "Char 1").unwrap()["boons"]["1187"][0], 2000);
    }

    /// A processor that saves any fight: one squad member and no minimum length.
    fn processor() -> Processor {
        Processor::new(WvwRules {
            min_squad: 1,
            min_duration_ms: 0,
            ..WvwRules::default()
        })
    }

    fn run(p: &mut Processor, l: &mut LiveStats, s: Step) {
        let o = crate::replay::apply(p, &s);
        l.after_step(p, &s, o.as_ref());
    }

    #[test]
    fn states_follow_the_processor_and_reset_per_fight() {
        let mut p = processor();
        let mut l = LiveStats::new();
        for e in world_events() {
            run(&mut p, &mut l, Step::Push(0, e));
        }
        run(
            &mut p,
            &mut l,
            Step::Push(
                0,
                Event::Map {
                    map_id: 38,
                    map_type: 11,
                    shard: 0,
                    gw2_build: 1,
                },
            ),
        );
        assert_eq!(l.state(), LiveState::Idle);
        assert_eq!(l.snapshot(p.registry(), 0)["state"], "idle");

        // Not counted: no fight yet.
        run(&mut p, &mut l, Step::Push(500, strike(1, NPC, 999, 0)));
        let enter = Event::Combat {
            id: 1,
            in_combat: true,
        };
        run(&mut p, &mut l, Step::Push(1000, enter));
        assert_eq!(l.state(), LiveState::Fight);
        run(&mut p, &mut l, Step::Push(2000, strike(1, NPC, 400, 0)));
        run(&mut p, &mut l, Step::Tick(3000));
        let v = l.snapshot(p.registry(), 0);
        assert_eq!(v["state"], "fight");
        assert_eq!(v["fight"]["duration_ms"], 2000);
        assert_eq!(v["fight"]["map"], 38);
        assert_eq!(player(&v, "Char 1").unwrap()["damage"], 400);

        // Inactivity ends the fight at its last activity (2000).
        let g = l.generation();
        run(&mut p, &mut l, Step::Tick(30_000));
        assert!(!p.recording());
        assert_eq!(l.state(), LiveState::Last);
        assert!(l.generation() > g);
        let v = l.snapshot(p.registry(), 0);
        assert_eq!(v["state"], "last");
        assert_eq!(v["fight"]["duration_ms"], 1000);
        let c1 = player(&v, "Char 1").unwrap();
        assert_eq!(c1["damage"], 400);
        assert_eq!(c1["dps"], 400.0);

        // Kept while idle; ticks and strikes outside a fight change nothing.
        let g = l.generation();
        run(&mut p, &mut l, Step::Tick(40_000));
        run(&mut p, &mut l, Step::Push(40_500, strike(1, NPC, 5, 0)));
        assert_eq!(l.generation(), g);
        assert_eq!(l.snapshot(p.registry(), 0)["state"], "last");

        // The next fight starts from zero.
        run(&mut p, &mut l, Step::Push(50_000, enter));
        let v = l.snapshot(p.registry(), 0);
        assert_eq!(v["state"], "fight");
        assert_eq!(v["fight"]["duration_ms"], 0);
        assert_eq!(player(&v, "Char 1").unwrap()["damage"], 0);
    }

    #[test]
    fn last_fight_keeps_its_players_after_a_map_change() {
        let mut p = processor();
        let mut l = LiveStats::new();
        for e in world_events() {
            run(&mut p, &mut l, Step::Push(0, e));
        }
        let map = |map_id| Event::Map {
            map_id,
            map_type: 11,
            shard: 0,
            gw2_build: 1,
        };
        run(&mut p, &mut l, Step::Push(0, map(38)));
        run(
            &mut p,
            &mut l,
            Step::Push(
                1000,
                Event::Combat {
                    id: 1,
                    in_combat: true,
                },
            ),
        );
        run(&mut p, &mut l, Step::Push(1500, strike(1, NPC, 100, 0)));
        // The map change ends the fight and clears the registry.
        run(&mut p, &mut l, Step::Push(2000, map(95)));
        assert_eq!(l.state(), LiveState::Last);
        assert!(p.registry().agent(1).is_none());
        let v = l.snapshot(p.registry(), 0);
        assert_eq!(v["fight"]["map"], 38);
        assert_eq!(player(&v, "Char 1").unwrap()["damage"], 100);
        assert_eq!(v["players"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn clear_forgets_the_last_fight_and_bumps_generation() {
        let mut s = LiveStats::new();
        s.begin(1_000, 38);
        s.end(5_000, &Registry::default());
        let g = s.generation();
        s.clear();
        assert_eq!(s.state(), LiveState::Idle);
        assert!(s.generation() > g);
        let v = s.snapshot(&Registry::default(), 0);
        assert_eq!(v["state"], "idle");
        assert_eq!(v["players"].as_array().map(Vec::len), Some(0));
    }

    #[test]
    fn writer_rate_limits_and_writes_atomically() {
        let dir = std::env::temp_dir().join(format!("axidps-live-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("live.json");
        let mut w = LiveWriter::new(path.clone());
        let t0 = Instant::now();
        assert!(w.due(t0, 0), "the first write is always due");
        w.write(t0, 0, &serde_json::json!({"v": 1})).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), r#"{"v":1}"#);
        assert!(!dir.join("live.json.tmp").exists());
        assert!(!w.due(t0 + Duration::from_secs(5), 0), "nothing changed");
        assert!(!w.due(t0 + Duration::from_millis(100), 1), "too soon");
        assert!(w.due(t0 + MIN_INTERVAL, 1));
        assert!(w.changed(1) && !w.changed(0));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn write_atomic_reports_errors() {
        let path = std::env::temp_dir()
            .join("axidps-live-missing-dir")
            .join("nope")
            .join("live.json");
        assert!(write_atomic(&path, b"{}").is_err());
    }
}
