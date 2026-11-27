//! axidps overlay host: Servo renders the in-game UI page offscreen and publishes frames to the
//! game through a shared file (`/dev/shm/axidps-overlay` on Linux, the temp folder on Windows),
//! which the DLL (feature `overlay`, `src/overlay.rs`) maps (as `Z:\dev\shm\axidps-overlay`
//! under Proton), uploads to a texture and draws over the game. Mouse input
//! comes back through a ring in the same file. Live numbers come from core's
//! `axidps-live.json` beside it, pushed into the page as `axidps.update(json)`.
//!
//! Shared layout v3: `crates/axidps-overlay-shm` (shared with the DLL). The page lays its
//! windows out side by side in a 2048x2048 atlas and reports them with
//! `console.log("axidps:<token>:" + JSON)`; only those rects are read back and published, with a window
//! table the DLL draws from. Layout and page settings persist in `~/.config/axidps/ui.json`.
//! While the DLL heartbeat is stale (no game) nothing is painted.

mod scheme;

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context, anyhow};
use servo::{
    ConsoleLogLevel, DeviceIntRect, DeviceIntSize, DevicePoint, EventLoopWaker, InputEvent,
    LoadStatus, MouseButton, MouseButtonAction, MouseButtonEvent, MouseMoveEvent,
    OffscreenRenderingContext, RenderingContext, Servo, ServoBuilder, ServoDelegate, ServoError,
    SoftwareRenderingContext, WebView, WebViewBuilder, WebViewDelegate, WheelDelta, WheelEvent,
    WheelMode, WindowRenderingContext,
};
use url::Url;
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
#[cfg(target_os = "linux")]
use winit::platform::wayland::WindowAttributesExtWayland;
use winit::raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use winit::window::{Window, WindowId};

use axidps_protocol::v2::RingHost;
use axidps_overlay_shm::{self as fmt, Input, Rect, Table, Window as ShmWindow};

/// No game frame for this long: stop painting.
const DLL_STALE: Duration = Duration::from_secs(5);

struct Shm {
    map: memmap2::MmapMut,
    /// The window table last published: the one the DLL aims clicks with.
    published: Option<Table>,
}

impl Shm {
    fn create(path: &std::path::Path) -> anyhow::Result<Self> {
        let f = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .with_context(|| format!("open {}", path.display()))?;
        // Never shrink: the game may have the file mapped, and reading a page past the end of a
        // shrunk file faults inside the game.
        if f.metadata()?.len() < fmt::FILE_LEN as u64 {
            f.set_len(fmt::FILE_LEN as u64)?;
        }
        let mut map = unsafe { memmap2::MmapMut::map_mut(&f)? };
        map[4..8].copy_from_slice(&fmt::VERSION.to_le_bytes());
        map[8..12].copy_from_slice(&fmt::MAX_W.to_le_bytes());
        map[12..16].copy_from_slice(&fmt::MAX_H.to_le_bytes());
        // Magic last: the DLL maps only a complete header.
        map[0..4].copy_from_slice(&fmt::MAGIC.to_le_bytes());
        Ok(Shm {
            map,
            published: None,
        })
    }

    fn atomic(&self, off: usize) -> &AtomicU64 {
        // Safety: 8-aligned offset inside a page-aligned shared mapping that lives as long as self.
        unsafe { &*(self.map.as_ptr().add(off) as *const AtomicU64) }
    }

    fn u32_at(&self, off: usize) -> u32 {
        u32::from_le_bytes(self.map[off..off + 4].try_into().unwrap())
    }

    fn put_u32(&mut self, off: usize, v: u32) {
        self.map[off..off + 4].copy_from_slice(&v.to_le_bytes());
    }

    /// Next input entry from the DLL, if any.
    fn pop_input(&self) -> Option<Input> {
        let w = self.atomic(fmt::IN_WRITE_OFF).load(Ordering::Acquire);
        let r = self.atomic(fmt::IN_READ_OFF).load(Ordering::Relaxed);
        if r >= w {
            return None;
        }
        let o = fmt::ring_slot(r);
        let e = Input::decode(&self.map[o..o + fmt::INPUT_LEN]);
        self.atomic(fmt::IN_READ_OFF)
            .store(r + 1, Ordering::Release);
        Some(e)
    }

    /// Publish the windows' pixels (tightly packed RGBA, one image per window) at their atlas
    /// rects in the next buffer, with the table.
    fn publish(
        &mut self,
        table: &Table,
        images: &[Option<servo::RgbaImage>],
        paint_us: u32,
        read_us: u32,
    ) {
        let seq = self.atomic(fmt::SEQ_OFF).load(Ordering::Relaxed) + 1;
        let b = (seq & 1) as usize;
        let base = fmt::FRAME_OFF + b * fmt::FRAME_LEN;
        let stride = fmt::MAX_W as usize * 4;
        let mut out = Table {
            edit_seq: table.edit_seq,
            flags: table.flags,
            windows: Vec::new(),
        };
        for (win, img) in table.windows.iter().zip(images) {
            let Some(img) = img else { continue };
            let r = win.atlas;
            let (w, h) = (
                (img.width() as usize).min(r.w as usize),
                (img.height() as usize).min(r.h as usize),
            );
            let src = img.as_raw();
            let src_stride = img.width() as usize * 4;
            for row in 0..h {
                let d = base + (r.y as usize + row) * stride + r.x as usize * 4;
                self.map[d..d + w * 4]
                    .copy_from_slice(&src[row * src_stride..row * src_stride + w * 4]);
            }
            let mut win = *win;
            win.atlas = Rect::new(r.x, r.y, w as i32, h as i32);
            out.windows.push(win);
        }
        out.write(&mut self.map, b);
        self.put_u32(fmt::PAINT_US_OFF, paint_us);
        self.put_u32(fmt::READ_US_OFF, read_us);
        self.atomic(fmt::SEQ_OFF).store(seq, Ordering::Release);
        self.published = Some(out);
    }
}

/// The page's window report: `{"windows":[{name, ax, ay, w, h, sx, sy, drag:[x,y,w,h], flags,
/// r (corner radius), blur (0-3)}],
/// "edit_seq": n, "hide_zones": bool, "text_focus": bool}`.
fn parse_windows(v: &serde_json::Value) -> Option<Table> {
    let i = |o: &serde_json::Value, k: &str| o.get(k).and_then(|x| x.as_i64()).map(|x| x as i32);
    let list = v.get("windows")?.as_array()?;
    let mut windows = Vec::new();
    for o in list.iter().take(fmt::MAX_WINDOWS) {
        let d: Vec<i32> = o
            .get("drag")
            .and_then(|d| d.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_i64())
                    .map(|x| x as i32)
                    .collect()
            })
            .unwrap_or_default();
        let mut w = ShmWindow {
            atlas: Rect::new(i(o, "ax")?, i(o, "ay")?, i(o, "w")?, i(o, "h")?),
            sx: i(o, "sx")?,
            sy: i(o, "sy")?,
            drag: if d.len() == 4 {
                Rect::new(d[0], d[1], d[2], d[3])
            } else {
                Rect::default()
            },
            flags: i(o, "flags").unwrap_or(0) as u32,
            radius: i(o, "r").unwrap_or(0).clamp(0, 255) as u8,
            blur: i(o, "blur").unwrap_or(0).clamp(0, 3) as u8,
            ..Default::default()
        };
        w.set_name(o.get("name")?.as_str()?);
        if fmt::clip_to_atlas(w.atlas).is_some() {
            windows.push(w);
        }
    }
    let on = |k: &str| v.get(k).and_then(|x| x.as_bool()) == Some(true);
    let mut flags = 0;
    if on("hide_zones") {
        flags |= fmt::TABLE_HIDE_ZONES;
    }
    if on("text_focus") {
        flags |= fmt::TABLE_TEXT_FOCUS;
    }
    Some(Table {
        edit_seq: i(v, "edit_seq").unwrap_or(0) as u32,
        flags,
        windows,
    })
}

/// Where a pointer event at atlas point `(x, y)` belongs now. The DLL turns a screen point into
/// an atlas point with the table it last read (`published`); when the page has laid its windows
/// out again since (a plugin frame resized, moving every window after it), the same atlas point is
/// in another window. So: the window that holds the point in `published`, and the same spot of
/// that window in the page's `latest` report. A point in no window, or in one the page no longer
/// has, passes through unchanged.
fn remap(x: f32, y: f32, published: Option<&Table>, latest: Option<&Table>) -> (f32, f32) {
    let (Some(published), Some(latest)) = (published, latest) else {
        return (x, y);
    };
    let inside = |r: Rect| {
        x >= r.x as f32 && x < (r.x + r.w) as f32 && y >= r.y as f32 && y < (r.y + r.h) as f32
    };
    let Some(was) = published.windows.iter().find(|w| inside(w.atlas)) else {
        return (x, y);
    };
    let Some(now) = latest.windows.iter().find(|w| w.name() == was.name()) else {
        return (x, y);
    };
    (
        x - was.atlas.x as f32 + now.atlas.x as f32,
        y - was.atlas.y as f32 + now.atlas.y as f32,
    )
}

/// The page call with the game's frame rate, ping and server tick rate (0 = unknown).
fn perf_call(p: &fmt::Perf) -> String {
    format!(
        "window.axidps && window.axidps.perf && window.axidps.perf({{\"fps\":{},\"ping\":{},\"tick_rate\":{}}})",
        p.fps, p.ping_ms, p.tick_rate
    )
}

/// The page call that tells it where the game's UI is: `axidps.gameui([{id, label, x, y, w,
/// h}])`, or an empty list when that isn't known.
fn gameui_call(ui: &fmt::GameUi, screen: (u32, u32)) -> String {
    let zones: Vec<serde_json::Value> = fmt::zones(ui, (screen.0 as i32, screen.1 as i32))
        .iter()
        .map(|z| {
            serde_json::json!({"id": z.id, "label": z.label, "x": z.rect.x, "y": z.rect.y,
                "w": z.rect.w, "h": z.rect.h})
        })
        .collect();
    format!("axidps.gameui({})", serde_json::Value::Array(zones))
}

/// The plugin folders and what the last scan of them found.
struct PluginHost {
    bundled: PathBuf,
    /// The player's plugin folder; none without a config folder (never the working directory).
    user: Option<PathBuf>,
    state: Arc<scheme::PluginState>,
}

impl PluginHost {
    /// Scan both folders, print the scan's log lines, refresh what the scheme handler serves and
    /// what the network backstop allows, and return the call that gives the page the list.
    fn scan(&self) -> String {
        // No player folder: an empty path, which lists nothing (as `axidps plugin preview` does).
        let user = self.user.as_deref().unwrap_or(std::path::Path::new(""));
        let (plugins, log) = axidps_plugins::scan::scan_logged(&self.bundled, user);
        for line in log {
            println!("{line}");
        }
        self.state.set(&plugins);
        format!(
            "axidps.plugins({})",
            axidps_plugins::scan::to_page_json(&plugins)
        )
    }
}

/// `id` if it looks like a plugin id (safe in a log line or a page call), else "?".
fn log_id(id: &str) -> &str {
    if !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
    {
        id
    } else {
        "?"
    }
}

/// A plugin's own log line, one line of at most 500 characters.
fn plugin_log_line(id: &str, msg: &str) -> String {
    format!(
        "plugin {}: {}",
        log_id(id),
        axidps_plugins::logline::clean(msg)
    )
}

/// Developer mode's view of the player plugin folder: which plugins changed, and whether a
/// manifest did (then the list must be rescanned before the frames reload).
struct DevWatch {
    dir: PathBuf,
    watcher: axidps_plugins::watch::Watcher,
    /// Each plugin id's manifest bytes, as last seen.
    manifests: std::collections::HashMap<String, Option<Vec<u8>>>,
}

impl DevWatch {
    fn new(dir: PathBuf) -> Self {
        let mut w = DevWatch {
            dir,
            watcher: axidps_plugins::watch::Watcher::new(),
            manifests: Default::default(),
        };
        // The baseline: the first snapshot reports nothing.
        let snap = axidps_plugins::watch::Snapshot::take(&w.dir);
        for id in snap
            .plugins
            .values()
            .map(|p| p.id.clone())
            .collect::<Vec<_>>()
        {
            let m = w.manifest(&snap, &id);
            w.manifests.insert(id, m);
        }
        w.watcher.tick(Instant::now(), snap);
        w
    }

    /// The manifest bytes of the folder(s) holding plugin `id` (the first, by folder name).
    fn manifest(&self, snap: &axidps_plugins::watch::Snapshot, id: &str) -> Option<Vec<u8>> {
        let (folder, _) = snap.plugins.iter().find(|(_, p)| p.id == id)?;
        std::fs::read(self.dir.join(folder).join(axidps_plugins::MANIFEST_FILE)).ok()
    }

    /// One poll: the plugins whose files changed and then stayed still, each with whether its
    /// manifest changed.
    fn poll(&mut self) -> Vec<(String, bool)> {
        let snap = axidps_plugins::watch::Snapshot::take(&self.dir);
        let ids = self.watcher.tick(Instant::now(), snap.clone());
        ids.into_iter()
            .map(|id| {
                let m = self.manifest(&snap, &id);
                let changed = self.manifests.get(&id) != Some(&m);
                self.manifests.insert(id.clone(), m);
                (id, changed)
            })
            .collect()
    }

    /// Poll every 250 ms on a thread of its own; send what changed and wake the event loop.
    fn spawn(mut self, tx: mpsc::Sender<(String, bool)>, waker: Waker) {
        std::thread::Builder::new()
            .name("plugin-watch".into())
            .spawn(move || {
                loop {
                    std::thread::sleep(Duration::from_millis(250));
                    let changed = self.poll();
                    if changed.is_empty() {
                        continue;
                    }
                    for c in changed {
                        if tx.send(c).is_err() {
                            return;
                        }
                    }
                    waker.wake();
                }
            })
            .expect("spawn the plugin watcher");
    }
}

/// End the process at once with `code`, skipping Servo's teardown (it segfaults in a
/// constellation thread). `_exit`, not `process::exit`: the atexit handlers run SpiderMonkey's
/// static destructors, whose MutexImpl teardown segfaults (core dump on every game close).
fn exit_now(code: i32) -> ! {
    use std::io::Write;
    let _ = std::io::stdout().flush();
    unsafe extern "C" {
        fn _exit(code: i32) -> !;
    }
    // SAFETY: ends the process at once; nothing after this needs to run.
    unsafe { _exit(code) }
}

#[derive(Default)]
struct Delegate {
    /// The per-run token every page -> host console message must carry (`axidps:<token>:`).
    token: String,
    /// An untokened `axidps:` message was seen and logged.
    forged_logged: Cell<bool>,
    /// The page asked for a rescan of the plugin folders (`{"plugins":"scan"}`).
    rescan: Cell<bool>,
    state: Arc<scheme::PluginState>,
    frame_ready: Cell<bool>,
    loaded: Cell<bool>,
    /// Untokened warnings and errors (the page's own, or any plugin frame's: they share the
    /// console) printed as `page: ...`: capped and rate-limited so a frame can't flood the log.
    page_lines: RefCell<Option<axidps_plugins::logline::Gate>>,
    /// The page's latest window report.
    table: RefCell<Option<Table>>,
    /// A layout/settings payload to persist.
    save: RefCell<Option<String>>,
    /// The page switched developer mode (`{"dev": bool}`).
    dev: Cell<Option<bool>>,
}

impl WebViewDelegate for Delegate {
    fn notify_new_frame_ready(&self, _webview: WebView) {
        self.frame_ready.set(true);
    }

    fn notify_load_status_changed(&self, _webview: WebView, status: LoadStatus) {
        if matches!(status, LoadStatus::Complete) {
            self.loaded.set(true);
        }
    }

    fn load_web_resource(&self, _webview: WebView, load: servo::WebResourceLoad) {
        self.state.guard(load);
    }

    fn show_console_message(&self, _webview: WebView, level: ConsoleLogLevel, message: String) {
        if message.starts_with("axidps:")
            && !message.starts_with(&format!("axidps:{}:", self.token))
        {
            // A plugin frame (or anything else) writing to the shared console: never act on it,
            // never echo it.
            if !self.forged_logged.replace(true) {
                println!("dropped untokened host message");
            }
            return;
        }
        let Some(json) = message
            .strip_prefix("axidps:")
            .and_then(|m| m.strip_prefix(self.token.as_str()))
            .and_then(|m| m.strip_prefix(':'))
        else {
            if matches!(level, ConsoleLogLevel::Warn | ConsoleLogLevel::Error) {
                let mut gate = self.page_lines.borrow_mut();
                let (pass, dropped) = gate
                    .get_or_insert_with(|| axidps_plugins::logline::Gate::new(20))
                    .pass(Instant::now());
                if let Some(d) = dropped {
                    println!("page: {d}");
                }
                if pass {
                    println!("page: {}", axidps_plugins::logline::clean(&message));
                }
            }
            return;
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else {
            println!("page: bad message {}", axidps_plugins::logline::clean(json));
            return;
        };
        if v.get("plugins").and_then(|x| x.as_str()) == Some("scan") {
            self.rescan.set(true);
        }
        if let Some(l) = v.get("log")
            && let (Some(id), Some(msg)) = (
                l.get("plugin").and_then(|x| x.as_str()),
                l.get("msg").and_then(|x| x.as_str()),
            )
            && matches!(l.get("level").and_then(|x| x.as_str()), Some("warn" | "error"))
        {
            println!("{}", plugin_log_line(id, msg));
        }
        if let Some(s) = v.get("save") {
            // What the scheme serves and the backstop allows follows the player's enables at once
            // (the page saves before it asks for a rescan).
            self.state
                .set_saved(s.get("plugins").cloned().unwrap_or_default());
            *self.save.borrow_mut() = Some(s.to_string());
        }
        if let Some(d) = v.get("dev").and_then(|x| x.as_bool()) {
            self.dev.set(Some(d));
        }
        // `?test` pages only (tools/overlay-fake-dll.py): the plugin windows' status and error
        // counts, as one log line.
        if let Some(d) = v.get("debug") {
            let line: String = d.to_string().chars().take(4000).collect();
            println!("debug {line}");
        }
        if let Some(t) = parse_windows(&v) {
            *self.table.borrow_mut() = Some(t);
            self.frame_ready.set(true);
        }
    }
}

/// Servo-wide events: the developer-tools server (`--devtools`, developer mode).
struct HostDelegate {
    state: Arc<scheme::PluginState>,
}

impl ServoDelegate for HostDelegate {
    fn load_web_resource(&self, load: servo::WebResourceLoad) {
        self.state.guard(load);
    }

    fn notify_error(&self, error: ServoError) {
        match error {
            ServoError::DevtoolsFailedToStart => println!("devtools: server failed to start"),
            e => println!("servo error: {e:?}"),
        }
    }

    fn notify_devtools_server_started(&self, port: u16, _token: String) {
        println!("devtools: server started on 127.0.0.1:{port}");
    }

    fn request_devtools_connection(&self, request: servo::AllowOrDenyRequest) {
        // Localhost only, and only with --devtools or developer mode: allow without a prompt.
        println!("devtools: client connected");
        request.allow();
    }
}

#[derive(Clone)]
struct Waker(EventLoopProxy<()>);

impl EventLoopWaker for Waker {
    fn clone_box(&self) -> Box<dyn EventLoopWaker> {
        Box::new(self.clone())
    }
    fn wake(&self) {
        let _ = self.0.send_event(());
    }
}

enum Ctx {
    Gpu {
        _window: Window,
        _parent: Rc<WindowRenderingContext>,
        ctx: Rc<OffscreenRenderingContext>,
    },
    Soft(Rc<SoftwareRenderingContext>),
}

impl Ctx {
    fn rc(&self) -> Rc<dyn RenderingContext> {
        match self {
            Ctx::Gpu { ctx, .. } => ctx.clone(),
            Ctx::Soft(c) => c.clone(),
        }
    }
}

struct Running {
    servo: Servo,
    ctx: Ctx,
    webview: WebView,
    delegate: Rc<Delegate>,
}

/// Core's live feed: re-read when its modification time or length changes.
struct LiveFeed {
    path: PathBuf,
    seen: Option<(SystemTime, u64)>,
    last_check: Instant,
    /// The page has not been given the current feed yet (new page, or the feed changed).
    pending: Option<String>,
}

impl LiveFeed {
    fn poll(&mut self) {
        if self.last_check.elapsed() < Duration::from_millis(200) {
            return;
        }
        self.last_check = Instant::now();
        let meta = std::fs::metadata(&self.path).ok();
        let key = meta.and_then(|m| Some((m.modified().ok()?, m.len())));
        if key == self.seen {
            return;
        }
        self.seen = key;
        self.pending = Some(match key {
            None => "null".into(),
            Some(_) => match std::fs::read_to_string(&self.path) {
                Ok(s) if serde_json::from_str::<serde_json::Value>(&s).is_ok() => s,
                _ => return, // mid-write or garbage: try again next poll
            },
        });
    }
}

/// The launcher's self-update status (`update-status.json` in the config folder): re-read when its
/// modification time changes, checked every 5 s.
struct UpdateFeed {
    path: PathBuf,
    seen: Option<SystemTime>,
    last_check: Option<Instant>,
    /// The page has not been given the current status yet (new page, or the file changed).
    pending: Option<String>,
}

impl UpdateFeed {
    fn poll(&mut self) {
        if self.last_check.is_some_and(|t| t.elapsed() < Duration::from_secs(5)) {
            return;
        }
        self.last_check = Some(Instant::now());
        let key = std::fs::metadata(&self.path)
            .ok()
            .and_then(|m| m.modified().ok());
        if key == self.seen {
            return;
        }
        self.pending = Some(match key {
            None => "null".into(),
            Some(_) => match std::fs::read_to_string(&self.path) {
                Ok(s) if serde_json::from_str::<serde_json::Value>(&s).is_ok() => s,
                _ => return, // mid-write or garbage: try again next poll
            },
        });
        self.seen = key;
    }
}

#[derive(Default)]
struct Stats {
    frames: u32,
    paint_us: u64,
    read_us: u64,
}

struct App {
    url: Url,
    software: bool,
    plugins: PluginHost,
    /// `<overlay>/sdk`: served as `axidps-plugin://<id>/_sdk/...`.
    sdk: PathBuf,
    /// This run's page token, in the page URL's fragment.
    token: String,
    /// `AXIDPS_TEST_FORGE`: once loaded, write forged untokened `axidps:` messages to the page's
    /// console (tools/overlay-fake-dll.py checks that the host drops them).
    forge_test: bool,
    forged: bool,
    /// `--devtools`, or developer mode: start Servo's developer-tools server on 127.0.0.1:6080.
    devtools: bool,
    /// Developer mode (`dev` in ui.json) as read at start; switching it restarts the host.
    dev: bool,
    /// Developer mode: the player plugins that changed (id, manifest changed), from the watcher.
    dev_changes: Option<mpsc::Receiver<(String, bool)>>,
    waker: Waker,
    shm: RefCell<Shm>,
    start: Instant,
    state: Option<Running>,
    live: LiveFeed,
    update: UpdateFeed,
    /// Last DLL heartbeat value and when it last changed.
    dll_hb: (u64, Instant),
    active: bool,
    stats: Stats,
    last_report: Instant,
    /// Saved layout and page settings.
    ui_path: PathBuf,
    /// The page has been given its saved state.
    restored: bool,
    /// Screen size and edit state last told to the page.
    told: Option<(u32, u32, u32)>,
    /// The game UI zones last told to the page (screen size and game UI).
    told_ui: Option<((u32, u32), fmt::GameUi)>,
    /// The last frame rate, ping and tick rate given to the page.
    told_perf: Option<fmt::Perf>,
}

impl App {
    fn start(&mut self, el: &ActiveEventLoop) -> anyhow::Result<Running> {
        let size = PhysicalSize::new(fmt::MAX_W, fmt::MAX_H);
        let ctx = if self.software {
            Ctx::Soft(Rc::new(
                SoftwareRenderingContext::new(size)
                    .map_err(|e| anyhow!("software context: {e:?}"))?,
            ))
        } else {
            let attrs = Window::default_attributes()
                .with_title("axidps overlay host")
                .with_visible(false)
                .with_inner_size(size);
            // install.sh adds a KWin rule keeping this app id off the taskbar.
            #[cfg(target_os = "linux")]
            let attrs = attrs.with_name("axidps-overlay", "axidps-overlay");
            let window = el.create_window(attrs).context("hidden window")?;
            let parent = Rc::new(
                WindowRenderingContext::new(el.display_handle()?, window.window_handle()?, size)
                    .map_err(|e| anyhow!("window context: {e:?}"))?,
            );
            let ctx = Rc::new(parent.offscreen_context(size));
            Ctx::Gpu {
                _window: window,
                _parent: parent,
                ctx,
            }
        };
        let rc = ctx.rc();
        rc.make_current()
            .map_err(|e| anyhow!("make_current: {e:?}"))?;
        // A transparent canvas: window pixels keep the page's own alpha (premultiplied), so the
        // DLL can draw see-through windows and rounded corners.
        let mut prefs = servo::Preferences::default();
        prefs.shell_background_color_rgba = [0.0, 0.0, 0.0, 0.0];
        if self.devtools {
            prefs.devtools_server_enabled = true;
            prefs.devtools_server_listen_address = "127.0.0.1:6080".into();
        }
        let mut protocols = servo::protocol_handler::ProtocolRegistry::default();
        protocols
            .register(
                scheme::SCHEME,
                scheme::PluginScheme::new(self.plugins.state.clone(), self.sdk.clone()),
            )
            .map_err(|e| anyhow!("register {}: {e:?}", scheme::SCHEME))?;
        let servo = ServoBuilder::default()
            .preferences(prefs)
            .protocol_registry(protocols)
            .event_loop_waker(Box::new(self.waker.clone()))
            .build();
        servo.set_delegate(Rc::new(HostDelegate {
            state: self.plugins.state.clone(),
        }));
        let delegate = Rc::new(Delegate {
            token: self.token.clone(),
            state: self.plugins.state.clone(),
            ..Default::default()
        });
        let webview = WebViewBuilder::new(&servo, rc)
            .url(self.url.clone())
            .delegate(delegate.clone())
            .build();
        Ok(Running {
            servo,
            ctx,
            webview,
            delegate,
        })
    }

    /// Follow the game's DLL heartbeat. Servo and its (hidden, but taskbar-visible on Wayland)
    /// window exist only while the game runs: they start when the DLL attaches, and the process
    /// exits when it goes away, so systemd restarts it as a windowless watcher.
    fn follow_game(&mut self, el: &ActiveEventLoop) {
        let hb = self
            .shm
            .borrow()
            .atomic(fmt::DLL_HB_OFF)
            .load(Ordering::Relaxed);
        if hb != self.dll_hb.0 {
            self.dll_hb = (hb, Instant::now());
        }
        let active = self.dll_hb.1.elapsed() < DLL_STALE && hb != 0;
        if active == self.active {
            return;
        }
        self.active = active;
        if !active {
            println!("game gone; exiting");
            // systemd (or the app's supervisor) restarts us.
            exit_now(0)
        }
        println!("game attached");
        if self.state.is_none() {
            match self.start(el) {
                Ok(s) => self.state = Some(s),
                Err(e) => {
                    eprintln!("start failed: {e:#}");
                    el.exit();
                    return;
                }
            }
        }
        if let Some(st) = &self.state {
            st.delegate.frame_ready.set(true);
        }
    }

    fn js(st: &Running, script: String) {
        st.webview.evaluate_javascript(script, |r| {
            if let Err(e) = r {
                println!("page call failed: {e:?}");
            }
        });
    }

    fn tick(&mut self) {
        self.shm
            .borrow()
            .atomic(fmt::HOST_HB_OFF)
            .store(self.start.elapsed().as_millis() as u64, Ordering::Relaxed);
        let Some(st) = &self.state else { return };
        let shm = &self.shm;
        let loaded = st.delegate.loaded.get();
        if let Some(d) = st
            .delegate
            .page_lines
            .borrow_mut()
            .as_mut()
            .and_then(|g| g.flush(Instant::now()))
        {
            println!("page: {d}");
        }

        while let Some(i) = shm.borrow().pop_input() {
            let p = match i.kind {
                fmt::kind::MOVE | fmt::kind::DOWN | fmt::kind::UP | fmt::kind::WHEEL => {
                    let (x, y) = remap(
                        i.x,
                        i.y,
                        shm.borrow().published.as_ref(),
                        st.delegate.table.borrow().as_ref(),
                    );
                    DevicePoint::new(x, y)
                }
                _ => DevicePoint::new(i.x, i.y),
            };
            let button = match i.button {
                1 => MouseButton::Secondary,
                2 => MouseButton::Auxiliary,
                _ => MouseButton::Primary,
            };
            let ev = match i.kind {
                fmt::kind::MOVE => InputEvent::MouseMove(MouseMoveEvent::new(p.into())),
                fmt::kind::DOWN => InputEvent::MouseButton(MouseButtonEvent::new(
                    MouseButtonAction::Down,
                    button,
                    p.into(),
                )),
                fmt::kind::UP => InputEvent::MouseButton(MouseButtonEvent::new(
                    MouseButtonAction::Up,
                    button,
                    p.into(),
                )),
                fmt::kind::WHEEL => InputEvent::Wheel(WheelEvent::new(
                    WheelDelta {
                        x: 0.0,
                        y: -(i.wheel as f64) * 76.0,
                        z: 0.0,
                        mode: WheelMode::DeltaPixel,
                    },
                    p.into(),
                    servo::Modifiers::default(),
                )),
                fmt::kind::MOVED if loaded => {
                    let name = serde_json::to_string(i.name()).unwrap_or_default();
                    Self::js(st, format!("axidps.moved({name}, {}, {})", i.x, i.y));
                    continue;
                }
                fmt::kind::DISMISS if loaded => {
                    Self::js(st, "axidps.dismiss()".into());
                    continue;
                }
                fmt::kind::KEY if loaded => {
                    let name = serde_json::to_string(i.name()).unwrap_or_default();
                    Self::js(st, format!("axidps.key({name})"));
                    continue;
                }
                fmt::kind::LEGACY_POS if loaded => {
                    Self::js(st, format!("axidps.legacy({}, {})", i.x, i.y));
                    continue;
                }
                _ => continue,
            };
            st.webview.notify_input_event(ev);
        }

        if loaded {
            if !self.restored {
                self.restored = true;
                let saved = std::fs::read_to_string(&self.ui_path)
                    .ok()
                    .filter(|s| serde_json::from_str::<serde_json::Value>(s).is_ok())
                    .unwrap_or_else(|| "null".into());
                Self::js(st, format!("axidps.restore({saved})"));
                Self::js(st, self.plugins.scan());
                if self.forge_test && !self.forged {
                    self.forged = true;
                    for m in [
                        r#"axidps:{"save":{"v":1,"forged":true}}"#,
                        r#"axidps:00:{"save":{"v":1,"forged":true}}"#,
                    ] {
                        Self::js(st, format!("console.log({})", serde_json::json!(m)));
                    }
                }
                self.told = None;
                self.told_ui = None;
                self.told_perf = None;
                self.update.seen = None;
                self.update.last_check = None;
            }
            let now = {
                let m = shm.borrow();
                (
                    m.u32_at(fmt::SCREEN_OFF),
                    m.u32_at(fmt::SCREEN_OFF + 4),
                    m.u32_at(fmt::EDIT_OFF),
                )
            };
            if self.told != Some(now) {
                let (sw, sh, edit) = now;
                if sw > 0 && sh > 0 && self.told.map(|t| (t.0, t.1)) != Some((sw, sh)) {
                    Self::js(st, format!("axidps.screen({sw}, {sh})"));
                }
                if self.told.map(|t| t.2) != Some(edit) {
                    Self::js(st, format!("axidps.edit({})", edit == 1));
                }
                self.told = Some(now);
            }
            let ui = fmt::GameUi::read(&shm.borrow().map);
            let key = ((now.0, now.1), ui);
            if self.told_ui != Some(key) {
                self.told_ui = Some(key);
                Self::js(st, gameui_call(&ui, (now.0, now.1)));
            }
            let perf = fmt::Perf::read(&shm.borrow().map);
            if self.told_perf != Some(perf) {
                self.told_perf = Some(perf);
                Self::js(st, perf_call(&perf));
            }
            self.live.poll();
            if let Some(json) = self.live.pending.take() {
                Self::js(st, format!("window.axidps && window.axidps.update({json})"));
            }
            self.update.poll();
            if let Some(json) = self.update.pending.take() {
                Self::js(st, format!("window.axidps && window.axidps.updateStatus({json})"));
            }
        }
        if st.delegate.rescan.take() {
            Self::js(st, self.plugins.scan());
        }
        if let Some(rx) = &self.dev_changes {
            let changed: Vec<(String, bool)> = rx.try_iter().collect();
            if loaded && !changed.is_empty() {
                if changed.iter().any(|(_, m)| *m) {
                    Self::js(st, self.plugins.scan());
                }
                let mut seen = std::collections::HashSet::new();
                for (id, _) in changed {
                    if !seen.insert(id.clone()) {
                        continue;
                    }
                    println!("dev: pluginChanged {}", log_id(&id));
                    let arg = serde_json::to_string(&id).unwrap_or_default();
                    Self::js(
                        st,
                        format!("window.axidps && window.axidps.pluginChanged({arg})"),
                    );
                }
            }
        }
        if let Some(body) = st.delegate.save.borrow_mut().take() {
            let tmp = self.ui_path.with_extension("json.tmp");
            let r = self
                .ui_path
                .parent()
                .map_or(Ok(()), std::fs::create_dir_all)
                .and_then(|_| std::fs::write(&tmp, body))
                .and_then(|_| std::fs::rename(&tmp, &self.ui_path));
            if let Err(e) = r {
                println!("saving {}: {e}", self.ui_path.display());
            }
        }
        // After the save above: the page saves before it sends `{"dev": ...}`.
        if let Some(d) = st.delegate.dev.take()
            && d != self.dev
        {
            println!(
                "developer mode {}; restarting",
                if d { "on" } else { "off" }
            );
            exit_now(0);
        }

        st.servo.spin_event_loop();
        let table = st.delegate.table.borrow().clone();
        if self.active
            && st.delegate.frame_ready.get()
            && let Some(table) = table
        {
            st.delegate.frame_ready.set(false);
            let rc = st.ctx.rc();
            let _ = rc.make_current();
            let t0 = Instant::now();
            st.webview.paint();
            let t1 = Instant::now();
            // GL reads from the bottom-left of the viewport.
            let images: Vec<_> = table
                .windows
                .iter()
                .map(|w| {
                    let r = w.atlas;
                    rc.read_to_image(DeviceIntRect::from_origin_and_size(
                        euclid::point2(r.x, fmt::MAX_H as i32 - r.y - r.h),
                        DeviceIntSize::new(r.w, r.h),
                    ))
                })
                .collect();
            let t2 = Instant::now();
            rc.present();
            let paint_us = (t1 - t0).as_micros() as u32;
            let read_us = (t2 - t1).as_micros() as u32;
            shm.borrow_mut().publish(&table, &images, paint_us, read_us);
            self.stats.frames += 1;
            self.stats.paint_us += u64::from(paint_us);
            self.stats.read_us += u64::from(read_us);
        }
        if self.last_report.elapsed() >= Duration::from_secs(60) {
            let s = std::mem::take(&mut self.stats);
            if s.frames > 0 {
                println!(
                    "60s: {} frames, paint avg {:.2} ms, readback avg {:.2} ms",
                    s.frames,
                    s.paint_us as f64 / 1000.0 / f64::from(s.frames),
                    s.read_us as f64 / 1000.0 / f64::from(s.frames)
                );
            }
            self.last_report = Instant::now();
        }
    }
}

impl ApplicationHandler<()> for App {
    fn resumed(&mut self, _el: &ActiveEventLoop) {}

    fn user_event(&mut self, _el: &ActiveEventLoop, _e: ()) {
        self.tick();
    }

    fn window_event(&mut self, _el: &ActiveEventLoop, _id: WindowId, _e: WindowEvent) {}

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        self.follow_game(el);
        self.tick();
        // Input arrives through shared memory: poll it every 2 ms in game, 250 ms without one.
        let wait = if self.active { 2 } else { 250 };
        el.set_control_flow(ControlFlow::WaitUntil(
            Instant::now() + Duration::from_millis(wait),
        ));
    }
}

/// `ui.json` in axidps's config folder: `%APPDATA%\axidps` on Windows, else
/// `$XDG_CONFIG_HOME/axidps` or `~/.config/axidps`.
fn ui_path() -> PathBuf {
    axidps_protocol::paths::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("ui.json")
}

/// The saved ui.json, if it reads as JSON.
fn saved_ui(ui: &std::path::Path) -> Option<serde_json::Value> {
    std::fs::read(ui)
        .ok()
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
}

/// Developer mode: `dev: true` in the saved ui.json.
fn saved_dev(ui: &std::path::Path) -> bool {
    saved_ui(ui).and_then(|v| v.get("dev").and_then(|d| d.as_bool())) == Some(true)
}

/// The player's plugin folder: `--user-plugins`, else `plugins` in the config folder; none
/// without one (plugins are never loaded from the working directory).
fn user_plugins_dir(arg: Option<PathBuf>, config: Option<PathBuf>) -> Option<PathBuf> {
    arg.or_else(|| config.map(|c| c.join("plugins")))
}

/// The page argument: a path, optionally with a `?query` for the page (`index.html?test`), kept
/// as part of the path if a file by that whole name exists.
fn split_page_arg(p: &str) -> (PathBuf, Option<String>) {
    match p.split_once('?') {
        Some((path, q)) if !std::path::Path::new(p).exists() => {
            (PathBuf::from(path), Some(q.to_string()))
        }
        _ => (PathBuf::from(p), None),
    }
}

fn main() -> anyhow::Result<()> {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let mut page = None;
    let mut software = false;
    let mut bundled_dir = None;
    let mut user_dir = None;
    let mut sdk_dir = None;
    let mut devtools = false;
    let mut live = axidps_protocol::paths::live_feed_path(RingHost::current_build());
    let mut shm_path = axidps_protocol::paths::overlay_frames_path(RingHost::current_build());
    let mut ui = ui_path();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--software" => software = true,
            "--devtools" => devtools = true,
            "--plugins" => {
                bundled_dir = Some(PathBuf::from(
                    args.next()
                        .ok_or_else(|| anyhow!("--plugins needs a folder"))?,
                ))
            }
            "--sdk" => {
                sdk_dir = Some(PathBuf::from(
                    args.next().ok_or_else(|| anyhow!("--sdk needs a folder"))?,
                ))
            }
            "--user-plugins" => {
                user_dir = Some(PathBuf::from(
                    args.next()
                        .ok_or_else(|| anyhow!("--user-plugins needs a folder"))?,
                ))
            }
            "--live" => {
                live = args
                    .next()
                    .ok_or_else(|| anyhow!("--live needs a path"))?
                    .into()
            }
            // For testing without the game (tools/overlay-fake-dll.py).
            "--shm" => {
                shm_path = args
                    .next()
                    .ok_or_else(|| anyhow!("--shm needs a path"))?
                    .into()
            }
            "--ui" => {
                ui = args
                    .next()
                    .ok_or_else(|| anyhow!("--ui needs a path"))?
                    .into()
            }
            p => page = Some(p.to_string()),
        }
    }
    let exe_dir = std::env::current_exe()?
        .parent()
        .ok_or_else(|| anyhow!("exe has no dir"))?
        .to_path_buf();
    let (page, query) = match page {
        Some(p) => split_page_arg(&p),
        None => (exe_dir.join("page/index.html"), None),
    };
    let dev = saved_dev(&ui);
    if dev {
        println!("developer mode on");
    }
    let plugins = PluginHost {
        bundled: bundled_dir.unwrap_or_else(|| exe_dir.join("plugins")),
        user: user_plugins_dir(user_dir, axidps_protocol::paths::config_dir()),
        state: Arc::default(),
    };
    // The player's enables as saved, then a scan, so the scheme serves from the first frame load;
    // the page gets the list after `axidps.restore`.
    plugins.state.set_saved(
        saved_ui(&ui)
            .and_then(|v| v.get("plugins").cloned())
            .unwrap_or_default(),
    );
    plugins.scan();
    let page = std::fs::canonicalize(&page).with_context(|| format!("{}", page.display()))?;
    let sdk = sdk_dir.unwrap_or_else(|| exe_dir.join("sdk"));
    // `file:` loads only from the page's folder and the SDK (the page loads ../sdk/axidps.js).
    let page_dir = page.parent().map(PathBuf::from).unwrap_or_default();
    let beside = page_dir.parent().map(|d| d.join("sdk")).unwrap_or_default();
    plugins
        .state
        .set_file_roots(&[page_dir, beside, sdk.clone()]);
    let url = Url::from_file_path(&page).map_err(|_| anyhow!("bad path"))?;
    let mut raw = [0u8; 16];
    getrandom::fill(&mut raw).map_err(|e| anyhow!("no random bytes for the page token: {e}"))?;
    let token: String = raw.iter().map(|b| format!("{b:02x}")).collect();
    let mut url = url;
    url.set_query(query.as_deref());
    url.set_fragment(Some(&format!("t={token}")));
    let el = EventLoop::<()>::with_user_event().build()?;
    let waker = Waker(el.create_proxy());
    let dev_changes = match (dev, &plugins.user) {
        (true, Some(user)) => {
            let (tx, rx) = mpsc::channel();
            DevWatch::new(user.clone()).spawn(tx, waker.clone());
            Some(rx)
        }
        _ => None,
    };
    let shm = Shm::create(&shm_path)?;
    // A heartbeat left over from an earlier game session must not count as a live game.
    let hb0 = shm.atomic(fmt::DLL_HB_OFF).load(Ordering::Relaxed);
    let mut app = App {
        url,
        software,
        plugins,
        sdk,
        token,
        forge_test: std::env::var_os("AXIDPS_TEST_FORGE").is_some(),
        forged: false,
        devtools: devtools || dev,
        dev,
        dev_changes,
        waker,
        shm: RefCell::new(shm),
        start: Instant::now(),
        state: None,
        live: LiveFeed {
            path: live,
            seen: None,
            last_check: Instant::now() - Duration::from_secs(1),
            pending: None,
        },
        update: UpdateFeed {
            path: axidps_protocol::paths::config_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("update-status.json"),
            seen: None,
            last_check: None,
            pending: None,
        },
        dll_hb: (hb0, Instant::now() - DLL_STALE),
        active: false,
        stats: Stats::default(),
        last_report: Instant::now(),
        ui_path: ui,
        restored: false,
        told: None,
        told_ui: None,
        told_perf: None,
    };
    println!(
        "axidps overlay host: {} -> {} ({})",
        page.display(),
        shm_path.display(),
        if software { "software GL" } else { "GPU" }
    );
    el.run_app(&mut app)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(wins: &[(&str, i32, i32, i32, i32)]) -> Table {
        Table {
            windows: wins
                .iter()
                .map(|&(name, x, y, w, h)| {
                    let mut win = ShmWindow {
                        atlas: Rect::new(x, y, w, h),
                        ..Default::default()
                    };
                    win.set_name(name);
                    win
                })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn parse_windows_reads_table_flags() {
        let flags = |v: serde_json::Value| parse_windows(&v).map(|t| t.flags);
        assert_eq!(flags(serde_json::json!({"windows": []})), Some(0));
        assert_eq!(
            flags(serde_json::json!({"windows": [], "hide_zones": true})),
            Some(fmt::TABLE_HIDE_ZONES)
        );
        assert_eq!(
            flags(serde_json::json!({"windows": [], "text_focus": true})),
            Some(fmt::TABLE_TEXT_FOCUS)
        );
        assert_eq!(
            flags(serde_json::json!({"windows": [], "hide_zones": true, "text_focus": true})),
            Some(fmt::TABLE_HIDE_ZONES | fmt::TABLE_TEXT_FOCUS)
        );
        assert_eq!(
            flags(serde_json::json!({"windows": [], "text_focus": 1})),
            Some(0)
        );
    }

    #[test]
    fn remap_follows_the_window_to_its_latest_place() {
        // p1 grew from 100 to 300 wide since the DLL read the table: p2 moved right.
        let published = table(&[("meter", 0, 0, 200, 100), ("p1", 204, 0, 100, 50), ("p2", 308, 0, 80, 40)]);
        let latest = table(&[("meter", 0, 0, 200, 100), ("p1", 204, 0, 300, 50), ("p2", 508, 0, 80, 40)]);
        let r = |x, y| remap(x, y, Some(&published), Some(&latest));
        assert_eq!(r(320.0, 10.0), (520.0, 10.0), "a click aimed at p2 lands in p2");
        assert_eq!(r(210.5, 20.5), (210.5, 20.5), "p1 didn't move");
        assert_eq!(r(50.0, 50.0), (50.0, 50.0), "the meter didn't move");
        assert_eq!(r(1000.0, 1000.0), (1000.0, 1000.0), "in no window: unchanged");
        assert_eq!(r(388.0, 10.0), (388.0, 10.0), "just past p2's right edge: in no window");
        // p2 closed since.
        let gone = table(&[("meter", 0, 0, 200, 100), ("p1", 204, 0, 300, 50)]);
        assert_eq!(remap(320.0, 10.0, Some(&published), Some(&gone)), (320.0, 10.0));
        // Nothing published yet, or no report: unchanged.
        assert_eq!(remap(320.0, 10.0, None, Some(&latest)), (320.0, 10.0));
        assert_eq!(remap(320.0, 10.0, Some(&published), None), (320.0, 10.0));
        // Wrapped to the next row.
        let wrapped = table(&[("meter", 0, 0, 200, 100), ("p1", 204, 0, 1800, 50), ("p2", 0, 104, 80, 40)]);
        assert_eq!(remap(310.0, 39.0, Some(&published), Some(&wrapped)), (2.0, 143.0));
    }

    #[test]
    fn user_plugins_never_from_the_working_directory() {
        assert_eq!(user_plugins_dir(None, None), None);
        assert_eq!(
            user_plugins_dir(None, Some(PathBuf::from("/c/axidps"))),
            Some(PathBuf::from("/c/axidps/plugins"))
        );
        assert_eq!(
            user_plugins_dir(Some(PathBuf::from("/p")), None),
            Some(PathBuf::from("/p"))
        );
        // No player folder lists nothing, whatever the working directory holds.
        let (list, log) = axidps_plugins::scan::scan_logged(
            std::path::Path::new("/nonexistent-axidps-bundled"),
            std::path::Path::new(""),
        );
        assert!(list.is_empty() && log.is_empty(), "{log:?}");
    }
}
