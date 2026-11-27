//! Agent registry: identities, names, addresses and squad membership, built from events.

use std::collections::BTreeMap;

use axidps_protocol::v2::{AgentKind, AgentState, Event, NameField};

use crate::names;

/// First agent address handed out (arcdps's POV addr in the fixture is 0x7d0).
pub const FIRST_ADDR: u64 = 2000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentInfo {
    pub id: u32,
    pub kind: AgentKind,
    pub prof: u32,
    pub elite: u32,
    pub species: u32,
    pub team: Option<u32>,
    pub subgroup: Option<u8>,
    pub character: String,
    pub account: String,
    /// `(pct_x100, max)`.
    pub health: Option<(u16, u32)>,
    pub in_combat: bool,
    /// Latest up/down/dead state, if one was seen.
    pub state: Option<AgentState>,
    /// Latest breakbar `(state, pct f32 bits)` the poll read, if any.
    pub breakbar: Option<(u16, u32)>,
    /// Latest action speed (f32 bits), if a message set one since spawn.
    pub speed: Option<u32>,
    /// Latest polled `(targetable, visible, named)` flags, if any.
    pub flags: Option<(u8, u8, u8)>,
    /// Agent info `(value, buff_dmg)` (sc 89), if the poll sent it since spawn.
    pub info: Option<(u32, u32)>,
    /// Gadget model info `(file, propid, flag36)` (sc 87), if the poll sent it since spawn.
    pub model: Option<(u64, u64, bool)>,
    /// Gadget capture outline points (f32 bits; sc 83), as the poll last sent them; empty =
    /// none. Cleared on despawn.
    pub shape: Vec<(u32, u32)>,
    /// Attack targets: the parent gadget (sc 23; also the master). Cleared on despawn.
    pub attack_parent: Option<u32>,
    /// Players: the WvW rank def id (1..=105) of the first rank read this life; arcdps latches
    /// the title once, at the agent's first init.
    pub wvw_rank_id: Option<u8>,
    /// The agent's master (owner) id, from the poll's `AgentMaster`; cleared on despawn.
    pub master: Option<u32>,
    /// Players: the guild GUID (client byte order) from the poll's `AgentGuild`; `None` = none
    /// seen (written as zero, like arcdps for a player without a guild). Cleared on despawn.
    pub guild: Option<[u8; 16]>,
    /// Players: the account GUID (client byte order) from the poll's
    /// `AgentGuid`; arcdps's key for every player but the POV. Identity: kept after despawn,
    /// cleared when the id starts a new life (with the names).
    pub account_guid: Option<[u8; 16]>,
}

impl AgentInfo {
    fn placeholder(id: u32) -> Self {
        Self {
            id,
            kind: AgentKind::Unknown,
            prof: 0,
            elite: 0,
            species: 0,
            team: None,
            subgroup: None,
            character: String::new(),
            account: String::new(),
            health: None,
            in_combat: false,
            state: None,
            breakbar: None,
            speed: None,
            flags: None,
            info: None,
            model: None,
            shape: Vec::new(),
            attack_parent: None,
            wvw_rank_id: None,
            master: None,
            guild: None,
            account_guid: None,
        }
    }
}

/// Squad by its own fields: a subgroup, an account, and not on a team other than the POV's (the
/// DLL has given enemy players a subgroup and an account).
pub fn squad_fields(a: &AgentInfo, pov_team: Option<u32>) -> bool {
    let other_team = matches!((a.team, pov_team), (Some(t), Some(p)) if t != p);
    matches!(a.subgroup, Some(1..=15)) && !a.account.is_empty() && !other_team
}

#[derive(Debug, Clone)]
struct IdAddr {
    addr: u64,
    /// The account this addr is keyed by, if it is a squad player's addr.
    owner: Option<String>,
}

#[derive(Debug, Clone)]
struct Pending {
    buf: Vec<u8>,
    got: usize,
}

#[derive(Debug, Clone)]
pub struct Registry {
    agents: BTreeMap<u32, AgentInfo>,
    id_addr: BTreeMap<u32, IdAddr>,
    account_addr: BTreeMap<String, u64>,
    /// Earlier lives of players that now live under another instance id: their addr and
    /// identity, so a fight that saw both lives still writes the first one's rows. Not live;
    /// any new use of the id replaces it.
    former: BTreeMap<u32, (u64, AgentInfo)>,
    pending: BTreeMap<(u32, u8), Pending>,
    pov: Option<u32>,
    next_addr: u64,
    /// Ids removed since their last `AgentAdd`: the next add is a new life.
    removed: std::collections::BTreeSet<u32>,
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}

impl Registry {
    pub fn new() -> Self {
        Self {
            agents: BTreeMap::new(),
            id_addr: BTreeMap::new(),
            account_addr: BTreeMap::new(),
            former: BTreeMap::new(),
            pending: BTreeMap::new(),
            pov: None,
            next_addr: FIRST_ADDR,
            removed: Default::default(),
        }
    }

    /// The agent under `id`, or the earlier life of a player that moved to another id.
    pub fn agent(&self, id: u32) -> Option<&AgentInfo> {
        self.agents
            .get(&id)
            .or_else(|| self.former.get(&id).map(|(_, a)| a))
    }

    pub fn agents(&self) -> impl Iterator<Item = &AgentInfo> {
        self.agents.values()
    }

    /// Agents not despawned: what a fight starting now can see.
    pub fn live_agents(&self) -> impl Iterator<Item = &AgentInfo> {
        self.agents
            .values()
            .filter(|a| !self.removed.contains(&a.id))
    }

    pub fn pov(&self) -> Option<u32> {
        self.pov
    }

    pub fn addr(&self, id: u32) -> Option<u64> {
        self.id_addr
            .get(&id)
            .map(|e| e.addr)
            .or_else(|| self.former.get(&id).map(|(addr, _)| *addr))
    }

    /// Squad member: subgroup in `1..=15` with a non-empty account, or the POV.
    pub fn is_squad(&self, id: u32) -> bool {
        if self.pov == Some(id) {
            return true;
        }
        let pov_team = self
            .pov
            .and_then(|p| self.agents.get(&p))
            .and_then(|a| a.team);
        self.agent(id).is_some_and(|a| squad_fields(a, pov_team))
    }

    /// Forget everything keyed by instance id (a map change: the game reuses instance ids per
    /// map). Agents, their combat flags, id → addr links, former lives, partial names and the
    /// POV go; `account_addr` stays, so a squad player gets the same addr when re-added on the
    /// new map. Squad id → addr links go too: the old id may now belong to a different agent.
    pub fn reset_instances(&mut self) {
        self.agents.clear();
        self.id_addr.clear();
        self.former.clear();
        self.pending.clear();
        self.pov = None;
    }

    pub fn apply(&mut self, _time_ms: u64, ev: &Event) {
        match *ev {
            Event::AgentAdd {
                id,
                kind,
                prof,
                elite,
                species,
                team,
                subgroup,
                wvw_rank,
                wvw_rank_def,
            } => {
                // The game reuses ids: after a map change (no `Map` event reaches core yet) or a
                // despawn, an id can belong to a different agent. A different kind, profession or
                // species is a different agent: its old identity and addr must not carry over.
                let other = self.agents.get(&id).is_some_and(|a| {
                    a.kind != AgentKind::Unknown
                        && (a.kind != kind || a.prof != prof || a.species != species)
                });
                let was_removed = self.removed.remove(&id);
                let new_life = was_removed || other || !self.agents.contains_key(&id);
                if other {
                    self.id_addr.remove(&id);
                }
                if was_removed || other {
                    // Names come again after the add; old ones may belong to someone else.
                    self.pending.retain(|(pid, _), _| *pid != id);
                    let a = self.ensure(id);
                    a.character.clear();
                    a.account.clear();
                    a.wvw_rank_id = None;
                    a.account_guid = None;
                }
                let a = self.ensure(id);
                a.kind = kind;
                a.prof = prof;
                a.elite = elite;
                a.species = species;
                a.team = Some(team);
                a.subgroup = (subgroup != 0).then_some(subgroup);
                // A (re)spawn starts up; an old Down/Dead belongs to a previous life. A repeated
                // add for a live agent is an identity update (the poll) and keeps its state.
                if new_life {
                    a.state = None;
                }
                if a.wvw_rank_id.is_none() {
                    a.wvw_rank_id = names::wvw_rank_id(wvw_rank, wvw_rank_def);
                }
                self.assign_addr(id);
            }
            Event::AgentName {
                id,
                field,
                offset,
                total,
                len,
                bytes,
            } => {
                self.ensure(id);
                self.name_chunk(id, field, offset, total, len, &bytes);
                self.assign_addr(id);
            }
            Event::AgentRemove { id } => {
                // Identity is kept so a log can still describe agents that despawned mid-fight.
                // A despawned agent's up/down/dead state no longer applies: a later fight must
                // not open with it marked down or dead.
                self.removed.insert(id);
                if let Some(a) = self.agents.get_mut(&id) {
                    a.in_combat = false;
                    a.state = None;
                    a.breakbar = None;
                    a.speed = None;
                    a.flags = None;
                    a.info = None;
                    a.model = None;
                    a.shape.clear();
                    a.attack_parent = None;
                    a.master = None;
                    a.guild = None;
                }
            }
            Event::Strike { src, dst, .. } => {
                self.ensure(src);
                self.assign_addr(src);
                self.ensure(dst);
                self.assign_addr(dst);
            }
            Event::RelayBuffDamage { src, dst, .. } => {
                for id in [src, dst] {
                    if id != 0 {
                        self.ensure(id);
                        self.assign_addr(id);
                    }
                }
            }
            Event::BuffDamage { src, dst, .. }
            | Event::Heal { src, dst, .. }
            | Event::RelayHeal { src, dst, .. } => {
                if src != 0 {
                    self.ensure(src);
                    self.assign_addr(src);
                }
                self.ensure(dst);
                self.assign_addr(dst);
            }
            Event::Combat { id, in_combat } => {
                self.ensure(id).in_combat = in_combat;
                self.assign_addr(id);
            }
            Event::Team { id, team } => {
                self.ensure(id).team = Some(team);
                self.assign_addr(id);
            }
            Event::Health { id, pct_x100, max } => {
                self.ensure(id).health = Some((pct_x100, max));
                self.assign_addr(id);
            }
            Event::Pov { id } => {
                self.ensure(id);
                self.pov = Some(id);
                self.assign_addr(id);
            }
            Event::State { id, state } => {
                self.ensure(id).state = Some(state);
                self.assign_addr(id);
            }
            Event::BuffApply { dst, src, .. } => {
                self.ensure(dst);
                self.assign_addr(dst);
                if src != 0 {
                    self.ensure(src);
                    self.assign_addr(src);
                }
            }
            Event::BuffRemove { dst, remover, .. } | Event::BuffRemoveAll { dst, remover, .. } => {
                self.ensure(dst);
                self.assign_addr(dst);
                if remover != 0 {
                    self.ensure(remover);
                    self.assign_addr(remover);
                }
            }
            Event::CastStart { agent, target, .. } => {
                self.ensure(agent);
                self.assign_addr(agent);
                if target != 0 {
                    self.ensure(target);
                    self.assign_addr(target);
                }
            }
            Event::Breakbar {
                id,
                state,
                pct_bits,
            } => {
                self.ensure(id).breakbar = Some((state, pct_bits));
                self.assign_addr(id);
            }
            Event::AgentFlags {
                id,
                targetable,
                visible,
                named,
            } => {
                self.ensure(id).flags = Some((targetable, visible, named));
                self.assign_addr(id);
            }
            Event::AgentInfo {
                id,
                value,
                buff_dmg,
            } => {
                self.ensure(id).info = Some((value, buff_dmg));
                self.assign_addr(id);
            }
            Event::GadgetModel {
                id,
                file,
                propid,
                flag36,
            } => {
                self.ensure(id).model = Some((file, propid, flag36));
                self.assign_addr(id);
            }
            Event::GadgetShape {
                id,
                index,
                count,
                x,
                y,
            } => {
                let shape = &mut self.ensure(id).shape;
                if index == 0 {
                    shape.clear();
                }
                if count > 0 && usize::from(index) == shape.len() {
                    shape.push((x, y));
                }
                self.assign_addr(id);
            }
            Event::ActionSpeed { id, speed_bits } => {
                self.ensure(id).speed = Some(speed_bits);
                self.assign_addr(id);
            }
            Event::AgentMaster { id, master } => {
                self.ensure(id).master = (master != 0).then_some(master);
                self.assign_addr(id);
            }
            Event::AttackTarget { id, parent } => {
                let a = self.ensure(id);
                a.master = Some(parent);
                a.attack_parent = Some(parent);
                self.ensure(parent);
                self.assign_addr(id);
                self.assign_addr(parent);
            }
            Event::AgentGuild { id, guid } => {
                self.ensure(id).guild = (guid != [0; 16]).then_some(guid);
                self.assign_addr(id);
            }
            Event::AgentGuid { id, guid } => {
                if guid != [0; 16] {
                    self.ensure(id).account_guid = Some(guid);
                    self.assign_addr(id);
                }
            }
            Event::CastEnd { agent, .. }
            | Event::WeaponSwap { id: agent, .. }
            | Event::Glider { id: agent, .. }
            | Event::GadgetCapture { id: agent, .. }
            | Event::Jump { id: agent, .. }
            | Event::FlyTo { id: agent, .. }
            | Event::Teleport { id: agent, .. }
            | Event::GadgetAnimation { id: agent, .. }
            | Event::Transform { id: agent, .. } => {
                self.ensure(agent);
                self.assign_addr(agent);
            }
            // Effect and missile rows name these agents (the missile remove's `other` is only
            // compared, never written).
            Event::EffectAgent { host, src, .. } => self.ensure_named(&[host, src]),
            Event::EffectAgentRemove { host, .. } => self.ensure_named(&[host]),
            Event::EffectGround { owner, .. }
            | Event::MissileCreate { owner, .. }
            | Event::MissileRemove { owner, .. }
            | Event::MissileEffect { owner, .. } => self.ensure_named(&[owner]),
            Event::MissileLaunch { owner, target, .. } => self.ensure_named(&[owner, target]),
            Event::Status { .. }
            | Event::Map { .. }
            | Event::LogControl { .. }
            | Event::Motion { .. }
            | Event::Barrier { .. }
            | Event::Tag { .. }
            | Event::BuffStack { .. }
            | Event::EffectGroundRemove { .. }
            | Event::ServerTick { .. }
            | Event::CoreStats { .. }
            | Event::GearAttrs { .. }
            | Event::GearSkill { .. }
            | Event::BuffFormula { .. }
            | Event::SquadMarker { .. }
            | Event::WvwObjective { .. }
            | Event::Reward { .. }
            | Event::WvwTeam { .. }
            | Event::GroupKey { .. }
            | Event::RelayCombat { .. }
            | Event::WvwWorld { .. } => {}
        }
    }

    fn ensure(&mut self, id: u32) -> &mut AgentInfo {
        self.agents
            .entry(id)
            .or_insert_with(|| AgentInfo::placeholder(id))
    }

    /// Make sure each non-zero id has an entry and an addr.
    fn ensure_named(&mut self, ids: &[u32]) {
        for &id in ids.iter().filter(|&&id| id != 0) {
            self.ensure(id);
            self.assign_addr(id);
        }
    }

    fn name_chunk(
        &mut self,
        id: u32,
        field: NameField,
        offset: u8,
        total: u8,
        len: u8,
        bytes: &[u8],
    ) {
        let (offset, total, len) = (offset as usize, total as usize, len as usize);
        let end = offset + len;
        let (Some(chunk), true) = (bytes.get(..len), end <= total && total > 0) else {
            return;
        };
        let key = (id, field as u8);
        if offset == 0 {
            self.pending.remove(&key);
        }
        let p = self.pending.entry(key).or_insert_with(|| Pending {
            buf: vec![0; total],
            got: 0,
        });
        if p.buf.len() != total {
            *p = Pending {
                buf: vec![0; total],
                got: 0,
            };
        }
        if let Some(dst) = p.buf.get_mut(offset..end) {
            dst.copy_from_slice(chunk);
            p.got += len;
        }
        if p.got < total {
            return;
        }
        let Some(p) = self.pending.remove(&key) else {
            return;
        };
        let used = p.buf.iter().position(|&b| b == 0).unwrap_or(p.buf.len());
        let name = String::from_utf8_lossy(p.buf.get(..used).unwrap_or(&[])).into_owned();
        let a = self.ensure(id);
        match field {
            NameField::Character => a.character = name,
            NameField::Account => a.account = name,
        }
    }

    fn alloc(&mut self) -> u64 {
        let a = self.next_addr;
        self.next_addr += 1;
        a
    }

    /// Give `id` an addr. Squad players are keyed by account (same addr across instance ids
    /// and fights); every other agent is keyed by its instance id. Other named players' lives
    /// are merged per fight when the log is built (`assemble::Lives`).
    fn assign_addr(&mut self, id: u32) {
        let account = match self.agents.get(&id) {
            Some(a) if self.is_squad(id) && !a.account.is_empty() => Some(a.account.clone()),
            _ => None,
        };
        let existing = self.id_addr.get(&id).cloned();
        let Some(account) = account else {
            // No longer a squad player (left the squad, or the id now belongs to someone else):
            // the account's addr is not this agent's.
            if existing.as_ref().is_none_or(|e| e.owner.is_some()) {
                let addr = self.alloc();
                self.id_addr.insert(id, IdAddr { addr, owner: None });
            }
            return;
        };
        if existing.as_ref().and_then(|e| e.owner.as_ref()) == Some(&account) {
            return;
        }
        let addr = if let Some(&addr) = self.account_addr.get(&account) {
            addr
        } else if let Some(e) = existing.filter(|e| e.owner.is_none()) {
            // First sight as a non-squad id, later gained an account: keep its addr.
            e.addr
        } else {
            self.alloc()
        };
        // The account now lives under `id`; any other id still holding it is an earlier life
        // (the player left range, changed map or reloaded).
        let stale: Vec<u32> = self
            .id_addr
            .iter()
            .filter(|(i, e)| **i != id && e.owner.as_ref() == Some(&account))
            .map(|(i, _)| *i)
            .collect();
        for i in stale {
            self.retire(i);
        }
        self.account_addr.insert(account.clone(), addr);
        self.id_addr.insert(
            id,
            IdAddr {
                addr,
                owner: Some(account),
            },
        );
    }

    /// `i` is an earlier life of a player now under another id: keep its addr and identity for
    /// the rows already written under it, but out of the live agents and the id → addr links,
    /// so it cannot describe a reused id.
    fn retire(&mut self, i: u32) {
        let (e, a) = (self.id_addr.remove(&i), self.agents.remove(&i));
        if let (Some(e), Some(a)) = (e, a) {
            self.former.insert(i, (e.addr, a));
        }
        self.removed.remove(&i);
        self.pending.retain(|(pid, _), _| *pid != i);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use axidps_protocol::v2::NAME_CHUNK_MAX;

    pub(crate) fn add(id: u32, subgroup: u8) -> Event {
        Event::AgentAdd {
            id,
            kind: AgentKind::Player,
            prof: 1,
            elite: 0,
            species: 0,
            team: 1,
            subgroup,
            wvw_rank: 0,
            wvw_rank_def: 0,
        }
    }

    /// A player (prof 1, team 1) with a WvW rank number and def index.
    pub(crate) fn ranked(id: u32, subgroup: u8, wvw_rank: u32, wvw_rank_def: u8) -> Event {
        match add(id, subgroup) {
            Event::AgentAdd {
                kind,
                prof,
                elite,
                species,
                team,
                ..
            } => Event::AgentAdd {
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
            _ => unreachable!(),
        }
    }

    /// arcdps latches the title once, at the agent's first init: the first title read stands
    /// for the agent's life (a rank-up or a lost read does not change it); a new life reads again.
    #[test]
    fn wvw_rank_title_is_latched_per_life() {
        let mut r = Registry::new();
        let id = |r: &Registry, id| r.agent(id).unwrap().wvw_rank_id;
        r.apply(0, &ranked(1, 0, 0, 0));
        assert_eq!(id(&r, 1), None, "no rank yet");
        r.apply(0, &ranked(1, 0, 6000, 0));
        assert_eq!(id(&r, 1), Some(88));
        r.apply(0, &ranked(1, 0, 9945, 0));
        assert_eq!(id(&r, 1), Some(88), "rank-up keeps the first title");
        r.apply(0, &ranked(1, 0, 0, 0));
        assert_eq!(id(&r, 1), Some(88), "a lost read keeps it");
        r.apply(0, &Event::AgentRemove { id: 1 });
        r.apply(0, &ranked(1, 0, 9945, 0));
        assert_eq!(id(&r, 1), Some(105), "a new life reads again");
        r.apply(0, &ranked(2, 0, 0, 12));
        assert_eq!(id(&r, 2), Some(12), "def index when no rank");
    }

    pub(crate) fn name_events(id: u32, field: NameField, s: &str) -> Vec<Event> {
        let b = s.as_bytes();
        let total = b.len() as u8;
        b.chunks(NAME_CHUNK_MAX)
            .enumerate()
            .map(|(i, c)| {
                let mut bytes = [0u8; NAME_CHUNK_MAX];
                bytes[..c.len()].copy_from_slice(c);
                Event::AgentName {
                    id,
                    field,
                    offset: (i * NAME_CHUNK_MAX) as u8,
                    total,
                    len: c.len() as u8,
                    bytes,
                }
            })
            .collect()
    }

    fn feed(r: &mut Registry, evs: &[Event]) {
        for e in evs {
            r.apply(0, e);
        }
    }

    #[test]
    fn registry_name_chunks_multi_part_and_unicode() {
        // 'é' is 2 bytes and sits across the 32-byte chunk boundary: 31 ASCII + é + 12 ASCII = 45.
        let name = format!("{}é{}", "a".repeat(31), "b".repeat(12));
        assert_eq!(name.len(), 45);
        let mut r = Registry::new();
        r.apply(0, &add(7, 0));
        let evs = name_events(7, NameField::Character, &name);
        assert_eq!(evs.len(), 2);
        r.apply(0, &evs[0]);
        assert_eq!(r.agent(7).unwrap().character, "");
        r.apply(0, &evs[1]);
        assert_eq!(r.agent(7).unwrap().character, name);
    }

    #[test]
    fn registry_invalid_utf8_is_lossy() {
        let mut r = Registry::new();
        let mut bytes = [0u8; NAME_CHUNK_MAX];
        bytes[..3].copy_from_slice(&[b'a', 0xFF, b'b']);
        r.apply(
            0,
            &Event::AgentName {
                id: 1,
                field: NameField::Character,
                offset: 0,
                total: 3,
                len: 3,
                bytes,
            },
        );
        assert_eq!(r.agent(1).unwrap().character, "a\u{FFFD}b");
    }

    #[test]
    fn registry_placeholders_and_stable_addrs() {
        let mut r = Registry::new();
        r.apply(
            0,
            &Event::Strike {
                src: 5,
                dst: 6,
                skill: 1,
                value: 1,
                overstack: 0,
                result: 0,
            },
        );
        assert_eq!(r.agent(5).unwrap().kind, AgentKind::Unknown);
        assert_eq!((r.addr(5), r.addr(6)), (Some(2000), Some(2001)));
        r.apply(
            0,
            &Event::Combat {
                id: 9,
                in_combat: true,
            },
        );
        r.apply(0, &Event::Team { id: 10, team: 3 });
        assert_eq!((r.addr(9), r.addr(10)), (Some(2002), Some(2003)));
        r.apply(
            0,
            &Event::Combat {
                id: 5,
                in_combat: true,
            },
        );
        assert_eq!(r.addr(5), Some(2000));
        assert_eq!(r.addr(77), None);
    }

    #[test]
    fn registry_squad_by_account_instance_reuse() {
        let mut r = Registry::new();
        // Player A under id 1: the addr is first given as a non-squad id, then adopted.
        feed(&mut r, &[add(1, 1)]);
        let first = r.addr(1).unwrap();
        assert_eq!(first, 2000);
        assert!(!r.is_squad(1));
        feed(&mut r, &name_events(1, NameField::Account, ":Alpha.1234"));
        assert!(r.is_squad(1));
        assert_eq!(r.addr(1), Some(first));
        // Same account under a new instance id: same addr.
        feed(&mut r, &[add(2, 2)]);
        feed(&mut r, &name_events(2, NameField::Account, ":Alpha.1234"));
        assert_eq!(r.addr(2), Some(first));
        // A different account reusing the old instance id 1: new addr.
        feed(&mut r, &name_events(1, NameField::Account, ":Beta.5678"));
        let b = r.addr(1).unwrap();
        assert_ne!(b, first);
        assert!(b >= FIRST_ADDR);
        // Non-squad agents are keyed by instance id.
        feed(&mut r, &[add(3, 0), add(4, 0)]);
        assert_ne!(r.addr(3), r.addr(4));
        // Squad rules: subgroup 0 or empty account is not squad; the POV always is.
        assert!(!r.is_squad(3));
        r.apply(0, &Event::Pov { id: 3 });
        assert!(r.is_squad(3));
    }

    #[test]
    fn reset_instances_keeps_squad_addrs_by_account() {
        let mut r = Registry::new();
        feed(&mut r, &[add(1, 1)]);
        feed(&mut r, &name_events(1, NameField::Account, ":Alpha.1234"));
        feed(&mut r, &[add(9, 0)]);
        feed(&mut r, &name_events(9, NameField::Character, "Some"));
        r.apply(
            0,
            &Event::Combat {
                id: 9,
                in_combat: true,
            },
        );
        let squad_addr = r.addr(1).unwrap();
        r.reset_instances();
        assert_eq!(r.agents().count(), 0);
        assert_eq!((r.addr(1), r.addr(9), r.pov()), (None, None, None));
        // Instance 1 is now an unrelated non-squad agent: a fresh addr.
        feed(&mut r, &[add(1, 0)]);
        assert_ne!(r.addr(1), Some(squad_addr));
        // The squad player comes back under a new id: the same addr as before.
        feed(&mut r, &[add(4, 2)]);
        feed(&mut r, &name_events(4, NameField::Account, ":Alpha.1234"));
        assert_eq!(r.addr(4), Some(squad_addr));
    }

    #[test]
    fn remove_clears_life_state() {
        let mut r = Registry::new();
        r.apply(0, &add(5, 0));
        r.apply(
            0,
            &Event::State {
                id: 5,
                state: axidps_protocol::v2::AgentState::Dead,
            },
        );
        r.apply(0, &Event::AgentRemove { id: 5 });
        assert_eq!(r.agent(5).unwrap().state, None);
    }

    #[test]
    fn identity_update_keeps_life_state() {
        // The poll re-sends AgentAdd when a field changes (e.g. subgroup); that is not a respawn.
        let mut r = Registry::new();
        r.apply(0, &add(5, 0));
        r.apply(
            0,
            &Event::State {
                id: 5,
                state: axidps_protocol::v2::AgentState::Down,
            },
        );
        r.apply(0, &add(5, 2));
        assert_eq!(
            r.agent(5).unwrap().state,
            Some(axidps_protocol::v2::AgentState::Down)
        );
        assert_eq!(r.agent(5).unwrap().subgroup, Some(2));
    }

    #[test]
    fn account_guid_is_identity_kept_after_despawn_until_a_new_life() {
        let mut r = Registry::new();
        let g = [0xa7u8; 16];
        r.apply(0, &add(5, 1));
        r.apply(0, &Event::AgentGuid { id: 5, guid: g });
        assert_eq!(r.agent(5).unwrap().account_guid, Some(g));
        r.apply(
            0,
            &Event::AgentGuid {
                id: 5,
                guid: [0; 16],
            },
        );
        assert_eq!(r.agent(5).unwrap().account_guid, Some(g), "zero is unread");
        r.apply(0, &add(5, 1));
        assert_eq!(
            r.agent(5).unwrap().account_guid,
            Some(g),
            "identity update keeps it"
        );
        r.apply(0, &Event::AgentRemove { id: 5 });
        assert_eq!(
            r.agent(5).unwrap().account_guid,
            Some(g),
            "despawn keeps it"
        );
        r.apply(0, &add(5, 1));
        assert_eq!(
            r.agent(5).unwrap().account_guid,
            None,
            "a new life starts unknown"
        );
    }

    #[test]
    fn guild_is_kept_until_cleared_or_despawn() {
        let mut r = Registry::new();
        let g = [7u8; 16];
        r.apply(0, &add(5, 1));
        assert_eq!(r.agent(5).unwrap().guild, None);
        r.apply(0, &Event::AgentGuild { id: 5, guid: g });
        assert_eq!(r.agent(5).unwrap().guild, Some(g));
        r.apply(0, &add(5, 1));
        assert_eq!(
            r.agent(5).unwrap().guild,
            Some(g),
            "identity update keeps it"
        );
        r.apply(
            0,
            &Event::AgentGuild {
                id: 5,
                guid: [0; 16],
            },
        );
        assert_eq!(r.agent(5).unwrap().guild, None, "zero = no guild");
        r.apply(0, &Event::AgentGuild { id: 5, guid: g });
        r.apply(0, &Event::AgentRemove { id: 5 });
        assert_eq!(r.agent(5).unwrap().guild, None, "despawn clears it");
    }

    #[test]
    fn master_is_kept_until_cleared_or_despawn() {
        let mut r = Registry::new();
        r.apply(0, &add(5, 0));
        assert_eq!(r.agent(5).unwrap().master, None);
        r.apply(0, &Event::AgentMaster { id: 5, master: 9 });
        assert_eq!(r.agent(5).unwrap().master, Some(9));
        r.apply(0, &Event::AgentMaster { id: 5, master: 0 });
        assert_eq!(r.agent(5).unwrap().master, None, "0 = no master");
        r.apply(0, &Event::AgentMaster { id: 5, master: 9 });
        r.apply(0, &Event::AgentRemove { id: 5 });
        assert_eq!(r.agent(5).unwrap().master, None, "despawn clears it");
        assert!(r.addr(5).is_some());
    }

    #[test]
    fn add_after_remove_starts_up() {
        let mut r = Registry::new();
        r.apply(0, &add(5, 0));
        r.apply(0, &Event::AgentRemove { id: 5 });
        r.apply(
            0,
            &Event::State {
                id: 5,
                state: axidps_protocol::v2::AgentState::Dead,
            },
        );
        r.apply(0, &add(5, 0));
        assert_eq!(r.agent(5).unwrap().state, None);
    }

    fn squad_player(r: &mut Registry, id: u32, account: &str) -> u64 {
        feed(r, &[add(id, 1)]);
        feed(r, &name_events(id, NameField::Account, account));
        feed(r, &name_events(id, NameField::Character, "Sentinel"));
        r.addr(id).unwrap()
    }

    #[test]
    fn kind_change_on_add_is_a_new_agent() {
        // Live 2026-11-24: id 168 was the POV, then an NPC on the next map with no remove between.
        let mut r = Registry::new();
        let squad_addr = squad_player(&mut r, 168, ":harasho.4281");
        r.apply(
            0,
            &Event::AgentAdd {
                id: 168,
                kind: AgentKind::Npc,
                prof: 19553,
                elite: 0,
                species: 19553,
                team: 1,
                subgroup: 0,
                wvw_rank: 0,
                wvw_rank_def: 0,
            },
        );
        let a = r.agent(168).unwrap();
        assert_eq!((a.character.as_str(), a.account.as_str()), ("", ""));
        assert!(!r.is_squad(168));
        assert_ne!(r.addr(168), Some(squad_addr));
    }

    #[test]
    fn profession_change_on_add_is_a_new_agent() {
        let mut r = Registry::new();
        let squad_addr = squad_player(&mut r, 7, ":Alpha.1234");
        r.apply(
            0,
            &Event::AgentAdd {
                id: 7,
                kind: AgentKind::Player,
                prof: 8,
                elite: 0,
                species: 0,
                team: 1,
                subgroup: 0,
                wvw_rank: 0,
                wvw_rank_def: 0,
            },
        );
        assert_eq!(r.agent(7).unwrap().account, "");
        assert_ne!(r.addr(7), Some(squad_addr));
    }

    #[test]
    fn re_add_after_remove_drops_old_names() {
        let mut r = Registry::new();
        squad_player(&mut r, 7, ":Alpha.1234");
        r.apply(0, &Event::AgentRemove { id: 7 });
        r.apply(0, &add(7, 0));
        let a = r.agent(7).unwrap();
        assert_eq!((a.character.as_str(), a.account.as_str()), ("", ""));
    }

    #[test]
    fn account_moving_to_a_new_id_retires_the_old_id() {
        // Live 2026-11-24: Foxy was id 496 on one map and 5641 on the next; 496 kept her name.
        let mut r = Registry::new();
        let addr = squad_player(&mut r, 496, ":Athena.4562");
        squad_player(&mut r, 5641, ":Athena.4562");
        assert_eq!(r.addr(5641), Some(addr));
        // The earlier life still describes its rows, but is not live.
        assert_eq!(r.addr(496), Some(addr));
        assert_eq!(r.agent(496).unwrap().account, ":Athena.4562");
        assert!(r.live_agents().all(|a| a.id != 496));
        assert!(!r.agents().any(|a| a.id == 496));
        // A reuse of 496 is someone else: no old names, a fresh addr.
        r.apply(0, &add(496, 0));
        let a = r.agent(496).unwrap();
        assert_eq!((a.character.as_str(), a.account.as_str()), ("", ""));
        assert_ne!(r.addr(496), Some(addr));
        // A map change forgets earlier lives.
        squad_player(&mut r, 7, ":Athena.4562");
        r.reset_instances();
        assert_eq!(r.addr(5641), None);
    }

    #[test]
    fn leaving_the_squad_gives_up_the_account_addr() {
        let mut r = Registry::new();
        let addr = squad_player(&mut r, 3, ":Alpha.1234");
        r.apply(0, &add(3, 0));
        r.apply(
            0,
            &Event::Strike {
                src: 3,
                dst: 4,
                skill: 1,
                value: 1,
                overstack: 0,
                result: 0,
            },
        );
        assert!(!r.is_squad(3));
        assert_ne!(r.addr(3), Some(addr));
    }

    #[test]
    fn a_player_on_another_team_is_never_squad() {
        // Live 2026-11-24: the DLL gave subgroup 1 and an account to some enemy players.
        let mut r = Registry::new();
        squad_player(&mut r, 1, ":Me.0001");
        r.apply(0, &Event::Pov { id: 1 });
        r.apply(
            0,
            &Event::AgentAdd {
                id: 2,
                kind: AgentKind::Player,
                prof: 1,
                elite: 0,
                species: 0,
                team: 9,
                subgroup: 1,
                wvw_rank: 0,
                wvw_rank_def: 0,
            },
        );
        feed(&mut r, &name_events(2, NameField::Account, ":diviman.8306"));
        assert!(!r.is_squad(2));
    }
}
