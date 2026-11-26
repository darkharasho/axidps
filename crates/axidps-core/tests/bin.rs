use std::io::{BufRead, BufReader, Read};
use std::path::PathBuf;
use std::process::{Child, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

use axidps_protocol::v2::{AgentKind, Event, NAME_CHUNK_MAX, NameField};
use axidps_ringbuf::v2::EventWriter;

fn tmp(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("axidps-bin-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn name(id: u32, field: NameField, s: &str) -> Event {
    let mut bytes = [0u8; NAME_CHUNK_MAX];
    bytes[..s.len()].copy_from_slice(s.as_bytes());
    Event::AgentName {
        id,
        field,
        offset: 0,
        total: s.len() as u8,
        len: s.len() as u8,
        bytes,
    }
}

fn zevtc_count(dir: &std::path::Path) -> usize {
    std::fs::read_dir(dir)
        .map(|d| {
            d.filter_map(Result::ok)
                .filter(|e| e.path().extension().is_some_and(|x| x == "zevtc"))
                .count()
        })
        .unwrap_or(0)
}

fn run_bin(ring: &std::path::Path, out: &std::path::Path) -> Child {
    Command::new(env!("CARGO_BIN_EXE_axidps-core"))
        .args([
            "--ring",
            ring.to_str().unwrap(),
            "--log-dir",
            out.to_str().unwrap(),
        ])
        .args(["--exit-when-idle", "--min-squad", "5"])
        .arg("--live-out")
        .arg(live_out(out))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}

/// The live feed path `run_bin` uses: next to the log folder, never the real `/dev/shm` one.
fn live_out(out: &std::path::Path) -> PathBuf {
    out.parent().unwrap().join("live.json")
}

/// Wait for the bin to exit (with a deadline) and return (success, stdout, stderr).
fn finish(mut child: Child, mut stdout: BufReader<ChildStdout>) -> (bool, String, String) {
    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break s;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("axidps-core did not exit");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let mut o = String::new();
    stdout.read_to_string(&mut o).unwrap();
    let mut e = String::new();
    child.stderr.take().unwrap().read_to_string(&mut e).unwrap();
    (status.success(), o, e)
}

/// A whole fight: squad of 6 joins at 1 s, fights 2 s..14 s.
fn push_fight(w: &EventWriter) {
    push_fight_events(w, true);
}

/// The fight's events; without `end`, the fight is still in progress at the last one.
fn push_fight_events(w: &EventWriter, end: bool) {
    for id in 1..=6u32 {
        w.push(
            1000,
            &Event::AgentAdd {
                id,
                kind: AgentKind::Player,
                prof: 1,
                elite: 0,
                species: 0,
                team: 1,
                subgroup: 1,
                wvw_rank: 0,
                wvw_rank_def: 0,
            },
        );
        w.push(1000, &name(id, NameField::Character, &format!("Char {id}")));
        w.push(
            1000,
            &name(id, NameField::Account, &format!(":Alpha.{id:04}")),
        );
    }
    w.push(
        2000,
        &Event::Combat {
            id: 1,
            in_combat: true,
        },
    );
    w.push(
        8000,
        &Event::Strike {
            src: 1,
            dst: 2,
            skill: 1,
            value: 10,
            overstack: 0,
            result: 0,
        },
    );
    if end {
        w.push(
            14000,
            &Event::Combat {
                id: 1,
                in_combat: false,
            },
        );
    } else {
        w.push(
            14000,
            &Event::Strike {
                src: 1,
                dst: 2,
                skill: 1,
                value: 10,
                overstack: 0,
                result: 0,
            },
        );
    }
}

#[test]
fn reopen_does_not_rewrite_fights() {
    let dir = tmp("reopen");
    let ring = dir.join("ring.shm");
    let out = dir.join("logs");
    let w = EventWriter::create_at(&ring).unwrap();
    w.push(
        0,
        &Event::Status {
            unix_s: 1_795_032_000,
            dll_version: 1,
            group_state: [0; 16],
        },
    );

    // Run 1: the fight is pushed after the bin attached, so it is live and gets written.
    let mut child = run_bin(&ring, &out);
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    stdout.read_line(&mut line).unwrap();
    assert!(line.starts_with("reading ring"), "first line: {line:?}");
    push_fight(&w);
    // Keep the writer (and so the file) alive; just clear the active flag.
    w.mark_inactive();
    let (ok, o, e) = finish(child, stdout);
    assert!(ok, "{e}");
    assert_eq!(zevtc_count(&out), 1, "stdout: {o}");
    // The live feed holds the fight that ended (2 s..14 s), its 6 squad members and 10 damage.
    let feed = std::fs::read_to_string(live_out(&out)).unwrap();
    assert!(feed.contains(r#""state":"last""#), "{feed}");
    assert!(feed.contains(r#""duration_ms":12000"#), "{feed}");
    assert_eq!(feed.matches(r#""account":":Alpha."#).count(), 6, "{feed}");
    assert!(feed.contains(r#""damage":10,"#), "{feed}");
    assert!(!dir.join("live.json.tmp").exists());

    // Run 2 on the same ring: the fight is history, replayed for state only.
    let mut child = run_bin(&ring, &out);
    let stdout = BufReader::new(child.stdout.take().unwrap());
    let (ok, o, e) = finish(child, stdout);
    drop(w);
    assert!(ok, "{e}");
    assert!(!o.contains("wrote"), "stdout: {o}");
    assert_eq!(zevtc_count(&out), 1, "stdout: {o}");
}

#[test]
// Windows always has a log folder (the Documents folder); only the Proton lookup can fail.
#[cfg(not(windows))]
fn bin_exits_2_without_log_dir() {
    let home = tmp("home");
    let o = Command::new(env!("CARGO_BIN_EXE_axidps-core"))
        .env("HOME", &home)
        .args(["--exit-when-idle"])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&o.stderr)
            .contains("no log folder: pass --log-dir or install GW2 under Steam/Proton")
    );
}

#[test]
// The Proton lookup is the only one that can fail (see above).
#[cfg(not(windows))]
fn bin_waits_for_log_dir_and_stops_on_the_stop_file() {
    let home = tmp("home-wait");
    let stop = home.join("core.stop");
    let mut child = Command::new(env!("CARGO_BIN_EXE_axidps-core"))
        .env("HOME", &home)
        .arg("--wait-for-log-dir")
        .arg("--stop-file")
        .arg(&stop)
        .arg("--live-out")
        .arg("none")
        .arg("--no-relay")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // Still waiting, not exited 2, after a while.
    std::thread::sleep(Duration::from_millis(1500));
    assert!(child.try_wait().unwrap().is_none(), "core exited while waiting");
    std::fs::write(&stop, b"").unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break s;
        }
        assert!(Instant::now() < deadline, "core ignored the stop file");
        std::thread::sleep(Duration::from_millis(50));
    };
    let mut e = String::new();
    child.stderr.take().unwrap().read_to_string(&mut e).unwrap();
    assert_eq!(status.code(), Some(0), "{e}");
    assert!(e.contains("waiting for one"), "{e}");
}

#[test]
fn failed_write_exits_1_when_idle() {
    let dir = tmp("writefail");
    let ring = dir.join("ring.shm");
    // The log folder is a regular file: every write fails.
    let out = dir.join("not-a-dir");
    std::fs::write(&out, b"x").unwrap();
    let w = EventWriter::create_at(&ring).unwrap();
    let mut child = run_bin(&ring, &out);
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    stdout.read_line(&mut line).unwrap();
    push_fight(&w);
    w.mark_inactive();
    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break s;
        }
        assert!(Instant::now() < deadline, "axidps-core did not exit");
        std::thread::sleep(Duration::from_millis(20));
    };
    let mut e = String::new();
    child.stderr.take().unwrap().read_to_string(&mut e).unwrap();
    assert_eq!(status.code(), Some(1), "stderr: {e}");
    assert!(e.contains("write failed"), "stderr: {e}");
}

/// SIGINT drains the ring and writes the fight in progress, then exits 0.
#[cfg(unix)]
#[test]
fn sigint_flushes_the_active_fight() {
    let dir = tmp("sigint");
    let ring = dir.join("ring.shm");
    let out = dir.join("logs");
    let w = EventWriter::create_at(&ring).unwrap();
    // No --exit-when-idle: only the signal ends this run.
    let mut child = Command::new(env!("CARGO_BIN_EXE_axidps-core"))
        .args([
            "--ring",
            ring.to_str().unwrap(),
            "--log-dir",
            out.to_str().unwrap(),
        ])
        .args(["--min-squad", "5"])
        .arg("--live-out")
        .arg(live_out(&out))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    stdout.read_line(&mut line).unwrap();
    assert!(line.starts_with("reading ring"), "first line: {line:?}");
    // Pushed after the bin attached, so live; the fight never ends on its own.
    push_fight_events(&w, false);
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(zevtc_count(&out), 0, "the fight must still be in progress");
    let killed = Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(killed.success());
    let (ok, o, e) = finish(child, stdout);
    drop(w);
    assert!(ok, "{e}");
    assert_eq!(zevtc_count(&out), 1, "stdout: {o}");
}

#[test]
fn says_which_ring_it_waits_for() {
    let dir = tmp("waiting");
    let ring = dir.join("no-such-ring");
    let mut child = Command::new(env!("CARGO_BIN_EXE_axidps-core"))
        .args([
            "--ring",
            ring.to_str().unwrap(),
            "--log-dir",
            dir.to_str().unwrap(),
            "--live-out",
            "none",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let _ = BufReader::new(stdout).read_line(&mut line);
        let _ = tx.send(line);
    });
    let line = rx.recv_timeout(Duration::from_secs(10)).unwrap_or_default();
    let _ = child.kill();
    let _ = child.wait();
    assert!(line.contains("waiting for the event ring at"), "{line:?}");
    assert!(line.contains(ring.to_str().unwrap()), "{line:?}");
    assert!(line.contains("--ring"), "{line:?}");
}

fn only_log(dir: &std::path::Path, ext: &str) -> PathBuf {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == ext))
        .collect();
    assert_eq!(v.len(), 1, "{ext} files in {}: {v:?}", dir.display());
    v.pop().unwrap()
}

#[test]
fn replay_of_a_recording_writes_the_same_log() {
    let dir = tmp("replay");
    let ring = dir.join("ring.shm");
    let live = dir.join("live");
    let again = dir.join("again");
    let recs = dir.join("recs");
    let w = EventWriter::create_at(&ring).unwrap();
    w.push(
        0,
        &Event::Status {
            unix_s: 1_795_032_000,
            dll_version: 1,
            group_state: [0; 16],
        },
    );

    let mut child = Command::new(env!("CARGO_BIN_EXE_axidps-core"))
        .args(["--ring", ring.to_str().unwrap()])
        .args(["--log-dir", live.to_str().unwrap()])
        .args(["--record", recs.to_str().unwrap()])
        .args(["--exit-when-idle", "--min-squad", "5"])
        .args(["--live-out", "none"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    stdout.read_line(&mut line).unwrap();
    assert!(line.starts_with("recording to"), "first line: {line:?}");
    line.clear();
    stdout.read_line(&mut line).unwrap();
    assert!(line.starts_with("reading ring"), "second line: {line:?}");
    push_fight(&w);
    w.mark_inactive();
    let (ok, o, e) = finish(child, stdout);
    drop(w);
    assert!(ok, "{e}");

    let rec = only_log(&recs, "axrec");
    let o2 = Command::new(env!("CARGO_BIN_EXE_axidps-core"))
        .args(["--replay", rec.to_str().unwrap()])
        .args(["--log-dir", again.to_str().unwrap()])
        .args(["--min-squad", "5"])
        .output()
        .unwrap();
    assert!(
        o2.status.success(),
        "{}",
        String::from_utf8_lossy(&o2.stderr)
    );

    let read = |p: PathBuf| axidps_evtc::unzevtc(&std::fs::read(p).unwrap()).unwrap();
    let a = read(only_log(&live, "zevtc"));
    let b = read(only_log(&again, "zevtc"));
    assert!(a == b, "replayed log differs (live stdout: {o})");
}

#[test]
fn replay_rejects_a_file_that_is_not_a_recording() {
    let dir = tmp("replaybad");
    let f = dir.join("x.axrec");
    std::fs::write(&f, b"nope").unwrap();
    let o = Command::new(env!("CARGO_BIN_EXE_axidps-core"))
        .args([
            "--replay",
            f.to_str().unwrap(),
            "--log-dir",
            dir.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(2));
}
