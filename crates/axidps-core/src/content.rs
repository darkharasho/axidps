//! Per-skill content rows arcdps writes at log save: the skill's GUID (46, content type 2), buff
//! info (30), buff formulas (31), skill info (32), skill timing (33) and timing steps (91). They are game content keyed by skill id,
//! so they come from a table generated from the owner's arcdps logs (the newest log holding an
//! id wins).
//!
//! arcdps writes timing (33/91) only for skills cast since it loaded; axidps writes every row it
//! knows for the log's skills, a superset.

use std::io::Read;
use std::sync::OnceLock;

use axidps_evtc::Event;
use flate2::read::GzDecoder;

/// Gzipped `u32 skill | u16 n | n x 64-byte rows`, sorted by skill.
static PACKED: &[u8] = include_bytes!("../data/content_rows.bin.gz");
/// Gzipped idtoguid (46) rows of the non-skill content types: `u8 type | u32 id | 64-byte row`,
/// sorted by `(type, id)`.
static GUID_PACKED: &[u8] = include_bytes!("../data/content_guids.bin.gz");

/// idtoguid content types (the row's `overstack`).
pub const GUID_EFFECT: u8 = 0;
pub const GUID_MARKER: u8 = 1;
pub const GUID_SPECIES: u8 = 3;
const GUID_RECORD: usize = 5 + ROW;

const ROW: usize = 64;

fn unpack(packed: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    if GzDecoder::new(packed).read_to_end(&mut out).is_err() {
        out.clear();
    }
    out
}

/// The unpacked per-skill table (empty if it does not decompress).
fn data() -> &'static [u8] {
    static DATA: OnceLock<Vec<u8>> = OnceLock::new();
    DATA.get_or_init(|| unpack(PACKED))
}

/// The unpacked idtoguid table, whole records only.
fn guid_data() -> &'static [u8] {
    static DATA: OnceLock<Vec<u8>> = OnceLock::new();
    DATA.get_or_init(|| {
        let mut d = unpack(GUID_PACKED);
        d.truncate(d.len() - d.len() % GUID_RECORD);
        d
    })
}

/// The idtoguid row arcdps writes for content `id` of `ctype`, if the table has one.
pub fn guid(ctype: u8, id: u32) -> Option<Event> {
    let d = guid_data();
    let key = |r: &[u8]| (r[0], u32::from_le_bytes([r[1], r[2], r[3], r[4]]));
    let n = d.len() / GUID_RECORD;
    let (mut lo, mut hi) = (0, n);
    while lo < hi {
        let mid = (lo + hi) / 2;
        let r = &d[mid * GUID_RECORD..(mid + 1) * GUID_RECORD];
        match key(r).cmp(&(ctype, id)) {
            std::cmp::Ordering::Less => lo = mid + 1,
            std::cmp::Ordering::Greater => hi = mid,
            std::cmp::Ordering::Equal => return r[5..].try_into().ok().map(Event::from_bytes),
        }
    }
    None
}

/// `(skill, offset of its first row, row count)`, sorted by skill.
fn index() -> &'static [(u32, usize, usize)] {
    static INDEX: OnceLock<Vec<(u32, usize, usize)>> = OnceLock::new();
    INDEX.get_or_init(|| {
        let data = data();
        let mut out = Vec::new();
        let mut o = 0;
        while let Some(head) = data.get(o..o + 6) {
            let skill = u32::from_le_bytes([head[0], head[1], head[2], head[3]]);
            let n = usize::from(u16::from_le_bytes([head[4], head[5]]));
            let end = o + 6 + n * ROW;
            if end > data.len() {
                break;
            }
            out.push((skill, o + 6, n));
            o = end;
        }
        out
    })
}

/// Every skill the table has rows for.
pub fn skills() -> impl Iterator<Item = u32> {
    index().iter().map(|&(s, _, _)| s)
}

/// The content rows arcdps writes for `skill` (none if the table has no entry).
pub fn rows(skill: u32) -> Vec<Event> {
    let idx = index();
    let Ok(i) = idx.binary_search_by_key(&skill, |&(s, _, _)| s) else {
        return Vec::new();
    };
    let (_, at, n) = idx[i];
    data()[at..at + n * ROW]
        .chunks_exact(ROW)
        .filter_map(|c| c.try_into().ok().map(Event::from_bytes))
        .collect()
}

/// arcdps's two buff-info flags behind the zero condition-tick results:
/// `invulnerable` (the sc 30 row's `is_flanking`: a target holding such a buff takes result 6)
/// and `invert` (the row's `is_shields`: result 13). Both false for buffs the table lacks.
pub fn buff_flags(buff: u32) -> (bool, bool) {
    static FLAGS: OnceLock<std::collections::BTreeMap<u32, (bool, bool)>> = OnceLock::new();
    FLAGS
        .get_or_init(|| {
            skills()
                .flat_map(rows)
                .filter(|e| e.is_statechange == 30 && (e.is_flanking != 0 || e.is_shields != 0))
                .map(|e| (e.skillid, (e.is_flanking != 0, e.is_shields != 0)))
                .collect()
        })
        .get(&buff)
        .copied()
        .unwrap_or((false, false))
}

/// A skill's animation timing as arcdps reduces it, variant 0, from the table's 32/33/91 rows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Timing {
    /// The activation point `t_a`, ms (40 ms steps; 0 = none).
    pub activation_ms: i64,
    /// The full animation, ms (40 ms steps; 0 = none).
    pub full_ms: i64,
    /// `t_b` (from the skill info), seconds.
    pub t_b: f32,
    /// Sub-def flag bit 16: action speed does not apply.
    pub speed_immune: bool,
}

impl Timing {
    /// arcdps's kind-1 cast start `value`: `min(t_a, t_b)` ms, `t_b` only when non-zero.
    pub fn start_value(&self) -> i32 {
        let a = self.activation_ms as f64 / 1000.0;
        let t_b = f64::from(self.t_b);
        let v = if a != 0.0 && t_b != 0.0 && t_b <= a {
            t_b
        } else {
            a
        };
        (v * 1000.0) as i32
    }
}

/// `q(x)`: the next 40 ms step above `x`, even on an exact multiple.
fn q40(x: i64) -> i64 {
    x.div_euclid(40) * 40 + 40
}

/// The skill's timing, if the table has its info (32) and timing (33) rows.
pub fn timing(skill: u32) -> Option<Timing> {
    let rows = rows(skill);
    let info = rows.iter().find(|e| e.is_statechange == 32)?;
    let mut acts: Vec<(i32, u64, u32)> = rows
        .iter()
        .filter(|e| e.is_statechange == 33 && e.iff == 0)
        .map(|e| (e.value, e.src_agent, e.dst_agent as u32))
        .collect();
    if acts.is_empty() {
        return None;
    }
    acts.sort_unstable();
    let mut steps: Vec<&Event> = rows
        .iter()
        .filter(|e| e.is_statechange == 91 && e.iff == 0)
        .collect();
    steps.sort_by_key(|e| e.value);
    Some(reduce(
        &acts,
        &steps,
        info.dst_agent as u32,
        f32::from_bits((info.src_agent >> 32) as u32),
        info.overstack & 0x1_0000 != 0,
    ))
}

/// The reduction over sorted actions `(index, code, ms)` and steps, loop count `n`.
fn reduce(
    acts: &[(i32, u64, u32)],
    steps: &[&Event],
    n: u32,
    t_b: f32,
    speed_immune: bool,
) -> Timing {
    const NONE: i64 = u32::MAX as i64;
    let (mut first, mut term) = (NONE, 0i64);
    let mut durs: Vec<Option<i64>> = Vec::with_capacity(steps.len());
    for s in steps {
        let flags = (s.src_agent >> 40) & 0xff;
        let i = i64::from(s.value);
        if flags & 0x10 != 0 {
            first = i;
        }
        if flags & 0x20 != 0 {
            term = i;
        }
        durs.push(if s.buff_dmg != -1 {
            Some(i64::from(s.buff_dmg))
        } else if s.overstack != u32::MAX {
            Some(i64::from(s.overstack))
        } else {
            None
        });
    }
    let looped = term >= first;
    let (mut total, mut looptime) = (0i64, 0i64);
    for (i, d) in durs.iter().enumerate() {
        let (i, Some(d)) = (i as i64, *d) else {
            continue;
        };
        if i <= term {
            total += d;
        }
        if first <= i && i <= term {
            looptime += d;
        }
    }
    let mut extra = 0i64;
    if looped && n != 0 {
        let (mut idx, mut wrapped) = (first, false);
        for _ in 0..n {
            if wrapped && let Some(Some(d)) = usize::try_from(idx).ok().and_then(|i| durs.get(i)) {
                extra += d;
            }
            if idx != term {
                idx += 1;
            } else {
                idx = first;
                wrapped = true;
            }
        }
    }
    let (mut full, mut activation) = (0i64, 0i64);
    for &(_, code, ms) in acts {
        let t = i64::from(ms);
        if t == 0 {
            continue;
        }
        let adj = if n == 0 && looped {
            extra - looptime
        } else {
            extra
        };
        if code == 0 {
            full = q40(adj + t);
        } else if activation == 0 {
            activation = q40(if total < t { t + adj } else { t });
        }
    }
    Timing {
        activation_ms: activation,
        full_ms: full,
        t_b,
        speed_immune,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A skill with info, timing and steps rows in every arcdps log (1066).
    pub(crate) const KNOWN_SKILL: u32 = 1066;

    #[test]
    fn table_is_well_formed_and_sorted() {
        let idx = index();
        assert!(idx.len() > 1000, "{} skills", idx.len());
        assert!(idx.windows(2).all(|w| w[0].0 < w[1].0));
        let (_, at, n) = *idx.last().unwrap();
        assert_eq!(at + n * ROW, data().len(), "no trailing bytes");
    }

    #[test]
    fn rows_carry_the_skill_and_a_content_statechange() {
        for &(skill, _, _) in index().iter().step_by(97) {
            for e in rows(skill) {
                assert_eq!(e.skillid, skill);
                assert!(matches!(e.is_statechange, 30 | 31 | 32 | 33 | 46 | 91));
                if e.is_statechange == 46 {
                    assert_eq!(e.overstack, 2, "skill content type");
                }
            }
        }
        assert!(rows(u32::MAX).is_empty());
    }

    /// Values from arcdps's kind-1 starts (24 576 of 24 576 starts in the six 20261124 logs match
    /// this reduction): a plain skill and the q40 rounding.
    #[test]
    fn timing_matches_arcdps_starts() {
        let t = timing(5491).unwrap();
        assert_eq!((t.start_value(), t.full_ms), (800, 1240));
        assert!(timing(u32::MAX).is_none());
        assert_eq!((q40(0), q40(39), q40(40), q40(-1)), (40, 40, 80, 0));
    }

    #[test]
    fn buff_flags_from_buffinfo() {
        assert_eq!(buff_flags(762), (true, false), "Determined");
        assert_eq!(buff_flags(21816), (false, true));
        assert_eq!(buff_flags(736), (false, false), "Bleeding");
    }

    #[test]
    fn guid_table_is_well_formed_and_sorted() {
        let d = guid_data();
        let recs: Vec<&[u8]> = d.chunks_exact(GUID_RECORD).collect();
        assert!(recs.len() > 10_000, "{} records", recs.len());
        let key = |r: &[u8]| (r[0], u32::from_le_bytes([r[1], r[2], r[3], r[4]]));
        assert!(recs.windows(2).all(|w| key(w[0]) < key(w[1])));
        for r in recs.iter().step_by(101) {
            let e = Event::from_bytes(r[5..].try_into().unwrap());
            assert_eq!(e.is_statechange, 46);
            assert_eq!((e.overstack, e.skillid), (u32::from(r[0]), key(r).1));
        }
    }

    #[test]
    fn guid_finds_effects_markers_and_species() {
        for (t, id) in [(GUID_EFFECT, 21), (GUID_MARKER, 35), (GUID_SPECIES, 1082)] {
            let e = guid(t, id).unwrap_or_else(|| panic!("{t} {id}"));
            assert_eq!(
                (e.is_statechange, e.overstack, e.skillid),
                (46, u32::from(t), id)
            );
        }
        assert!(guid(GUID_MARKER, u32::MAX).is_none());
    }
}
