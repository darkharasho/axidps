//! Relay client: one background thread owns the WebSocket to the relay room, so core's
//! processing loop only ever touches two channels and never blocks on the network.
//!
//! The thread is connected only while there is a room, a known account, and a fight on (or
//! a fight ended / a heal was sent / a peer batch arrived less than `Timing::idle` ago). It
//! sends hello first, then at most one batch per `Timing::batch`, each under the server's
//! size limit. Heals and condition ticks wait at most `Timing::buffer` for a socket. Lost connections are retried
//! with backoff 1 s doubling to `Timing::backoff_max` while still wanted.

use std::collections::VecDeque;
use std::io::ErrorKind;
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use tungstenite::stream::MaybeTlsStream;
use tungstenite::{HandshakeError, Message, WebSocket};

use super::wire;
pub use super::wire::{Batch, WireCombat, WireCondi, WireHeal};

/// The `client` field of hello.
pub const CLIENT: &str = concat!("axidps-core/", env!("CARGO_PKG_VERSION"));
/// The server closes a socket on a larger message.
pub const MAX_MESSAGE: usize = 16384;

const READ_TIMEOUT: Duration = Duration::from_millis(50);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const BACKOFF_MIN: Duration = Duration::from_secs(1);
const CLOSE_WAIT: Duration = Duration::from_millis(500);

/// Commands from core's loop to the relay thread.
#[derive(Debug, Clone)]
pub enum Cmd {
    /// The squad/party room id; `None` for no group.
    Room(Option<String>),
    /// The POV's account name.
    Account(String),
    /// A fight started (`true`) or ended (`false`).
    Active(bool),
    /// A local heal to share.
    Heal(WireHeal),
    /// A condition tick the POV (or its minion) dealt, to share.
    Condi(WireCondi),
    /// The POV entered or left combat. The latest state is sent again on every connect, so
    /// peers who join later still know it.
    Combat(WireCombat),
    /// Close the socket and end the thread.
    Stop,
}

#[derive(Debug, Clone, Copy)]
pub struct Timing {
    /// Close after this long with no fight and no relay traffic.
    pub idle: Duration,
    /// Send at most one batch this often.
    pub batch: Duration,
    /// Drop heals that waited this long for a socket.
    pub buffer: Duration,
    /// Reconnect backoff ceiling (starts at 1 s, or this if lower).
    pub backoff_max: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            idle: Duration::from_secs(60),
            batch: Duration::from_millis(250),
            buffer: Duration::from_secs(5),
            backoff_max: Duration::from_secs(30),
        }
    }
}

/// Handle to the relay thread. Dropping it stops the thread.
pub struct Relay {
    tx: Sender<Cmd>,
    rx: Receiver<Batch>,
}

impl Relay {
    /// Starts the relay thread for `base_url` (`wss://host` or `ws://host:port`).
    pub fn start(base_url: String) -> Relay {
        Self::start_with(base_url, Timing::default())
    }

    pub fn start_with(base_url: String, timing: Timing) -> Relay {
        let (tx, cmds) = mpsc::channel();
        let (out, rx) = mpsc::channel();
        let worker = Worker::new(base_url, timing, cmds, out);
        // Without a thread the relay simply stays silent.
        let _ = thread::Builder::new()
            .name("relay".into())
            .spawn(move || worker.run());
        Relay { tx, rx }
    }

    pub fn send(&self, c: Cmd) {
        let _ = self.tx.send(c);
    }

    /// Peer batches received since the last call. Never blocks.
    pub fn try_recv(&self) -> Vec<Batch> {
        self.rx.try_iter().collect()
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        let _ = self.tx.send(Cmd::Stop);
    }
}

type Ws = WebSocket<MaybeTlsStream<TcpStream>>;

struct Conn {
    ws: Ws,
    opened: Instant,
    last_batch: Instant,
}

struct Worker {
    base: String,
    timing: Timing,
    cmds: Receiver<Cmd>,
    out: Sender<Batch>,
    room: Option<String>,
    account: Option<String>,
    active: bool,
    last_activity: Option<Instant>,
    pending: VecDeque<(Instant, Item)>,
    /// Combat changes not sent yet, and the latest one.
    combat: Vec<WireCombat>,
    last_combat: Option<WireCombat>,
    conn: Option<Conn>,
    backoff: Duration,
    next_attempt: Instant,
}

/// A heal or condition tick waiting to be sent.
#[derive(Debug, Clone)]
enum Item {
    Heal(WireHeal),
    Condi(WireCondi),
}

impl Item {
    /// Its size in a batch's list, with the separating comma.
    fn size(&self) -> usize {
        1 + match self {
            Item::Heal(h) => h.to_json().to_string().len(),
            Item::Condi(c) => c.to_json().to_string().len(),
        }
    }
}

fn short(id: &str) -> &str {
    id.get(..8).unwrap_or(id)
}

fn fmt_delay(d: Duration) -> String {
    if d >= Duration::from_secs(1) {
        format!("{} s", d.as_secs())
    } else {
        format!("{} ms", d.as_millis())
    }
}

fn would_block(e: &tungstenite::Error) -> bool {
    matches!(e, tungstenite::Error::Io(e)
        if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut))
}

impl Worker {
    fn new(base: String, timing: Timing, cmds: Receiver<Cmd>, out: Sender<Batch>) -> Self {
        Self {
            base,
            timing,
            cmds,
            out,
            room: None,
            account: None,
            active: false,
            last_activity: None,
            pending: VecDeque::new(),
            combat: Vec::new(),
            last_combat: None,
            conn: None,
            backoff: BACKOFF_MIN.min(timing.backoff_max),
            next_attempt: Instant::now(),
        }
    }

    fn run(mut self) {
        while self.drain_cmds() {
            let now = Instant::now();
            while self
                .pending
                .front()
                .is_some_and(|(t, _)| now.duration_since(*t) > self.timing.buffer)
            {
                self.pending.pop_front();
            }
            if !self.wanted(now) {
                if self.conn.is_some() {
                    self.close("idle");
                }
                self.backoff = BACKOFF_MIN.min(self.timing.backoff_max);
                self.next_attempt = now;
            } else if self.conn.is_none() {
                if now >= self.next_attempt {
                    self.connect();
                }
            } else {
                self.step(now);
            }
        }
        self.close("stopped");
    }

    fn wanted(&self, now: Instant) -> bool {
        self.room.is_some()
            && self.account.is_some()
            && (self.active
                || self
                    .last_activity
                    .is_some_and(|t| now.duration_since(t) < self.timing.idle))
    }

    /// Applies queued commands; waits up to the read timeout for one when there is no
    /// socket to pace the loop. `false` on Stop or when the handle is gone.
    fn drain_cmds(&mut self) -> bool {
        if self.conn.is_none() {
            match self.cmds.recv_timeout(READ_TIMEOUT) {
                Ok(c) => {
                    if !self.apply(c) {
                        return false;
                    }
                }
                Err(RecvTimeoutError::Timeout) => return true,
                Err(RecvTimeoutError::Disconnected) => return false,
            }
        }
        loop {
            match self.cmds.try_recv() {
                Ok(c) => {
                    if !self.apply(c) {
                        return false;
                    }
                }
                Err(mpsc::TryRecvError::Empty) => return true,
                Err(mpsc::TryRecvError::Disconnected) => return false,
            }
        }
    }

    fn apply(&mut self, c: Cmd) -> bool {
        let now = Instant::now();
        match c {
            Cmd::Room(r) => {
                if r != self.room {
                    if self.conn.is_some() {
                        self.close(if r.is_some() {
                            "room change"
                        } else {
                            "no group"
                        });
                    }
                    self.room = r;
                    // Heals queued for the old group never go to the new one.
                    self.pending.clear();
                    self.next_attempt = now;
                }
            }
            Cmd::Account(a) => {
                if self.account.as_ref() != Some(&a) {
                    if self.conn.is_some() {
                        self.close("account change");
                    }
                    // Only replacing another account drops queued heals; the first account
                    // keeps what the pre-connect buffer already holds.
                    if self.account.is_some() {
                        self.pending.clear();
                    }
                    self.account = Some(a);
                    self.next_attempt = now;
                }
            }
            Cmd::Active(on) => {
                if on || self.active {
                    self.last_activity = Some(now);
                }
                self.active = on;
            }
            Cmd::Heal(h) => {
                self.last_activity = Some(now);
                self.pending.push_back((now, Item::Heal(h)));
            }
            Cmd::Condi(c) => {
                self.last_activity = Some(now);
                self.pending.push_back((now, Item::Condi(c)));
            }
            Cmd::Combat(c) => {
                self.last_combat = Some(c);
                self.combat.push(c);
            }
            Cmd::Stop => return false,
        }
        true
    }

    fn connect(&mut self) {
        let (Some(room), Some(account)) = (self.room.clone(), self.account.clone()) else {
            return;
        };
        let url = format!("{}/v1/room/{room}", self.base.trim_end_matches('/'));
        let opened = open(&url).and_then(|mut ws| {
            ws.send(Message::text(wire::hello(&account, CLIENT)))
                .map_err(|e| e.to_string())?;
            Ok(ws)
        });
        match opened {
            Ok(ws) => {
                eprintln!("relay: connected to room {}", short(&room));
                // A new room's peers know nothing yet: the current state is enough.
                self.combat.clear();
                self.combat.extend(self.last_combat);
                let now = Instant::now();
                self.conn = Some(Conn {
                    ws,
                    opened: now,
                    // Anything buffered while connecting goes out at once.
                    last_batch: now.checked_sub(self.timing.batch).unwrap_or(now),
                });
            }
            Err(e) => {
                let delay = self.schedule_retry();
                eprintln!(
                    "relay: cannot connect: {}; retrying in {}",
                    e.replace(&room, short(&room)),
                    fmt_delay(delay)
                );
            }
        }
    }

    fn schedule_retry(&mut self) -> Duration {
        let delay = self.backoff;
        self.next_attempt = Instant::now() + delay;
        self.backoff = (self.backoff * 2).min(self.timing.backoff_max);
        delay
    }

    /// One connected iteration: maybe send a batch, then read for up to the read timeout.
    fn step(&mut self, now: Instant) {
        let Some(conn) = self.conn.as_mut() else {
            return;
        };
        let mut result = Ok(());
        if (!self.pending.is_empty() || !self.combat.is_empty())
            && now.duration_since(conn.last_batch) >= self.timing.batch
        {
            conn.last_batch = now;
            let from = self.account.as_deref().unwrap_or_default();
            let combat = std::mem::take(&mut self.combat);
            if let Some(text) = take_batch(from, &mut self.pending, &combat) {
                result = conn.ws.send(Message::text(text));
            }
        }
        if result.is_ok() {
            match conn.ws.read() {
                Ok(Message::Text(t)) => {
                    if let Some(b) = wire::parse(&t) {
                        self.last_activity = Some(now);
                        let _ = self.out.send(b);
                    }
                }
                Ok(_) => {}
                Err(e) if would_block(&e) => {}
                Err(e) => result = Err(e),
            }
        }
        if let Err(e) = result {
            let lived = conn.opened.elapsed();
            self.conn = None;
            if lived >= self.timing.backoff_max {
                self.backoff = BACKOFF_MIN.min(self.timing.backoff_max);
            }
            let delay = self.schedule_retry();
            eprintln!(
                "relay: connection lost: {e}; retrying in {}",
                fmt_delay(delay)
            );
        }
    }

    fn close(&mut self, reason: &str) {
        let Some(mut c) = self.conn.take() else {
            return;
        };
        let _ = c.ws.close(None);
        let end = Instant::now() + CLOSE_WAIT;
        while Instant::now() < end {
            match c.ws.read() {
                Err(e) if !would_block(&e) => break,
                _ => {}
            }
        }
        eprintln!("relay: closed ({reason})");
    }
}

/// Removes from the front of `pending` as many heals and condition ticks as fit in one message
/// beside `combat` and returns that message. An item too large to send at all is dropped.
fn take_batch(
    from: &str,
    pending: &mut VecDeque<(Instant, Item)>,
    combat: &[WireCombat],
) -> Option<String> {
    // Room for the `"condi":[]` key the first tick adds.
    const CONDI_KEY: usize = 12;
    let mut size = wire::batch(from, &[], &[], combat).len() + CONDI_KEY;
    let mut n = 0;
    for (_, item) in pending.iter() {
        let add = item.size();
        if size + add > MAX_MESSAGE {
            break;
        }
        size += add;
        n += 1;
    }
    if n == 0 {
        // Nothing fits (or nothing is waiting): an item too large is dropped, combat goes alone.
        pending.pop_front();
        return (!combat.is_empty()).then(|| wire::batch(from, &[], &[], combat));
    }
    let (mut heals, mut condi) = (Vec::new(), Vec::new());
    for (_, item) in pending.drain(..n) {
        match item {
            Item::Heal(h) => heals.push(h),
            Item::Condi(c) => condi.push(c),
        }
    }
    Some(wire::batch(from, &heals, &condi, combat))
}

/// Opens the socket with bounded connect/handshake times, then switches to the short read
/// timeout the thread loop relies on.
fn open(url: &str) -> Result<Ws, String> {
    let uri: tungstenite::http::Uri = url.parse().map_err(|e| format!("bad relay url: {e}"))?;
    let port = match uri.scheme_str() {
        Some("wss") => 443,
        Some("ws") => 80,
        _ => return Err("relay url must start with ws:// or wss://".into()),
    };
    let port = uri.port_u16().unwrap_or(port);
    let host = uri.host().ok_or("relay url has no host")?;
    let host = host.trim_start_matches('[').trim_end_matches(']');
    let mut last = String::from("no address");
    let mut stream = None;
    for addr in (host, port).to_socket_addrs().map_err(|e| e.to_string())? {
        match TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT) {
            Ok(s) => {
                stream = Some(s);
                break;
            }
            Err(e) => last = e.to_string(),
        }
    }
    let stream = stream.ok_or(last)?;
    let _ = stream.set_nodelay(true);
    stream
        .set_read_timeout(Some(CONNECT_TIMEOUT))
        .and_then(|()| stream.set_write_timeout(Some(CONNECT_TIMEOUT)))
        .map_err(|e| e.to_string())?;
    let (ws, _) = tungstenite::client_tls_with_config(url, stream, Some(ws_config()), None)
        .map_err(|e| match e {
            HandshakeError::Failure(e) => e.to_string(),
            HandshakeError::Interrupted(_) => "handshake timed out".into(),
        })?;
    let tcp = match ws.get_ref() {
        MaybeTlsStream::Plain(s) => s,
        MaybeTlsStream::Rustls(s) => s.get_ref(),
        _ => return Err("unsupported stream".into()),
    };
    tcp.set_read_timeout(Some(READ_TIMEOUT))
        .map_err(|e| e.to_string())?;
    Ok(ws)
}

/// Frames and messages from the relay are refused well before tungstenite's defaults (16 MiB
/// frames, 64 MiB messages): ours are at most [`MAX_MESSAGE`], and a relay sending more is
/// misbehaving.
fn ws_config() -> tungstenite::protocol::WebSocketConfig {
    tungstenite::protocol::WebSocketConfig::default()
        .max_message_size(Some(4 * MAX_MESSAGE))
        .max_frame_size(Some(4 * MAX_MESSAGE))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relay::wire::{self, Endpoint};
    use std::net::{TcpListener, TcpStream};
    use std::sync::mpsc::{self, Receiver, Sender};
    use std::thread;
    use std::time::{Duration, Instant};
    use tungstenite::Message;
    use tungstenite::handshake::server::{Request, Response};

    const ROOM_A: &str = "1a2b3c4d00000000000000000000000000000000000000000000000000000000";
    const ROOM_B: &str = "5e6f7a8b00000000000000000000000000000000000000000000000000000000";
    const WAIT: Duration = Duration::from_secs(3);
    /// Pushed to the test server: close the socket from the server side.
    const CLOSE: &str = "\0close";

    #[derive(Debug, PartialEq)]
    enum Ev {
        Open(String),
        Text(String),
        Closed,
    }

    struct Server {
        port: u16,
        events: Receiver<Ev>,
        push: Sender<String>,
    }

    impl Server {
        fn start() -> Server {
            Self::on(TcpListener::bind("127.0.0.1:0").unwrap())
        }

        fn on(listener: TcpListener) -> Server {
            let port = listener.local_addr().unwrap().port();
            let (ev_tx, events) = mpsc::channel();
            let (push, push_rx) = mpsc::channel::<String>();
            thread::spawn(move || {
                for stream in listener.incoming() {
                    let Ok(stream) = stream else { break };
                    serve(stream, &ev_tx, &push_rx);
                }
            });
            Server { port, events, push }
        }

        fn base(&self) -> String {
            format!("ws://127.0.0.1:{}", self.port)
        }

        fn next(&self) -> Ev {
            self.events.recv_timeout(WAIT).expect("server event")
        }
    }

    fn serve(stream: TcpStream, ev: &Sender<Ev>, push: &Receiver<String>) {
        let mut path = String::new();
        // The callback's error type is tungstenite's, not ours.
        #[allow(clippy::result_large_err)]
        let cb = |req: &Request, resp: Response| {
            path = req.uri().path().to_string();
            Ok(resp)
        };
        let Ok(mut ws) = tungstenite::accept_hdr(stream, cb) else {
            return;
        };
        ws.get_ref()
            .set_read_timeout(Some(Duration::from_millis(10)))
            .unwrap();
        let _ = ev.send(Ev::Open(path));
        loop {
            while let Ok(t) = push.try_recv() {
                let _ = if t == CLOSE {
                    ws.close(None)
                } else {
                    ws.send(Message::text(t))
                };
            }
            match ws.read() {
                Ok(Message::Text(t)) => {
                    let _ = ev.send(Ev::Text(t.to_string()));
                }
                Ok(_) => {}
                Err(tungstenite::Error::Io(e))
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) => {}
                Err(_) => {
                    let _ = ev.send(Ev::Closed);
                    return;
                }
            }
        }
    }

    fn fast() -> Timing {
        Timing {
            idle: Duration::from_millis(200),
            batch: Duration::from_millis(20),
            buffer: Duration::from_secs(5),
            backoff_max: Duration::from_millis(100),
        }
    }

    fn heal(t: u64) -> WireHeal {
        WireHeal {
            t_unix_ms: t,
            src: Endpoint::Player(":a.1".into()),
            dst: Endpoint::Player(":b.2".into()),
            skill: 5,
            heal: 100,
            barrier: 0,
            buff: false,
            downed: false,
        }
    }

    fn join(r: &Relay, room: &str) {
        r.send(Cmd::Account(":a.1".into()));
        r.send(Cmd::Room(Some(room.into())));
        r.send(Cmd::Active(true));
    }

    #[test]
    fn default_timing_matches_constraints() {
        let t = Timing::default();
        assert_eq!(t.idle, Duration::from_secs(60));
        assert_eq!(t.batch, Duration::from_millis(250));
        assert_eq!(t.buffer, Duration::from_secs(5));
        assert_eq!(t.backoff_max, Duration::from_secs(30));
    }

    #[test]
    fn tls_has_a_crypto_provider() {
        // tungstenite builds its default rustls config with `ClientConfig::builder()`,
        // which panics without a provider.
        let _ = rustls::ClientConfig::builder();
    }

    #[test]
    fn connects_and_sends_hello_then_batch() {
        let s = Server::start();
        let r = Relay::start_with(s.base(), fast());
        r.send(Cmd::Account(":a.1".into()));
        r.send(Cmd::Room(Some(ROOM_A.into())));
        // A heal alone starts the connection and is buffered until it is open.
        r.send(Cmd::Heal(heal(1)));
        assert_eq!(s.next(), Ev::Open(format!("/v1/room/{ROOM_A}")));
        assert_eq!(s.next(), Ev::Text(wire::hello(":a.1", CLIENT)));
        assert_eq!(
            s.next(),
            Ev::Text(wire::batch(":a.1", &[heal(1)], &[], &[]))
        );
        r.send(Cmd::Heal(heal(2)));
        r.send(Cmd::Heal(heal(3)));
        let Ev::Text(t) = s.next() else { panic!() };
        let b = wire::parse(&t).unwrap();
        assert_eq!(b.from, ":a.1");
        let mut got = b.heals;
        if got.len() < 2 {
            let Ev::Text(t) = s.next() else { panic!() };
            got.extend(wire::parse(&t).unwrap().heals);
        }
        assert_eq!(got, vec![heal(2), heal(3)]);
        r.send(Cmd::Stop);
        assert_eq!(s.next(), Ev::Closed);
    }

    // The POV's combat goes out on its own, and its latest state again after a reconnect.
    #[test]
    fn sends_combat_and_repeats_the_latest_on_connect() {
        let s = Server::start();
        let r = Relay::start_with(s.base(), fast());
        let enter = WireCombat {
            t_unix_ms: 5,
            in_combat: true,
        };
        r.send(Cmd::Combat(enter));
        join(&r, ROOM_A);
        assert!(matches!(s.next(), Ev::Open(_)));
        assert!(matches!(s.next(), Ev::Text(_))); // hello
        assert_eq!(s.next(), Ev::Text(wire::batch(":a.1", &[], &[], &[enter])));
        r.send(Cmd::Room(Some(ROOM_B.into())));
        assert!(matches!(s.next(), Ev::Closed));
        assert!(matches!(s.next(), Ev::Open(_)));
        assert!(matches!(s.next(), Ev::Text(_))); // hello
        assert_eq!(s.next(), Ev::Text(wire::batch(":a.1", &[], &[], &[enter])));
    }

    #[test]
    fn receives_peer_batches() {
        let s = Server::start();
        let r = Relay::start_with(s.base(), fast());
        join(&r, ROOM_A);
        assert!(matches!(s.next(), Ev::Open(_)));
        assert!(matches!(s.next(), Ev::Text(_))); // hello
        s.push
            .send(wire::batch(":b.2", &[heal(7)], &[], &[]))
            .unwrap();
        s.push.send("not json".into()).unwrap();
        let end = Instant::now() + WAIT;
        let mut got = Vec::new();
        while got.is_empty() && Instant::now() < end {
            got = r.try_recv();
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            got,
            vec![Batch {
                from: ":b.2".to_string(),
                heals: vec![heal(7)],
                condi: vec![],
                combat: vec![]
            }]
        );
        thread::sleep(Duration::from_millis(100));
        assert!(r.try_recv().is_empty());
    }

    #[test]
    fn room_change_reconnects() {
        let s = Server::start();
        let r = Relay::start_with(s.base(), fast());
        join(&r, ROOM_A);
        assert_eq!(s.next(), Ev::Open(format!("/v1/room/{ROOM_A}")));
        assert!(matches!(s.next(), Ev::Text(_)));
        r.send(Cmd::Room(Some(ROOM_B.into())));
        assert_eq!(s.next(), Ev::Closed);
        assert_eq!(s.next(), Ev::Open(format!("/v1/room/{ROOM_B}")));
        assert_eq!(s.next(), Ev::Text(wire::hello(":a.1", CLIENT)));
        r.send(Cmd::Room(None));
        assert_eq!(s.next(), Ev::Closed);
        assert!(s.events.recv_timeout(Duration::from_millis(300)).is_err());
    }

    #[test]
    fn idle_closes_after_timeout() {
        let s = Server::start();
        let r = Relay::start_with(s.base(), fast());
        join(&r, ROOM_A);
        assert!(matches!(s.next(), Ev::Open(_)));
        assert!(matches!(s.next(), Ev::Text(_)));
        // Still open while the fight is on, well past the idle time.
        thread::sleep(Duration::from_millis(400));
        r.send(Cmd::Active(false));
        let ended = Instant::now();
        assert_eq!(s.next(), Ev::Closed);
        assert!(ended.elapsed() >= Duration::from_millis(150));
        assert!(s.events.recv_timeout(Duration::from_millis(300)).is_err());
    }

    #[test]
    fn server_gone_backs_off() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        drop(l);
        let r = Relay::start_with(format!("ws://127.0.0.1:{port}"), fast());
        join(&r, ROOM_A);
        thread::sleep(Duration::from_millis(400));
        assert!(r.try_recv().is_empty());
        let s = Server::on(TcpListener::bind(("127.0.0.1", port)).unwrap());
        assert_eq!(s.next(), Ev::Open(format!("/v1/room/{ROOM_A}")));
    }

    #[test]
    fn room_change_drops_queued_heals() {
        // Nothing listens yet, so the heal queues while the socket is down.
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        drop(l);
        let r = Relay::start_with(format!("ws://127.0.0.1:{port}"), fast());
        r.send(Cmd::Account(":a.1".into()));
        r.send(Cmd::Room(Some(ROOM_A.into())));
        r.send(Cmd::Heal(heal(1)));
        thread::sleep(Duration::from_millis(100));
        r.send(Cmd::Room(Some(ROOM_B.into())));
        r.send(Cmd::Active(true));
        thread::sleep(Duration::from_millis(100));
        let s = Server::on(TcpListener::bind(("127.0.0.1", port)).unwrap());
        // Windows retries a refused loopback connect for about 2 s, so the connect to room A
        // can still be pending when the server starts and land in A. The client then leaves A
        // for B before sending anything but its hello: the heal queued for A never goes out.
        let mut ev = s.next();
        if ev == Ev::Open(format!("/v1/room/{ROOM_A}")) {
            assert_eq!(s.next(), Ev::Text(wire::hello(":a.1", CLIENT)));
            assert_eq!(s.next(), Ev::Closed);
            ev = s.next();
        }
        assert_eq!(ev, Ev::Open(format!("/v1/room/{ROOM_B}")));
        assert_eq!(s.next(), Ev::Text(wire::hello(":a.1", CLIENT)));
        r.send(Cmd::Heal(heal(2)));
        assert_eq!(
            s.next(),
            Ev::Text(wire::batch(":a.1", &[heal(2)], &[], &[]))
        );
    }

    #[test]
    fn first_account_keeps_queued_heals() {
        let s = Server::start();
        let r = Relay::start_with(s.base(), fast());
        r.send(Cmd::Room(Some(ROOM_A.into())));
        r.send(Cmd::Heal(heal(1)));
        r.send(Cmd::Account(":a.1".into()));
        assert_eq!(s.next(), Ev::Open(format!("/v1/room/{ROOM_A}")));
        assert_eq!(s.next(), Ev::Text(wire::hello(":a.1", CLIENT)));
        assert_eq!(
            s.next(),
            Ev::Text(wire::batch(":a.1", &[heal(1)], &[], &[]))
        );
    }

    /// A message far beyond our own size limit ends the connection instead of being buffered
    /// and parsed (tungstenite's defaults allow 64 MiB).
    #[test]
    fn oversized_message_drops_the_connection() {
        let s = Server::start();
        let r = Relay::start_with(s.base(), fast());
        join(&r, ROOM_A);
        assert!(matches!(s.next(), Ev::Open(_)));
        assert!(matches!(s.next(), Ev::Text(_)));
        s.push.send("x".repeat(1 << 20)).unwrap();
        assert_eq!(s.next(), Ev::Closed);
        assert_eq!(s.next(), Ev::Open(format!("/v1/room/{ROOM_A}")));
    }

    #[test]
    fn server_close_reconnects() {
        let s = Server::start();
        let r = Relay::start_with(s.base(), fast());
        join(&r, ROOM_A);
        assert!(matches!(s.next(), Ev::Open(_)));
        assert!(matches!(s.next(), Ev::Text(_)));
        s.push.send(CLOSE.into()).unwrap();
        assert_eq!(s.next(), Ev::Closed);
        assert_eq!(s.next(), Ev::Open(format!("/v1/room/{ROOM_A}")));
        assert_eq!(s.next(), Ev::Text(wire::hello(":a.1", CLIENT)));
    }

    #[test]
    fn big_batches_split_under_the_size_limit() {
        let s = Server::start();
        let r = Relay::start_with(s.base(), fast());
        join(&r, ROOM_A);
        assert!(matches!(s.next(), Ev::Open(_)));
        assert!(matches!(s.next(), Ev::Text(_)));
        let want: Vec<WireHeal> = (0..1000).map(|t| heal(1_700_000_000_000 + t)).collect();
        assert!(wire::batch(":a.1", &want, &[], &[]).len() > MAX_MESSAGE);
        for h in &want {
            r.send(Cmd::Heal(h.clone()));
        }
        let mut got = Vec::new();
        let mut msgs = 0;
        while got.len() < want.len() {
            let Ev::Text(t) = s.next() else { panic!() };
            assert!(t.len() <= MAX_MESSAGE);
            got.extend(wire::parse(&t).unwrap().heals);
            msgs += 1;
        }
        assert_eq!(got, want);
        assert!(msgs >= 2);
    }

    // Heals and condition ticks share the queue and the size limit, in order.
    #[test]
    fn condi_ticks_go_out_with_heals() {
        let s = Server::start();
        let r = Relay::start_with(s.base(), fast());
        join(&r, ROOM_A);
        assert!(matches!(s.next(), Ev::Open(_)));
        assert!(matches!(s.next(), Ev::Text(_)));
        let tick = |t: u64| WireCondi {
            t_unix_ms: t,
            src: Endpoint::Player(":a.1".into()),
            dst: Endpoint::Foe {
                prof: 2,
                elite: 18,
                team: 708,
                xyz: [100_000, -20_000, 3_000],
            },
            buff: 736,
            damage: 1234,
            barrier: 56,
        };
        let want: Vec<WireCondi> = (0..600).map(|t| tick(1_700_000_000_000 + t)).collect();
        r.send(Cmd::Heal(heal(1)));
        for c in &want {
            r.send(Cmd::Condi(c.clone()));
        }
        let (mut heals, mut got, mut msgs) = (Vec::new(), Vec::new(), 0);
        while got.len() < want.len() {
            let Ev::Text(t) = s.next() else { panic!() };
            assert!(t.len() <= MAX_MESSAGE, "{}", t.len());
            let b = wire::parse(&t).unwrap();
            heals.extend(b.heals);
            got.extend(b.condi);
            msgs += 1;
        }
        assert_eq!(heals, vec![heal(1)]);
        assert_eq!(got, want);
        assert!(msgs >= 2);
    }

    #[test]
    fn inactive_never_connects() {
        let s = Server::start();
        let r = Relay::start_with(s.base(), fast());
        r.send(Cmd::Account(":a.1".into()));
        r.send(Cmd::Room(Some(ROOM_A.into())));
        r.send(Cmd::Active(false));
        assert!(s.events.recv_timeout(Duration::from_millis(400)).is_err());
        // No account yet: still nothing even when active.
        let s2 = Server::start();
        let r2 = Relay::start_with(s2.base(), fast());
        r2.send(Cmd::Room(Some(ROOM_A.into())));
        r2.send(Cmd::Active(true));
        assert!(s2.events.recv_timeout(Duration::from_millis(300)).is_err());
        drop(r);
    }
}
