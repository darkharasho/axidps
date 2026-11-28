//! Shared memory between the overlay host (`apps/axidps-overlay`, Servo) and the DLL,
//! version 3, plus the window snapping both rely on.
//!
//! ```text
//! 0     u32 magic, u32 version, u32 max_w, u32 max_h (atlas buffer size)
//! 16    u64 seq (completed frames; latest is buffer seq&1)
//! 24    u32 paint_us, u32 read_us, u64 reserved, u64 host heartbeat ms
//! 48    u64 input write counter (DLL), u64 input read counter (host)
//! 64    input ring: RING entries of INPUT_LEN bytes (see `Input`)
//! 12352 DLL -> host: u64 DLL heartbeat (frames presented), u32 screen_w, u32 screen_h,
//!       u32 edit (1 while the DLL is in edit mode)
//! 12372 DLL -> host: the game's UI from MumbleLink (see `GameUi`): u32 bit 31 known | uiState,
//!       u32 interface size (0 small .. 3 larger), u16 compass width, u16 compass height
//! 12384 DLL -> host: performance over the last second (see `Perf`): u32 frames per second,
//!       u32 ping ms, u32 server ticks per second (each 0 while unknown)
//! 12416 window table of buffer 0, 13504 of buffer 1 (see `Table`): u32 count, u32 edit_seq,
//!       u32 table flags (`TABLE_HIDE_ZONES`, `TABLE_TEXT_FOCUS`), u32 pad, then per window name[16],
//!       i32 atlas x/y/w/h, i32 sx/sy, i32 drag x/y/w/h, u32 flags, u8 radius, u8 blur, u16 pad
//! 16384 frame buffers 0 and 1: max_w x max_h RGBA8 premultiplied, row stride max_w*4
//! ```
//!
//! Every multi-byte value is little-endian. Only the window rects of a buffer are meaningful;
//! the rest of the atlas is undefined.

pub mod outline;
pub mod texstyle;

pub const MAGIC: u32 = 0x5645_4f41; // public-check:ok
pub const VERSION: u32 = 3;
pub const MAX_W: u32 = 2048;
pub const MAX_H: u32 = 2048;

pub const SEQ_OFF: usize = 16;
pub const PAINT_US_OFF: usize = 24;
pub const READ_US_OFF: usize = 28;
pub const HOST_HB_OFF: usize = 40;
pub const IN_WRITE_OFF: usize = 48;
pub const IN_READ_OFF: usize = 56;
pub const RING_OFF: usize = 64;
pub const RING: u64 = 256;
pub const INPUT_LEN: usize = 48;
pub const DLL_HB_OFF: usize = 12352;
pub const SCREEN_OFF: usize = 12360;
pub const EDIT_OFF: usize = 12368;
pub const GAMEUI_OFF: usize = 12372;
pub const PERF_OFF: usize = 12384;
pub const TABLE_OFF: [usize; 2] = [12416, 13504];
pub const MAX_WINDOWS: usize = 16;
pub const WIN_LEN: usize = 64;
pub const FRAME_OFF: usize = 16384;
pub const FRAME_LEN: usize = (MAX_W * MAX_H * 4) as usize;
pub const FILE_LEN: usize = FRAME_OFF + 2 * FRAME_LEN;

/// Window flag: takes normal input in edit mode (the Layout window) instead of being dragged.
pub const FLAG_EDIT_INTERACTIVE: u32 = 1;
/// Window flag: modal (the hub). Outside edit mode the DLL dims the game behind it, gives it all
/// input, and turns a click outside it or Esc into a `kind::DISMISS`.
pub const FLAG_MODAL: u32 = 2;
/// Window flag: several windows merged into one (the page's merge option). In edit mode a press
/// inside its `drag` rect (its title bar) drags it as usual; a press anywhere else starts a pull:
/// the DLL leaves the window where it is and sends `kind::PULL` on each move and `kind::PULL_END` at
/// the release, never `kind::MOVED`.
pub const FLAG_GROUP: u32 = 4;
/// Window flag: during a pull, take over the drag. The DLL drags this window from the grab point
/// (`drag.x`, `drag.y`, window-local) as if the press had been on it. Ignored outside a pull.
pub const FLAG_GRAB: u32 = 8;

/// `kind::DRAG`'s `button`: the grab point inside the dragged window, each half clamped to 0-65535.
pub fn pack_grab(gx: i32, gy: i32) -> u32 {
    let c = |v: i32| v.clamp(0, 0xffff) as u32;
    c(gx) | c(gy) << 16
}
pub fn unpack_grab(b: u32) -> (i32, i32) {
    ((b & 0xffff) as i32, (b >> 16) as i32)
}

/// During a pull on window `pulled`, the window that takes the drag over: the first other window
/// with `FLAG_GRAB`, as (key, grab x, grab y). `windows` = (key, flags, drag zone).
pub fn grab_target(
    windows: impl IntoIterator<Item = (u64, u32, Rect)>,
    pulled: u64,
) -> Option<(u64, i32, i32)> {
    windows
        .into_iter()
        .find(|&(k, f, _)| k != pulled && f & FLAG_GRAB != 0)
        .map(|(k, _, d)| (k, d.x, d.y))
}

/// Table flag: don't draw the game's UI areas in edit mode (the Layout window's switch). Clear
/// in tables from hosts that predate it, so those show them.
pub const TABLE_HIDE_ZONES: u32 = 1;
/// Table flag: a text field in the modal window has focus. While it is set (and the modal is up),
/// the DLL forwards keys as `kind::KEY` and keeps them from the game.
pub const TABLE_TEXT_FOCUS: u32 = 2;

/// Input kinds (DLL -> host).
pub mod kind {
    pub const MOVE: u32 = 1;
    pub const DOWN: u32 = 2;
    pub const UP: u32 = 3;
    pub const WHEEL: u32 = 4;
    /// A window was dropped: x/y = its new screen position, name = the window.
    pub const MOVED: u32 = 5;
    /// The v1 ini's meter position (x/y), sent once per host connection.
    pub const LEGACY_POS: u32 = 6;
    /// The DLL entered (button 1) or left (button 0) edit mode by itself (hotkey, Esc).
    pub const EDIT: u32 = 7;
    /// A modal window was dismissed: a click outside it, or Esc.
    pub const DISMISS: u32 = 8;
    /// A key while a text field has focus: name = the character (UTF-8) or one of Backspace,
    /// Delete, ArrowLeft, ArrowRight, Home, End, Enter, Escape.
    pub const KEY: u32 = 9;
    /// A window is being dragged (edit mode): x/y = its position now (snapped), name = the window,
    /// button = the grab point (`pack_grab`). Sent when the position changes.
    pub const DRAG: u32 = 10;
    /// A pull on a `FLAG_GROUP` window: x/y = the pointer, window-local; name = the window.
    pub const PULL: u32 = 11;
    /// The pull ended with the button's release without a hand-off (x/y unused, 0).
    pub const PULL_END: u32 = 12;
}

fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn i32_at(b: &[u8], o: usize) -> i32 {
    i32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn f32_at(b: &[u8], o: usize) -> f32 {
    f32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

fn name_bytes<const N: usize>(name: &str) -> [u8; N] {
    let mut out = [0u8; N];
    let n = name.len().min(N);
    out[..n].copy_from_slice(&name.as_bytes()[..n]);
    out
}
fn name_str(b: &[u8]) -> &str {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    std::str::from_utf8(&b[..end]).unwrap_or("")
}

/// A screen- or atlas-space rectangle.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Rect { x, y, w, h }
    }
    pub fn contains(&self, x: i32, y: i32) -> bool {
        // In i64: a far edge past i32::MAX must not wrap.
        let (x, y) = (i64::from(x), i64::from(y));
        x >= i64::from(self.x) && y >= i64::from(self.y) && x < self.right() && y < self.bottom()
    }

    /// The far edges, in i64 so that hostile sizes cannot wrap.
    fn right(&self) -> i64 {
        i64::from(self.x) + i64::from(self.w)
    }

    fn bottom(&self) -> i64 {
        i64::from(self.y) + i64::from(self.h)
    }
}

/// One window in a buffer's table.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Window {
    pub name: [u8; 16],
    /// Where the window's pixels are in the atlas buffer.
    pub atlas: Rect,
    /// Saved screen position (top-left), in backbuffer pixels.
    pub sx: i32,
    pub sy: i32,
    /// Drag zone, window-local (the title bar minus its buttons).
    pub drag: Rect,
    pub flags: u32,
    /// Corner radius in pixels: the DLL clips the window (and its backdrop blur) to it.
    pub radius: u8,
    /// Backdrop blur strength behind the window: 0 none, 1 soft, 2 medium, 3 strong.
    pub blur: u8,
}

impl Window {
    pub fn name(&self) -> &str {
        name_str(&self.name)
    }
    pub fn set_name(&mut self, name: &str) {
        self.name = name_bytes(name);
    }
    fn read(b: &[u8]) -> Self {
        let i = |k: usize| i32_at(b, 16 + 4 * k);
        Window {
            name: b[..16].try_into().unwrap(),
            atlas: Rect::new(i(0), i(1), i(2), i(3)),
            sx: i(4),
            sy: i(5),
            drag: Rect::new(i(6), i(7), i(8), i(9)),
            flags: u32_at(b, 56),
            radius: b[60],
            blur: b[61].min(3),
        }
    }
    fn write(&self, b: &mut [u8]) {
        b[..16].copy_from_slice(&self.name);
        let v = [
            self.atlas.x,
            self.atlas.y,
            self.atlas.w,
            self.atlas.h,
            self.sx,
            self.sy,
            self.drag.x,
            self.drag.y,
            self.drag.w,
            self.drag.h,
        ];
        for (k, x) in v.iter().enumerate() {
            b[16 + 4 * k..20 + 4 * k].copy_from_slice(&x.to_le_bytes());
        }
        b[56..60].copy_from_slice(&self.flags.to_le_bytes());
        b[60] = self.radius;
        b[61] = self.blur;
        b[62..64].fill(0);
    }
}

/// A buffer's window table: `u32 count, u32 edit_seq, u64 pad`, then `MAX_WINDOWS` entries.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Table {
    /// The page's edit-mode requests: odd = on, even = off. The DLL follows changes.
    pub edit_seq: u32,
    /// `TABLE_HIDE_ZONES`, `TABLE_TEXT_FOCUS`.
    pub flags: u32,
    pub windows: Vec<Window>,
}

impl Table {
    /// Read the table at `TABLE_OFF[b]` of a mapped file (`shm` is the whole file).
    pub fn read(shm: &[u8], b: usize) -> Self {
        let t = &shm[TABLE_OFF[b]..];
        let n = (u32_at(t, 0) as usize).min(MAX_WINDOWS);
        Table {
            edit_seq: u32_at(t, 4),
            flags: u32_at(t, 8),
            windows: (0..n)
                .map(|i| Window::read(&t[16 + i * WIN_LEN..16 + (i + 1) * WIN_LEN]))
                .filter(|w| clip_to_atlas(w.atlas) == Some(w.atlas))
                .collect(),
        }
    }

    pub fn write(&self, shm: &mut [u8], b: usize) {
        let t = &mut shm[TABLE_OFF[b]..TABLE_OFF[b] + 16 + MAX_WINDOWS * WIN_LEN];
        let n = self.windows.len().min(MAX_WINDOWS);
        t[0..4].copy_from_slice(&(n as u32).to_le_bytes());
        t[4..8].copy_from_slice(&self.edit_seq.to_le_bytes());
        t[8..12].copy_from_slice(&self.flags.to_le_bytes());
        t[12..16].fill(0);
        for (i, w) in self.windows.iter().take(n).enumerate() {
            w.write(&mut t[16 + i * WIN_LEN..16 + (i + 1) * WIN_LEN]);
        }
    }
}

/// `r` if it lies inside the atlas with a positive size.
pub fn clip_to_atlas(r: Rect) -> Option<Rect> {
    (r.w > 0
        && r.h > 0
        && r.x >= 0
        && r.y >= 0
        && r.right() <= MAX_W as i64
        && r.bottom() <= MAX_H as i64)
        .then_some(r)
}

/// One input ring entry: `u32 kind, u32 button, f32 x, f32 y, f32 wheel, u32 pad, name[24]`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Input {
    pub kind: u32,
    pub button: u32,
    pub x: f32,
    pub y: f32,
    pub wheel: f32,
    pub name: [u8; 24],
}

impl Input {
    pub fn new(kind: u32, button: u32, x: f32, y: f32, wheel: f32) -> Self {
        Input {
            kind,
            button,
            x,
            y,
            wheel,
            name: [0; 24],
        }
    }
    pub fn named(mut self, name: &str) -> Self {
        self.name = name_bytes(name);
        self
    }
    pub fn name(&self) -> &str {
        name_str(&self.name)
    }
    pub fn encode(&self) -> [u8; INPUT_LEN] {
        let mut b = [0u8; INPUT_LEN];
        b[0..4].copy_from_slice(&self.kind.to_le_bytes());
        b[4..8].copy_from_slice(&self.button.to_le_bytes());
        b[8..12].copy_from_slice(&self.x.to_le_bytes());
        b[12..16].copy_from_slice(&self.y.to_le_bytes());
        b[16..20].copy_from_slice(&self.wheel.to_le_bytes());
        b[24..48].copy_from_slice(&self.name);
        b
    }
    pub fn decode(b: &[u8]) -> Self {
        Input {
            kind: u32_at(b, 0),
            button: u32_at(b, 4),
            x: f32_at(b, 8),
            y: f32_at(b, 12),
            wheel: f32_at(b, 16),
            name: b[24..48].try_into().unwrap(),
        }
    }
}

/// Byte offset of ring slot `n` (a monotonically increasing counter).
pub fn ring_slot(n: u64) -> usize {
    RING_OFF + (n % RING) as usize * INPUT_LEN
}

/// What MumbleLink tells about the game's own UI: enough to estimate where its parts are.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GameUi {
    /// The link was read (the game is in gameplay); everything else is meaningless without it.
    pub known: bool,
    /// MumbleLink `uiState`: bit 0 map open, bit 1 compass at the top right, bit 2 compass
    /// rotation, bit 3 game has focus, bit 4 competitive mode, bit 5 textbox focus, bit 6 combat.
    pub ui_state: u32,
    /// Interface size (identity `uisz`): 0 small, 1 normal, 2 large, 3 larger.
    pub uisz: u8,
    /// The compass (minimap) size in pixels, as the game reports it.
    pub compass_w: u16,
    pub compass_h: u16,
    /// Blish HUD runs: its corner icons sit in a row right after the game's menu icons.
    pub blish: bool,
    /// Nexus is loaded: its quick-access icons sit in the same row as Blish's.
    pub nexus: bool,
}

/// Bit of the first GameUi word (above `uiState`) set while Blish HUD runs.
const BLISH_BIT: u32 = 1 << 16;
/// Bit of the first GameUi word set while Nexus is loaded.
const NEXUS_BIT: u32 = 1 << 17;

pub const UI_MAP_OPEN: u32 = 1;
pub const UI_COMPASS_TOP_RIGHT: u32 = 2;

impl GameUi {
    pub fn read(b: &[u8]) -> Self {
        let f = u32_at(b, GAMEUI_OFF);
        let c = u32_at(b, GAMEUI_OFF + 8);
        GameUi {
            known: f & 1 << 31 != 0,
            ui_state: f & 0xffff,
            blish: f & BLISH_BIT != 0,
            nexus: f & NEXUS_BIT != 0,
            uisz: u32_at(b, GAMEUI_OFF + 4).min(3) as u8,
            compass_w: c as u16,
            compass_h: (c >> 16) as u16,
        }
    }
    pub fn encode(&self) -> [u32; 3] {
        [
            if self.known { 1 << 31 } else { 0 }
                | if self.blish { BLISH_BIT } else { 0 }
                | if self.nexus { NEXUS_BIT } else { 0 }
                | (self.ui_state & 0xffff),
            u32::from(self.uisz.min(3)),
            u32::from(self.compass_w) | u32::from(self.compass_h) << 16,
        ]
    }
}

/// The game's frame rate, ping and server tick rate. 0 = unknown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Perf {
    pub fps: u32,
    pub ping_ms: u32,
    pub tick_rate: u32,
}

impl Perf {
    pub fn read(b: &[u8]) -> Self {
        Perf {
            fps: u32_at(b, PERF_OFF),
            ping_ms: u32_at(b, PERF_OFF + 4),
            tick_rate: u32_at(b, PERF_OFF + 8),
        }
    }
    pub fn encode(&self) -> [u32; 3] {
        [self.fps, self.ping_ms, self.tick_rate]
    }
}

/// Turns presented frames and the latest server tick into a [`Perf`] once a second.
#[derive(Debug, Default)]
pub struct PerfCounter {
    /// Start of the current window (ms) and the server tick then.
    since: Option<(u64, u32)>,
    frames: u32,
}

impl PerfCounter {
    /// One presented frame at `now_ms`, with the newest server tick (0 = none yet) and ping.
    /// Returns the second's numbers when one has passed.
    pub fn frame(&mut self, now_ms: u64, tick: u32, ping_ms: u32) -> Option<Perf> {
        let Some((t0, tick0)) = self.since else {
            self.since = Some((now_ms, tick));
            self.frames = 0;
            return None;
        };
        self.frames += 1;
        let dt = now_ms.saturating_sub(t0);
        if dt < 1000 {
            return None;
        }
        let per_s = |n: u64| u32::try_from((n * 1000 + dt / 2) / dt).unwrap_or(u32::MAX);
        // A tick that is unknown or went backwards (a map change) gives no rate this second.
        let tick_rate = if tick0 != 0 && tick > tick0 {
            per_s(u64::from(tick - tick0))
        } else {
            0
        };
        let perf = Perf {
            fps: per_s(u64::from(self.frames)),
            ping_ms,
            tick_rate,
        };
        self.since = Some((now_ms, tick));
        self.frames = 0;
        Some(perf)
    }
}

/// A part of the game's UI that windows should not cover.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Zone {
    /// Stable id (`minimap`, `skills`, ...); `label` is for people.
    pub id: &'static str,
    pub label: &'static str,
    pub rect: Rect,
}

/// The game's UI scale per interface size (small, normal, large, larger), relative to normal.
const UI_SCALE: [f32; 4] = [0.9, 1.0, 1.11, 1.22];

/// Where the game's fixed UI parts are on a `screen`-sized backbuffer: estimates from the
/// interface size, scaled from sizes measured at "normal" (3440x1440, 2026-11-26), except the
/// minimap when the game reports its size. In WvW and PvP (competitive mode) the game reports
/// the compass as 0x0, so the minimap falls back to its measured default size. Chat is at its
/// default size (the player can resize it). None while the map is open (it covers the screen)
/// or before the link was read.
pub fn zones(ui: &GameUi, screen: (i32, i32)) -> Vec<Zone> {
    let (sw, sh) = screen;
    if !ui.known || ui.ui_state & UI_MAP_OPEN != 0 || sw <= 0 || sh <= 0 {
        return Vec::new();
    }
    let k = UI_SCALE[usize::from(ui.uisz.min(3))];
    let px = |n: f32| (n * k).round() as i32;
    let top_right = ui.ui_state & UI_COMPASS_TOP_RIGHT != 0;
    let mut out = Vec::new();
    let mut add = |id, label, x: i32, y: i32, w: i32, h: i32| {
        let (x0, y0) = (x.clamp(0, sw), y.clamp(0, sh));
        let (x1, y1) = ((x + w).clamp(0, sw), (y + h).clamp(0, sh));
        if x1 > x0 && y1 > y0 {
            out.push(Zone {
                id,
                label,
                rect: Rect::new(x0, y0, x1 - x0, y1 - y0),
            });
        }
    };
    // The minimap and the strip of buttons on its inner edge. With the minimap at the bottom
    // the event tracker starts at the top of the screen; at the top, below the minimap.
    let (cw, h) = if ui.compass_w > 0 && ui.compass_h > 0 {
        let ch = i32::from(ui.compass_h);
        (i32::from(ui.compass_w), ch + px(36.0))
    } else {
        (px(362.0), px(450.0))
    };
    let tracker_y = if top_right {
        add("minimap", "Minimap", sw - cw, 0, cw, h);
        h + px(16.0)
    } else {
        add("minimap", "Minimap", sw - cw, sh - h, cw, h);
        0
    };
    let skills = (px(800.0), px(175.0));
    add(
        "skills",
        "Skill bar",
        (sw - skills.0) / 2,
        sh - skills.1,
        skills.0,
        skills.1,
    );
    add(
        "chat",
        "Chat (default size)",
        0,
        sh - px(290.0),
        px(550.0),
        px(290.0),
    );
    add("menu", "Menu bar", 0, 0, px(320.0), px(42.0));
    // Blish HUD's corner icons: a row of 32 px icons after the menu, as many as its modules add.
    if ui.blish {
        add("blish", "Blish HUD icons", px(320.0), 0, px(32.0 * 12.0), px(42.0));
    }
    // Nexus's quick-access row: one icon per addon with a shortcut, in the same slot.
    if ui.nexus {
        add("nexus", "Nexus quick access", px(320.0), 0, px(32.0 * 12.0), px(42.0));
    }
    // Wide enough for the squad panel, which is wider than the party one.
    add("party", "Party / squad", 0, px(44.0), px(400.0), px(330.0));
    let target = (px(560.0), px(120.0));
    add(
        "target",
        "Target",
        (sw - target.0) / 2,
        0,
        target.0,
        target.1,
    );
    add(
        "tracker",
        "Event tracker",
        sw - px(320.0),
        tracker_y,
        px(320.0),
        px(560.0),
    );
    out
}

/// True if `a` and `b` overlap.
pub fn overlaps(a: Rect, b: Rect) -> bool {
    let (ax, ay, bx, by) = (
        i64::from(a.x),
        i64::from(a.y),
        i64::from(b.x),
        i64::from(b.y),
    );
    ax < b.right() && bx < a.right() && ay < b.bottom() && by < a.bottom()
}

/// Snap a window being dragged to `r` (screen space) onto the screen's edges and centre lines and
/// onto the edges of `others`, if one lies within `threshold` pixels. A window placed beside,
/// above or below another keeps `gap` pixels from it; aligned edges stay flush. Each axis snaps on
/// its own to the nearest candidate.
pub fn snap(r: Rect, others: &[Rect], screen: (i32, i32), threshold: i32, gap: i32) -> (i32, i32) {
    let (sw, sh) = screen;
    // Candidate positions for the window's left (top) edge, per axis.
    let mut xs = vec![0, sw - r.w, (sw - r.w) / 2];
    let mut ys = vec![0, sh - r.h, (sh - r.h) / 2];
    for o in others {
        // Only edges the window could actually sit against or line up with: overlapping in the
        // other axis (with the threshold as slack) for side-by-side placement, any for alignment.
        let near_y = r.y < o.y + o.h + threshold + gap && o.y < r.y + r.h + threshold + gap;
        let near_x = r.x < o.x + o.w + threshold + gap && o.x < r.x + r.w + threshold + gap;
        if near_y {
            xs.extend([o.x + o.w + gap, o.x - r.w - gap, o.x, o.x + o.w - r.w]);
        }
        if near_x {
            ys.extend([o.y + o.h + gap, o.y - r.h - gap, o.y, o.y + o.h - r.h]);
        }
    }
    let pick = |v: i32, c: &[i32]| {
        c.iter()
            .copied()
            .filter(|&p| (p - v).abs() <= threshold)
            .min_by_key(|&p| (p - v).abs())
            .unwrap_or(v)
    };
    (pick(r.x, &xs), pick(r.y, &ys))
}

/// Keep at least `keep` pixels of a `w`x`h` window at (x, y) on a `sw`x`sh` screen, with its top
/// edge (the title bar) always on screen.
pub fn keep_on_screen(x: i32, y: i32, w: i32, h: i32, sw: i32, sh: i32, keep: i32) -> (i32, i32) {
    let keep_x = keep.min(w.max(1));
    let x = x.clamp(keep_x - w, (sw - keep_x).max(0));
    let y = y.clamp(0, (sh - keep.min(h.max(1))).max(0));
    (x, y)
}

#[cfg(test)]
mod tests {
    #[test]
    fn grab_target_finds_the_hand_off() {
        let r = |x, y| Rect::new(x, y, 0, 0);
        let ws = [(1u64, FLAG_GROUP, r(0, 0)), (2, 0, r(0, 0)), (3, FLAG_GRAB, r(171, 12))];
        assert_eq!(grab_target(ws, 1), Some((3, 171, 12)));
        // The pulled window itself never takes it; nothing flagged: none.
        assert_eq!(grab_target([(1u64, FLAG_GROUP | FLAG_GRAB, r(5, 5))], 1), None);
        assert_eq!(grab_target([(2u64, 0, r(0, 0))], 1), None);
    }

    #[test]
    fn grab_packs_round_trip() {
        assert_eq!(unpack_grab(pack_grab(5, 300)), (5, 300));
        assert_eq!(unpack_grab(pack_grab(0, 0)), (0, 0));
        assert_eq!(unpack_grab(pack_grab(-4, 70_000)), (0, 65535), "clamped");
    }

    use super::*;

    /// A rect whose far edge overflows i32 is refused, not wrapped into the atlas: the DLL copies
    /// pixels by these numbers on the game's render thread.
    #[test]
    fn huge_rects_are_refused() {
        let r = |x, y, w, h| Rect { x, y, w, h };
        assert_eq!(clip_to_atlas(r(i32::MAX - 255, 0, 512, 1)), None);
        assert_eq!(clip_to_atlas(r(0, i32::MAX - 255, 1, 512)), None);
        assert_eq!(clip_to_atlas(r(0, 0, i32::MAX, i32::MAX)), None);
        assert_eq!(clip_to_atlas(r(0, 0, 10, 10)), Some(r(0, 0, 10, 10)));
        assert!(!r(i32::MAX - 1, 0, 10, 10).contains(0, 5));
        assert!(!overlaps(r(i32::MAX - 1, 0, 10, 10), r(0, 0, 5, 5)));
    }

    #[test]
    fn layout_fits_header_area() {
        assert!(RING_OFF + RING as usize * INPUT_LEN <= DLL_HB_OFF);
        assert!(EDIT_OFF + 4 <= GAMEUI_OFF);
        assert!(GAMEUI_OFF + 12 <= TABLE_OFF[0]);
        assert!(TABLE_OFF[0] + 16 + MAX_WINDOWS * WIN_LEN <= TABLE_OFF[1]);
        assert!(TABLE_OFF[1] + 16 + MAX_WINDOWS * WIN_LEN <= FRAME_OFF);
    }

    #[test]
    fn table_round_trip() {
        let mut shm = vec![0u8; FRAME_OFF];
        let mut w = Window {
            atlas: Rect::new(10, 0, 530, 400),
            sx: 60,
            sy: -5,
            drag: Rect::new(0, 0, 400, 28),
            flags: FLAG_EDIT_INTERACTIVE,
            radius: 12,
            blur: 2,
            ..Default::default()
        };
        w.set_name("meter");
        let t = Table {
            edit_seq: 3,
            flags: TABLE_HIDE_ZONES | TABLE_TEXT_FOCUS,
            windows: vec![w],
        };
        t.write(&mut shm, 1);
        assert_eq!(Table::read(&shm, 1), t);
        assert_eq!(Table::read(&shm, 1).windows[0].name(), "meter");
        assert_eq!(Table::read(&shm, 0), Table::default());
    }

    #[test]
    fn key_input_round_trips() {
        for name in ["a", "M", "é", "€", "Backspace", "ArrowRight"] {
            let i = Input::new(kind::KEY, 0, 0.0, 0.0, 0.0).named(name);
            let back = Input::decode(&i.encode());
            assert_eq!(back, i);
            assert_eq!(back.kind, kind::KEY);
            assert_eq!(back.name(), name);
        }
    }

    #[test]
    fn table_drops_rects_outside_the_atlas() {
        let mut shm = vec![0u8; FRAME_OFF];
        let ok = Window {
            atlas: Rect::new(0, 0, 10, 10),
            ..Default::default()
        };
        let bad = Window {
            atlas: Rect::new(2040, 0, 10, 10),
            ..Default::default()
        };
        Table {
            edit_seq: 0,
            flags: 0,
            windows: vec![ok, bad],
        }
        .write(&mut shm, 0);
        assert_eq!(Table::read(&shm, 0).windows, vec![ok]);
    }

    #[test]
    fn game_ui_round_trip() {
        let mut shm = vec![0u8; FRAME_OFF];
        let ui = GameUi {
            known: true,
            ui_state: UI_COMPASS_TOP_RIGHT | 8,
            uisz: 2,
            compass_w: 362,
            compass_h: 330,
            blish: true,
            nexus: false,
        };
        for (i, v) in ui.encode().iter().enumerate() {
            shm[GAMEUI_OFF + 4 * i..GAMEUI_OFF + 4 * i + 4].copy_from_slice(&v.to_le_bytes());
        }
        assert_eq!(GameUi::read(&shm), ui);
        assert_eq!(GameUi::read(&vec![0u8; FRAME_OFF]), GameUi::default());
        // Nexus has a bit of its own, independent of Blish's.
        for (blish, nexus) in [(false, true), (true, true)] {
            let ui = GameUi { blish, nexus, ..ui };
            for (i, v) in ui.encode().iter().enumerate() {
                shm[GAMEUI_OFF + 4 * i..GAMEUI_OFF + 4 * i + 4].copy_from_slice(&v.to_le_bytes());
            }
            assert_eq!(GameUi::read(&shm), ui);
        }
    }

    #[test]
    fn zones_follow_the_compass_corner_and_interface_size() {
        let s = (2560, 1440);
        let mut ui = GameUi {
            known: true,
            ui_state: 0,
            uisz: 1,
            compass_w: 360,
            compass_h: 340,
            blish: false,
            nexus: false,
        };
        let z = zones(&ui, s);
        let get = |z: &[Zone], id: &str| z.iter().find(|z| z.id == id).unwrap().rect;
        assert_eq!(get(&z, "minimap"), Rect::new(2200, 1440 - 376, 360, 376));
        assert_eq!(
            get(&z, "skills"),
            Rect::new((2560 - 800) / 2, 1440 - 175, 800, 175)
        );
        assert_eq!(get(&z, "chat").x, 0);
        assert_eq!(get(&z, "tracker").y, 0);
        ui.ui_state = UI_COMPASS_TOP_RIGHT;
        let z = zones(&ui, s);
        assert_eq!(get(&z, "minimap").y, 0);
        assert!(get(&z, "tracker").y >= 376);
        ui.uisz = 3;
        assert_eq!(get(&zones(&ui, s), "skills").w, 976);
        // Every zone lies on screen; none while the map is open or the link is unknown.
        assert!(
            zones(&ui, s).iter().all(|z| z.rect.x >= 0
                && z.rect.x + z.rect.w <= 2560
                && z.rect.y + z.rect.h <= 1440)
        );
        ui.ui_state |= UI_MAP_OPEN;
        assert!(zones(&ui, s).is_empty());
        assert!(zones(&GameUi::default(), s).is_empty());
    }

    // Measured in WvW at normal interface size, 3440x1440, where the game reports a 0x0 compass.
    #[test]
    fn zones_measured_in_wvw() {
        let ui = GameUi {
            known: true,
            ui_state: 0b11000,
            uisz: 1,
            compass_w: 0,
            compass_h: 0,
            blish: false,
            nexus: false,
        };
        let z = zones(&ui, (3440, 1440));
        let get = |id: &str| z.iter().find(|z| z.id == id).unwrap().rect;
        assert_eq!(z.len(), 7);
        assert_eq!(get("minimap"), Rect::new(3440 - 362, 1440 - 450, 362, 450));
        assert_eq!(get("tracker"), Rect::new(3440 - 320, 0, 320, 560));
        assert_eq!(get("skills"), Rect::new(1320, 1440 - 175, 800, 175));
        assert_eq!(get("party"), Rect::new(0, 44, 400, 330));
        assert_eq!(get("chat"), Rect::new(0, 1440 - 290, 550, 290));
        assert!(z.iter().all(|z| z.id != "blish" && z.id != "nexus"));
        let z = zones(&GameUi { blish: true, ..ui }, (3440, 1440));
        assert_eq!(
            z.iter().find(|z| z.id == "blish").unwrap().rect,
            Rect::new(320, 0, 384, 42)
        );
        assert!(z.iter().all(|z| z.id != "nexus"));
        // Nexus's quick-access icons take the same slot.
        let z = zones(&GameUi { nexus: true, ..ui }, (3440, 1440));
        assert_eq!(
            z.iter().find(|z| z.id == "nexus").unwrap().rect,
            Rect::new(320, 0, 384, 42)
        );
        assert!(z.iter().all(|z| z.id != "blish"));
        let z = zones(&GameUi { blish: true, nexus: true, ..ui }, (3440, 1440));
        assert!(z.iter().any(|z| z.id == "blish") && z.iter().any(|z| z.id == "nexus"));
    }

    #[test]
    fn perf_counts_a_second() {
        let mut c = PerfCounter::default();
        assert_eq!(c.frame(0, 1000, 80), None);
        // 60 frames over one second, 25 server ticks.
        let mut out = None;
        for i in 1..=60u64 {
            out = c.frame(i * 1000 / 60, 1000 + (i as u32 * 25 / 60), 80);
            if i < 60 {
                assert_eq!(out, None);
            }
        }
        assert_eq!(
            out,
            Some(Perf {
                fps: 60,
                ping_ms: 80,
                tick_rate: 25
            })
        );
        // A tick reset (map change) gives no rate; no tick yet gives none either.
        for i in 1..=30u64 {
            out = c.frame(1000 + i * 1000 / 30, 5, 90);
        }
        assert_eq!(out.unwrap().tick_rate, 0);
        assert_eq!(out.unwrap().fps, 30);
        let mut shm = vec![0u8; FRAME_OFF];
        let p = Perf {
            fps: 60,
            ping_ms: 80,
            tick_rate: 25,
        };
        for (i, v) in p.encode().iter().enumerate() {
            shm[PERF_OFF + 4 * i..PERF_OFF + 4 * i + 4].copy_from_slice(&v.to_le_bytes());
        }
        assert_eq!(Perf::read(&shm), p);
        assert!(PERF_OFF + 12 <= TABLE_OFF[0]);
    }

    #[test]
    fn overlap_is_strict() {
        let a = Rect::new(0, 0, 10, 10);
        assert!(overlaps(a, Rect::new(9, 9, 5, 5)));
        assert!(!overlaps(a, Rect::new(10, 0, 5, 5)));
    }

    #[test]
    fn input_round_trip() {
        let i = Input::new(kind::MOVED, 2, 61.0, 220.0, 0.0).named("support");
        let d = Input::decode(&i.encode());
        assert_eq!(d, i);
        assert_eq!(d.name(), "support");
    }

    #[test]
    fn snaps_to_screen_edges_and_centre() {
        let s = (2560, 1440);
        assert_eq!(snap(Rect::new(7, 300, 200, 100), &[], s, 12, 0), (0, 300));
        assert_eq!(
            snap(Rect::new(2355, 1335, 200, 100), &[], s, 12, 0),
            (2360, 1340)
        );
        assert_eq!(
            snap(Rect::new(1175, 300, 200, 100), &[], s, 12, 0),
            (1180, 300)
        );
        assert_eq!(
            snap(Rect::new(500, 300, 200, 100), &[], s, 12, 0),
            (500, 300)
        );
    }

    #[test]
    fn snaps_beside_and_in_line_with_other_windows() {
        let s = (2560, 1440);
        let meter = Rect::new(60, 220, 530, 400);
        // Right of the meter, tops nearly level: sits against it, tops aligned.
        assert_eq!(
            snap(Rect::new(596, 226, 300, 200), &[meter], s, 12, 0),
            (590, 220)
        );
        // Below the meter, left edges nearly level.
        assert_eq!(
            snap(Rect::new(64, 628, 300, 200), &[meter], s, 12, 0),
            (60, 620)
        );
        // With a gap: beside and below keep it, aligned edges stay flush.
        assert_eq!(
            snap(Rect::new(600, 226, 300, 200), &[meter], s, 12, 6),
            (596, 220)
        );
        assert_eq!(
            snap(Rect::new(64, 630, 300, 200), &[meter], s, 12, 6),
            (60, 626)
        );
        assert_eq!(
            snap(Rect::new(52, 220, 300, 200), &[meter], s, 12, 6).1,
            220
        );
        // Far away in y: no side-by-side snap to the meter's right edge.
        assert_eq!(
            snap(Rect::new(596, 900, 300, 200), &[meter], s, 12, 0),
            (596, 900)
        );
    }

    #[test]
    fn keeps_title_bar_on_screen() {
        assert_eq!(
            keep_on_screen(-600, -20, 530, 400, 1920, 1080, 40),
            (-490, 0)
        );
        assert_eq!(
            keep_on_screen(1900, 1070, 530, 400, 1920, 1080, 40),
            (1880, 1040)
        );
        assert_eq!(
            keep_on_screen(100, 100, 530, 400, 1920, 1080, 40),
            (100, 100)
        );
    }
}
