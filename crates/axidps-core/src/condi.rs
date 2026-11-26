//! The condition model: other players' condition ticks, estimated the way arcdps does,
//! from game data only:
//!
//! - **When:** a target ticks every 1000 ms once its first ticking condition lands (the phase
//!   [`crate::buffs::Stacks`] already keeps for arcdps's overstack). Every stack active at the
//!   tick counts in full, one per stack. Duration and expertise play no part.
//! - **How much:** the buff's damage modifier from game content (`BuffFormula`, kind 50),
//!   evaluated as the game's own formula evaluator does, with the source's attributes from
//!   `CoreStats` (kind 49). Minions use their master's stats.
//! - **Which modifier is damage:** learned from Poison (723) like arcdps: the `target` kind of
//!   its first condition-damage-scaled modifier (formula type 1 or 2).
//! - **Which modifiers apply:** a buff lists trait and mode variants next to its base formula
//!   (Bleeding has 9 damage modifiers). Only modifiers whose game-mode context fits the map
//!   ([`mode_applies`]) and that need no trait or buff (`prereq`) are summed. Recordings from
//!   before `prereq` was sent have no usable formula.
//!
//! Not modelled yet: outgoing/incoming condition-damage % (arcdps builds them from buffs and
//! traits), the level downscale (1 in WvW, everyone is 80), and arcdps's per-trait special
//! cases. The self-check against the POV's exact ticks measures what that costs.

use std::collections::BTreeMap;

use axidps_protocol::v2::Event;

use crate::buffs::Fired;

/// Poison: the buff whose condition-damage modifier names the damage kind.
pub const POISON: u32 = 723;
/// Attribute indexes in `CoreStats` (game enum; *guess* from arcdps/EI, P6).
pub const POWER: usize = 1;
pub const FEROCITY: usize = 5;
pub const HEALING: usize = 6;
pub const CONDITION_DAMAGE: usize = 7;
/// `BuffFormula::prereq` bit 7: the prerequisite refs were read; bits 0-3: refs set.
pub const PREREQ_READ: u8 = 0x80;

/// The game mode a modifier's context is checked against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameMode {
    Pve = 1,
    Pvp = 2,
    Wvw = 3,
}

impl GameMode {
    /// From the map type (GW2 `MapType`: 2, 6, 8 PvP; 9-15 WvW; the rest PvE).
    pub fn from_map_type(map_type: Option<u32>) -> Self {
        match map_type {
            Some(t) if crate::names::is_wvw_map(t) => GameMode::Wvw,
            Some(2 | 6 | 8) => GameMode::Pvp,
            _ => GameMode::Pve,
        }
    }
}

/// The game's check of a modifier's mode context: 0 any mode, 1 all but PvP,
/// 2 PvP, 3 WvW, 4 PvE. Context 5 calls a check the model does not have: never applies.
pub fn mode_applies(context: u32, mode: GameMode) -> bool {
    match context {
        0 => true,
        1 => mode != GameMode::Pvp,
        2 => mode == GameMode::Pvp,
        3 => mode == GameMode::Wvw,
        4 => mode == GameMode::Pve,
        _ => false,
    }
}

/// Players' flat base (as arcdps, level 80): +1000 to Power, Precision,
/// Toughness and Vitality.
pub const PLAYER_BASE: [(usize, i16); 4] = [(1, 1000), (2, 1000), (3, 1000), (4, 1000)];

/// Buff modifier kind -> attribute index (CHAR_ATTRIBUTE, as `CoreStats` and itemstats number
/// them). Kinds come in pairs: even flat (gear, food, infusions), odd level-scaled (boons,
/// signets); arcdps maps both to one slot (Might is 0x7 condition damage and
/// 0xf power, +30 each at level 80).
pub const KIND_ATTRS: &[(i32, usize)] = &[
    (0x4, 8),
    (0x5, 8), // Concentration
    (0x6, CONDITION_DAMAGE),
    (0x7, CONDITION_DAMAGE),
    (0x8, 9),
    (0x9, 9), // Expertise
    (0xa, FEROCITY),
    (0xb, FEROCITY),
    (0xc, HEALING),
    (0xd, HEALING),
    (0xe, POWER),
    (0xf, POWER),
    (0x10, 2),
    (0x11, 2), // Precision
    (0x12, 3),
    (0x13, 3), // Toughness
    (0x14, 4),
    (0x15, 4), // Vitality
    (0x1, 10),
    (0x2, 10), // Agony Resistance
    (0x16, 11),
];

/// A player's gear as the DLL sent it (kinds 58, 59).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Gear {
    /// Itemstat points from the normal lists and from the WvW (alternate) lists.
    pub normal: Option<[i16; 12]>,
    pub alt: Option<[i16; 12]>,
    /// `(skill, copies)` by index; complete once every slot is filled.
    pub skills: Vec<Option<(u32, u16)>>,
}

/// Self-check pass threshold per condition per fight (design §4: about 2%).
pub const SELF_CHECK_TOLERANCE: f64 = 0.02;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Modifier {
    pub target: i32,
    pub ftype: u8,
    pub base: f32,
    pub c1: f32,
    pub c2: f32,
    pub mode: u32,
    /// [`PREREQ_READ`] alone: needs nothing. 0: not known (an older recording).
    pub prereq: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stats {
    pub level: u16,
    pub attrs: [i16; 12],
}

impl Stats {
    fn attr(&self, i: usize) -> f32 {
        f32::from(self.attrs.get(i).copied().unwrap_or(0))
    }

    /// The levels-only snapshot leaves every attribute at 0.
    pub fn has_attributes(&self) -> bool {
        self.attrs.iter().any(|&a| a != 0)
    }
}

/// Formulas and stats known so far: formulas for the session, stats per live agent.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Known {
    formulas: BTreeMap<u32, Vec<Option<Modifier>>>,
    stats: BTreeMap<u32, Stats>,
    /// Equipped gear per player (the source of attributes for those whose `CoreStats` carry
    /// levels only).
    gear: BTreeMap<u32, Gear>,
}

impl Known {
    pub fn apply(&mut self, ev: &Event) {
        match *ev {
            Event::BuffFormula {
                buff,
                index,
                count,
                ftype,
                target,
                base,
                c1,
                c2,
                mode,
                prereq,
                ..
            } => {
                let mods = self.formulas.entry(buff).or_default();
                if mods.len() != usize::from(count) {
                    *mods = vec![None; usize::from(count)];
                }
                if let Some(slot) = mods.get_mut(usize::from(index)) {
                    *slot = Some(Modifier {
                        target,
                        ftype,
                        base: f32::from_bits(base),
                        c1: f32::from_bits(c1),
                        c2: f32::from_bits(c2),
                        mode,
                        prereq,
                    });
                }
            }
            Event::CoreStats { id, level, attrs } => {
                self.stats.insert(id, Stats { level, attrs });
            }
            Event::GearAttrs { id, alt, attrs } => {
                let g = self.gear.entry(id).or_default();
                if alt == 0 {
                    g.normal = Some(attrs);
                } else {
                    g.alt = Some(attrs);
                }
            }
            Event::GearSkill {
                id,
                index,
                count,
                skill,
                n,
            } => {
                let g = self.gear.entry(id).or_default();
                if g.skills.len() != usize::from(count) {
                    g.skills = vec![None; usize::from(count)];
                }
                if let Some(slot) = g.skills.get_mut(usize::from(index)) {
                    *slot = Some((skill, n));
                }
            }
            Event::AgentRemove { id } => {
                self.stats.remove(&id);
                self.gear.remove(&id);
            }
            _ => {}
        }
    }

    /// A new map: instance ids are reused, formulas stay (content data).
    pub fn forget_instances(&mut self) {
        self.stats.clear();
        self.gear.clear();
    }

    pub fn gear(&self, id: u32) -> Option<&Gear> {
        self.gear.get(&id)
    }

    pub fn stats(&self, id: u32) -> Option<&Stats> {
        self.stats.get(&id)
    }

    /// Every modifier of `buff`, once all of them arrived.
    pub fn modifiers(&self, buff: u32) -> Option<Vec<Modifier>> {
        self.formulas.get(&buff)?.iter().copied().collect()
    }

    /// The damage modifier kind, learned from Poison.
    pub fn damage_kind(&self) -> Option<i32> {
        self.modifiers(POISON)?
            .iter()
            .find(|m| matches!(m.ftype, 1 | 2))
            .map(|m| m.target)
    }
}

/// The game's formula evaluation for a stack: `level` is the stack's level byte,
/// `src` the source's stats. `None` for the types that need data the model lacks (12, 15).
pub fn eval(m: &Modifier, level: u8, src: &Stats) -> Option<f32> {
    let l = f32::from(level);
    let ls = f32::from(src.level);
    let (b, c1, c2) = (m.base, m.c1, m.c2);
    Some(match m.ftype {
        0 | 5 | 9 | 10 => b + l * c1,
        1 => b + l * c1 + src.attr(CONDITION_DAMAGE) * c2,
        2 => b + l * l * c1 + src.attr(CONDITION_DAMAGE) * c2,
        3 => b + l * c1 + src.attr(FEROCITY) * c2,
        4 => b + l * l * c1 + src.attr(FEROCITY) * c2,
        6 => b,
        7 => b + ls * c1 + src.attr(HEALING) * c2,
        8 => b + ls * ls * c1 + src.attr(HEALING) * c2,
        11 => (b + l * c1).min(c2),
        13 => b + l * c1 + src.attr(POWER) * c2,
        14 => b + l * l * c1 + src.attr(POWER) * c2,
        _ => return None,
    })
}

/// One estimated condition tick: arcdps's row for `(src, dst, buff)` at `time`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tick {
    pub time: u64,
    pub src: u32,
    pub dst: u32,
    pub buff: u32,
    pub damage: i32,
    pub stacks: u32,
}

/// Per-condition sums for one fight: model vs the game's exact POV ticks.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SelfCheck {
    /// `buff → (model, exact)` damage.
    pub sums: BTreeMap<u32, (i64, i64)>,
    /// `buff → (model ticks, exact rows)`: one model tick per (target, tick) the POV's stacks
    /// of it were active at, one exact row per game tick. Checks the schedule without formulas.
    pub ticks: BTreeMap<u32, (u32, u32)>,
    /// POV stack-ticks the model could not estimate (no formula or no stats).
    pub unmodelled: u32,
    /// The POV's attributes rebuilt from gear vs the game's own, at the fight's end: the gear
    /// model's calibration (traits are not rebuilt, so a gap there is expected).
    pub pov_stats: Option<([i16; 12], [i16; 12])>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// The POV dealt no condition ticks this fight.
    NotRun,
    /// The POV dealt condition ticks, but some could not be modelled (no formula or no stats).
    Incomplete,
    Pass,
    /// The worst condition's relative gap, in per mille.
    Fail { buff: u32, gap_permille: u32 },
}

impl SelfCheck {
    pub fn exact(&mut self, buff: u32, value: i32) {
        self.sums.entry(buff).or_default().1 += i64::from(value);
        self.ticks.entry(buff).or_default().1 += 1;
    }

    pub fn verdict(&self) -> Verdict {
        if self.sums.values().all(|&(_, exact)| exact <= 0) {
            return Verdict::NotRun;
        }
        if self.unmodelled > 0 {
            return Verdict::Incomplete;
        }
        let mut worst: Option<(u32, f64)> = None;
        for (&buff, &(model, exact)) in &self.sums {
            if exact <= 0 {
                continue;
            }
            let gap = (model - exact).abs() as f64 / exact as f64;
            if worst.is_none_or(|(_, g)| gap > g) {
                worst = Some((buff, gap));
            }
        }
        match worst {
            None => Verdict::NotRun,
            Some((_, g)) if g <= SELF_CHECK_TOLERANCE => Verdict::Pass,
            Some((buff, g)) => Verdict::Fail {
                buff,
                gap_permille: (g * 1000.0).round().min(u32::MAX as f64) as u32,
            },
        }
    }
}

/// Counts for the log line after a fight.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Coverage {
    /// Other sources' stack-ticks estimated.
    pub modelled: u32,
    /// Stack-ticks with no formula yet for their buff.
    pub no_formula: u32,
    /// Stack-ticks whose source has no attributes (none sent, or levels only).
    pub no_stats: u32,
}

/// The model for one fight.
#[derive(Debug, Clone)]
pub struct Model {
    pub known: Known,
    pov: Option<u32>,
    mode: GameMode,
    pub check: SelfCheck,
    pub coverage: Coverage,
}

impl Model {
    pub fn new(known: Known, pov: Option<u32>, mode: GameMode) -> Self {
        Model {
            known,
            pov,
            mode,
            check: SelfCheck::default(),
            coverage: Coverage::default(),
        }
    }

    /// The estimated rows of one target tick. `master` gives a minion's owner (stats source).
    /// The POV's own ticks (as source or target) are exact in the log already: they only feed
    /// the self-check.
    pub fn tick(
        &mut self,
        f: &Fired,
        master: impl Fn(u32) -> Option<u32>,
        buffs: impl Fn(u32) -> Vec<(u32, u8)>,
    ) -> Vec<Tick> {
        let mut out: BTreeMap<(u32, u32), (f32, u32)> = BTreeMap::new();
        let mut pov_groups: std::collections::BTreeSet<(u32, u32)> = Default::default();
        let kind = self.known.damage_kind();
        for &(src, buff, level) in &f.stacks {
            let pov_src = self.pov == Some(src) || master(src).is_some_and(|m| self.pov == Some(m));
            if pov_src {
                pov_groups.insert((src, buff));
            }
            let mods = kind.and_then(|k| self.damage_mods(k, buff));
            let dmg = mods
                .as_deref()
                .and_then(|mods| self.stack_damage(mods, src, level, &master, &buffs));
            match dmg {
                Some(d) => {
                    let e = out.entry((src, buff)).or_default();
                    e.0 += d;
                    e.1 += 1;
                }
                None if mods.is_none() => self.coverage.no_formula += 1,
                None => self.coverage.no_stats += 1,
            }
            if pov_src && dmg.is_none() {
                self.check.unmodelled += 1;
            }
        }
        for &(_, buff) in &pov_groups {
            self.check.ticks.entry(buff).or_default().0 += 1;
        }
        let mut ticks = Vec::new();
        for ((src, buff), (dmg, stacks)) in out {
            let damage = dmg.round() as i32;
            let pov_src = self.pov == Some(src) || master(src).is_some_and(|m| self.pov == Some(m));
            if pov_src {
                self.check.sums.entry(buff).or_default().0 += i64::from(damage);
                continue;
            }
            if self.pov == Some(f.target) {
                continue;
            }
            self.coverage.modelled += stacks;
            ticks.push(Tick {
                time: f.time,
                src,
                dst: f.target,
                buff,
                damage,
                stacks,
            });
        }
        ticks
    }

    /// `buff`'s damage modifiers that apply in this mode with no trait or buff needed. `None`
    /// when the formula is missing, incomplete, from before `prereq` was sent, or has none.
    fn damage_mods(&self, kind: i32, buff: u32) -> Option<Vec<Modifier>> {
        let mods = self.known.modifiers(buff)?;
        if mods.iter().any(|m| m.prereq & PREREQ_READ == 0) {
            return None;
        }
        let out: Vec<Modifier> = mods
            .into_iter()
            .filter(|m| m.target == kind && m.prereq == PREREQ_READ)
            .filter(|m| mode_applies(m.mode, self.mode))
            .collect();
        (!out.is_empty()).then_some(out)
    }

    fn stack_damage(
        &self,
        mods: &[Modifier],
        src: u32,
        level: u8,
        master: &impl Fn(u32) -> Option<u32>,
        buffs: &impl Fn(u32) -> Vec<(u32, u8)>,
    ) -> Option<f32> {
        let stats = self
            .source_stats(src, buffs)
            .or_else(|| master(src).and_then(|m| self.source_stats(m, buffs)))?;
        let mut sum = None;
        for m in mods {
            *sum.get_or_insert(0.0) += eval(m, level, &stats)?;
        }
        sum
    }

    /// Record the POV's rebuilt attributes next to its exact ones (see [`SelfCheck::pov_stats`]).
    pub fn check_pov_stats(&mut self, buffs: impl Fn(u32) -> Vec<(u32, u8)>) {
        let Some(pov) = self.pov else {
            return;
        };
        let exact = self.known.stats(pov).filter(|s| s.has_attributes()).copied();
        if let (Some(rebuilt), Some(exact)) = (self.rebuilt_stats(pov, &buffs), exact) {
            self.check.pov_stats = Some((rebuilt.attrs, exact.attrs));
        }
    }

    /// `id`'s attributes: the game's own when it sent them (the POV), else rebuilt from gear.
    pub fn source_stats(&self, id: u32, buffs: &impl Fn(u32) -> Vec<(u32, u8)>) -> Option<Stats> {
        if let Some(s) = self.known.stats(id).filter(|s| s.has_attributes()) {
            return Some(*s);
        }
        self.rebuilt_stats(id, buffs)
    }

    /// `id`'s attributes rebuilt like arcdps: the player base, itemstat points (the WvW lists
    /// in WvW), and the modifiers of its gear skills and active buffs `buffs(id)` (`(buff,
    /// level)` per active instance). A skill that is both gear and an active buff (item infix
    /// buffs show on the buff bar too) counts max(copies, instances) times. Traits are not
    /// modelled. `None` until the gear is complete.
    pub fn rebuilt_stats(&self, id: u32, buffs: &impl Fn(u32) -> Vec<(u32, u8)>) -> Option<Stats> {
        let gear = self.known.gear(id)?;
        let points = match self.mode {
            GameMode::Wvw => gear.alt.or(gear.normal),
            _ => gear.normal,
        }?;
        let skills: Vec<(u32, u16)> = gear.skills.iter().copied().collect::<Option<_>>()?;
        let mut attrs = points;
        for (i, v) in PLAYER_BASE {
            attrs[i] = attrs[i].saturating_add(v);
        }
        let level = self.known.stats(id).map_or(80, |s| s.level);
        let base = Stats { level, attrs };
        // skill -> (level, copies): gear skills at level 80, active buffs at their level.
        let mut counts: BTreeMap<u32, (u8, u32)> = BTreeMap::new();
        let mut held: BTreeMap<u32, (u8, u32)> = BTreeMap::new();
        for (buff, lvl) in buffs(id) {
            let e = held.entry(buff).or_insert((lvl, 0));
            e.1 += 1;
        }
        for &(skill, n) in &skills {
            let e = counts.entry(skill).or_insert((80, 0));
            e.1 += u32::from(n);
        }
        for (buff, (lvl, n)) in held {
            let e = counts.entry(buff).or_insert((lvl, 0));
            e.0 = lvl;
            e.1 = e.1.max(n);
        }
        let mut add = [0f32; 12];
        for (skill, (lvl, n)) in counts {
            let Some(mods) = self.known.modifiers(skill) else {
                continue;
            };
            for m in mods {
                let Some(&(_, attr)) = KIND_ATTRS.iter().find(|(k, _)| *k == m.target) else {
                    continue;
                };
                if m.prereq != PREREQ_READ || !mode_applies(m.mode, self.mode) {
                    continue;
                }
                if let Some(v) = eval(&m, lvl, &base) {
                    add[attr] += v * n as f32;
                }
            }
        }
        for (a, v) in attrs.iter_mut().zip(add) {
            *a = (f32::from(*a) + v.round()).clamp(f32::from(i16::MIN), f32::from(i16::MAX)) as i16;
        }
        Some(Stats { level, attrs })
    }
}

/// Enemy health as a check on the model (the server sends every agent's health; it sends
/// condition ticks only for the POV's). Per target, between two health samples taken while the
/// target is up: health lost (net of healing), strike damage, the POV's exact condition ticks and
/// the model's estimates for everyone else. What strikes and the POV's ticks do not explain is
/// mostly other players' condition damage. Downed/dead time and the interval a down ends are
/// left out (the downed pool is another bar).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HealthCheck {
    /// target -> (last up sample hp, pending since then).
    open: BTreeMap<u32, (f64, Interval)>,
    pub totals: Interval,
    /// Health lost over the closed intervals (net: healing lowers it), and the drops alone
    /// (healing between two samples still hides damage).
    pub lost: f64,
    pub dropped: f64,
    /// Closed intervals, and those with a drop.
    pub intervals: u32,
    pub targets: std::collections::BTreeSet<u32>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Interval {
    pub strikes: f64,
    pub pov_exact: f64,
    pub model: f64,
}

impl HealthCheck {
    /// `id` (a tracked target) sent health `pct_x100` of `max` while up: close its interval.
    pub fn health(&mut self, id: u32, pct_x100: u16, max: u32) {
        if max == 0 {
            return;
        }
        let hp = f64::from(pct_x100) / 10_000.0 * f64::from(max);
        if let Some((prev, pending)) = self.open.insert(id, (hp, Interval::default())) {
            self.lost += prev - hp;
            self.dropped += (prev - hp).max(0.0);
            self.intervals += 1;
            self.totals.strikes += pending.strikes;
            self.totals.pov_exact += pending.pov_exact;
            self.totals.model += pending.model;
            self.targets.insert(id);
        }
    }

    /// `id` went down, died or left: its open interval is dropped.
    pub fn interrupt(&mut self, id: u32) {
        self.open.remove(&id);
    }

    fn pending(&mut self, id: u32) -> Option<&mut Interval> {
        self.open.get_mut(&id).map(|(_, p)| p)
    }

    pub fn strike(&mut self, dst: u32, dmg: i32) {
        if let Some(p) = self.pending(dst) {
            p.strikes += f64::from(dmg.max(0));
        }
    }

    pub fn pov_tick(&mut self, dst: u32, dmg: i32) {
        if let Some(p) = self.pending(dst) {
            p.pov_exact += f64::from(dmg.max(0));
        }
    }

    pub fn model_tick(&mut self, dst: u32, dmg: i32) {
        if let Some(p) = self.pending(dst) {
            p.model += f64::from(dmg.max(0));
        }
    }

    /// Condition damage from others that health implies: drops − strikes − the POV's ticks.
    /// Drops, not net loss: in WvW healing on enemies is several times their net loss (replay
    /// of 2026-11-25 09:15: net 0.4 M vs drops 2.0 M vs strikes 2.0 M). Healing inside a sample
    /// still hides some damage, so this runs low.
    pub fn residual(&self) -> f64 {
        self.dropped - self.totals.strikes - self.totals.pov_exact
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLEEDING: u32 = 736;
    const DMG: i32 = 0x77;

    fn formula(buff: u32, mods: &[(i32, u8, f32, f32, f32)]) -> Vec<Event> {
        mods.iter()
            .enumerate()
            .map(|(i, &(target, ftype, base, c1, c2))| Event::BuffFormula {
                buff,
                index: i as u8,
                count: mods.len() as u8,
                category: 2,
                ftype,
                target,
                base: base.to_bits(),
                c1: c1.to_bits(),
                c2: c2.to_bits(),
                mode: 0,
                prereq: PREREQ_READ,
            })
            .collect()
    }

    fn stats(id: u32, condi: i16) -> Event {
        let mut attrs = [0i16; 12];
        attrs[POWER] = 1000;
        attrs[CONDITION_DAMAGE] = condi;
        Event::CoreStats {
            id,
            level: 80,
            attrs,
        }
    }

    /// Poison names the damage kind; Bleeding is 2 + 0.25·L + 0.06·CD per stack (made-up
    /// numbers in the game's shape), plus a non-damage modifier.
    fn known() -> Known {
        let mut k = Known::default();
        let mut evs = formula(POISON, &[(9, 6, 0.0, 0.0, 0.0), (DMG, 1, 8.0, 0.4, 0.06)]);
        evs.extend(formula(BLEEDING, &[(DMG, 1, 2.0, 0.25, 0.06), (9, 6, 1.0, 0.0, 0.0)]));
        evs.push(stats(10, 1000));
        evs.push(Event::CoreStats {
            id: 11,
            level: 80,
            attrs: [0; 12],
        }); // levels-only snapshot
        evs.push(stats(1, 1500)); // POV
        for e in &evs {
            k.apply(e);
        }
        k
    }

    #[test]
    fn eval_matches_the_game_types() {
        let s = Stats {
            level: 80,
            attrs: [0, 100, 0, 0, 0, 50, 30, 1000, 0, 0, 0, 0],
        };
        let m = |ftype, base, c1, c2| Modifier {
            target: 0,
            ftype,
            base,
            c1,
            c2,
            mode: 0,
            prereq: PREREQ_READ,
        };
        assert_eq!(eval(&m(0, 1.0, 2.0, 9.0), 80, &s), Some(161.0));
        assert_eq!(eval(&m(1, 2.0, 0.25, 0.06), 80, &s), Some(82.0));
        assert_eq!(eval(&m(2, 0.0, 0.5, 0.0), 10, &s), Some(50.0));
        assert_eq!(eval(&m(6, 7.0, 3.0, 3.0), 80, &s), Some(7.0));
        assert_eq!(eval(&m(11, 10.0, 1.0, 50.0), 80, &s), Some(50.0), "clamped");
        assert_eq!(eval(&m(13, 0.0, 0.0, 2.0), 80, &s), Some(200.0));
        assert_eq!(eval(&m(12, 0.0, 0.0, 0.0), 80, &s), None);
    }

    #[test]
    fn damage_kind_comes_from_poison() {
        assert_eq!(known().damage_kind(), Some(DMG));
        assert_eq!(Known::default().damage_kind(), None);
    }

    #[test]
    fn partial_formula_is_not_used() {
        let mut k = Known::default();
        k.apply(&formula(BLEEDING, &[(DMG, 1, 2.0, 0.25, 0.06), (9, 6, 1.0, 0.0, 0.0)])[0]);
        assert_eq!(k.modifiers(BLEEDING), None);
    }

    #[test]
    fn ticks_sum_stacks_per_source_and_skip_the_pov() {
        let mut m = Model::new(known(), Some(1), GameMode::Wvw);
        let f = Fired {
            target: 50,
            time: 3000,
            stacks: vec![
                (10, BLEEDING, 80),
                (10, BLEEDING, 80),
                (11, BLEEDING, 80), // no attributes
                (12, BLEEDING, 80), // minion of 10
                (1, BLEEDING, 80),  // POV: self-check only
                (10, 999, 80),      // no formula
            ],
        };
        let ticks = m.tick(&f, |id| (id == 12).then_some(10), |_| vec![]);
        // 2 + 20 + 60 = 82 per stack.
        assert_eq!(
            ticks,
            vec![
                Tick { time: 3000, src: 10, dst: 50, buff: BLEEDING, damage: 164, stacks: 2 },
                Tick { time: 3000, src: 12, dst: 50, buff: BLEEDING, damage: 82, stacks: 1 },
            ]
        );
        assert_eq!(
            m.coverage,
            Coverage {
                modelled: 3,
                no_formula: 1,
                no_stats: 1
            }
        );
        // POV: 2 + 20 + 90 = 112.
        assert_eq!(m.check.sums.get(&BLEEDING), Some(&(112, 0)));
        assert_eq!(m.check.ticks.get(&BLEEDING), Some(&(1, 0)));
    }

    /// Bleeding as the game lists it: a trait variant, PvE/PvP/WvW variants and the base.
    /// Only the base and the WvW-context modifier with no prerequisite apply in WvW.
    #[test]
    fn only_modifiers_for_this_mode_with_no_prerequisite_apply() {
        let mut k = known();
        let mods: [(u8, u32, f32); 5] = [
            (PREREQ_READ | 1, 0, 9.0), // needs a trait
            (PREREQ_READ, 4, 100.0),   // PvE only
            (PREREQ_READ, 2, 200.0),   // PvP only
            (PREREQ_READ, 3, 0.5),     // WvW only
            (PREREQ_READ, 0, 2.0),     // base, any mode
        ];
        for (i, &(prereq, mode, base)) in mods.iter().enumerate() {
            k.apply(&Event::BuffFormula {
                buff: BLEEDING,
                index: i as u8,
                count: mods.len() as u8,
                category: 2,
                ftype: 1,
                target: DMG,
                base: base.to_bits(),
                c1: 0.25f32.to_bits(),
                c2: 0.0f32.to_bits(),
                mode,
                prereq,
            });
        }
        let f = Fired {
            target: 50,
            time: 1000,
            stacks: vec![(10, BLEEDING, 80)],
        };
        let mut m = Model::new(k.clone(), Some(1), GameMode::Wvw);
        // (2 + 20) + (0.5 + 20)
        assert_eq!(m.tick(&f, |_| None, |_| vec![])[0].damage, 43);
        let mut m = Model::new(k.clone(), Some(1), GameMode::Pve);
        assert_eq!(m.tick(&f, |_| None, |_| vec![])[0].damage, 142);
        // An older recording (prereq not read): no formula.
        k.apply(&Event::BuffFormula {
            buff: BLEEDING,
            index: 4,
            count: 5,
            category: 2,
            ftype: 1,
            target: DMG,
            base: 2.0f32.to_bits(),
            c1: 0.25f32.to_bits(),
            c2: 0.0f32.to_bits(),
            mode: 0,
            prereq: 0,
        });
        let mut m = Model::new(k, Some(1), GameMode::Wvw);
        assert!(m.tick(&f, |_| None, |_| vec![]).is_empty());
        assert_eq!(m.coverage.no_formula, 1);
    }

    /// Other players' attributes come from gear: base + itemstat points (WvW list in WvW) +
    /// gear skills and active buffs, a skill on both counting max(copies, instances).
    #[test]
    fn rebuilt_stats_from_gear_skills_and_buffs() {
        let mut k = Known::default();
        let flat = |buff: u32, kind: i32, v: f32| Event::BuffFormula {
            buff,
            index: 0,
            count: 1,
            category: 0,
            ftype: 6,
            target: kind,
            base: v.to_bits(),
            c1: 0,
            c2: 0,
            mode: 0,
            prereq: PREREQ_READ,
        };
        let mut gear = [0i16; 12];
        gear[CONDITION_DAMAGE] = 900;
        let mut alt = gear;
        alt[CONDITION_DAMAGE] = 1000;
        for e in [
            Event::GearAttrs { id: 20, alt: 0, attrs: gear },
            Event::GearAttrs { id: 20, alt: 1, attrs: alt },
            Event::GearSkill { id: 20, index: 0, count: 2, skill: 500, n: 6 },
            Event::GearSkill { id: 20, index: 1, count: 2, skill: 501, n: 1 },
            flat(500, 0x6, 5.0),  // +5 condition damage infusion, 6 of them
            flat(501, 0xe, 32.0), // +32 power infix
            // Might: +30 condition damage per stack (level-scaled kind 0x7).
            Event::BuffFormula {
                buff: 740,
                index: 0,
                count: 1,
                category: 0,
                ftype: 10,
                target: 0x7,
                base: 5.0f32.to_bits(),
                c1: 0.3125f32.to_bits(),
                c2: 0,
                mode: 0,
                prereq: PREREQ_READ,
            },
        ] {
            k.apply(&e);
        }
        // 8 active instances of the infusion skill (more than gear's 6), 2 Might stacks.
        let buffs = |id: u32| {
            assert_eq!(id, 20);
            let mut v = vec![(500, 80); 8];
            v.extend([(740, 80), (740, 80)]);
            v
        };
        let m = Model::new(k.clone(), Some(1), GameMode::Wvw);
        let s = m.rebuilt_stats(20, &buffs).unwrap();
        assert_eq!(s.attrs[CONDITION_DAMAGE], 1000 + 8 * 5 + 60);
        assert_eq!(s.attrs[POWER], 1000 + 32);
        let m = Model::new(k.clone(), Some(1), GameMode::Pve);
        assert_eq!(m.rebuilt_stats(20, &|_| vec![]).unwrap().attrs[CONDITION_DAMAGE], 900 + 30);
        // Incomplete skill list: nothing rebuilt.
        k.apply(&Event::GearSkill { id: 21, index: 1, count: 2, skill: 501, n: 1 });
        k.apply(&Event::GearAttrs { id: 21, alt: 0, attrs: gear });
        assert!(Model::new(k, None, GameMode::Pve).rebuilt_stats(21, &|_| vec![]).is_none());
    }

    #[test]
    fn health_check_closes_intervals_only_while_up() {
        let mut h = HealthCheck::default();
        h.strike(5, 999); // before any sample: not counted
        h.health(5, 10_000, 20_000); // 20 000
        h.strike(5, 3000);
        h.pov_tick(5, 500);
        h.model_tick(5, 1200);
        h.health(5, 7_500, 20_000); // 15 000: 5 000 lost
        h.strike(5, 4000);
        h.interrupt(5); // downed: that interval is dropped
        h.health(5, 10_000, 20_000); // revived: a new first sample
        h.strike(5, 100);
        h.health(5, 9_900, 20_000); // 200 lost
        assert_eq!(h.lost, 5200.0);
        assert_eq!((h.dropped, h.intervals), (5200.0, 2));
        assert_eq!(h.totals, Interval { strikes: 3100.0, pov_exact: 500.0, model: 1200.0 });
        assert_eq!(h.residual(), 1600.0);
        assert_eq!(h.targets.len(), 1);
    }

    #[test]
    fn mode_contexts_follow_the_game() {
        use GameMode::*;
        let all = [Pve, Pvp, Wvw];
        let fits = |c| all.iter().filter(|&&m| mode_applies(c, m)).copied().collect::<Vec<_>>();
        assert_eq!(fits(0), all);
        assert_eq!(fits(1), [Pve, Wvw]);
        assert_eq!(fits(2), [Pvp]);
        assert_eq!(fits(3), [Wvw]);
        assert_eq!(fits(4), [Pve]);
        assert!(fits(5).is_empty());
        assert_eq!(GameMode::from_map_type(Some(11)), Wvw);
        assert_eq!(GameMode::from_map_type(Some(6)), Pvp);
        assert_eq!(GameMode::from_map_type(Some(5)), Pve);
        assert_eq!(GameMode::from_map_type(None), Pve);
    }

    #[test]
    fn ticks_on_the_pov_are_not_estimated() {
        let mut m = Model::new(known(), Some(1), GameMode::Wvw);
        let f = Fired {
            target: 1,
            time: 1000,
            stacks: vec![(10, BLEEDING, 80)],
        };
        assert!(m.tick(&f, |_| None, |_| vec![]).is_empty());
    }

    #[test]
    fn self_check_verdict() {
        let mut c = SelfCheck::default();
        assert_eq!(c.verdict(), Verdict::NotRun);
        c.exact(BLEEDING, 500);
        c.unmodelled = 3;
        assert_eq!(c.verdict(), Verdict::Incomplete, "no formula yet");
        c.unmodelled = 0;
        c.sums.insert(BLEEDING, (1010, 0));
        c.exact(BLEEDING, 1000);
        assert_eq!(c.verdict(), Verdict::Pass);
        c.sums.insert(737, (900, 1000));
        assert_eq!(
            c.verdict(),
            Verdict::Fail {
                buff: 737,
                gap_permille: 100
            }
        );
    }
}
