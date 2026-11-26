//! Recording and replay of everything the ring delivers to the processor.
//!
//! The live loop turns each ring read into [`Step`]s and applies them with [`apply`]. With
//! `--record` the same steps are written to a file; `--replay` reads them back and applies them
//! to a fresh processor, so a play session can be re-run offline with exactly the inputs (and
//! clock ticks) the live run saw.
//!
//! File: the 8-byte [`MAGIC`], then steps. Each step is a tag byte and a fixed body, all
//! little-endian. Events are stored in the ring's own encoding (kind + 40-byte payload).

use std::io::{self, Read, Write};

use axidps_protocol::v2::{Event, PAYLOAD_SIZE};

use crate::processor::{FightOutcome, Processor};

pub const MAGIC: &[u8; 8] = b"AXREC001";

const T_HEADER: u8 = 1;
const T_DROPPED: u8 = 2;
const T_PRIME: u8 = 3;
const T_PUSH: u8 = 4;
const T_TICK: u8 = 5;
const T_FLUSH: u8 = 6;

/// One input to the processor, in the order the live loop gave it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Header {
        group_state: [u8; 16],
        dll_version: u32,
    },
    Dropped(u64),
    Prime(u64, Event),
    Push(u64, Event),
    Tick(u64),
    /// The producer left (or core is stopping): finish the fight in progress.
    Flush,
}

/// Apply one step to the processor.
pub fn apply(p: &mut Processor, step: &Step) -> Option<FightOutcome> {
    match *step {
        Step::Header {
            group_state,
            dll_version,
        } => {
            p.set_header_status(group_state, dll_version);
            None
        }
        Step::Dropped(n) => {
            p.note_dropped(n);
            None
        }
        Step::Prime(t, ev) => {
            p.prime(t, &ev);
            None
        }
        Step::Push(t, ev) => p.push(t, &ev),
        Step::Tick(t) => p.tick(t),
        Step::Flush => p.flush(),
    }
}

/// Writes steps to a recording.
pub struct Recorder<W: Write> {
    w: W,
}

impl<W: Write> Recorder<W> {
    pub fn new(mut w: W) -> io::Result<Self> {
        w.write_all(MAGIC)?;
        Ok(Recorder { w })
    }

    pub fn write(&mut self, step: &Step) -> io::Result<()> {
        let w = &mut self.w;
        match *step {
            Step::Header {
                group_state,
                dll_version,
            } => {
                w.write_all(&[T_HEADER])?;
                w.write_all(&group_state)?;
                w.write_all(&dll_version.to_le_bytes())
            }
            Step::Dropped(n) => {
                w.write_all(&[T_DROPPED])?;
                w.write_all(&n.to_le_bytes())
            }
            Step::Prime(t, ev) | Step::Push(t, ev) => {
                let tag = if matches!(step, Step::Prime(..)) {
                    T_PRIME
                } else {
                    T_PUSH
                };
                let (kind, payload) = ev.encode();
                w.write_all(&[tag])?;
                w.write_all(&t.to_le_bytes())?;
                w.write_all(&kind.to_le_bytes())?;
                w.write_all(&payload)
            }
            Step::Tick(t) => {
                w.write_all(&[T_TICK])?;
                w.write_all(&t.to_le_bytes())
            }
            Step::Flush => w.write_all(&[T_FLUSH]),
        }
    }

    pub fn flush(&mut self) -> io::Result<()> {
        self.w.flush()
    }
}

/// Reads steps back from a recording.
pub struct Player<R: Read> {
    r: R,
}

fn read_n<const N: usize>(r: &mut impl Read) -> io::Result<[u8; N]> {
    let mut b = [0u8; N];
    r.read_exact(&mut b)?;
    Ok(b)
}

fn bad(msg: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg)
}

impl<R: Read> Player<R> {
    pub fn new(mut r: R) -> io::Result<Self> {
        if &read_n::<8>(&mut r)? != MAGIC {
            return Err(bad("not an axidps recording".into()));
        }
        Ok(Player { r })
    }

    /// The next step; `Ok(None)` at the end. A step cut off at the end (core was killed mid
    /// write) also ends the recording.
    pub fn next_step(&mut self) -> io::Result<Option<Step>> {
        let mut tag = [0u8; 1];
        if self.r.read(&mut tag)? == 0 {
            return Ok(None);
        }
        let step = (|| -> io::Result<Step> {
            let r = &mut self.r;
            Ok(match tag[0] {
                T_HEADER => Step::Header {
                    group_state: read_n::<16>(r)?,
                    dll_version: u32::from_le_bytes(read_n(r)?),
                },
                T_DROPPED => Step::Dropped(u64::from_le_bytes(read_n(r)?)),
                T_PRIME | T_PUSH => {
                    let t = u64::from_le_bytes(read_n(r)?);
                    let kind = u16::from_le_bytes(read_n(r)?);
                    let payload = read_n::<PAYLOAD_SIZE>(r)?;
                    let ev = Event::decode(kind, &payload)
                        .ok_or_else(|| bad(format!("event kind {kind} does not decode")))?;
                    if tag[0] == T_PRIME {
                        Step::Prime(t, ev)
                    } else {
                        Step::Push(t, ev)
                    }
                }
                T_TICK => Step::Tick(u64::from_le_bytes(read_n(r)?)),
                T_FLUSH => Step::Flush,
                t => return Err(bad(format!("unknown step tag {t}"))),
            })
        })();
        match step {
            Ok(s) => Ok(Some(s)),
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => Ok(None),
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assemble::build_log;
    use crate::registry::tests::{add, name_events};
    use crate::rules::WvwRules;
    use axidps_evtc::encode;
    use axidps_protocol::v2::NameField;

    /// A short session: six squad players fight, one is dropped, the clock runs out.
    fn session() -> Vec<Step> {
        let mut s = vec![Step::Header {
            group_state: [1; 16],
            dll_version: 7,
        }];
        for id in 1..=6 {
            s.push(Step::Prime(0, add(id, 1)));
            for e in name_events(id, NameField::Character, &format!("Char {id}")) {
                s.push(Step::Push(0, e));
            }
            for e in name_events(id, NameField::Account, &format!(":Alpha.{id:04}")) {
                s.push(Step::Push(0, e));
            }
        }
        s.push(Step::Push(
            1000,
            Event::Combat {
                id: 1,
                in_combat: true,
            },
        ));
        s.push(Step::Dropped(3));
        for t in 0..40u64 {
            s.push(Step::Push(
                1000 + t * 500,
                Event::Strike {
                    src: 1,
                    dst: 2,
                    skill: 5,
                    value: -100,
                    overstack: 0,
                    result: 0,
                },
            ));
            s.push(Step::Tick(1000 + t * 500 + 20));
        }
        s.push(Step::Tick(60_000));
        s.push(Step::Flush);
        s
    }

    /// Run steps through a processor; the encoded logs it writes.
    fn run(steps: impl IntoIterator<Item = Step>) -> Vec<Vec<u8>> {
        let mut p = Processor::new(WvwRules::default());
        let mut logs = Vec::new();
        for s in steps {
            if let Some(FightOutcome::Ended { fight, registry }) = apply(&mut p, &s) {
                logs.push(encode(&build_log(&fight, &registry)));
            }
        }
        logs
    }

    fn record(steps: &[Step]) -> Vec<u8> {
        let mut r = Recorder::new(Vec::new()).unwrap();
        for s in steps {
            r.write(s).unwrap();
        }
        r.w
    }

    fn play(bytes: &[u8]) -> Vec<Step> {
        let mut p = Player::new(bytes).unwrap();
        let mut out = Vec::new();
        while let Some(s) = p.next_step().unwrap() {
            out.push(s);
        }
        out
    }

    #[test]
    fn every_step_round_trips() {
        let steps = session();
        assert_eq!(play(&record(&steps)), steps);
    }

    #[test]
    fn replay_writes_the_same_logs_as_the_live_run() {
        let steps = session();
        let live = run(steps.iter().copied());
        assert_eq!(live.len(), 1, "the session should produce one fight");
        assert_eq!(run(play(&record(&steps))), live);
    }

    #[test]
    fn a_cut_off_last_step_ends_the_recording() {
        let steps = session();
        let mut bytes = record(&steps);
        bytes.truncate(bytes.len() - 1 - 9); // drop Flush and half of the last Tick
        bytes.extend_from_slice(&[T_TICK, 1, 2, 3]);
        assert_eq!(play(&bytes), steps[..steps.len() - 2]);
    }

    #[test]
    fn rejects_a_file_that_is_not_a_recording() {
        assert!(Player::new(&b"EVTC2026"[..]).is_err());
    }

    #[test]
    fn unknown_tag_is_an_error() {
        let mut bytes = MAGIC.to_vec();
        bytes.push(99);
        assert!(Player::new(&bytes[..]).unwrap().next_step().is_err());
    }
}
