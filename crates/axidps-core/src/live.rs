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
use crate::condi::GameMode;
use crate::live_condi::Estimator;
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

/// Where the tracker's ids for unnamed agents that left start: above the account ids.
const ORPHAN_IDS: u32 = 3 << 30;

/// The game's defiance damage skill: its zero-value strikes on players are not taking part.
const DEFIANCE_DAMAGE: u32 = 23276;

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

/// One copy of a player's healing one way, as one reporter saw it: the total, and for the
/// Details window by skill (healing, barrier, hits), per [`BUCKET_MS`], and by the other player.
#[derive(Debug, Clone, Default)]
struct HealCopy {
    sum: HealSum,
    skills: BTreeMap<u32, (u64, u64, u32)>,
    series: Vec<u64>,
    skill_series: SkillSeries,
    peers: BTreeMap<u32, HealSum>,
}

/// One heal as [`CopySums::add`] counts it.
#[derive(Debug, Clone, Copy)]
struct HealHit {
    sum: HealSum,
    skill: u32,
    /// Bucket index into the fight.
    at: usize,
    /// The other side: the target for healing given, the healer for healing received.
    peer: u32,
}

/// One player's healing one way (given or received) as copies by who reported them: `own` is
/// the player's own copies, `by_sender` everyone else's. Every heal reaches the meter once per
/// sharing squadmate, so the copies are alternatives, not additions (Elite Insights' rule).
#[derive(Debug, Clone, Default)]
struct CopySums {
    own: Option<HealCopy>,
    by_sender: BTreeMap<u32, HealCopy>,
}

impl CopySums {
    fn add(&mut self, own: bool, sender: u32, h: HealHit) {
        let e = if own {
            self.own.get_or_insert_with(HealCopy::default)
        } else {
            self.by_sender.entry(sender).or_default()
        };
        add_heal_sum(&mut e.sum, h.sum);
        let k = e.skills.entry(h.skill).or_default();
        k.0 = k.0.saturating_add(h.sum.healing);
        k.1 = k.1.saturating_add(h.sum.barrier);
        k.2 = k.2.saturating_add(1);
        add_at(&mut e.series, h.at, h.sum.healing.saturating_add(h.sum.barrier));
        add_at(
            e.skill_series.entry(h.skill).or_default(),
            h.at,
            h.sum.healing.saturating_add(h.sum.barrier),
        );
        if h.peer != 0 {
            add_heal_sum(e.peers.entry(h.peer).or_default(), h.sum);
        }
    }

    /// The copy shown: the player's own if any, else the largest sender's.
    fn shown_copy(&self) -> Option<&HealCopy> {
        self.own.as_ref().or_else(|| {
            self.by_sender
                .values()
                .max_by_key(|c| c.sum.healing.saturating_add(c.sum.barrier))
        })
    }

    fn shown(&self) -> HealSum {
        self.shown_copy().map(|c| c.sum).unwrap_or_default()
    }
}

fn add_heal_sum(e: &mut HealSum, h: HealSum) {
    e.healing = e.healing.saturating_add(h.healing);
    e.barrier = e.barrier.saturating_add(h.barrier);
}

/// Boons (Elite Insights' list): stripping one from someone else is a strip.
const BOONS: &[u32] = &[
    717, 718, 719, 725, 726, 740, 743, 873, 1122, 1187, 26980, 30328,
];
/// Conditions: removing one is a cleanse.
pub(crate) const CONDITIONS: &[u32] = &[
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
    /// The condition part of `out`.
    out_condi: Vec<u64>,
    /// Boon stacks stripped and condition stacks cleansed per bucket.
    strips: Vec<u64>,
    cleanses: Vec<u64>,
}

/// One skill's damage one way, with its hits by result (the details window's skill panel).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct SkillStat {
    damage: u64,
    /// Hits that did damage, and the least and most one did.
    hits: u32,
    min: u32,
    max: u32,
    crit: u32,
    glance: u32,
    /// Strikes blocked, evaded, interrupted, absorbed (invulnerable) or missed (blind).
    block: u32,
    evade: u32,
    interrupt: u32,
    absorb: u32,
    miss: u32,
    /// The part of `damage` that hit barrier.
    barrier: u64,
}

impl SkillStat {
    fn hit(&mut self, dmg: u64) {
        let v = u32::try_from(dmg).unwrap_or(u32::MAX);
        self.min = if self.hits == 0 { v } else { self.min.min(v) };
        self.max = self.max.max(v);
        self.damage = self.damage.saturating_add(dmg);
        self.hits = self.hits.saturating_add(1);
    }

    fn merge(&mut self, o: &SkillStat) {
        if o.hits > 0 {
            self.min = if self.hits == 0 { o.min } else { self.min.min(o.min) };
        }
        self.max = self.max.max(o.max);
        self.damage = self.damage.saturating_add(o.damage);
        self.hits += o.hits;
        self.crit += o.crit;
        self.glance += o.glance;
        self.block += o.block;
        self.evade += o.evade;
        self.interrupt += o.interrupt;
        self.absorb += o.absorb;
        self.miss += o.miss;
        self.barrier = self.barrier.saturating_add(o.barrier);
    }
}

/// One agent's damage by skill, one way.
type BySkill = BTreeMap<u32, SkillStat>;
/// Healing by skill: skill -> (healing, barrier, hits).
type HealSkills = BTreeMap<u32, (u64, u64, u32)>;

/// Skills listed per direction in the feed's `detail`.
const DETAIL_SKILLS: usize = 60;
/// Casts kept per agent per fight for the details chart's cast marks.
const MAX_CASTS: usize = 4000;

fn add_skill(m: &mut BTreeMap<u32, BySkill>, id: u32, skill: u32, dmg: u64) {
    m.entry(id).or_default().entry(skill).or_default().hit(dmg);
}

/// A strike of `skill` by `id` (or on it): its damage (0: none) and how it landed.
fn add_strike(m: &mut BTreeMap<u32, BySkill>, id: u32, skill: u32, dmg: u64, result: u8, barrier: u32) {
    let e = m.entry(id).or_default().entry(skill).or_default();
    if dmg > 0 {
        e.hit(dmg);
        e.barrier = e.barrier.saturating_add(u64::from(barrier).min(dmg));
    }
    match result {
        1 => e.crit += 1,
        2 => e.glance += 1,
        3 => e.block += 1,
        4 => e.evade += 1,
        5 => e.interrupt += 1,
        6 => e.absorb += 1,
        7 => e.miss += 1,
        _ => {}
    }
}

/// One skill's amount per [`BUCKET_MS`] (the details chart's bars for a selected skill).
type SkillSeries = BTreeMap<u32, Vec<u64>>;

fn add_skill_at(m: &mut BTreeMap<u32, SkillSeries>, id: u32, skill: u32, i: usize, n: u64) {
    add_at(m.entry(id).or_default().entry(skill).or_default(), i, n);
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
    /// Live buff instances (tracked in and out of fights): instance -> (target, buff).
    stacks: BTreeMap<u32, (u32, u32)>,
    /// Squad members as last seen in the registry this fight.
    idents: BTreeMap<u32, Ident>,
    /// Damage dealt and taken per agent per [`BUCKET_MS`] of the fight (dealt by owner).
    series: BTreeMap<u32, Series>,
    /// Damage dealt (by owner) and taken per agent by skill, this fight (kept after it ends).
    skills_out: BTreeMap<u32, BySkill>,
    skills_in: BTreeMap<u32, BySkill>,
    /// Damage dealt (by owner) per skill per bucket, exact and estimated.
    skill_series: BTreeMap<u32, SkillSeries>,
    est_skill_series: BTreeMap<u32, SkillSeries>,
    /// The skill the details window selected (its casts and damage per bucket on the chart).
    detail_skill: Option<u32>,
    /// Each agent's downs, deaths and rallies this fight: (ms into the fight, state).
    state_log: BTreeMap<u32, Vec<(u64, AgentState)>>,
    /// Each agent's cast starts this fight (by owner): (ms into the fight, skill).
    casts: BTreeMap<u32, Vec<(u64, u32)>>,
    /// The account whose breakdown the feed carries (`detail`): the overlay's details window.
    detail: Option<String>,
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
    /// Orphan ids given out (see [`ORPHAN_IDS`]).
    orphans: u32,
    /// Per account id: whether it was squad, and its subgroup, when last seen this fight.
    boon_seen: BTreeMap<u32, (bool, Option<u8>)>,
    /// Tracker ids of the squad players this fight has seen (live at its start, named during it,
    /// or live at a tick): the players our log of the fight lists as squad.
    boon_squad: BTreeSet<u32>,
    /// Tracker ids of the players that took part in this fight's combat (struck or were struck,
    /// ticked a condition or were ticked, cast, or entered combat). axilog leaves idle squad
    /// members out of its players, so they are not targets.
    boon_active: BTreeSet<u32>,
    /// Of those, the squad players with a subgroup at the last tick: their subgroup, and whether
    /// they count as group and squad targets (they took part in the fight).
    boon_targets: BTreeMap<u32, (u8, bool)>,
    /// Game time of the tracker's last tick.
    boon_tick: Option<u64>,
    /// Per tracker id and boon: its uptime total (ms, stack-ms for Might and Stability) at the
    /// end of each bucket, as sampled at the tracker's ticks (the details chart's boon line).
    boon_hist: BTreeMap<u32, BTreeMap<u32, Vec<u64>>>,
    /// The condition model estimating this fight's ticks nobody reports exactly.
    estimator: Option<Estimator>,
    /// Estimated condition damage this fight per owner: total, per bucket and per skill. Shown
    /// only for players without exact ticks (see [`LiveStats::estimated`]).
    est_condi: BTreeMap<u32, u64>,
    est_series: BTreeMap<u32, Vec<u64>>,
    est_skills: BTreeMap<u32, BySkill>,
    /// Peers who relayed their own exact condition ticks this fight.
    exact_condi: BTreeSet<u32>,
    /// Boon and condition stacks each player (owner) removed this fight: buff -> stacks.
    removed: BTreeMap<u32, BTreeMap<u32, u64>>,
    /// Ids seen as condition damage (buff ids): the Details window tells them from strikes.
    condi_skills: BTreeSet<u32>,
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
            stacks: BTreeMap::new(),
            idents: BTreeMap::new(),
            series: BTreeMap::new(),
            skills_out: BTreeMap::new(),
            skills_in: BTreeMap::new(),
            state_log: BTreeMap::new(),
            casts: BTreeMap::new(),
            skill_series: BTreeMap::new(),
            est_skill_series: BTreeMap::new(),
            detail_skill: None,
            detail: None,
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
            orphans: 0,
            boon_seen: BTreeMap::new(),
            boon_squad: BTreeSet::new(),
            boon_active: BTreeSet::new(),
            boon_targets: BTreeMap::new(),
            boon_tick: None,
            boon_hist: BTreeMap::new(),
            estimator: None,
            est_condi: BTreeMap::new(),
            est_series: BTreeMap::new(),
            est_skills: BTreeMap::new(),
            exact_condi: BTreeSet::new(),
            removed: BTreeMap::new(),
            condi_skills: BTreeSet::new(),
        }
    }

    /// Change what is counted from now on (numbers already counted stay).
    pub fn set_counting(&mut self, c: Counting) {
        self.counting = c;
    }

    /// The counting settings leave `ev` out: a strike or condition tick on a WvW gadget or
    /// NPC that is switched off.
    fn skips(&self, reg: &Registry, ev: &Event) -> bool {
        match *ev {
            Event::Strike { dst, .. }
            | Event::BuffDamage { dst, .. }
            | Event::RelayBuffDamage { dst, .. } => self.skips_target(reg, dst),
            _ => false,
        }
    }

    /// Damage to `dst` is left out by the counting settings (see [`LiveStats::skips`]).
    fn skips_target(&self, reg: &Registry, dst: u32) -> bool {
        let c = self.counting;
        if !self.wvw || (c.wvw_gadgets && c.wvw_npcs) {
            return false;
        }
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
    #[allow(clippy::too_many_arguments)]
    fn count_heal(
        &mut self,
        t: u64,
        sender: u32,
        has_src: bool,
        by: u32,
        to: u32,
        skill: u32,
        sum: HealSum,
    ) {
        let at = self.bucket(t);
        if has_src {
            let h = HealHit {
                sum,
                skill,
                at,
                peer: to,
            };
            self.given.entry(by).or_default().add(sender == by, sender, h);
        }
        let h = HealHit {
            sum,
            skill,
            at,
            peer: if has_src { by } else { 0 },
        };
        self.received.entry(to).or_default().add(sender == to, sender, h);
    }

    /// The timeline bucket of game time `t`.
    fn bucket(&self, t: u64) -> usize {
        (t.saturating_sub(self.start_ms) / BUCKET_MS) as usize
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
        self.skills_out.clear();
        self.skills_in.clear();
        self.state_log.clear();
        self.casts.clear();
        self.skill_series.clear();
        self.est_skill_series.clear();
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
        self.boon_squad.clear();
        self.boon_active.clear();
        self.boon_targets.clear();
        self.boon_tick = None;
        self.boon_hist.clear();
        self.estimator = None;
        self.est_condi.clear();
        self.est_series.clear();
        self.est_skills.clear();
        self.exact_condi.clear();
        self.removed.clear();
        // The stacks held at the start are the fight's first rows.
        let initial = self.buff_stacks.initial(start_ms);
        self.feed_boons(start_ms, initial);
        self.generation += 1;
    }

    /// Forget everything: a new game run (its agent ids mean nothing to the old numbers).
    pub fn clear(&mut self) {
        let (generation, counting) = (self.generation + 1, self.counting);
        let detail = self.detail.take();
        *self = Self::new();
        self.generation = generation;
        self.counting = counting;
        self.detail = detail;
    }

    /// The fight ended at `end_ms`; `reg` is the registry as of the end. Its rows are kept.
    pub fn end(&mut self, end_ms: u64, reg: &Registry) {
        if self.state != LiveState::Fight {
            return;
        }
        self.now_ms = end_ms.max(self.start_ms);
        self.tick_boons(reg);
        self.last_rows = self.rows(reg);
        self.boons.clear();
        self.last_timeline = self.timeline();
        self.last_healing = self.healing(reg);
        self.heal_out.clear();
        self.heal_in.clear();
        self.given.clear();
        self.received.clear();
        self.state = LiveState::Last;
        // The series and skills stay for the details window until the next fight.
        self.estimator = None;
        self.est_condi.clear();
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
        if self.state != LiveState::Fight {
            return;
        }
        if let Some((remover, dst, buff, n)) = removed
            && remover != 0
            && n > 0
        {
            let by = self.owner(remover);
            let at = self.bucket(t);
            let a = self.acc.entry(by).or_default();
            let strip = BOONS.contains(&buff) && by != dst;
            if strip {
                a.strips += n;
            } else if CONDITIONS.contains(&buff) {
                a.cleanses += n;
            } else {
                return;
            }
            let series = self.series.entry(by).or_default();
            add_at(if strip { &mut series.strips } else { &mut series.cleanses }, at, n);
            *self.removed.entry(by).or_default().entry(buff).or_default() += n;
            self.generation += 1;
            return;
        }
        self.advance(t);
        match *ev {
            Event::Strike {
                src,
                dst,
                skill,
                value,
                overstack,
                result,
                ..
            } => {
                let by = self.owner(src);
                if DAMAGE_RESULTS.contains(&result) && value <= 0 && result >= 3 {
                    // A strike avoided: no damage, but the skill panel counts it.
                    if by == self.owner(dst) || src == 0 {
                        return;
                    }
                    add_strike(&mut self.skills_out, by, skill, 0, result, 0);
                    add_strike(&mut self.skills_in, dst, skill, 0, result, 0);
                } else if DAMAGE_RESULTS.contains(&result) && value > 0 {
                    if by == self.owner(dst) {
                        return;
                    }
                    let dmg = u64::from(value.unsigned_abs());
                    let a = self.acc.entry(by).or_default();
                    a.power = a.power.saturating_add(dmg);
                    let d = self.acc.entry(dst).or_default();
                    d.taken = d.taken.saturating_add(dmg);
                    self.add_series(t, by, dst, dmg, false);
                    if src != 0 {
                        add_strike(&mut self.skills_out, by, skill, dmg, result, overstack);
                        let i = self.bucket(t);
                        add_skill_at(&mut self.skill_series, by, skill, i, dmg);
                    }
                    add_strike(&mut self.skills_in, dst, skill, dmg, result, overstack);
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
                src,
                dst,
                buff,
                value,
                ..
            } => self.count_condi(t, src, dst, buff, value),
            // A squadmate's own condition ticks, relayed (kind 63); `dst` 0: no local target.
            Event::RelayBuffDamage {
                sender,
                src,
                dst,
                buff,
                value,
                ..
            } if self.pov != Some(sender) => {
                // From now on this fight shows its ticks, not the estimate.
                if self.exact_condi.insert(sender) {
                    self.generation += 1;
                }
                self.count_condi(t, src, dst, buff, value);
            }
            // The local player's heals and barrier, given and received (kind 43).
            Event::Heal {
                src,
                dst,
                skill,
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
                self.count_heal(t, sender, src != 0, by, to, skill, sum);
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
                skill,
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
                self.count_heal(t, sender, src != 0, by, to, skill, sum);
                self.generation += 1;
            }
            Event::State { id, state } => {
                if self.states.insert(id, state) == Some(state) {
                    return;
                }
                let at = t.saturating_sub(self.start_ms);
                self.state_log.entry(id).or_default().push((at, state));
                if state == AgentState::Up {
                    self.generation += 1;
                }
                let a = self.acc.entry(id).or_default();
                match state {
                    AgentState::Down => a.downed += 1,
                    AgentState::Dead => a.deaths += 1,
                    AgentState::Up => return,
                }
                self.generation += 1;
            }
            // A cast start: a mark on the details chart (no number changes).
            Event::CastStart { agent, skill, .. } if skill != 0 => {
                let at = t.saturating_sub(self.start_ms);
                let casts = self.casts.entry(self.owner(agent)).or_default();
                if casts.len() < MAX_CASTS {
                    casts.push((at, skill));
                }
            }
            _ => {}
        }
    }

    /// Keep our stack copy, the roster and the map current (in and out of fights), and feed the
    /// rows a stack event wrote to the boon tracker while a fight records.
    fn track_boons(&mut self, t: u64, ev: &Event) {
        if self.state == LiveState::Fight {
            self.track_active(ev);
        }
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
                // Left before its account was known: its rows, and whether it was squad and took
                // part, keep an id of their own, so the id's next owner does not get them. (The
                // orphan's squad flag is inert: with no subgroup known, it is never a target.)
                let rows = self.uncanon.remove(&id);
                let active = self.boon_active.remove(&id);
                let squad = self.boon_squad.remove(&id);
                if rows || active || squad {
                    let orphan = ORPHAN_IDS + self.orphans;
                    self.orphans = self.orphans.wrapping_add(1) % (1 << 30);
                    if rows {
                        self.boons.rename(id, orphan);
                    }
                    if active {
                        self.boon_active.insert(orphan);
                    }
                    if squad {
                        self.boon_squad.insert(orphan);
                    }
                }
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

    /// Note the players `ev` shows taking part in the fight, as axilog reads our log's rows: a
    /// strike's source and target, a condition tick's (with damage), a cast's source, and an
    /// enter-combat.
    fn track_active(&mut self, ev: &Event) {
        let ids = match *ev {
            // A zero-value defiance strike on a player is not taking part (arcdps never writes
            // these rows; our log still does).
            Event::Strike {
                dst,
                skill: DEFIANCE_DAMAGE,
                value: 0,
                result: 10,
                ..
            } if self.roster.get(&dst).is_some_and(|r| r.0) => return,
            Event::Strike { src, dst, .. } => [Some(src), Some(dst)],
            Event::BuffDamage {
                src, dst, value, ..
            }
            | Event::RelayBuffDamage {
                src, dst, value, ..
            } if value != 0 => [Some(src), Some(dst)],
            Event::CastStart { agent, .. } => [Some(agent), None],
            Event::Combat {
                id,
                in_combat: true,
            } => [Some(id), None],
            _ => return,
        };
        for id in ids.into_iter().flatten() {
            if id != 0 && !self.foreign(id) {
                self.boon_active.insert(self.canon(id));
            }
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
        if self.boon_active.remove(&id) {
            self.boon_active.insert(c);
        }
        if self.boon_squad.remove(&id) {
            self.boon_squad.insert(c);
        }
        if self.state == LiveState::Fight {
            // What it is now, in case it leaves before the next tick records it.
            self.boon_seen.insert(c, (reg.is_squad(id), a.subgroup));
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
                if id != 0 && id < ACCOUNT_IDS && !self.foreign(id) {
                    self.uncanon.insert(id);
                }
            }
            keep.push(r);
        }
        self.boons.rows(t, &keep);
    }

    /// Bring the boon totals to the latest game time (each feed's totals are as of its
    /// duration).
    fn tick_boons(&mut self, reg: &Registry) {
        let now = self.now_ms;
        if self.boon_tick == Some(now) {
            return;
        }
        self.boon_tick = Some(now);
        for (&c, &id) in &self.live_of {
            if let Some(a) = reg.agent(id) {
                self.boon_seen.insert(c, (reg.is_squad(id), a.subgroup));
            }
        }
        let squad_now: Vec<u32> = reg
            .live_agents()
            .filter(|a| reg.is_squad(a.id))
            .map(|a| self.canon(a.id))
            .collect();
        self.boon_squad.extend(squad_now);
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
        // Group and squad targets, as axilog takes them from the log's agent table: the fight's
        // squad players with a subgroup that took part in it (the POV always counts).
        let pov = reg.pov().map(|id| self.canon(id));
        let active = &self.boon_active;
        let targets: BTreeMap<u32, (u8, bool)> = self
            .boon_squad
            .iter()
            .filter(|&&c| squad(c))
            .filter_map(|&c| {
                let g = groups(c).filter(|g| (1..=15).contains(g))?;
                Some((c, (g, active.contains(&c) || pov == Some(c))))
            })
            .collect();
        let target = |c: u32| targets.get(&c).is_some_and(|t| t.1);
        // Regeneration's stack order reads the agent table's healing field, which arcdps and our
        // writer leave at 0 for every player: 0 is what axilog sees on both logs.
        self.boons.tick(now, &groups, &squad, &target, &|_| 0);
        self.boon_targets = targets;
        if self.state == LiveState::Fight {
            self.sample_boons(now);
        }
    }

    /// Note each squad player's boon uptime totals at game time `t` (its bucket). Buckets the
    /// ticks skipped are filled in a straight line from the last sample.
    fn sample_boons(&mut self, t: u64) {
        // The totals at `t` close the bucket that ends there.
        let i = self.bucket(t.saturating_sub(1).max(self.start_ms));
        if i >= MAX_BUCKETS {
            return;
        }
        for &c in &self.boon_squad {
            let Some(totals) = self.boons.totals(c) else {
                continue;
            };
            let hist = self.boon_hist.entry(c).or_default();
            for (&b, v) in totals {
                let cum = hist.entry(b).or_default();
                let (from, last) = cum.last().map_or((0, 0), |&l| (cum.len(), l));
                let now = v[0].max(last);
                if i < from {
                    // Still the last sample's bucket: it moves up.
                    if let Some(l) = cum.last_mut() {
                        *l = now;
                    }
                    continue;
                }
                let span = (i + 1 - from) as u64;
                for k in from..=i {
                    let done = (k + 1 - from) as u64;
                    cum.push(last + (now - last) * done / span);
                }
            }
        }
    }

    /// A tracker total with group and squad as per-target averages, as axilog's rollup takes
    /// them: ms over the other targets of the source's subgroup, and over every other target
    /// (rounded; 0 with no targets).
    fn per_target(&self, source: u32, v: [u64; 4]) -> [u64; 4] {
        let group = self.boon_targets.get(&source).map(|t| t.0);
        let others = self
            .boon_targets
            .iter()
            .filter(|&(&c, t)| c != source && t.1);
        let (mut n_group, mut n_squad) = (0u64, 0u64);
        for (_, t) in others {
            n_squad += 1;
            n_group += u64::from(group == Some(t.0));
        }
        let avg = |ms: u64, n: u64| (ms + n / 2).checked_div(n).unwrap_or(0);
        [v[0], v[1], avg(v[2], n_group), avg(v[3], n_squad)]
    }

    /// The accounts the boon numbers count as group and squad targets as of the last tick, with
    /// their subgroup (for checks against a log's agent table).
    pub fn boon_targets(&self) -> BTreeMap<String, u8> {
        self.by_account
            .iter()
            .filter_map(|(a, c)| match self.boon_targets.get(c) {
                Some(&(g, true)) => Some((a.clone(), g)),
                _ => None,
            })
            .collect()
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
        // The fight started with this step: the estimator starts from the stacks after it.
        let mut started = false;
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
                self.estimator = Some(Estimator::new(
                    p.buff_stacks(),
                    p.condi_known(),
                    reg.pov(),
                    GameMode::from_map_type(p.map_type()),
                ));
                started = true;
                // The squad at the start is in the fight, even if it leaves before a tick.
                let squad: Vec<u32> = reg
                    .live_agents()
                    .filter(|a| reg.is_squad(a.id))
                    .map(|a| a.id)
                    .collect();
                for id in squad {
                    self.boon_squad.insert(self.canon(id));
                }
                // Players in combat at the start enter combat in our log at the start.
                let fighting: Vec<u32> = reg
                    .live_agents()
                    .filter(|a| a.in_combat && a.kind == AgentKind::Player)
                    .map(|a| a.id)
                    .collect();
                for id in fighting {
                    self.boon_active.insert(self.canon(id));
                }
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
            if self.state == LiveState::Fight && p.registry().is_squad(id) {
                self.boon_squad.insert(self.canon(id));
            }
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
        // The model's estimates for the ticks nobody reports, due by this step.
        if self.state == LiveState::Fight
            && !started
            && let Some(t) = time
            && let Some(est) = self.estimator.as_mut()
        {
            let ticks = match ev {
                Some(ev) => est.event(p.registry(), t, &ev),
                None => est.advance(p.registry(), t),
            };
            if !ticks.is_empty() {
                self.count_estimates(p.registry(), ticks);
            }
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
            Event::Strike { .. } | Event::BuffDamage { .. } | Event::RelayBuffDamage { .. }
                if self.state == LiveState::Fight =>
            {
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
                    }
                    | Event::RelayBuffDamage {
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

    /// Whether `id`'s condition damage is estimated: no exact ticks of its own reach us (it is
    /// not the POV and relayed none this fight).
    fn estimated(&self, id: u32) -> bool {
        self.pov != Some(id) && !self.exact_condi.contains(&id)
    }

    /// The estimated condition damage shown for `id`.
    fn est_for(&self, id: u32) -> u64 {
        if self.estimated(id) {
            self.est_condi.get(&id).copied().unwrap_or(0)
        } else {
            0
        }
    }

    /// Count the model's estimated ticks (kept apart from exact damage).
    fn count_estimates(&mut self, reg: &Registry, ticks: Vec<crate::condi::Tick>) {
        for t in ticks {
            let by = self.owner(t.src);
            if by == self.owner(t.dst) || self.skips_target(reg, t.dst) {
                continue;
            }
            let dmg = u64::from(t.damage.unsigned_abs());
            *self.est_condi.entry(by).or_default() += dmg;
            let i = (t.time.saturating_sub(self.start_ms) / BUCKET_MS) as usize;
            add_at(self.est_series.entry(by).or_default(), i, dmg);
            add_skill(&mut self.est_skills, by, t.buff, dmg);
            add_skill_at(&mut self.est_skill_series, by, t.buff, i, dmg);
            self.condi_skills.insert(t.buff);
            self.generation += 1;
        }
    }

    /// A condition tick of `value` at game time `t`, dealt by `src` (0: no one) to `dst` (0: an
    /// agent unknown here, for a relayed tick).
    fn count_condi(&mut self, t: u64, src: u32, dst: u32, buff: u32, value: i32) {
        let by = self.owner(src);
        if value <= 0 || (dst != 0 && by == self.owner(dst)) {
            return;
        }
        let dmg = u64::from(value.unsigned_abs());
        if src != 0 {
            let a = self.acc.entry(by).or_default();
            a.condi = a.condi.saturating_add(dmg);
            add_skill(&mut self.skills_out, by, buff, dmg);
            let i = self.bucket(t);
            add_skill_at(&mut self.skill_series, by, buff, i, dmg);
        }
        if dst != 0 {
            let d = self.acc.entry(dst).or_default();
            d.taken = d.taken.saturating_add(dmg);
            add_skill(&mut self.skills_in, dst, buff, dmg);
        }
        self.condi_skills.insert(buff);
        self.add_series(t, if src != 0 { by } else { 0 }, dst, dmg, true);
        self.generation += 1;
    }

    /// `dmg` at game time `t`, dealt by `by` (0: no one to credit) to `dst` (0: no one known);
    /// `condi`: condition damage.
    fn add_series(&mut self, t: u64, by: u32, dst: u32, dmg: u64, condi: bool) {
        let i = self.bucket(t);
        if by != 0 {
            let s = self.series.entry(by).or_default();
            add_at(&mut s.out, i, dmg);
            if condi {
                add_at(&mut s.out_condi, i, dmg);
            }
        }
        if dst != 0 {
            add_at(&mut self.series.entry(dst).or_default().inc, i, dmg);
        }
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
        for (id, s) in &self.est_series {
            if self.idents.contains_key(id) && self.estimated(*id) {
                for (o, v) in out.iter_mut().zip(s) {
                    *o += v;
                }
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
            self.tick_boons(reg);
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
            let est = self.est_for(id);
            let condi = n.condi.saturating_add(est);
            let damage = n.power.saturating_add(condi);
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
            let c = self.boon_id(id, &who.account);
            let boons: serde_json::Map<String, Value> = self
                .boons
                .totals(c)
                .into_iter()
                .flatten()
                .map(|(b, v)| (b, self.per_target(c, *v)))
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
                    "condi": condi,
                    // The part of `condi` the model estimated (no exact ticks for this player).
                    "condi_est": est,
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

    /// Carry `account`'s breakdown in the feed (`None`: no details window open).
    pub fn set_detail(&mut self, account: Option<String>) {
        if self.detail != account {
            self.detail = account;
            self.generation += 1;
        }
    }

    /// Carry `skill`'s damage and healing per bucket in the breakdown (`None`: none selected).
    pub fn set_detail_skill(&mut self, skill: Option<u32>) {
        if self.detail_skill != skill {
            self.detail_skill = skill;
            self.generation += 1;
        }
    }

    /// The selected account's breakdown this fight (or the last): damage dealt and taken per
    /// bucket and by skill, and its downs and deaths. Every id the account had is summed (a
    /// player that left and came back has a new one).
    fn detail_value(&self, reg: &Registry) -> Value {
        let Some(account) = self.detail.as_deref() else {
            return Value::Null;
        };
        if self.state == LiveState::Idle {
            return Value::Null;
        }
        let ids: Vec<u32> = self
            .idents
            .iter()
            .filter(|(_, w)| w.account == account)
            .map(|(&id, _)| id)
            .collect();
        let Some(who) = ids.first().and_then(|id| self.idents.get(id)) else {
            return Value::Null;
        };
        let n = (self.duration_ms().div_ceil(BUCKET_MS) as usize).min(MAX_BUCKETS);
        let (mut out, mut inc) = (vec![0u64; n], vec![0u64; n]);
        let (mut out_condi, mut strip_s, mut cleanse_s) = (vec![0u64; n], vec![0u64; n], vec![0u64; n]);
        let (mut heal_s, mut heal_in_s) = (vec![0u64; n], vec![0u64; n]);
        let (mut by_out, mut by_in) = (BySkill::new(), BySkill::new());
        let (mut heals, mut heals_in) = (HealSkills::new(), HealSkills::new());
        let (mut targets, mut sources) = (BTreeMap::new(), BTreeMap::new());
        let mut removed: BTreeMap<u32, u64> = BTreeMap::new();
        let mut heal_full = who.me;
        let mut states = Vec::new();
        let mut casts = Vec::new();
        let (mut sel_out, mut sel_heal) = (vec![0u64; n], vec![0u64; n]);
        let sum_into = |to: &mut Vec<u64>, from: &[u64]| {
            for (o, v) in to.iter_mut().zip(from) {
                *o += v;
            }
        };
        for id in &ids {
            if let Some(s) = self.series.get(id) {
                sum_into(&mut out, &s.out);
                sum_into(&mut inc, &s.inc);
                sum_into(&mut out_condi, &s.out_condi);
                sum_into(&mut strip_s, &s.strips);
                sum_into(&mut cleanse_s, &s.cleanses);
            }
            // Healing given and received: the copy the rows show.
            for (given, copies, skills, series, peers) in [
                (true, &self.given, &mut heals, &mut heal_s, &mut targets),
                (false, &self.received, &mut heals_in, &mut heal_in_s, &mut sources),
            ] {
                let Some(c) = copies.get(id).and_then(CopySums::shown_copy) else {
                    continue;
                };
                for (&k, &(h, b, n)) in &c.skills {
                    let e = skills.entry(k).or_default();
                    e.0 += h;
                    e.1 += b;
                    e.2 += n;
                }
                sum_into(series, &c.series);
                if given
                    && let Some(v) = self.detail_skill.and_then(|k| c.skill_series.get(&k))
                {
                    sum_into(&mut sel_heal, v);
                }
                for (&peer, &h) in &c.peers {
                    add_heal_sum(peers.entry(self.owner(peer)).or_default(), h);
                }
            }
            heal_full |= self.given.get(id).is_some_and(|c| c.own.is_some());
            for (&buff, &n) in self.removed.get(id).into_iter().flatten() {
                *removed.entry(buff).or_default() += n;
            }
            for (from, to) in [
                (&self.skills_out, &mut by_out),
                (&self.skills_in, &mut by_in),
            ] {
                for (&skill, st) in from.get(id).into_iter().flatten() {
                    to.entry(skill).or_default().merge(st);
                }
            }
            if let Some(k) = self.detail_skill {
                if let Some(v) = self.skill_series.get(id).and_then(|m| m.get(&k)) {
                    sum_into(&mut sel_out, v);
                }
                if self.estimated(*id)
                    && let Some(v) = self.est_skill_series.get(id).and_then(|m| m.get(&k))
                {
                    sum_into(&mut sel_out, v);
                }
            }
            if self.estimated(*id) {
                let est = self.est_series.get(id).map_or(&[][..], Vec::as_slice);
                sum_into(&mut out, est);
                sum_into(&mut out_condi, est);
                for (&skill, st) in self.est_skills.get(id).into_iter().flatten() {
                    by_out.entry(skill).or_default().merge(st);
                }
            }
            states.extend(self.state_log.get(id).into_iter().flatten().copied());
            casts.extend(self.casts.get(id).into_iter().flatten().copied());
        }
        states.sort_by_key(|s| s.0);
        casts.sort_by_key(|c| c.0);
        let casts: Vec<Value> = casts.into_iter().map(|(at, skill)| json!([at, skill])).collect();
        let list = |m: BySkill| {
            let mut v: Vec<(u32, SkillStat)> = m.into_iter().collect();
            v.sort_by_key(|&(_, s)| std::cmp::Reverse(s.damage));
            v.truncate(DETAIL_SKILLS);
            v.into_iter()
                .map(|(id, s)| {
                    let mut row = json!({"id": id, "name": crate::assemble::skill_name(id),
                        "damage": s.damage, "hits": s.hits, "condi": self.condi_skills.contains(&id)});
                    // The skill panel's breakdown, only what happened (most skills have no blocks).
                    for (k, n) in [
                        ("min", u64::from(s.min)),
                        ("max", u64::from(s.max)),
                        ("crit", u64::from(s.crit)),
                        ("glance", u64::from(s.glance)),
                        ("block", u64::from(s.block)),
                        ("evade", u64::from(s.evade)),
                        ("interrupt", u64::from(s.interrupt)),
                        ("absorb", u64::from(s.absorb)),
                        ("miss", u64::from(s.miss)),
                        ("barrier", s.barrier),
                    ] {
                        if n > 0 {
                            row[k] = json!(n);
                        }
                    }
                    row
                })
                .collect::<Vec<_>>()
        };
        let heal_list = |m: HealSkills| {
            let mut v: Vec<(u32, (u64, u64, u32))> = m.into_iter().collect();
            v.sort_by_key(|&(_, (h, b, _))| std::cmp::Reverse(h + b));
            v.truncate(DETAIL_SKILLS);
            v.into_iter()
                .map(|(id, (healing, barrier, hits))| {
                    json!({"id": id, "name": crate::assemble::skill_name(id), "healing": healing,
                        "barrier": barrier, "hits": hits})
                })
                .collect::<Vec<_>>()
        };
        let peer_list = |m: BTreeMap<u32, HealSum>| {
            let mut v: Vec<(u32, HealSum)> = m.into_iter().collect();
            v.sort_by_key(|(_, h)| std::cmp::Reverse(h.healing + h.barrier));
            v.truncate(DETAIL_SKILLS);
            v.into_iter()
                .map(|(id, h)| {
                    let (name, account, prof, elite) = match self.idents.get(&id) {
                        Some(w) => (w.name.clone(), w.account.clone(), w.prof, w.elite),
                        None => reg
                            .agent(id)
                            .map_or((String::new(), String::new(), 0, 0), |a| {
                                (a.character.clone(), a.account.clone(), a.prof, a.elite)
                            }),
                    };
                    json!({"name": name, "account": account, "prof": prof,
                        "spec": crate::names::spec_name(prof, elite), "healing": h.healing,
                        "barrier": h.barrier})
                })
                .collect::<Vec<_>>()
        };
        let removed_list = |boons: bool| {
            let mut v: Vec<(u32, u64)> = removed
                .iter()
                .filter(|(b, _)| BOONS.contains(b) == boons)
                .map(|(&b, &n)| (b, n))
                .collect();
            v.sort_by_key(|&(_, n)| std::cmp::Reverse(n));
            v.into_iter()
                .map(|(id, n)| json!({"id": id, "name": crate::assemble::skill_name(id), "count": n}))
                .collect::<Vec<_>>()
        };
        let states: Vec<Value> = states
            .into_iter()
            .map(|(at, s)| {
                let s = match s {
                    AgentState::Up => "up",
                    AgentState::Down => "down",
                    AgentState::Dead => "dead",
                };
                json!([at, s])
            })
            .collect();
        json!({
            "account": who.account,
            "name": who.name,
            "prof": who.prof,
            "spec": crate::names::spec_name(who.prof, who.elite),
            "bucket_ms": BUCKET_MS,
            "out": out,
            "in": inc,
            "skills_out": list(by_out),
            "skills_in": list(by_in),
            "states": states,
            "casts": casts,
            "skill": self.detail_skill.map_or(Value::Null, |id| json!({"id": id, "out": sel_out, "heal": sel_heal})),
            // A selected boon: its uptime per bucket (ms; stack-ms for Might and Stability).
            "boon": self.detail_skill.filter(|b| SQUAD_BOONS.contains(b)).map_or(Value::Null, |b| {
                let c = self.boon_id(ids[0], account);
                let cum = self.boon_hist.get(&c).and_then(|h| h.get(&b)).map_or(&[][..], Vec::as_slice);
                let up: Vec<u64> = cum.iter().scan(0u64, |prev, &v| {
                    let d = v.saturating_sub(*prev);
                    *prev = v;
                    Some(d)
                }).collect();
                json!({"id": b, "up": up})
            }),
            "out_condi": out_condi,
            "heal_full": heal_full,
            "heal_out": heal_s,
            "heal_in": heal_in_s,
            "heals": heal_list(heals),
            "heals_in": heal_list(heals_in),
            "heal_targets": peer_list(targets),
            "heal_sources": peer_list(sources),
            "strip_series": strip_s,
            "cleanse_series": cleanse_s,
            "strips": removed_list(true),
            "cleanses": removed_list(false),
        })
    }

    /// The schema v1 value at wall time `unix_ms`. `reg` is the registry now (used while a
    /// fight is recording; an ended fight shows its players as of its end).
    pub fn snapshot(&mut self, reg: &Registry, unix_ms: u64) -> Value {
        let (state, players, timeline, healing) = match self.state {
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
            "detail": self.detail_value(reg),
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

/// Write the live feed: the stats snapshot plus `around`, when either changed and a write is
/// due (`last` ignores the interval, for a final write on exit). `Ok(true)` if it wrote.
pub fn write_feed(
    writer: &mut LiveWriter,
    stats: &mut LiveStats,
    around: &crate::around::Around,
    p: &Processor,
    now: Instant,
    unix_ms: u64,
    last: bool,
) -> io::Result<bool> {
    let generation = stats.generation().wrapping_add(around.generation());
    let due = if last {
        writer.changed(generation)
    } else {
        writer.due(now, generation)
    };
    if !due {
        return Ok(false);
    }
    let mut json = stats.snapshot(p.registry(), unix_ms);
    if let Some(o) = json.as_object_mut() {
        o.insert("around".to_owned(), around.value(p));
    }
    writer.write(now, generation, &json)?;
    Ok(true)
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

    /// Healing starts over with each fight, like damage: the POV's and a sharer's.
    #[test]
    fn healing_resets_each_fight() {
        let r = world();
        let mut l = LiveStats::new();
        l.set_pov(1);
        l.begin(1_000, 38);
        feed(
            &mut l,
            &r,
            &[(1_500, local_heal(1, 4, 300)), (1_600, relay(4, 4, 1, 900))],
        );
        l.end(2_000, &r);
        l.begin(5_000, 38);
        feed(
            &mut l,
            &r,
            &[(5_500, local_heal(1, 4, 40)), (5_600, relay(4, 4, 1, 70))],
        );
        let v = l.snapshot(&r, 0);
        assert_eq!(player(&v, "Char 1").unwrap()["healing"], 40);
        let p4 = player(&v, "Char 4").unwrap();
        assert_eq!(p4["healing"], 70);
        assert_eq!(p4["heal_full"], true);
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

    /// The details window's player: damage per second and by skill both ways, and its downs,
    /// kept after the fight ends.
    #[test]
    fn detail_breaks_down_one_players_damage() {
        let r = world();
        let mut l = LiveStats::new();
        l.begin(1000, 38);
        let hit = |src, dst, skill, value| Event::Strike {
            src,
            dst,
            skill,
            value,
            overstack: 0,
            result: 0,
        };
        feed(
            &mut l,
            &r,
            &[
                (1100, hit(1, NPC, 5, 500)),
                (1200, hit(1, NPC, 5, 100)),
                (
                    1300,
                    Event::Strike {
                        src: 1,
                        dst: NPC,
                        skill: 5,
                        value: 0,
                        overstack: 0,
                        result: 3,
                    },
                ),
                (2100, hit(NPC, 1, 7, 200)),
                (
                    2500,
                    Event::BuffDamage {
                        src: NPC,
                        dst: 1,
                        buff: 736,
                        value: 30,
                        barrier: 0,
                        stacks: 1,
                    },
                ),
                (
                    2800,
                    Event::State {
                        id: 1,
                        state: AgentState::Down,
                    },
                ),
            ],
        );
        l.advance(3000);
        assert!(l.snapshot(&r, 0)["detail"].is_null());
        let account = l.snapshot(&r, 0)["players"][0]["account"]
            .as_str()
            .unwrap()
            .to_owned();
        l.set_detail(Some(account.clone()));
        let d = l.snapshot(&r, 0)["detail"].clone();
        assert_eq!(d["account"], account);
        assert_eq!(d["out"], json!([600, 0]));
        assert_eq!(d["in"], json!([0, 230]));
        assert_eq!(
            d["skills_out"],
            json!([{"id": 5, "name": "", "damage": 600, "hits": 2, "condi": false,
                "min": 100, "max": 500, "block": 1}])
        );
        assert_eq!(d["skills_in"][0]["damage"], 200);
        assert_eq!(
            d["skills_in"][1],
            json!({"id": 736, "name": "Bleeding", "damage": 30, "hits": 1, "condi": true,
                "min": 30, "max": 30})
        );
        assert_eq!(d["states"], json!([[1800, "down"]]));
        l.end(3000, &r);
        assert_eq!(l.snapshot(&r, 0)["detail"]["out"], json!([600, 0]));
        l.begin(9000, 38);
        assert_eq!(l.snapshot(&r, 0)["detail"]["skills_out"], json!([]));
    }

    /// The breakdown also carries the player's condition damage, heals given and received by
    /// skill and by player, and the boons it stripped and conditions it cleansed.
    #[test]
    fn detail_lists_casts_in_time_order() {
        let r = world();
        let mut l = LiveStats::new();
        l.set_pov(1);
        l.begin(1000, 38);
        let cast = |agent, skill| Event::CastStart {
            agent,
            target: 0,
            skill,
            kind: 1,
            x: 0,
        };
        feed(
            &mut l,
            &r,
            &[
                (1100, tick(1, NPC, 40)),
                (1200, cast(1, 5491)),
                (1900, cast(4, 777)),
                (2500, cast(1, 5492)),
                (2600, cast(1, 0)),
            ],
        );
        l.advance(3000);
        let account = player(&l.snapshot(&r, 0), "Char 1").unwrap()["account"]
            .as_str()
            .unwrap()
            .to_owned();
        l.set_detail(Some(account));
        let d = l.snapshot(&r, 0)["detail"].clone();
        assert_eq!(d["casts"], json!([[200, 5491], [1500, 5492]]));
        assert_eq!(d["skill"], Value::Null);
        l.set_detail_skill(Some(5491));
        let d = l.snapshot(&r, 0)["detail"].clone();
        assert_eq!(d["skill"]["id"], 5491);
        assert_eq!(d["skill"]["out"], json!([0, 0]));
        // A strike of the skill lands in its bucket.
        let hit = Event::Strike {
            src: 1,
            dst: NPC,
            skill: 5491,
            value: 300,
            overstack: 0,
            result: 0,
        };
        feed(&mut l, &r, &[(2200, hit)]);
        let d = l.snapshot(&r, 0)["detail"].clone();
        assert_eq!(d["skill"]["out"], json!([0, 300]));
    }

    #[test]
    fn detail_breaks_down_heals_strips_and_cleanses() {
        let r = world();
        let mut l = LiveStats::new();
        l.set_pov(1);
        l.begin(1000, 38);
        let heal = |src, dst, skill, value, barrier| Event::Heal {
            src,
            dst,
            skill,
            value,
            barrier,
            buff: false,
        };
        let remove = |dst, inst| Event::BuffRemove {
            dst,
            remover: 1,
            inst,
        };
        feed(
            &mut l,
            &r,
            &[
                (1100, tick(1, NPC, 40)),
                (1200, heal(1, 4, 9, 300, 100)),
                (1300, heal(1, 4, 9, 50, 0)),
                (2100, heal(4, 1, 12, 80, 0)),
                (1400, boon_apply(ENEMY, ENEMY, 740, 70, 9000)),
                (1500, remove(ENEMY, 70)),
                (1600, boon_apply(4, NPC, 736, 71, 9000)),
                (2600, remove(4, 71)),
            ],
        );
        l.advance(3000);
        let account = player(&l.snapshot(&r, 0), "Char 1").unwrap()["account"]
            .as_str()
            .unwrap()
            .to_owned();
        l.set_detail(Some(account));
        let d = l.snapshot(&r, 0)["detail"].clone();
        assert_eq!(d["out_condi"], json!([40, 0]));
        assert_eq!(d["heal_full"], true);
        assert_eq!(d["heal_out"], json!([350, 0]));
        assert_eq!(d["heal_in"], json!([0, 80]));
        assert_eq!(
            d["heals"],
            json!([{"id": 9, "name": crate::assemble::skill_name(9), "healing": 250, "barrier": 100, "hits": 2}])
        );
        assert_eq!(d["heals_in"][0]["id"], 12);
        assert_eq!(d["heal_targets"][0]["name"], "Char 4");
        assert_eq!(d["heal_targets"][0]["healing"], 250);
        assert!(d["heal_targets"][0]["spec"].is_string());
        assert_eq!(d["heal_sources"][0]["name"], "Char 4");
        assert_eq!(d["strips"], json!([{"id": 740, "name": "Might", "count": 1}]));
        assert_eq!(d["cleanses"], json!([{"id": 736, "name": "Bleeding", "count": 1}]));
        assert_eq!(d["strip_series"], json!([1, 0]));
        assert_eq!(d["cleanse_series"], json!([0, 1]));
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

    /// A squadmate's relayed condition ticks (kind 63) fill its `condi`: its minions credit it,
    /// a tick with no local target still counts, and the POV's own copies are ignored.
    #[test]
    fn relayed_condi_counts_for_its_sender() {
        let r = relay_world();
        let mut l = LiveStats::new();
        l.set_pov(1);
        l.set_master(PET, 6);
        l.begin(0, 38);
        let relayed = |sender, src, dst, value| Event::RelayBuffDamage {
            sender,
            src,
            dst,
            buff: 736,
            value,
            barrier: 0,
        };
        feed(
            &mut l,
            &r,
            &[
                (100, relayed(6, 6, ENEMY, 200)),
                (200, relayed(6, PET, ENEMY, 50)),
                (300, relayed(6, 6, 0, 25)),
                (400, relayed(1, 1, ENEMY, 999)),
                (500, relayed(6, 6, ENEMY, 0)),
            ],
        );
        l.advance(1000);
        let v = l.snapshot(&r, 0);
        let p = player(&v, "Char 6").unwrap();
        assert_eq!(p["condi"], 275);
        assert_eq!(p["damage"], 275);
        assert_eq!(player(&v, "Char 1").unwrap()["condi"], 0);
    }

    /// Estimated ticks show in `condi` (and `condi_est`) for a player nobody reports exactly;
    /// once it relays its own, the estimate goes, including what came before.
    #[test]
    fn estimates_give_way_to_relayed_ticks() {
        let r = relay_world();
        let mut l = LiveStats::new();
        l.set_pov(1);
        l.begin(0, 38);
        let est = |time, src, damage| crate::condi::Tick {
            time,
            src,
            dst: ENEMY,
            buff: 736,
            damage,
            stacks: 1,
        };
        l.count_estimates(&r, vec![est(1_000, 6, 120), est(2_000, 6, 80)]);
        l.advance(3_000);
        let v = l.snapshot(&r, 0);
        let p = player(&v, "Char 6").unwrap();
        assert_eq!((p["condi"].as_u64(), p["condi_est"].as_u64()), (Some(200), Some(200)));
        assert_eq!(v["timeline"]["out"][1], 120);
        let relayed = Event::RelayBuffDamage {
            sender: 6,
            src: 6,
            dst: ENEMY,
            buff: 736,
            value: 300,
            barrier: 0,
        };
        feed(&mut l, &r, &[(3_000, relayed)]);
        let v = l.snapshot(&r, 0);
        let p = player(&v, "Char 6").unwrap();
        assert_eq!((p["condi"].as_u64(), p["condi_est"].as_u64()), (Some(300), Some(0)));
        assert_eq!(v["timeline"]["out"][1], 0);
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
            [
                "detail", "fight", "healing", "players", "state", "t", "timeline", "v"
            ]
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
            "condi_est",
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
            "condi_est",
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

    /// The boon totals are as of the feed's duration at every feed, not only once a second: a
    /// boon held the whole fight reads exactly 100% uptime at each feed written 500 ms apart.
    #[test]
    fn boons_are_as_of_every_feed() {
        let r = world();
        let mut l = LiveStats::new();
        l.begin(0, 38);
        l.event(0, &boon_apply(1, 4, PROTECTION, 900, 600_000));
        for t in (500..=20_000).step_by(500) {
            l.advance(t);
            let v = l.snapshot(&r, 1);
            let duration = v["fight"]["duration_ms"].as_u64().unwrap();
            assert_eq!(duration, t);
            assert_eq!(
                player(&v, "Char 1").unwrap()["boons"]["717"][0],
                duration,
                "uptime at {t}"
            );
        }
    }

    /// Group and squad generation are per target, averaged as axilog does: player 4 (group 2)
    /// protects player 1 (group 3) and player 5 (group 2) for 2 s each; player 6 (group 2) gets
    /// nothing. Squad: 4000 ms over 3 targets; group: 2000 ms over 2 (5 and 6).
    #[test]
    fn group_and_squad_generation_are_per_target_averages() {
        let mut r = world();
        for id in [5, 6] {
            for e in squad_events(id, 2) {
                r.apply(0, &e);
            }
        }
        let mut l = LiveStats::new();
        l.begin(0, 38);
        // 5 strikes, 6 enters combat: both take part (1 is the POV, which always counts).
        l.event(500, &strike(5, NPC, 10, 0));
        l.event(
            500,
            &Event::Combat {
                id: 6,
                in_combat: true,
            },
        );
        l.event(1000, &boon_apply(1, 4, PROTECTION, 900, 2000));
        l.event(1000, &boon_apply(5, 4, PROTECTION, 901, 2000));
        l.advance(5000);
        let v = l.snapshot(&r, 1);
        assert_eq!(
            player(&v, "Char 4").unwrap()["boons"],
            json!({"717": [0, 0, 1000, 1333]})
        );
        // Two squad mates at 2000 ms each: 2000 per target, not the 4000 sum.
        let mut r = world();
        for e in squad_events(5, 2) {
            r.apply(0, &e);
        }
        let mut l = LiveStats::new();
        l.begin(0, 38);
        l.event(500, &strike(NPC, 5, 10, 0));
        l.event(1000, &boon_apply(1, 4, PROTECTION, 900, 2000));
        l.event(1000, &boon_apply(5, 4, PROTECTION, 901, 2000));
        l.advance(5000);
        let v = l.snapshot(&r, 1);
        assert_eq!(
            player(&v, "Char 4").unwrap()["boons"],
            json!({"717": [0, 0, 2000, 2000]})
        );
    }

    /// A squad member who never takes part in the fight (axilog leaves idle squad members out of
    /// its players) is not a target: 5 holds the boon but does nothing else.
    #[test]
    fn idle_squad_members_are_not_targets() {
        let mut r = world();
        for e in squad_events(5, 2) {
            r.apply(0, &e);
        }
        let mut l = LiveStats::new();
        l.begin(0, 38);
        l.event(1000, &boon_apply(1, 4, PROTECTION, 900, 2000));
        l.event(1000, &boon_apply(5, 4, PROTECTION, 901, 2000));
        l.advance(5000);
        let v = l.snapshot(&r, 1);
        assert_eq!(
            player(&v, "Char 4").unwrap()["boons"],
            json!({"717": [0, 0, 0, 2000]})
        );
        // 5 still has its uptime.
        assert_eq!(player(&v, "Char 5").unwrap()["boons"]["717"][0], 2000);
    }

    /// A source with no other squad player in the fight has no group or squad generation.
    #[test]
    fn no_targets_means_no_group_or_squad_generation() {
        let mut r = Registry::new();
        for e in squad_events(1, 3) {
            r.apply(0, &e);
        }
        r.apply(0, &Event::Pov { id: 1 });
        let mut l = LiveStats::new();
        l.begin(0, 38);
        l.event(1000, &boon_apply(1, 1, PROTECTION, 900, 2000));
        l.advance(5000);
        let v = l.snapshot(&r, 1);
        assert_eq!(
            player(&v, "Char 1").unwrap()["boons"],
            json!({"717": [2000, 2000, 0, 0]})
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

    /// A boon picked in the details window: its uptime per second.
    #[test]
    fn detail_carries_a_picked_boons_uptime() {
        let r = world();
        let mut l = LiveStats::new();
        l.begin(0, 38);
        l.event(1000, &boon_apply(4, 1, PROTECTION, 900, 2000));
        let account = player(&l.snapshot(&r, 1), "Char 4").unwrap()["account"]
            .as_str()
            .unwrap()
            .to_owned();
        l.set_detail(Some(account));
        l.set_detail_skill(Some(PROTECTION));
        for t in [1000, 2000, 3000, 4000, 5000] {
            l.advance(t);
            l.snapshot(&r, 1);
        }
        let v = l.snapshot(&r, 1);
        assert_eq!(
            v["detail"]["boon"],
            json!({"id": PROTECTION, "up": [0, 1000, 1000, 0, 0]})
        );
        l.set_detail_skill(Some(5));
        assert!(l.snapshot(&r, 1)["detail"]["boon"].is_null());
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
        // Per target: Char 4 never takes part in the fight, so Char 1 is Xena's only target.
        assert_eq!(x["1187"][3], 3500);
    }

    /// Player A at id 7 fights and leaves before its account name arrives; the game gives id 7 to
    /// B, who never fights. A's "took part" stays A's: B is not a target, so Char 4's protection
    /// on Char 1 is over one target, not two.
    #[test]
    fn unnamed_leavers_activity_does_not_go_to_the_next_owner_of_the_id() {
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
        run(&mut p, &mut l, Step::Push(1000, strike(7, NPC, 400, 0)));
        run(
            &mut p,
            &mut l,
            Step::Push(1000, boon_apply(1, 4, PROTECTION, 900, 2000)),
        );
        run(
            &mut p,
            &mut l,
            Step::Push(2000, Event::AgentRemove { id: 7 }),
        );
        for e in named(7, 2, "Yara", ":Y.0002") {
            run(&mut p, &mut l, Step::Push(2500, e));
        }
        run(&mut p, &mut l, Step::Push(4000, strike(1, NPC, 400, 0)));
        run(&mut p, &mut l, Step::Tick(5000));
        let v = l.snapshot(p.registry(), 0);
        assert_eq!(v["state"], "fight");
        assert_eq!(
            player(&v, "Char 4").unwrap()["boons"],
            json!({"717": [0, 0, 0, 2000]})
        );
        assert!(!l.boon_targets().contains_key(":Y.0002"));
    }

    /// Player 7 joins mid-fight unnamed, takes part, gets its account name and leaves, all
    /// between two feeds: it counts as a squad target under its account (axilog counts every
    /// squad player of the log), so Char 4's protection on Char 1 is over two targets.
    #[test]
    fn a_leaver_named_between_ticks_stays_a_target() {
        let mut p = processor();
        let mut l = LiveStats::new();
        for e in world_events() {
            run(&mut p, &mut l, Step::Push(0, e));
        }
        run(&mut p, &mut l, Step::Push(0, map_event()));
        let enter = Event::Combat {
            id: 1,
            in_combat: true,
        };
        run(&mut p, &mut l, Step::Push(1000, enter));
        run(&mut p, &mut l, Step::Push(1100, add(7, 2)));
        run(&mut p, &mut l, Step::Push(1200, strike(7, NPC, 400, 0)));
        run(
            &mut p,
            &mut l,
            Step::Push(1200, boon_apply(1, 4, PROTECTION, 900, 2000)),
        );
        for e in name_events(7, NameField::Character, "Xena")
            .into_iter()
            .chain(name_events(7, NameField::Account, ":X.0001"))
        {
            run(&mut p, &mut l, Step::Push(1500, e));
        }
        run(
            &mut p,
            &mut l,
            Step::Push(1800, Event::AgentRemove { id: 7 }),
        );
        run(&mut p, &mut l, Step::Push(4000, strike(1, NPC, 400, 0)));
        run(&mut p, &mut l, Step::Tick(5000));
        let v = l.snapshot(p.registry(), 0);
        assert_eq!(v["state"], "fight");
        assert_eq!(l.boon_targets().get(":X.0001"), Some(&2));
        assert_eq!(
            player(&v, "Char 4").unwrap()["boons"],
            json!({"717": [0, 0, 0, 1000]})
        );
        // The raw id is not left behind as a squad player of its own.
        assert!(!l.boon_squad.contains(&7));
    }

    /// Zero-value defiance strikes on a player (rows arcdps never writes) do not make the player
    /// take part: 5 gets only those and is not a target; 6 gets a real strike and is.
    #[test]
    fn zero_defiance_strikes_on_players_are_not_taking_part() {
        let mut r = world();
        let mut l = LiveStats::new();
        for id in [5, 6] {
            for e in squad_events(id, 2) {
                r.apply(0, &e);
                l.event(0, &e);
            }
        }
        l.begin(0, 38);
        let defiance = |dst| Event::Strike {
            src: 0,
            dst,
            skill: DEFIANCE_DAMAGE,
            value: 0,
            overstack: 0,
            result: 10,
        };
        l.event(500, &defiance(5));
        l.event(500, &defiance(6));
        l.event(600, &strike(NPC, 6, 10, 0));
        l.event(1000, &boon_apply(1, 4, PROTECTION, 900, 2000));
        l.advance(5000);
        let v = l.snapshot(&r, 1);
        // Targets for 4: 1 (the POV) and 6; 2000 ms over 2.
        assert_eq!(
            player(&v, "Char 4").unwrap()["boons"],
            json!({"717": [0, 0, 0, 1000]})
        );
    }

    /// Player A at id 7 leaves before its account name arrives, and the game gives id 7 to B,
    /// whose name does arrive: A's rows stay A's, not B's (as holder or as source).
    #[test]
    fn unnamed_leaver_rows_do_not_go_to_the_next_owner_of_the_id() {
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
            Step::Push(1000, boon_apply(7, 4, PROTECTION, 900, 600_000)),
        );
        run(
            &mut p,
            &mut l,
            Step::Push(1000, boon_apply(1, 7, 1187, 901, 600_000)),
        );
        run(&mut p, &mut l, Step::Push(1500, strike(1, NPC, 400, 0)));
        run(
            &mut p,
            &mut l,
            Step::Push(2000, Event::AgentRemove { id: 7 }),
        );
        for e in named(7, 2, "Yara", ":Y.0002") {
            run(&mut p, &mut l, Step::Push(2500, e));
        }
        run(&mut p, &mut l, Step::Push(4000, strike(7, NPC, 400, 0)));
        run(&mut p, &mut l, Step::Tick(5000));
        let v = l.snapshot(p.registry(), 0);
        assert_eq!(v["state"], "fight");
        assert!(player(&v, "Yara").unwrap().get("boons").is_none());
        // Char 1 still held the quickness A gave.
        assert_eq!(player(&v, "Char 1").unwrap()["boons"]["1187"][0], 4000);
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
        // Per target: 2000 ms on one of two other squad players (Xena counts after leaving).
        assert_eq!(player(&v, "Char 4").unwrap()["boons"]["717"][3], 1000);
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

    /// The boon tick's budget on a big, long fight: 50 squad players in 10 subgroups, 10
    /// minutes, every squad boon from many sources (subgroup and squad-wide pulses every few
    /// seconds, self boons, might and stability stacking, queued duration stacks going active,
    /// strips), a feed every 500 ms. Prints each feed's `snapshot` cost (it runs the boon tick)
    /// and checks p99 is under 5 ms in a release build. Run it with
    /// `cargo test --release -p axidps-core --lib -- --ignored --nocapture boon_tick_budget`.
    #[test]
    #[ignore = "timing; run in release"]
    fn boon_tick_budget_50_players_10_minutes() {
        let (n, fight_ms) = (50u32, 600_000u64);
        let ids: Vec<u32> = (10..10 + n).collect();
        let group = |id: u32| ((id - 10) / 5 + 1) as u8;
        let mut r = Registry::new();
        let mut l = LiveStats::new();
        for &id in &ids {
            for e in squad_events(id, group(id)) {
                r.apply(0, &e);
                l.event(0, &e);
            }
        }
        r.apply(0, &Event::Pov { id: 10 });
        l.event(0, &Event::Pov { id: 10 });
        l.begin(0, 38);
        for &id in &ids {
            l.event(1, &strike(id, NPC, 100, 0));
        }
        let mut sim = crate::boons::tests::Traffic::new(9_377_917, ids.clone(), ENEMY);
        let (mut costs, mut base) = (Vec::new(), Vec::new());
        let (mut events, mut feeds) = (0usize, 0usize);
        for t in (100..=fight_ms).step_by(100) {
            for e in sim.step(t) {
                l.event(t, &e);
                events += 1;
            }
            if t % 500 == 0 {
                l.advance(t);
                let t0 = Instant::now();
                let v = l.snapshot(&r, 0);
                costs.push(t0.elapsed().as_secs_f64() * 1e3);
                feeds += 1;
                std::hint::black_box(v);
                // The same feed again, with no boon tick due.
                let t0 = Instant::now();
                std::hint::black_box(l.snapshot(&r, 0));
                base.push(t0.elapsed().as_secs_f64() * 1e3);
            }
        }
        let pct = |v: &[f64], p: f64| {
            let mut v = v.to_vec();
            v.sort_by(f64::total_cmp);
            v[((v.len() - 1) as f64 * p).round() as usize]
        };
        println!(
            "boon tick: {feeds} feeds, {events} buff events ({:.0}/s), {} rows held",
            events as f64 / (fight_ms as f64 / 1000.0),
            l.boons.rows_stored()
        );
        for (i, (minute, without)) in costs.chunks(120).zip(base.chunks(120)).enumerate() {
            println!(
                "  minute {}: p50 {:.3} p99 {:.3} max {:.3} ms (no tick: p50 {:.3} p99 {:.3})",
                i + 1,
                pct(minute, 0.5),
                pct(minute, 0.99),
                pct(minute, 1.0),
                pct(without, 0.5),
                pct(without, 0.99)
            );
        }
        let (p50, p99, max) = (pct(&costs, 0.5), pct(&costs, 0.99), pct(&costs, 1.0));
        println!(
            "  all: p50 {p50:.3} p99 {p99:.3} max {max:.3} ms (no tick: p50 {:.3} p99 {:.3})",
            pct(&base, 0.5),
            pct(&base, 0.99)
        );
        let v = l.snapshot(&r, 0);
        let up = player(&v, "Char 30").unwrap()["boons"]["740"][0].as_u64();
        assert!(up.is_some_and(|u| u > 0), "might counted");
        if !cfg!(debug_assertions) {
            assert!(p99 < 5.0, "p99 {p99:.3} ms");
        }
    }

    #[test]
    fn write_atomic_reports_errors() {
        let path = std::env::temp_dir()
            .join("axidps-live-missing-dir")
            .join("nope")
            .join("live.json");
        assert!(write_atomic(&path, b"{}").is_err());
    }

    fn feed_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("axidps-feed-{name}-{}.json", std::process::id()))
    }

    fn add_player(id: u32) -> Event {
        Event::AgentAdd {
            id,
            kind: AgentKind::Player,
            prof: 1,
            elite: 0,
            species: 0,
            team: 1,
            subgroup: 0,
            wvw_rank: 0,
            wvw_rank_def: 0,
        }
    }

    fn feed_step(
        p: &mut Processor,
        l: &mut LiveStats,
        a: &mut crate::around::Around,
        ev: Event,
    ) {
        let s = Step::Push(0, ev);
        let o = crate::replay::apply(p, &s);
        l.after_step(p, &s, o.as_ref());
        a.after_step(p, &s);
    }

    #[test]
    fn feed_carries_around() {
        let path = feed_path("carries");
        let mut w = LiveWriter::new(path.clone());
        let (mut p, mut l, mut a) = (processor(), LiveStats::new(), crate::around::Around::new());
        feed_step(&mut p, &mut l, &mut a, add_player(5));
        assert!(write_feed(&mut w, &mut l, &a, &p, Instant::now(), 0, false).unwrap());
        let v: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert!(v["around"]["players"].is_array());
        for k in ["v", "t", "state", "fight", "players", "timeline", "healing", "detail"] {
            assert!(v.get(k).is_some(), "missing {k}");
        }
    }

    #[test]
    fn around_alone_triggers_a_write() {
        let path = feed_path("alone");
        let mut w = LiveWriter::new(path.clone());
        let (mut p, mut l, mut a) = (processor(), LiveStats::new(), crate::around::Around::new());
        let t0 = Instant::now();
        assert!(write_feed(&mut w, &mut l, &a, &p, t0, 0, false).unwrap());
        let later = t0 + Duration::from_millis(600);
        assert!(!write_feed(&mut w, &mut l, &a, &p, later, 0, false).unwrap());
        let before = l.generation();
        // Only Around sees this: LiveStats is not fed.
        let s = Step::Push(0, add_player(5));
        crate::replay::apply(&mut p, &s);
        a.after_step(&p, &s);
        assert_eq!(l.generation(), before);
        assert!(write_feed(&mut w, &mut l, &a, &p, later, 0, false).unwrap());
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn existing_keys_are_unchanged() {
        let path = feed_path("unchanged");
        let mut w = LiveWriter::new(path.clone());
        let (mut p, mut l, mut a) = (processor(), LiveStats::new(), crate::around::Around::new());
        for id in [1, 4, 5] {
            feed_step(&mut p, &mut l, &mut a, add_player(id));
        }
        feed_step(&mut p, &mut l, &mut a, Event::Pov { id: 1 });
        assert!(write_feed(&mut w, &mut l, &a, &p, Instant::now(), 7, false).unwrap());
        let mut v: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert!(v.as_object_mut().unwrap().remove("around").is_some());
        assert_eq!(v, l.snapshot(p.registry(), 7));
    }
}
