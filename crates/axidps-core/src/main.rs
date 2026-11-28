//! `axidps-core`: reads the shared-memory event ring, decides when fights start and end, and
//! writes `.zevtc` logs.

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use axidps_core::around::Around;
use axidps_core::assemble::{CondiReport, build_log_report};
use axidps_core::condi::Verdict;
use axidps_core::live::{self, LiveStats, LiveWriter};
use axidps_core::output::{resolve_log_dir, write_fight};
use axidps_core::processor::{FightOutcome, Processor};
use axidps_core::relay::client::{Cmd, Relay};
use axidps_core::relay::wire::{WireCombat, room_id};
use axidps_core::relay::{DEFAULT_RELAY_URL, link};
use axidps_core::replay::{Player, Recorder, Step, apply};
use axidps_core::rules::{Trigger, WvwRules};
use axidps_protocol::v2::{Event, RingHost, ring_path};
use axidps_ringbuf::v2::{EventReader, STUCK_TIMEOUT_MS};
use chrono::TimeZone;
use clap::{Parser, ValueEnum};

const POLL: Duration = Duration::from_millis(20);
const REOPEN: Duration = Duration::from_millis(500);
/// How often the `--stop-file` is looked for.
const STOP_CHECK: Duration = Duration::from_millis(250);

#[derive(Clone, Copy, ValueEnum)]
enum TriggerArg {
    Squad,
    Any,
}

#[derive(Parser)]
#[command(name = "axidps-core", about = "axidps log capture")]
struct Args {
    /// Log folder (default: the GW2 arcdps.cbtlogs folder in the Documents folder).
    #[arg(long)]
    log_dir: Option<PathBuf>,
    /// Event ring path (default: the platform's standard ring).
    #[arg(long)]
    ring: Option<PathBuf>,
    /// What starts a log.
    #[arg(long, value_enum, default_value = "squad")]
    trigger: TriggerArg,
    /// Minimum squad members seen for a log to be saved.
    #[arg(long)]
    min_squad: Option<usize>,
    /// Minimum log length, in seconds.
    #[arg(long)]
    min_duration_s: Option<u64>,
    /// Flush and exit 0 once the producer is gone and the ring is drained.
    #[arg(long)]
    exit_when_idle: bool,
    /// Also save everything read from the ring to a new `.axrec` file in this folder, so the
    /// session can be replayed offline with `--replay`. The oldest recordings are deleted once
    /// the folder holds more than 2 GiB.
    #[arg(long)]
    record: Option<PathBuf>,
    /// Write logs from a recording instead of reading the ring, then exit.
    #[arg(long, conflicts_with_all = ["ring", "record", "exit_when_idle"])]
    replay: Option<PathBuf>,
    /// Live stats feed for the overlay meter, rewritten atomically at most every 500 ms while
    /// it changes; `none` disables it. Not written with `--replay`.
    #[arg(long, default_value_os_t = axidps_protocol::paths::live_feed_path(RingHost::current_build()))]
    live_out: PathBuf,
    /// Write the condition model's estimated ticks for other players. Without it the model only
    /// runs its self-check against your own exact ticks and reports it after each log.
    #[arg(long)]
    condi_write: bool,
    /// Heal relay base URL (`wss://host`), shared with the squad/party while reading the ring.
    #[arg(long, default_value = DEFAULT_RELAY_URL)]
    relay_url: String,
    /// Do not connect to the heal relay.
    #[arg(long)]
    no_relay: bool,
    /// Finish the fight in progress and exit once this file exists, as on Ctrl-C. The launcher
    /// uses it on Windows, where a console signal can't reach a process without a console.
    #[arg(long)]
    stop_file: Option<PathBuf>,
    /// When no log folder is found, say so once and wait for one (looking again every 10 s)
    /// instead of exiting 2, so a supervisor does not restart core in a loop. The wait ends
    /// with exit 0 on the `--stop-file` or a signal.
    #[arg(long, conflicts_with = "replay")]
    wait_for_log_dir: bool,
}

/// How often a waiting core looks for the log folder again.
const LOG_DIR_RETRY: Duration = Duration::from_secs(10);

/// Watches for the `--stop-file`, at most every [`STOP_CHECK`].
struct StopFile {
    path: Option<PathBuf>,
    next: Instant,
}

impl StopFile {
    fn new(path: Option<PathBuf>, now: Instant) -> Self {
        StopFile { path, next: now }
    }

    fn requested(&mut self, now: Instant) -> bool {
        let Some(path) = &self.path else {
            return false;
        };
        if now < self.next {
            return false;
        }
        self.next = now + STOP_CHECK;
        path.exists()
    }
}

/// One line per written log: the condition model's self-check and coverage.
fn condi_line(r: &CondiReport) -> String {
    let verdict = match r.verdict {
        Verdict::NotRun => "self-check not run".to_string(),
        Verdict::Incomplete => "self-check incomplete (formulas or stats missing)".to_string(),
        Verdict::Pass => "self-check pass".to_string(),
        Verdict::Fail { buff, gap_permille } => {
            format!(
                "self-check FAIL (buff {buff}, gap {}.{}%)",
                gap_permille / 10,
                gap_permille % 10
            )
        }
    };
    let sums: Vec<String> = r
        .check
        .sums
        .iter()
        .map(|(b, (m, e))| {
            let (mt, et) = r.check.ticks.get(b).copied().unwrap_or_default();
            format!("{b}: damage {m}/{e}, ticks {mt}/{et}")
        })
        .collect();
    // The gear model's calibration: POV attributes rebuilt from gear / the game's own.
    let stats = match r.check.pov_stats {
        Some((m, e)) => {
            let names = [
                (1, "power"),
                (2, "precision"),
                (3, "toughness"),
                (4, "vitality"),
                (5, "ferocity"),
                (6, "healing"),
                (7, "condi"),
                (8, "concentration"),
                (9, "expertise"),
            ];
            let parts: Vec<String> = names
                .iter()
                .map(|&(i, n)| format!("{n} {}/{}", m[i], e[i]))
                .collect();
            format!("; pov stats from gear/exact: {}", parts.join(", "))
        }
        None => "; pov stats from gear: none".to_string(),
    };
    // Enemy health vs what explains it (others' condition damage is the residual).
    let h = &r.health;
    let health = if h.targets.is_empty() {
        "; enemy health: no samples".to_string()
    } else {
        let res = h.residual();
        let ratio = if res > 0.0 {
            format!("{:.0}%", h.totals.model / res * 100.0)
        } else {
            "n/a".to_string()
        };
        format!(
            "; enemy health over {} players ({} samples): lost {:.0} (drops {:.0}), strikes {:.0}, pov condi {:.0}, residual {:.0}, model others {:.0} ({ratio} of residual)",
            h.targets.len(),
            h.intervals,
            h.lost,
            h.dropped,
            h.totals.strikes,
            h.totals.pov_exact,
            res,
            h.totals.model
        )
    };
    format!(
        "condi model: {verdict} [{}]; others: {} stack-ticks modelled, {} no formula, {} no stats; pov unmodelled {}; {} rows written{stats}{health}",
        sums.join(", "),
        r.coverage.modelled,
        r.coverage.no_formula,
        r.coverage.no_stats,
        r.check.unmodelled,
        r.written
    )
}

/// The live stats feed and where it goes.
struct Live {
    stats: LiveStats,
    around: Around,
    writer: LiveWriter,
    /// A write failed and was reported; cleared by the next good write.
    failing: bool,
    /// The overlay's ui.json (its `core` settings), when it was last checked and its mtime.
    settings: Option<PathBuf>,
    settings_checked: Option<Instant>,
    settings_mtime: Option<std::time::SystemTime>,
}

/// The heal relay thread and what core last told it.
struct RelayLink {
    relay: Relay,
    /// Last ring event time (game ms) and the unix ms when it was read.
    clock: Option<(u64, u64)>,
    /// Last pushed or ticked time (game ms): relayed heals are never placed before it.
    last_time: u64,
    /// ui.json `core.relay` (default on). Off sends no room, so no connection.
    enabled: bool,
    /// The group's room, as the last `GroupKey` gave it.
    room: Option<String>,
    account: Option<String>,
    active: bool,
    /// Recent agent positions: enemy players in relayed condition ticks are named by place.
    positions: link::Positions,
}

impl RelayLink {
    /// `enabled`: false until ui.json has been read once (no setting file: on).
    fn new(relay: Relay, enabled: bool) -> Self {
        Self {
            relay,
            clock: None,
            last_time: 0,
            enabled,
            room: None,
            account: None,
            active: false,
            positions: link::Positions::default(),
        }
    }

    fn set_room(&mut self, room: Option<String>) {
        if room == self.room {
            return;
        }
        match &room {
            Some(r) => err(format_args!("relay: room {}", r.get(..8).unwrap_or(r))),
            None => err(format_args!("relay: no room")),
        }
        self.room = room;
        if self.enabled {
            self.relay.send(Cmd::Room(self.room.clone()));
        }
    }

    fn set_enabled(&mut self, on: bool) {
        if on == self.enabled {
            return;
        }
        self.enabled = on;
        err(format_args!("relay: {}", if on { "on" } else { "off" }));
        self.relay
            .send(Cmd::Room(if on { self.room.clone() } else { None }));
    }
}

/// `ui.json` in axidps's config folder (the overlay host's file): `%APPDATA%\axidps` on
/// Windows, else `$XDG_CONFIG_HOME/axidps` or `~/.config/axidps`.
fn ui_json_path() -> Option<PathBuf> {
    axidps_protocol::paths::config_dir().map(|d| d.join("ui.json"))
}

fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// Recordings kept in a `--record` folder, in bytes.
const RECORD_KEEP: u64 = 2 << 30;

/// Delete the oldest `.axrec` files in `dir` until the rest fit in `keep` bytes.
fn prune_recordings(dir: &std::path::Path, keep: u64) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<(std::time::SystemTime, u64, PathBuf)> = rd
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|x| x == "axrec"))
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            Some((m.modified().ok()?, m.len(), e.path()))
        })
        .collect();
    files.sort();
    let mut total: u64 = files.iter().map(|f| f.1).sum();
    for (_, len, path) in files {
        if total <= keep {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total -= len;
        }
    }
}

type RecFile = Recorder<std::io::BufWriter<std::fs::File>>;

/// A new recording in `dir`, named after the local start time.
fn open_recording(dir: &std::path::Path) -> std::io::Result<(PathBuf, RecFile)> {
    std::fs::create_dir_all(dir)?;
    prune_recordings(dir, RECORD_KEEP);
    let stem = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let path = dir.join(format!("{stem}.axrec"));
    let f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    Ok((path.clone(), Recorder::new(std::io::BufWriter::new(f))?))
}

fn ring_of(ring: &Option<PathBuf>) -> PathBuf {
    ring.clone()
        .unwrap_or_else(|| ring_path(RingHost::current_build()))
}

/// The one-time line printed while no ring can be opened yet.
fn waiting_line(path: &std::path::Path) -> String {
    let mut s = format!(
        "waiting for the event ring at {} (start the game with axidps, or pass --ring <path>)",
        path.display()
    );
    if cfg!(windows) {
        s.push_str("; the game and axidps-core must see the same TEMP folder");
    }
    s
}

fn now_unix_s() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0))
}

fn to_local(s: i64) -> chrono::NaiveDateTime {
    chrono::Local
        .timestamp_opt(s, 0)
        .earliest()
        .map_or_else(chrono::NaiveDateTime::default, |d| d.naive_local())
}

/// Print a line to stdout; a closed or broken stdout is not fatal.
fn out(msg: std::fmt::Arguments) {
    let _ = writeln!(std::io::stdout(), "{msg}");
}

/// Print a line to stderr; a closed or broken stderr is not fatal.
fn err(msg: std::fmt::Arguments) {
    let _ = writeln!(std::io::stderr(), "{msg}");
}

struct App {
    processor: Processor,
    dir: PathBuf,
    /// Last event time seen (game ms) and when it was read, to estimate "now" for `tick`.
    clock: Option<(u64, Instant)>,
    /// A log could not be written.
    write_failed: bool,
    /// `--record`: where the steps go. Dropped (with one message) after a write error.
    rec: Option<RecFile>,
    /// The last header status applied, so an unchanged one is not recorded every read.
    header: Option<([u8; 16], u32)>,
    /// `--live-out`: the overlay's stats feed (live mode only).
    live: Option<Live>,
    /// The heal relay (live mode only, unless `--no-relay`).
    relay: Option<RelayLink>,
}

impl App {
    /// Record a step, apply it, and report its outcome.
    fn step(&mut self, s: Step) {
        // Motion outside a fight changes nothing (no state, no log), and is most of the traffic:
        // keep it out of the recording. A replay gives the same logs without it.
        let idle_motion = matches!(
            s,
            Step::Push(_, Event::Motion { .. }) | Step::Prime(_, Event::Motion { .. })
        ) && !self.processor.recording();
        if !idle_motion
            && let Some(r) = self.rec.as_mut()
            && let Err(e) = r.write(&s)
        {
            err(format_args!("recording stopped: {e}"));
            self.rec = None;
        }
        let outcome = apply(&mut self.processor, &s);
        if let Some(l) = self.live.as_mut() {
            l.stats.after_step(&self.processor, &s, outcome.as_ref());
            l.around.after_step(&self.processor, &s);
        }
        self.relay_after_step(&s);
        if let Some(o) = outcome {
            self.report(o);
        }
    }

    /// Tell the relay what the step changed: room, POV account, fight on/off, a local heal.
    fn relay_after_step(&mut self, s: &Step) {
        let Some(link) = self.relay.as_mut() else {
            return;
        };
        let reg = self.processor.registry();
        if let Step::Push(t, _) | Step::Tick(t) = s {
            link.last_time = link.last_time.max(*t);
        }
        match s {
            Step::Push(_, Event::Map { .. }) | Step::Prime(_, Event::Map { .. }) => {
                link.positions.clear()
            }
            Step::Push(t, ev) | Step::Prime(t, ev) => link.positions.note(*t, ev),
            _ => {}
        }
        match s {
            Step::Push(_, Event::GroupKey { key }) | Step::Prime(_, Event::GroupKey { key }) => {
                link.set_room(room_id(*key));
            }
            Step::Push(_, Event::Combat { id, in_combat })
                if link.enabled && reg.pov() == Some(*id) =>
            {
                link.relay.send(Cmd::Combat(WireCombat {
                    t_unix_ms: now_unix_ms(),
                    in_combat: *in_combat,
                }));
            }
            Step::Push(_, ev @ Event::Heal { .. }) if link.enabled => {
                if let Some(h) = link::outgoing(reg, now_unix_ms(), ev) {
                    link.relay.send(Cmd::Heal(h));
                }
            }
            Step::Push(t, ev @ Event::BuffDamage { .. }) if link.enabled => {
                if let Some(c) = link::outgoing_condi(reg, &link.positions, *t, now_unix_ms(), ev) {
                    link.relay.send(Cmd::Condi(c));
                }
            }
            _ => {}
        }
        if let Some(acc) = link::pov_account(reg)
            && link.account.as_deref() != Some(acc)
        {
            link.account = Some(acc.to_string());
            link.relay.send(Cmd::Account(acc.to_string()));
        }
        let rec = self.processor.recording();
        if rec != link.active {
            link.active = rec;
            link.relay.send(Cmd::Active(rec));
        }
    }

    /// Push the heals and condition ticks peers relayed since the last call, at the local times
    /// for their clocks.
    fn pump_relay(&mut self) {
        let Some(link) = self.relay.as_ref() else {
            return;
        };
        let batches = link.relay.try_recv();
        // No live event yet to place them by: they are dropped.
        let Some(clock) = link.clock else {
            return;
        };
        // Not after "now" either, so ring events read next do not go backwards.
        let now = clock
            .0
            .saturating_add(now_unix_ms().saturating_sub(clock.1));
        for b in batches {
            // Combat changes and heals in time order: an enter must come before the heals in it.
            let floor = self.relay.as_ref().map_or(0, |l| l.last_time);
            let reg = self.processor.registry();
            let mut evs: Vec<(u64, Event)> = b
                .combat
                .iter()
                .filter_map(|c| link::incoming_combat(reg, &b.from, c, clock, floor))
                .chain(
                    b.heals
                        .iter()
                        .filter_map(|h| link::incoming(reg, &b.from, h, clock, floor)),
                )
                .chain(b.condi.iter().filter_map(|c| {
                    let pos = &self.relay.as_ref()?.positions;
                    link::incoming_condi(reg, pos, &b.from, c, clock, floor)
                }))
                .collect();
            evs.sort_by_key(|(t, _)| *t);
            for (t, ev) in evs {
                let floor = self.relay.as_ref().map_or(0, |l| l.last_time);
                self.step(Step::Push(t.max(floor).min(now.max(floor)), ev));
            }
        }
    }

    /// Write the live feed if it changed and the last write is old enough (`last`: on exit,
    /// write any change now). A failure is reported once (until a write succeeds again) and
    /// never affects logs.
    fn write_live(&mut self, last: bool) {
        let Some(l) = self.live.as_mut() else {
            return;
        };
        let now = Instant::now();
        // Follow the hub's counting settings: re-read ui.json when it changed (checked every 2 s).
        if let Some(path) = &l.settings
            && l.settings_checked
                .is_none_or(|t| now.duration_since(t) >= Duration::from_secs(2))
        {
            l.settings_checked = Some(now);
            let mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok();
            if mtime != l.settings_mtime {
                // Unreadable right now: keep the current settings and retry next check.
                if let Some(v) = read_ui_json(path) {
                    l.settings_mtime = mtime;
                    l.stats.set_counting(live::Counting::from_ui_json(&v));
                    // The overlay's details window: whose breakdown the feed carries.
                    l.stats.set_detail(
                        v.get("detail")
                            .and_then(|d| d.as_str())
                            .filter(|a| !a.is_empty())
                            .map(str::to_owned),
                    );
                    l.stats.set_detail_skill(
                        v.get("detail_skill")
                            .and_then(serde_json::Value::as_u64)
                            .and_then(|k| u32::try_from(k).ok())
                            .filter(|&k| k != 0),
                    );
                    if let Some(link) = self.relay.as_mut() {
                        link.set_enabled(relay_enabled(&v));
                    }
                }
            }
        }
        let wrote = live::write_feed(
            &mut l.writer,
            &mut l.stats,
            &l.around,
            &self.processor,
            now,
            now_unix_ms(),
            last,
        );
        if let Some(e) = note_write(&mut l.failing, wrote) {
            err(format_args!(
                "cannot write the live feed {}: {e}",
                l.writer.path().display()
            ));
        }
    }

    /// Write buffered recording bytes out, so a killed core loses at most one read.
    fn flush_recording(&mut self) {
        if let Some(r) = self.rec.as_mut()
            && let Err(e) = r.flush()
        {
            err(format_args!("recording stopped: {e}"));
            self.rec = None;
        }
    }

    fn report(&mut self, outcome: FightOutcome) {
        match outcome {
            FightOutcome::Ended { fight, registry } => {
                let end_unix_s = match fight.unix_at {
                    Some((t0, u0)) => {
                        let delta = (i128::from(fight.end_ms) - i128::from(t0)) / 1000;
                        i64::try_from(i128::from(u0) + delta).unwrap_or_else(|_| now_unix_s())
                    }
                    None => now_unix_s(),
                };
                let (log, condi) = build_log_report(&fight, &registry);
                match write_fight(&self.dir, &log, end_unix_s, to_local) {
                    Ok(p) => {
                        out(format_args!("wrote {}", p.display()));
                        out(format_args!("{}", condi_line(&condi)));
                    }
                    Err(e) => {
                        self.write_failed = true;
                        err(format_args!("write failed: {e}"));
                    }
                }
            }
            FightOutcome::Discarded {
                reason,
                duration_ms,
            } => out(format_args!("discarded {reason:?} {duration_ms}")),
        }
    }

    /// One read: history is primed, live events are processed. Returns true if the read
    /// delivered or lost anything.
    fn pump(&mut self, r: &mut EventReader, buf: &mut Vec<(u64, Event)>) -> bool {
        buf.clear();
        let stats = r.read(buf);
        // A batch is either all history (primed) or all live (processed).
        let priming = stats.history;
        let header = (r.group_state(), r.dll_version());
        if self.header != Some(header) {
            self.header = Some(header);
            self.step(Step::Header {
                group_state: header.0,
                dll_version: header.1,
            });
        }
        // Losses precede the delivered events, so they belong to the fight recording now
        // (or the next one). In a history batch only the claims pushed after the open count.
        let lost = if priming {
            stats.dropped_live
        } else {
            stats.dropped.saturating_add(stats.unknown)
        };
        if lost > 0 {
            self.step(Step::Dropped(lost));
        }
        if !priming
            && let Some(&(t, _)) = buf.last()
            && let Some(link) = self.relay.as_mut()
        {
            link.clock = Some((t, now_unix_ms()));
        }
        for &(t, ev) in buf.iter() {
            self.clock = Some((t, Instant::now()));
            self.step(if priming {
                Step::Prime(t, ev)
            } else {
                Step::Push(t, ev)
            });
        }
        if let Some((t, at)) = self.clock {
            let now = t.saturating_add(u64::try_from(at.elapsed().as_millis()).unwrap_or(u64::MAX));
            self.step(Step::Tick(now));
        }
        self.flush_recording();
        !buf.is_empty() || stats.dropped > 0 || stats.unknown > 0
    }

    /// Read until the ring is drained (a read delivers nothing and no claim is outstanding),
    /// or until a stuck producer has had twice the reader's timeout to commit.
    fn drain(&mut self, r: &mut EventReader, buf: &mut Vec<(u64, Event)>) {
        let deadline = Instant::now() + Duration::from_millis(2 * STUCK_TIMEOUT_MS);
        loop {
            let got = self.pump(r, buf);
            if (!got && !r.has_pending()) || Instant::now() >= deadline {
                return;
            }
            if !got {
                std::thread::sleep(POLL);
            }
        }
    }
}

/// `--replay`: apply every step of a recording, then finish any fight still open.
fn replay(app: &mut App, path: &std::path::Path) -> ExitCode {
    let mut player =
        match std::fs::File::open(path).and_then(|f| Player::new(std::io::BufReader::new(f))) {
            Ok(p) => p,
            Err(e) => {
                err(format_args!("cannot replay {}: {e}", path.display()));
                return ExitCode::from(2);
            }
        };
    let mut failed = false;
    loop {
        match player.next_step() {
            Ok(Some(s)) => app.step(s),
            Ok(None) => break,
            Err(e) => {
                err(format_args!("replay stopped: {e}"));
                failed = true;
                break;
            }
        }
    }
    // A recording cut off by a killed core has no final flush.
    app.step(Step::Flush);
    if failed || app.write_failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// Set by SIGINT/SIGTERM (Ctrl+Break on Windows). The handler is installed on first use: ctrlc
/// takes only one per process, and the log-folder wait and the main loop share it.
fn stop_flag() -> Arc<AtomicBool> {
    static STOP: std::sync::OnceLock<Arc<AtomicBool>> = std::sync::OnceLock::new();
    Arc::clone(STOP.get_or_init(|| {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        if let Err(e) = ctrlc::set_handler(move || flag.store(true, Ordering::SeqCst)) {
            err(format_args!("cannot install the signal handler: {e}"));
        }
        stop
    }))
}

/// Looks for the log folder every [`LOG_DIR_RETRY`] until it is found, or `None` once the stop
/// file appears or a signal arrives (nothing is recorded yet, so there is nothing to finish).
fn wait_for_log_dir(args: &Args) -> Option<PathBuf> {
    let stop = stop_flag();
    let mut stop_file = StopFile::new(args.stop_file.clone(), Instant::now());
    let mut next = Instant::now() + LOG_DIR_RETRY;
    loop {
        std::thread::sleep(STOP_CHECK);
        let now = Instant::now();
        if stop.load(Ordering::SeqCst) || stop_file.requested(now) {
            return None;
        }
        if now >= next {
            next = now + LOG_DIR_RETRY;
            if let Some(dir) = resolve_log_dir(None) {
                return Some(dir);
            }
        }
    }
}

fn main() -> ExitCode {
    let args = Args::parse();
    let dir = match resolve_log_dir(args.log_dir.clone()) {
        Some(dir) => dir,
        None if args.wait_for_log_dir => {
            err(format_args!(
                "no log folder: pass --log-dir or install GW2 under Steam/Proton; waiting for one"
            ));
            match wait_for_log_dir(&args) {
                Some(dir) => {
                    out(format_args!("log folder found: {}", dir.display()));
                    dir
                }
                None => return ExitCode::SUCCESS,
            }
        }
        None => {
            err(format_args!(
                "no log folder: pass --log-dir or install GW2 under Steam/Proton"
            ));
            return ExitCode::from(2);
        }
    };
    let mut rules = WvwRules {
        trigger: match args.trigger {
            TriggerArg::Squad => Trigger::Squad,
            TriggerArg::Any => Trigger::Any,
        },
        ..WvwRules::default()
    };
    if let Some(n) = args.min_squad {
        rules.min_squad = n;
    }
    if let Some(s) = args.min_duration_s {
        rules.min_duration_ms = s.saturating_mul(1000);
    }
    rules.condi_write = args.condi_write;

    let mut app = App {
        processor: Processor::new(rules),
        dir,
        clock: None,
        write_failed: false,
        rec: None,
        header: None,
        live: None,
        relay: None,
    };

    if let Some(path) = &args.replay {
        return replay(&mut app, path);
    }
    if args.live_out.as_os_str() != "none" {
        app.live = Some(Live {
            stats: LiveStats::new(),
            around: Around::new(),
            writer: LiveWriter::new(args.live_out.clone()),
            failing: false,
            settings: ui_json_path(),
            settings_checked: None,
            settings_mtime: None,
        });
    }
    // The relay shares heals only in live mode: reading the ring with the live feed on (its
    // ui.json `core.relay` setting can turn it off).
    if app.live.is_some() && !args.no_relay {
        let read_first = app.live.as_ref().is_some_and(|l| l.settings.is_some());
        app.relay = Some(RelayLink::new(
            Relay::start(args.relay_url.clone()),
            !read_first,
        ));
    }
    if let Some(dir) = &args.record {
        match open_recording(dir) {
            Ok((path, r)) => {
                out(format_args!("recording to {}", path.display()));
                app.rec = Some(r);
            }
            Err(e) => err(format_args!("cannot record to {}: {e}", dir.display())),
        }
    }
    let ring = ring_of(&args.ring);
    let mut said_waiting = false;
    let mut last_open_err: Option<std::io::ErrorKind> = None;
    let mut reader: Option<EventReader> = None;
    let mut last_open = Instant::now()
        .checked_sub(REOPEN)
        .unwrap_or_else(Instant::now);
    // The producer went inactive and the processor has been flushed for it.
    let mut flushed = false;
    let mut buf = Vec::new();

    // SIGINT/SIGTERM (and Ctrl+Break on Windows): finish the fight in progress, then exit.
    let stop = stop_flag();

    let mut stop_file = StopFile::new(args.stop_file.clone(), Instant::now());

    loop {
        app.write_live(false);
        std::thread::sleep(POLL);
        if stop.load(Ordering::SeqCst) || stop_file.requested(Instant::now()) {
            if let Some(r) = reader.as_mut() {
                app.drain(r, &mut buf);
            }
            app.step(Step::Flush);
            app.flush_recording();
            app.write_live(true);
            return if app.write_failed {
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            };
        }
        let Some(r) = reader.as_mut() else {
            if last_open.elapsed() >= REOPEN {
                last_open = Instant::now();
                let opened = EventReader::open_at(&ring);
                if let Err(e) = &opened {
                    if !said_waiting {
                        said_waiting = true;
                        out(format_args!("{}", waiting_line(&ring)));
                    }
                    // A file that exists but is not a usable ring is reported once per kind.
                    if e.kind() != std::io::ErrorKind::NotFound && last_open_err != Some(e.kind()) {
                        err(format_args!(
                            "cannot open the ring at {}: {e}",
                            ring.display()
                        ));
                    }
                    last_open_err = Some(e.kind());
                }
                if let Ok(r) = opened {
                    // A new game run: the meter must not show the previous run's last fight.
                    if let Some(l) = app.live.as_mut() {
                        l.stats.clear();
                        l.around.clear();
                    }
                    said_waiting = false;
                    last_open_err = None;
                    out(format_args!(
                        "reading ring (producer {})",
                        if r.is_producer_active() {
                            "active"
                        } else {
                            "inactive"
                        }
                    ));
                    reader = Some(r);
                    flushed = false;
                    app.clock = None;
                    // A new game run restarts game time.
                    if let Some(link) = app.relay.as_mut() {
                        link.clock = None;
                        link.last_time = 0;
                    }
                }
            }
            continue;
        };

        // Sample the flags before reading: the producer clears its flag after its last push, so
        // a read after seeing the flag clear drains the ring.
        let replaced = r.is_replaced();
        let inactive = !r.is_producer_active();

        if app.pump(r, &mut buf) {
            flushed = false;
        }
        app.pump_relay();

        if replaced || inactive {
            if !flushed {
                app.drain(r, &mut buf);
                app.step(Step::Flush);
                app.flush_recording();
                flushed = true;
                // The game stopped: leave the room and stop placing peer heals (the clock no
                // longer advances). The next run's GroupKey joins again.
                if let Some(link) = app.relay.as_mut() {
                    link.set_room(None);
                    link.clock = None;
                }
            }
            if args.exit_when_idle {
                app.write_live(true);
                return if app.write_failed {
                    ExitCode::FAILURE
                } else {
                    ExitCode::SUCCESS
                };
            }
            // A replaced ring (new game run, new inode) is reopened. An inactive ring that
            // still exists is kept: reopening it would replay its events.
            if replaced {
                reader = None;
                last_open = Instant::now();
            }
        }
    }
}

/// ui.json as a value: a missing file is the defaults (`Null`); an existing file that cannot
/// be read or parsed is `None`.
fn read_ui_json(path: &std::path::Path) -> Option<serde_json::Value> {
    match std::fs::read_to_string(path) {
        Ok(s) => serde_json::from_str(&s).ok(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Some(serde_json::Value::Null),
        Err(_) => None,
    }
}

/// Whether the relay is on in ui.json (on unless switched off).
fn relay_enabled(v: &serde_json::Value) -> bool {
    v["core"]["relay"].as_bool().unwrap_or(true)
}

/// Track a feed write's result in `failing`; returns the error when it is a new failure that
/// should be reported (once, until a write succeeds again).
fn note_write(failing: &mut bool, wrote: std::io::Result<bool>) -> Option<std::io::Error> {
    match wrote {
        Ok(true) => {
            *failing = false;
            None
        }
        Ok(false) => None,
        Err(e) => {
            if *failing {
                None
            } else {
                *failing = true;
                Some(e)
            }
        }
    }
}

#[cfg(test)]
mod write_report_tests {
    use super::*;

    #[test]
    fn a_failed_write_is_reported_once_until_a_good_write() {
        let bad = || Err(std::io::Error::other("x"));
        let mut failing = false;
        assert!(note_write(&mut failing, bad()).is_some());
        // Not due (nothing written): still failing, so no second report.
        assert!(note_write(&mut failing, Ok(false)).is_none());
        assert!(failing);
        assert!(note_write(&mut failing, bad()).is_none());
        assert!(note_write(&mut failing, Ok(true)).is_none());
        assert!(!failing);
        assert!(note_write(&mut failing, bad()).is_some());
    }
}

#[cfg(test)]
mod stop_file_tests {
    use super::*;

    #[test]
    fn stop_file_is_seen_and_checked_at_most_every_250ms() {
        let dir = std::env::temp_dir().join(format!("axidps-stop-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("core.stop");
        let _ = std::fs::remove_file(&path);
        let t0 = Instant::now();
        let mut stop = StopFile::new(Some(path.clone()), t0);
        assert!(!stop.requested(t0));
        std::fs::write(&path, b"").unwrap();
        // Too soon after the last look.
        assert!(!stop.requested(t0 + Duration::from_millis(100)));
        assert!(stop.requested(t0 + STOP_CHECK));
        assert!(!StopFile::new(None, t0).requested(t0));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod ui_json_tests {
    use super::*;

    #[test]
    fn relay_setting_reads() {
        let off: serde_json::Value = serde_json::from_str(r#"{"core":{"relay":false}}"#).unwrap();
        assert!(!relay_enabled(&off));
        let missing: serde_json::Value = serde_json::from_str(r#"{"core":{}}"#).unwrap();
        assert!(relay_enabled(&missing));
        assert!(relay_enabled(&serde_json::Value::Null));
    }

    #[test]
    fn relay_link_waits_for_the_first_read() {
        let mut link = RelayLink::new(Relay::start("ws://127.0.0.1:1".into()), false);
        assert!(!link.enabled);
        link.set_enabled(relay_enabled(&serde_json::Value::Null));
        assert!(link.enabled);
        assert!(RelayLink::new(Relay::start("ws://127.0.0.1:1".into()), true).enabled);
    }

    #[test]
    fn unreadable_ui_json_is_not_defaults() {
        let dir = std::env::temp_dir().join(format!("axidps-uij-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let bad = dir.join("bad.json");
        std::fs::write(&bad, "{ not json").unwrap();
        assert!(read_ui_json(&bad).is_none());
        assert_eq!(
            read_ui_json(&dir.join("missing.json")),
            Some(serde_json::Value::Null)
        );
        let good = dir.join("good.json");
        std::fs::write(&good, r#"{"core":{"relay":false}}"#).unwrap();
        assert!(!relay_enabled(&read_ui_json(&good).unwrap()));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
