// axidps in-game window manager and built-in windows. See index.html for the host contract.
"use strict";

// The helpers plugins get too (sdk/axidps.js, loaded before this file): number and time
// formatting, HTML escaping, profession colours.
const {fmt, esc, PROF} = axidpsKit;
// Rows a player table shows at most (Settings; any value up to a full squad), the rest by wheel.
const MAX_ROWS = 50;
function maxRows() { return Math.max(1, Math.min(MAX_ROWS, Math.round(Number(saved.rows) || 15))); }


// ---- themes --------------------------------------------------------------------------------

// A theme is a full style: every colour token index.html uses (the glyph and chart inks read them
// too), the shape and type (s), and the look it starts with (opacity, blur), which the hub can
// change afterwards.
//   s.font: CSS font stack (IBM Plex Sans, Fira Code and Lato are bundled in fonts/)
//   s.fs: font size, s.r: window corner radius, s.rs: control radius, s.bw: border width,
//   s.rh: table row height, s.bh: title bar height (px)
//   s.bar: title bar "fill" | "flat" (window colour) | "line" (accent underline)
//   s.rows: meter row bars "fill" (behind the name) | "line" (thin bar under it)
//   s.caps: uppercase titles and headers
//   look: [opacity 0.5-1, backdrop blur 0-3]
const TOKENS = ["bg", "bar", "raise", "raise-hi", "btn", "btn-hi", "line", "line-hi", "text", "title",
  "dim", "faint", "accent", "accent-hi", "accent-line", "accent-soft", "good", "bad", "tip"];
// The tokens that are window surfaces: the opacity setting applies to them.
const SURFACES = ["bg", "bar", "raise", "raise-hi"];
const SYS = 'system-ui, "DejaVu Sans", sans-serif';
const THEMES = [
  {id: "midnight", label: "Midnight", t: ["#12151b", "#12151b", "#1a1e26", "#222833", "#232a35", "#2e3746",
    "#242a34", "#343c49", "#e8eaee", "#f2f4f7", "#8d96a5", "#646d7a", "#7aa2f7", "#a9c1ff", "#34456a",
    "rgba(122,162,247,.13)", "#9ece6a", "#f7768e", "#0b0d11"],
  s: {font: '"IBM Plex Sans", ' + SYS, fs: 12, r: 10, rs: 6, bw: 1, rh: 21, bh: 32, bar: "flat", rows: "fill"},
  look: [0.94, 1]},
  {id: "glass", label: "Glass", t: ["#0e1422", "#0e1422", "#1a2336", "#24304a", "#223050", "#2d3f66",
    "#2a3550", "#3d4c6e", "#f1f5fb", "#dfe7f5", "#9aa8c0", "#71809a", "#8ab4ff", "#bcd3ff", "#33507f",
    "rgba(138,180,255,.16)", "#7ee0a1", "#ff8fa3", "#0a0f1a"],
  s: {font: '"IBM Plex Sans", ' + SYS, fs: 12, r: 14, rs: 8, bw: 1, rh: 21, bh: 30, bar: "flat", rows: "line"},
  look: [0.62, 3]},
  {id: "terminal", label: "Terminal", t: ["#040805", "#07100a", "#0a160d", "#0f2114", "#0c1a10", "#143020",
    "#173a22", "#24593a", "#b8f5c0", "#7dffa0", "#5f9a6a", "#3f6b49", "#39ff88", "#a4ffc6", "#1d5c34",
    "rgba(57,255,136,.12)", "#39ff88", "#ff5f56", "#020403"],
  s: {font: '"Fira Code", monospace', fs: 11.5, r: 0, rs: 0, bw: 1, rh: 18, bh: 26, bar: "line", rows: "line",
    caps: true}, look: [0.9, 0]},
  {id: "ember", label: "Ember", t: ["#1a1512", "#241c17", "#211a15", "#2b221b", "#2e241d", "#3d3026",
    "#3a2e24", "#4d3d30", "#f0e6d8", "#e8c77f", "#a89580", "#7d6c5a", "#e8b04a", "#f6cd7d", "#5c4523",
    "rgba(232,176,74,.14)", "#a7c957", "#e86a5a", "#110d0b"],
  s: {font: "Lato, " + SYS, fs: 12.5, r: 3, rs: 2, bw: 1, rh: 20, bh: 30, bar: "fill", rows: "fill", caps: true},
  look: [1, 0]},
  {id: "verdant", label: "Verdant", t: ["#121a17", "#18231f", "#1b2924", "#22332c", "#1f2c27", "#2a3b34",
    "#25342e", "#33463e", "#e2ece7", "#c9d8d1", "#8aa198", "#66796f", "#4fd1a5", "#8ee5c6", "#2a5446",
    "rgba(79,209,165,.14)", "#a3d977", "#f08a7e", "#0b110f"],
  s: {font: '"IBM Plex Sans", ' + SYS, fs: 12, r: 10, rs: 6, bw: 1, rh: 21, bh: 30, bar: "flat", rows: "fill"},
  look: [0.85, 1]},
  {id: "frost", label: "Frost", t: ["#f5f7fa", "#e8ecf2", "#e9edf3", "#dfe5ee", "#e2e7ee", "#d4dbe5",
    "#cdd5df", "#b5c0ce", "#1d2430", "#2e3747", "#5c6878", "#808b9a", "#2f6fdb", "#1d55b5", "#a9c2ee",
    "rgba(47,111,219,.12)", "#2f8a3e", "#c8324a", "#ffffff"],
  s: {font: "Lato, " + SYS, fs: 12.5, r: 10, rs: 6, bw: 1, rh: 20, bh: 30, bar: "flat", rows: "line"},
  look: [0.8, 2]},
  {id: "contrast", label: "Contrast", t: ["#000000", "#0d0d0d", "#0a0a0a", "#1a1a1a", "#141414", "#2a2a2a",
    "#5a5a5a", "#8a8a8a", "#ffffff", "#ffffff", "#d0d0d0", "#b0b0b0", "#ffd400", "#ffe866", "#8a7300",
    "rgba(255,212,0,.18)", "#3dff7a", "#ff4d6a", "#000000"],
  s: {font: SYS, fs: 13, r: 0, rs: 0, bw: 2, rh: 21, bh: 30, bar: "fill", rows: "fill"}, look: [1, 0]},
];
const OPACITY = [1, 0.9, 0.8, 0.7, 0.6, 0.5];
const BLUR = ["Off", "Soft", "Medium", "Strong"];
let theme = THEMES[0];
// The token values applyTheme last set on :root (opacity applied); plugin frames get the same.
let lookTokens = {};
// A theme token's value, e.g. T("accent").
function T(k) { return theme.t[TOKENS.indexOf(k)]; }
// "#rrggbb" at opacity a, as rgba().
function alpha(hex, a) {
  if (a >= 1 || hex[0] !== "#") return hex;
  const n = parseInt(hex.slice(1), 16);
  return `rgba(${n >> 16},${(n >> 8) & 255},${n & 255},${a})`;
}
// Apply the saved theme and look (saved.theme, saved.opacity, saved.blur).
function applyTheme() {
  theme = THEMES.find(t => t.id === saved.theme) || THEMES[0];
  const op = Math.max(0.5, Math.min(1, Number(saved.opacity) || 1));
  const root = document.documentElement.style;
  lookTokens = {};
  TOKENS.forEach((k, i) => {
    lookTokens[k] = SURFACES.includes(k) ? alpha(theme.t[i], op) : theme.t[i];
    root.setProperty("--" + k, lookTokens[k]);
  });
  const st = theme.s;
  root.setProperty("--font", st.font);
  for (const k of ["fs", "r", "rs", "bw", "rh", "bh"]) root.setProperty("--" + k, st[k] + "px");
  // The hub stays readable whatever the windows' opacity.
  root.setProperty("--bg-modal", alpha(T("bg"), Math.max(op, 0.9)));
  root.setProperty("--nav-modal", alpha(T("raise"), Math.max(op, 0.9)));
  const b = document.body.classList;
  b.toggle("bar-flat", st.bar === "flat");
  b.toggle("bar-line", st.bar === "line");
  b.toggle("caps", !!st.caps);
  // Fonts load asynchronously: lay out and report again once they have.
  setTimeout(renderAll, 300);
  setTimeout(renderAll, 1500);
}
// Pick a theme: it brings its own look.
function pickTheme(id) {
  const t = THEMES.find(x => x.id === id) || THEMES[0];
  Object.assign(saved, {theme: t.id, opacity: t.look[0], blur: t.look[1]});
  applyTheme();
  save();
  renderAll();
}
// The backdrop blur to ask the DLL for (none behind opaque windows).
function blurLevel() {
  return (Number(saved.opacity) || 1) < 1 ? Math.max(0, Math.min(3, saved.blur | 0)) : 0;
}

// Glyphs from the axi design language (axi-design/icons, rule 12): 24 canvas, 3px stroke,
// mitered, butt caps, 0/45/90 degree angles, no curves. "chart" is drawn here to the same rules.
const GLYPHS = {
  chart: ["M5.5 20.5 L5.5 13.5", "M12 20.5 L12 8.5", "M18.5 20.5 L18.5 3.5"],
  users: ["M9 2.5 L13 6.5 L9 10.5 L5 6.5 Z", "M1.5 20.5 L1.5 18.5 L5.5 14.5 L12.5 14.5 L16.5 18.5 L16.5 20.5",
    "M17 5.5 L20 8.5 L17 11.5", "M17 14.5 L19.5 14.5 L22.5 17.5 L22.5 20.5"],
  shield: ["M5.5 3.5 L18.5 3.5 L18.5 14.5 L12 21 L5.5 14.5 Z"],
  swords: ["M3.5 6.5 L16.5 19.5", "M20.5 6.5 L7.5 19.5", "M12.5 18.5 L15.5 15.5", "M8.5 15.5 L11.5 18.5"],
  activity: ["M2.5 12 L6 12 L9.5 15.5 L15.5 9.5 L18 12 L21.5 12"],
  "maximize-2": ["M14.5 3.5 L20.5 3.5 L20.5 9.5", "M9.5 20.5 L3.5 20.5 L3.5 14.5", "M20.5 3.5 L3.5 20.5"],
  "trash-2": ["M2.5 6.5 L21.5 6.5", "M5.5 6.5 L5.5 21.5 L18.5 21.5 L18.5 6.5", "M9.5 6.5 L9.5 2.5 L14.5 2.5 L14.5 6.5"],
  x: ["M5.5 5.5 L18.5 18.5", "M18.5 5.5 L5.5 18.5"],
  grid: ["M3.5 3.5 L10.5 3.5 L10.5 10.5 L3.5 10.5 Z", "M13.5 3.5 L20.5 3.5 L20.5 10.5 L13.5 10.5 Z",
    "M3.5 13.5 L10.5 13.5 L10.5 20.5 L3.5 20.5 Z", "M13.5 13.5 L20.5 13.5 L20.5 20.5 L13.5 20.5 Z"],
  layers: ["M12 3.5 L20.5 8 L12 12.5 L3.5 8 Z", "M3.5 12.5 L12 17 L20.5 12.5", "M3.5 16.5 L12 21 L20.5 16.5"],
  sliders: ["M3.5 6.5 L20.5 6.5", "M3.5 17.5 L20.5 17.5", "M8.5 3.5 L8.5 9.5", "M15.5 14.5 L15.5 20.5"],
  plug: ["M8.5 2.5 L8.5 7.5", "M15.5 2.5 L15.5 7.5", "M4.5 7.5 L19.5 7.5 L19.5 12.5 L14.5 17.5 L9.5 17.5 L4.5 12.5 Z",
    "M12 17.5 L12 21.5"],
  download: ["M12 2.5 L12 15.5", "M6.5 10 L12 15.5 L17.5 10", "M3.5 20.5 L20.5 20.5"],
  columns: ["M3.5 3.5 L20.5 3.5 L20.5 20.5 L3.5 20.5 Z", "M9.5 3.5 L9.5 20.5", "M14.5 3.5 L14.5 20.5"],
  plus: ["M12 3.5 L12 20.5", "M3.5 12 L20.5 12"],
};
// Glyphs are drawn on canvases, not inline SVG: Servo rasterises new SVG asynchronously, so the
// first frame after one is inserted paints its window white. A canvas draws synchronously.
// ink: a theme token name (dim, accent, ...), resolved when drawn, so a theme change repaints.
function icon(name, size, ink = "dim") {
  return `<canvas class="ico" data-g="${name}" data-ink="${ink}" width="${size}" height="${size}"></canvas>`;
}
// Draw every glyph canvas under root whose glyph or ink changed.
function paintIcons(root) {
  root.querySelectorAll("canvas.ico").forEach(cv => {
    const key = cv.dataset.g + cv.dataset.ink + theme.id;
    if (cv.dataset.drawn === key) return;
    cv.dataset.drawn = key;
    const g = cv.getContext("2d"), k = cv.width / 24;
    g.clearRect(0, 0, cv.width, cv.height);
    g.save(); g.scale(k, k);
    g.strokeStyle = T(cv.dataset.ink); g.lineWidth = 3; g.lineJoin = "miter"; g.lineCap = "butt";
    for (const d of GLYPHS[cv.dataset.g]) {
      g.beginPath();
      for (const [, op, x, y] of d.matchAll(/([MLZ])\s*(?:([-\d.]+)\s+([-\d.]+))?/g)) {
        if (op === "M") g.moveTo(+x, +y); else if (op === "L") g.lineTo(+x, +y); else g.closePath();
      }
      g.stroke();
    }
    g.restore();
  });
}

// ---- state ---------------------------------------------------------------------------------

const ui = {
  live: null,          // core's live feed
  cleared: false,      // Clear fight (hub): blank until the next fight starts
  screen: [2560, 1440],
  editing: false,      // the DLL's edit mode, as reported by the host
  editSeq: 0,          // our edit requests: odd = on, even = off
  session: {},         // open state of windows that always start closed (the hub)
  update: null,        // the launcher's self-update status (host, from update-status.json)
  zones: [],           // the game's own UI (host, from MumbleLink): [{id, label, x, y, w, h}]
  notice: "",          // the hub's Windows tab note (a window that could not open)
};
// Persisted by the host in ~/.config/axidps/ui.json.
// pos/open/cfg are keyed by window key (w.key: a plugin window's full id `<plugin>/<window>`,
// else its id); plugins: {<plugin id>: {enabled, granted}}; dev: developer mode (the host reads it at
// start).
let saved = {v: 1, pos: {}, open: {}, cfg: {}, theme: "midnight", opacity: 0.94, blur: 1, rows: 15, core: {}, app: {},
  tables: [], hideZones: false, compact: false, plugins: {}, dev: false};
const wins = [];

// Every page -> host message goes through `toHost`: the host puts a per-run token in the page URL's
// fragment, read once here and cleared, and accepts only messages prefixed with it. Plugin frames
// share this webview's console but cannot see the token, so they cannot forge host messages.
// Run by the host (it passes the token), else the browser demo.
const HOSTED = /(?:^#|&)t=[0-9a-f]+/.test(location.hash);
const toHost = (() => {
  const m = /(?:^#|&)t=([0-9a-f]+)/.exec(location.hash);
  const prefix = "axidps:" + (m ? m[1] : "") + ":";
  if (m) history.replaceState(null, "", location.pathname + location.search);
  return text => console.log(prefix + text);
})();
function send(obj) { toHost(JSON.stringify(obj)); }
// index.html?test (the harness): plugin windows' status and error counts go to the host log.
const TEST = /(?:^\?|&)test(?:[&=]|$)/.test(location.search);

let saveTimer = 0;
function save() {
  clearTimeout(saveTimer);
  saveTimer = setTimeout(() => send({save: saved}), 300);
}
// Save now (before the host restarts).
function saveNow() {
  clearTimeout(saveTimer);
  send({save: saved});
}
// Developer mode switches at the host's next start: save it, then tell the host, which restarts.
function setDev(on) {
  saved.dev = !!on;
  saveNow();
  send({dev: saved.dev});
  renderAll();
}

// ---- window manager ------------------------------------------------------------------------

// def: {id, key (saved under; default id), title, open (default), editOnly, ed (interactive in edit mode),
//       bare (no title bar), session (always starts closed), modal (dims the game, takes all
//       input; a click outside or Esc closes it), fixed (always at its default spot),
//       tools: [{id, tip, label}], cfg: defaults, at([sw, sh], el) -> default [x, y],
//       mount(w), render(w), tool(w, id)}
function defineWindow(def) {
  const el = document.createElement("div");
  el.className = "win" + (def.ed ? " ed" : "") + (def.bare ? " bare" : "") + (def.modal ? " modal" : "");
  el.dataset.id = def.id;
  const tools = (def.tools || []).map(t =>
    `<button data-tool="${t.id}" data-tip="${esc(t.tip)}">${esc(t.label)}</button>`).join("");
  el.innerHTML = (def.bare ? "" : `<div class="bar"><b>${esc(def.title)}</b><span class="state"></span>` +
    `<div class="tools">${tools}</div></div>`) + `<div class="body"></div><div class="tip"></div>` +
    `<div class="edit"><b>${esc(def.blurb ? def.blurb[0] : def.title)}</b><small>drag to move</small></div>`;
  document.getElementById("atlas").appendChild(el);
  const w = {def, el, id: def.id, key: def.key || def.id, bar: el.querySelector(".bar"), tools: el.querySelector(".tools"),
    state: el.querySelector(".state"), body: el.querySelector(".body"), tip: el.querySelector(".tip"),
    offset: 0};
  Object.defineProperty(w, "cfg", {get() {
    return saved.cfg[w.key] = Object.assign({}, def.cfg || {}, saved.cfg[w.key] || {});
  }});
  // Windows open and close only from the hub (Windows tab).
  el.querySelectorAll("[data-tool]").forEach(b => b.onclick = () => def.tool && def.tool(w, b.dataset.tool));
  if (def.mount) def.mount(w);
  paintIcons(el);
  wins.push(w);
  return w;
}
function removeWindow(w) {
  w.el.remove();
  const i = wins.indexOf(w);
  if (i >= 0) wins.splice(i, 1);
}

// Open by the player's choice or the window's default, before the window cap.
function wantsOpen(w) {
  if (w.def.editOnly) return ui.editing;
  if (w.def.session) return !!ui.session[w.id] && !ui.editing;
  const o = saved.open[w.key];
  return o === undefined ? !!w.def.open : o;
}
function isOpen(w) { return wantsOpen(w) && !w.overCap && !w.overAtlas; }
// The DLL's window table holds 16 windows. The hub and the Arrange window always get a slot (they
// are never refused, so the player can always reach the hub); every other window shares the rest.
const MAX_OPEN = 16;
const CAP = MAX_OPEN - 2;
const CAP_NOTE = `${MAX_OPEN} windows are open; close one first`;
const capped = w => !w.def.session && !w.def.editOnly;
// Windows open without a setOpen (defaults, a restored layout, plugin windows) past the cap count
// as closed, the later ones first, and the hub says so.
function applyCap() {
  const over = pluginHost.overCap(wins.filter(capped), w => wantsOpen(w) && !w.overAtlas, CAP);
  for (const w of wins) w.overCap = over.includes(w);
  if (over.length) ui.notice = CAP_NOTE;
}
// The atlas (2048x2048, the shared frame's size) holds every open window side by side; a window
// that doesn't fit there can't be shown. Like the cap: the first window that falls outside counts
// as closed (the later ones move up into its place) and the hub says why; checked after each
// layout (`report`), and again from scratch when the player opens or closes a window or the plugin
// list changes.
const ATLAS = 2048;
const ATLAS_NOTE = "No room for more windows; close one first";
function refit() {
  for (const w of wins) w.overAtlas = false;
}
function overflowing() {
  return wins.find(w => {
    if (!isOpen(w) || !capped(w)) return false;
    const r = w.el.getBoundingClientRect();
    return Math.floor(r.left) + Math.ceil(r.width) > ATLAS || Math.floor(r.top) + Math.ceil(r.height) > ATLAS;
  });
}
function setOpen(w, on) {
  if (on && capped(w) && !isOpen(w) && wins.filter(x => capped(x) && isOpen(x)).length >= CAP) {
    ui.notice = CAP_NOTE;
    renderAll();
    return;
  }
  ui.notice = "";
  refit();
  if (w.def.session) ui.session[w.id] = on;
  else { saved.open[w.key] = on; save(); }
  renderAll();
}
function posOf(w) {
  if (w.def.fixed) return w.def.at(ui.screen, w.el);
  const p = saved.pos[w.key];
  if (Array.isArray(p) && p.length === 2) return p;
  return w.def.at ? w.def.at(ui.screen, w.el) : [80, 80];
}
// Default spot for a window next to the meter: `col` windows to the right (by the widths of
// `after`), `dy` below the meter's top; kept on screen.
function besideMeter(after, dy) {
  const m = wins.find(w => w.id === "meter");
  const [mx, my] = posOf(m);
  let x = mx + m.el.offsetWidth + 6;
  for (const id of after) { const w = wins.find(v => v.id === id); x += (w ? w.el.offsetWidth : 0) + 6; }
  const [sw, sh] = ui.screen;
  return [Math.max(0, Math.min(x, sw - 340)), Math.max(0, Math.min(my + dy, sh - 200))];
}
// ---- the game's own UI ----------------------------------------------------------------------

const overlap = (a, b) => a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h;
function rectOf(w, at = posOf(w)) {
  return {x: at[0], y: at[1], w: w.el.offsetWidth, h: w.el.offsetHeight};
}
// The game UI areas an open window covers.
function covers(w) {
  if (!isOpen(w)) return [];
  const r = rectOf(w);
  return ui.zones.filter(z => overlap(r, z));
}
// Windows the player arranges (not the edit-mode tools or the hub).
const movable = () => wins.filter(w => !w.def.editOnly && !w.def.fixed && !w.def.session);
// Move `w` to the nearest spot that covers no game UI (and preferably no other window): next to
// each covered area, then next to the areas those spots cover. Returns false if none fits.
function unblock(w) {
  const r = rectOf(w), [sw, sh] = ui.screen;
  const fits = c => c.x >= 0 && c.y >= 0 && c.x + r.w <= sw && c.y + r.h <= sh;
  const around = c => ui.zones.filter(z => overlap(c, z)).flatMap(z => [
    {x: z.x - r.w, y: c.y}, {x: z.x + z.w, y: c.y}, {x: c.x, y: z.y - r.h}, {x: c.x, y: z.y + z.h}]
    .map(p => ({x: p.x, y: p.y, w: r.w, h: r.h})));
  let cand = around(r);
  cand = cand.concat(cand.flatMap(around));
  const others = movable().filter(o => o !== w && isOpen(o)).map(o => rectOf(o));
  const best = cand.filter(c => fits(c) && !ui.zones.some(z => overlap(c, z)))
    .map(c => [Math.hypot(c.x - r.x, c.y - r.y) + (others.some(o => overlap(c, o)) ? 1e5 : 0), c])
    .sort((a, b) => a[0] - b[0])[0];
  if (!best) return false;
  saved.pos[w.key] = [Math.round(best[1].x), Math.round(best[1].y)];
  return true;
}

// Ask the DLL to enter/leave edit mode: any change of editSeq with the wanted parity. Always a
// change, so a request the DLL missed (its first table after a restart) can be repeated.
function requestEdit(on) {
  if (ui.editing === on) return;
  ui.editSeq += (ui.editSeq & 1) === (on ? 1 : 0) ? 2 : 1;
  scheduleReport();
}

function renderAll() {
  applyCap();
  document.body.classList.toggle("editing", ui.editing);
  document.body.classList.toggle("compact", !!saved.compact);
  for (const w of wins) {
    const open = isOpen(w);
    w.el.classList.toggle("closed", !open);
    if (open && w.def.render) { w.def.render(w); paintIcons(w.el); }
  }
  toPlugins();
  scheduleReport();
}

// ---- what plugin frames follow ---------------------------------------------------------------

// The feed as a window sees it: Clear fight applied (cleared, no players or timeline).
function feedView() {
  const d = ui.live;
  if (!d) return null;
  if (!ui.cleared) return Object.assign({}, d, {cleared: false});
  return Object.assign({}, d, {cleared: true, players: [],
    timeline: {bucket_ms: (d.timeline && d.timeline.bucket_ms) || 1000, out: [], in: []}, healing: {out: [], in: []}});
}
// The theme and display settings (plugin `look`).
function lookOf() {
  return {theme: theme.id, tokens: Object.assign({}, lookTokens), style: Object.assign({}, theme.s),
    opacity: Math.max(0.5, Math.min(1, Number(saved.opacity) || 1)), compact: !!saved.compact, rows: maxRows()};
}
const sentToPlugins = {look: "", edit: false, live: undefined, cleared: false};
// Forward what changed since the last render to the frames.
function toPlugins() {
  const s = sentToPlugins, look = JSON.stringify(lookOf());
  if (look !== s.look) { s.look = look; pluginHost.look(JSON.parse(look)); }
  if (ui.editing !== s.edit) { s.edit = ui.editing; pluginHost.edit(ui.editing); }
  if (ui.live !== s.live || ui.cleared !== s.cleared) {
    s.live = ui.live; s.cleared = ui.cleared;
    pluginHost.fight(feedView());
  }
  pluginHost.sync();
}

let reportTimer = 0, lastReport = "";
function scheduleReport() {
  if (!reportTimer) reportTimer = setTimeout(report, 0);
}
// Tell the host which windows exist, where they are in the atlas and on screen.
function report() {
  reportTimer = 0;
  const over = overflowing();
  if (over) {
    over.overAtlas = true;
    ui.notice = ATLAS_NOTE;
    renderAll(); // lays out again and reports (or finds the next one that doesn't fit)
    return;
  }
  const windows = [];
  for (const w of wins) {
    if (!isOpen(w)) continue;
    const r = w.el.getBoundingClientRect();
    const [sx, sy] = posOf(w);
    // No drag zone: windows are locked in place and move only in edit mode (Arrange).
    windows.push({name: w.id, ax: Math.floor(r.left), ay: Math.floor(r.top),
      w: Math.ceil(r.width), h: Math.ceil(r.height), sx: Math.round(sx), sy: Math.round(sy),
      drag: [0, 0, 0, 0], r: w.def.bare && !w.def.modal ? 0 : theme.s.r,
      blur: w.def.bare && !w.def.modal ? 0 : w.def.modal ? Math.max(2, blurLevel()) : blurLevel(),
      flags: (w.def.ed ? 1 : 0) | (w.def.modal ? 2 : 0)});
  }
  // Never more than the DLL's table holds; the interactive and modal windows go first.
  if (windows.length > MAX_OPEN) {
    const first = windows.filter(x => x.flags), rest = windows.filter(x => !x.flags);
    windows.splice(0, windows.length, ...first.concat(rest).slice(0, MAX_OPEN));
  }
  const msg = JSON.stringify({windows, edit_seq: ui.editSeq, hide_zones: !!saved.hideZones});
  if (msg !== lastReport) { lastReport = msg; toHost(msg); }
}

// Tooltips for [data-tip], drawn inside the hovered window (only window rects reach the game).
addEventListener("mouseover", e => {
  const el = e.target.closest && e.target.closest("[data-tip]");
  const winEl = e.target.closest && e.target.closest(".win");
  for (const w of wins) if (w.el !== winEl) w.tip.style.display = "none";
  if (!winEl) return;
  const tip = winEl.querySelector(".tip");
  if (!el || !el.dataset.tip) { tip.style.display = "none"; return; }
  tip.textContent = el.dataset.tip;
  tip.style.display = "block";
  const wr = winEl.getBoundingClientRect(), r = el.getBoundingClientRect();
  const tw = tip.offsetWidth, h = tip.offsetHeight;
  let x = r.left - wr.left + (r.width - tw) / 2, y = r.bottom - wr.top + 2;
  x = Math.max(2, Math.min(x, wr.width - tw - 4));
  if (y + h > wr.height - 2) y = r.top - wr.top - h - 2;
  tip.style.left = x + "px"; tip.style.top = y + "px";
});
addEventListener("wheel", e => {
  const winEl = e.target.closest && e.target.closest(".win");
  const w = wins.find(w => w.el === winEl);
  if (!w) return;
  w.tip.style.display = "none";
  w.offset += e.deltaY > 0 ? 3 : -3;
  if (w.def.render) w.def.render(w);
}, {passive: true});

// ---- player tables (meter, support, defense) -----------------------------------------------

// The feed with Clear fight marked, for the kit's texts.
const statusView = () => ui.live && Object.assign({}, ui.live, {cleared: ui.cleared});
function fightState(w) {
  const s = axidpsKit.fightStatus(statusView());
  w.state.className = "state" + (s.live ? " live" : "");
  w.state.textContent = s.text;
}
function players() {
  return ui.live && !ui.cleared ? (ui.live.players || []) : [];
}
function emptyText() { return axidpsKit.emptyText(statusView()); }

const int = n => String(n || 0);
function clearFight() { ui.cleared = true; for (const x of wins) x.offset = 0; renderAll(); }
const sumOf = (ps, k) => ps.reduce((s, p) => s + (p[k] || 0), 0);

// Every column a player table can show (Tables tab): label, tip, width (px), formatter. heal: the
// game reports heals only for you, so anyone else's value is just what passed between you.
const COLS = {
  damage: {label: "Dmg", tip: "damage dealt", w: 62},
  dps: {label: "DPS", tip: "damage per second", w: 52},
  power: {label: "Power", tip: "strike damage dealt", w: 58},
  condi: {label: "Condi", tip: "condition damage dealt", w: 52},
  down_contrib: {label: "DnC", tip: "down contribution: damage that led to enemy downs", w: 52},
  downs: {label: "Dn", tip: "enemies downed", w: 30, num: int},
  kills: {label: "K", tip: "enemies killed", w: 30, num: int},
  deaths: {label: "D", tip: "deaths", w: 26, num: int},
  downed: {label: "Dn'd", tip: "times downed", w: 40, num: int},
  taken: {label: "Taken", tip: "damage taken", w: 52},
  cleanses: {label: "Cleanse", tip: "condition stacks cleansed from allies", w: 62, num: int},
  strips: {label: "Strip", tip: "boon stacks stripped from enemies", w: 52, num: int},
  healing: {label: "Heal", tip: "healing given (others: only what they gave you)", w: 56, heal: true},
  barrier: {label: "Barr", tip: "barrier given (others: only what they gave you)", w: 52, heal: true},
  hps: {label: "HPS", tip: "healing and barrier given per second (others: only to you)", w: 50, heal: true},
  healing_in: {label: "Heal in", tip: "healing received (others: only what you gave them)", w: 58, heal: true},
  barrier_in: {label: "Barr in", tip: "barrier received (others: only what you gave them)", w: 58, heal: true},
};
const MAX_TABLES = 4;
// The columns a table shows: the player's choice (Tables tab), else its defaults.
function colsOf(w) {
  const c = (w.cfg.cols || []).filter(k => COLS[k]);
  return c.length ? c : w.def.cols;
}

// What a table's layout depends on: its columns and compact mode.
const tableKey = w => colsOf(w).join() + (saved.compact ? "/c" : "");
// def: {cols: default column keys, nameW: name column width, sum(ps): footer}; the rank and
// name columns are added automatically. "My subgroup only" (cfg.groupOnly) is set in the hub
// (Tables tab).
function tableWindow(def) {
  const build = w => {
    // Compact (Settings) narrows every column with the smaller type.
    const k = saved.compact ? 0.8 : 1, px = n => Math.round(n * k);
    const cols = colsOf(w).map(c => Object.assign({k: c}, COLS[c], {w: px(COLS[c].w)}));
    w.colKey = tableKey(w);
    w.body.style.width = (Math.round(def.nameW * (saved.compact ? 0.72 : 1)) + px(28) +
      cols.reduce((n, c) => n + c.w, 0)) + "px";
    w.body.innerHTML = `<table><colgroup><col style="width:${px(28)}px"><col>` +
      cols.map(c => `<col style="width:${c.w}px">`).join("") + `</colgroup><thead><tr><th>#</th>` +
      `<th class="l" data-k="name">Name</th>` +
      cols.map(c => `<th data-k="${c.k}" data-tip="${esc(c.tip)}">${esc(c.label)}</th>`).join("") +
      `</tr></thead><tbody></tbody></table><div class="empty"></div>` +
      `<div class="foot"><span class="sum"></span><span class="scroll"></span></div>`;
    w.body.querySelectorAll("th[data-k]").forEach(th => th.onclick = () => {
      const c = w.cfg, key = th.dataset.k;
      if (c.sortKey === key) c.sortAsc = !c.sortAsc; else { c.sortKey = key; c.sortAsc = key === "name"; }
      save(); w.def.render(w);
    });
  };
  return defineWindow(Object.assign({}, def, {
    table: true,
    mount: build,
    render(w) {
      if (tableKey(w) !== w.colKey) build(w);
      const cols = colsOf(w).map(k => Object.assign({k}, COLS[k]));
      const c = w.cfg;
      const k = c.sortKey === "name" || cols.some(x => x.k === c.sortKey) ? c.sortKey : cols[0].k;
      fightState(w);
      let ps = players();
      const me = ps.find(p => p.me);
      if (c.groupOnly && me) ps = ps.filter(p => p.group === me.group);
      ps = ps.slice().sort((a, b) => {
        const x = a[k], y = b[k];
        const d = typeof x === "string" ? String(x).localeCompare(String(y)) : (x || 0) - (y || 0);
        return c.sortAsc ? d : -d;
      });
      const rows = maxRows();
      w.offset = Math.max(0, Math.min(w.offset, ps.length - rows));
      const top = Math.max(1, ...ps.map(p => p[k] || 0).filter(v => typeof v === "number"));
      const shown = ps.slice(w.offset, w.offset + rows);
      w.body.querySelector("tbody").innerHTML = shown.map((p, i) => {
        const col = PROF[p.prof] || PROF[0];
        const frac = (typeof p[k] === "number" ? Math.max(0, p[k]) / top * 100 : 0).toFixed(1);
        return `<tr class="p${p.me ? " me" : ""}"><td>${w.offset + i + 1}</td>` +
          `<td class="l name" data-tip="${esc(p.account)}" style="border-left-color:${col};` +
          `background:${theme.s.rows === "line"
            ? `linear-gradient(to right, ${col} ${frac}%, transparent ${frac}%) left bottom / 100% 2px no-repeat`
            : `linear-gradient(to right, ${col}38 ${frac}%, transparent ${frac}%)`}">` +
          `${p.spec ? `<img src="icons/${encodeURIComponent(p.spec)}.png" alt="">` : ""}${esc(p.name)}</td>` +
          cols.map(cc => `<td${cc.heal && !p.heal_full ? ' class="part"' : ""}>${(cc.num || fmt)(p[cc.k])}</td>`)
            .join("") + `</tr>`;
      }).join("");
      const empty = w.body.querySelector(".empty");
      empty.style.display = shown.length ? "none" : "block";
      empty.textContent = emptyText();
      w.body.querySelector(".sum").textContent = ps.length ? def.sum(ps, cols) : "";
      w.body.querySelector(".scroll").textContent = ps.length > rows
        ? `${w.offset + 1}-${Math.min(ps.length, w.offset + rows)} of ${ps.length} (wheel)` : "";
      w.body.querySelectorAll("th[data-k]").forEach(th => th.classList.toggle("sort", th.dataset.k === k));
    },
  }));
}

tableWindow({
  id: "meter", blurb: ["Meter", "damage and DPS", "chart"], title: "axidps", open: true, nameW: 198,
  cfg: {sortKey: "damage", sortAsc: false, groupOnly: false},
  at: () => [60, 220],
  cols: ["damage", "dps", "down_contrib", "downs", "kills", "deaths", "taken"],
  sum: ps => `${ps.length} players · ${fmt(sumOf(ps, "damage"))} dmg · ${fmt(sumOf(ps, "dps"))} dps`,
});

tableWindow({
  id: "support", blurb: ["Support", "cleanses and strips", "users"], title: "Support", nameW: 158,
  cfg: {sortKey: "cleanses", sortAsc: false, groupOnly: false},
  at: () => besideMeter([], 0),
  cols: ["cleanses", "strips"],
  sum: ps => `${fmt(sumOf(ps, "cleanses"))} cleansed · ${fmt(sumOf(ps, "strips"))} stripped`,
});

tableWindow({
  id: "defense", blurb: ["Defense", "damage taken", "shield"], title: "Defense", nameW: 170,
  cfg: {sortKey: "taken", sortAsc: false, groupOnly: false},
  // Below the Support window (its height depends on the rows shown).
  at: () => {
    const sup = wins.find(v => v.id === "support");
    return besideMeter([], sup && isOpen(sup) ? sup.el.offsetHeight + 6 : 0);
  },
  cols: ["taken", "downed", "deaths"],
  sum: ps => `${fmt(sumOf(ps, "taken"))} taken · ${sumOf(ps, "downed")} downs · ${sumOf(ps, "deaths")} deaths`,
});

// The player's own tables (Tables tab): saved.tables [{id, title}], columns in saved.cfg[id].cols.
function customTable(t) {
  return tableWindow({
    id: t.id, blurb: [t.title, "your columns", "columns"], title: t.title, nameW: 170, custom: true,
    open: true, cfg: {sortKey: "damage", sortAsc: false, groupOnly: false},
    at: () => besideMeter(["support"], 140),
    cols: ["damage", "dps"],
    sum: (ps, cols) => `${ps.length} players · ` +
      cols.filter(c => !c.num).slice(0, 2).map(c => `${fmt(sumOf(ps, c.k))} ${c.label.toLowerCase()}`).join(" · "),
  });
}
// Make the windows match saved.tables (after a restore, an add or a removal).
function syncTables() {
  saved.tables = (saved.tables || []).filter(t => t && /^t\d$/.test(t.id)).slice(0, MAX_TABLES);
  for (const t of saved.tables) {
    const w = wins.find(x => x.id === t.id);
    if (!w) customTable(t);
    else if (w.def.title !== t.title) {
      w.def.title = t.title; w.def.blurb[0] = t.title;
      w.el.querySelector(".bar b").textContent = t.title;
    }
  }
  for (const w of wins.filter(x => x.def.custom && !saved.tables.some(t => t.id === x.id))) removeWindow(w);
}
function addTable() {
  if (saved.tables.length >= MAX_TABLES) return;
  const n = [1, 2, 3, 4].find(i => !saved.tables.some(t => t.id === "t" + i));
  saved.tables.push({id: "t" + n, title: "Table " + n});
  delete saved.pos["t" + n]; delete saved.cfg["t" + n]; saved.open["t" + n] = true;
  syncTables(); save(); renderAll();
}
function removeTable(id) {
  saved.tables = saved.tables.filter(t => t.id !== id);
  delete saved.pos[id]; delete saved.cfg[id]; delete saved.open[id];
  syncTables(); save(); renderAll();
}

// Fight, Timeline and Healing are bundled plugins (plugins/axidps.fight, axidps.timeline,
// axidps.healing), on the same API as anyone's.

// Shown only in edit mode; interactive there (the DLL drags every other window by any point).
defineWindow({
  id: "layout", title: "Arrange windows", editOnly: true, ed: true,
  at: ([sw, sh]) => [Math.round(sw / 2 - 160), Math.round(sh * 0.16)],
  mount(w) {
    w.body.innerHTML = `<div class="lay" style="width:320px"><p>Drag any window to move it; it snaps to ` +
      `the screen edges, other windows and the game's own UI (the orange areas). Windows stay locked ` +
      `in place outside this mode.</p><div class="list"></div>` +
      `<label data-act="zones" class="zl"><span class="tog"><i></i></span>Show the game's UI<small></small></label>` +
      `<div class="row"><button data-act="unblock" data-tip="move windows off the minimap, skill bar, chat and the rest">` +
      `Move off game UI</button><span class="grow"></span>` +
      `<button data-act="reset" data-tip="put every window back in its default place">Reset</button>` +
      `<button class="primary" data-act="done">Done</button></div></div>`;
    const q = sel => w.body.querySelector(sel);
    q('[data-act="done"]').onclick = () => requestEdit(false);
    q('[data-act="reset"]').onclick = () => { saved.pos = {}; save(); renderAll(); };
    q('[data-act="zones"]').onclick = () => { saved.hideZones = !saved.hideZones; save(); renderAll(); };
    q('[data-act="unblock"]').onclick = () => {
      let moved = 0;
      for (const x of movable()) if (covers(x).length && unblock(x)) moved++;
      if (moved) { save(); renderAll(); }
    };
    q(".list").onclick = e => {
      const l = e.target.closest("label[data-w]");
      const t = l && wins.find(x => x.id === l.dataset.w);
      if (t) setOpen(t, !isOpen(t));
    };
  },
  render(w) {
    w.state.textContent = `${ui.screen[0]} x ${ui.screen[1]}`;
    w.body.querySelector(".list").innerHTML = wins.filter(x => x.def.blurb).map(x => {
      const c = covers(x);
      const note = !isOpen(x) ? "hidden" : c.length ? "covers " + c.map(z => z.label.replace(/ \(.*/, "")).join(", ")
        : posOf(x).join(", ");
      return `<label data-w="${x.id}" class="${isOpen(x) ? "on" : ""}"><span class="tog"><i></i></span>` +
        `${esc(x.def.blurb[0])}<small${c.length ? ' class="bad"' : ""}>${esc(note)}</small></label>`;
    }).join("");
    const zl = w.body.querySelector(".zl");
    zl.querySelector(".tog").classList.toggle("on", !saved.hideZones);
    zl.querySelector("small").textContent = ui.zones.length ? `${ui.zones.length} areas` : "not known yet";
    w.body.querySelector('[data-act="unblock"]').disabled = !movable().some(x => covers(x).length);
  },
});

// ---- the axidps button and hub -------------------------------------------------------------

// The button is a bare glyph drawn to sit with GW2's own menu icons (light, embossed, shadowed),
// beside them at the top left by default; moved in edit mode. A click opens the hub.
function paintLauncher(cv, hot, live) {
  const g = cv.getContext("2d"), W = cv.width;
  g.clearRect(0, 0, W, W);
  // Meter bars: an axis and three horizontal bars (x, top, right; height 5).
  const bars = [[6.5, 5, 21], [6.5, 13, 28], [6.5, 21, 16]];
  const meter = (dx, dy) => {
    g.beginPath();
    g.rect(3 + dx, 3 + dy, 2.5, 26);
    for (const [l0, t0, r0] of bars) {
      const l = l0 + dx, t = t0 + dy, r = r0 + dx, b = t + 5;
      g.moveTo(l, t); g.lineTo(r - 1.5, t); g.quadraticCurveTo(r, t, r, t + 1.5);
      g.lineTo(r, b - 1.5); g.quadraticCurveTo(r, b, r - 1.5, b); g.lineTo(l, b); g.closePath();
    }
  };
  // A sword in the bottom-right corner, its point up and to the left.
  const poly = (pts, dx, dy) => {
    g.moveTo(pts[0][0] + dx, pts[0][1] + dy);
    for (const [x, y] of pts.slice(1)) g.lineTo(x + dx, y + dy);
    g.closePath();
  };
  const SWORD = [
    [[16.0, 16.0], [19.4, 16.8], [25.1, 22.5], [22.5, 25.1], [16.8, 19.4]],    // blade
    [[27.3, 20.3], [29.0, 22.0], [22.0, 29.0], [20.3, 27.3]],    // crossguard
    [[26.3, 24.6], [30.0, 28.3], [28.3, 30.0], [24.6, 26.3]],     // grip
  ];
  const sword = (dx, dy) => { g.beginPath(); for (const p of SWORD) poly(p, dx, dy); };
  const ink = (top, bottom) => {
    const grad = g.createLinearGradient(0, 3, 0, 30);
    grad.addColorStop(0, top); grad.addColorStop(1, bottom);
    return grad;
  };
  const light = ink(hot ? "#fffbf0" : "#eee6d2", hot ? "#d9cfb6" : "#aaa089");
  meter(1, 1.5); g.fillStyle = "rgba(0,0,0,.5)"; g.fill();
  meter(0, 0); g.lineWidth = 2; g.strokeStyle = "rgba(24,20,14,.85)"; g.stroke();
  g.fillStyle = light; g.fill();
  // Cut the meter away around the sword so the two read apart.
  g.save(); g.globalCompositeOperation = "destination-out";
  sword(0, 0); g.lineWidth = 5; g.lineJoin = "round"; g.stroke(); g.fill();
  g.restore();
  sword(1, 1.5); g.fillStyle = "rgba(0,0,0,.5)"; g.fill();
  sword(0, 0); g.lineWidth = 2; g.lineJoin = "miter"; g.strokeStyle = "rgba(24,20,14,.85)"; g.stroke();
  g.fillStyle = light; g.fill();
  if (live) {
    g.beginPath(); g.arc(27, 6, 3.5, 0, Math.PI * 2);
    g.fillStyle = "#16130e"; g.fill();
    g.beginPath(); g.arc(27, 6, 2.5, 0, Math.PI * 2);
    g.fillStyle = "#8fdc5c"; g.fill();
  }
}
const launcher = defineWindow({
  id: "launcher", title: "axidps button", open: true, bare: true,
  at: ([sw, sh]) => {
    const m = ui.zones.find(z => z.id === "menu");
    return [m ? m.x + m.w + 4 : Math.round(sh * 0.235), 4];
  },
  mount(w) {
    w.body.innerHTML = `<div class="launch"><canvas width="32" height="32"></canvas></div>`;
    const b = w.body.firstChild;
    b.onclick = () => { if (!ui.editing) setOpen(hub, !isOpen(hub)); };
    b.onmouseenter = () => { w.hot = true; w.def.render(w); };
    b.onmouseleave = () => { w.hot = false; w.def.render(w); };
  },
  render(w) {
    const live = !!(ui.live && ui.live.state === "fight" && !ui.cleared);
    const hot = !!w.hot || isOpen(hub);
    const key = `${hot}${live}`;
    if (w.drawn === key) return;
    w.drawn = key;
    paintLauncher(w.body.querySelector("canvas"), hot, live);
  },
});

const BUILD = window.AXIDPS_BUILD || "dev";
const TABS = [
  ["windows", "Windows", "grid", "Show and hide the axidps windows."],
  ["tables", "Tables", "columns", "Pick and order each table's columns, or add tables of your own."],
  ["appearance", "Appearance", "layers", "Themes, opacity and blur for every window."],
  ["settings", "Settings", "sliders", "What axidps counts, and how."],
  ["plugins", "Plugins", "plug", "Add windows and tools made by others."],
  ["updates", "Updates", "download", "Versions of the axidps parts on this machine."],
];
// Core settings (saved.core, read by axidps core from ui.json): [key, label, detail, default].
const CORE_OPTS = [
  ["wvw_gadgets", "WvW: count siege, gates and walls", "Damage to gadgets in WvW and Armistice Bastion shows in the meter.", true],
  ["wvw_npcs", "WvW: count NPCs", "Guards, lords, supervisors and other NPCs in WvW, and the golems in Armistice Bastion.", true],
  ["relay", "Share healing with my squad", "Sends your heals to squad and party members running axidps and shows theirs, so everyone sees the group's healing that any axidps user sees.", true],
];
function coreOpt(k) {
  const o = CORE_OPTS.find(x => x[0] === k), c = saved.core || {};
  return c[k] === undefined ? o[3] : !!c[k];
}

// Install updates automatically (saved.app.autoUpdate, read by the launcher): on unless set false.
function autoUpdate() { return !(saved.app && saved.app.autoUpdate === false); }

// A theme's preview card: its own colours, font, title bar and row bars.
function themeCard(t) {
  const c = k => t.t[TOKENS.indexOf(k)], r = Math.min(t.s.r, 10);
  const bar = (col, f) => t.s.rows === "line"
    ? `<i style="width:${f}%;height:3px;background:${col};border-radius:2px"></i>`
    : `<i style="width:${f}%;background:${col}66;border-left:2px solid ${col};border-radius:${Math.min(r, 3)}px"></i>`;
  const line = t.s.bar === "line";
  return `<div class="th" data-theme="${t.id}" style="background:${c("bg")};border-radius:${r}px;` +
    `font-family:${esc(t.s.font)}"><div class="tb" style="color:${line ? c("accent") : c("title")};` +
    `background:${t.s.bar === "fill" ? c("bar") : "transparent"};` +
    `border-bottom:${line ? 2 : 1}px solid ${line ? c("accent-line") : c("line")};` +
    `${t.s.caps ? "text-transform:uppercase;letter-spacing:.08em" : ""}">${t.label}</div>` +
    `<div class="rows">${bar(c("accent"), 92)}${bar(c("good"), 70)}${bar(c("dim"), 48)}</div></div>`;
}

// Everything in one place: a full window over the dimmed game, centred, closed by a click
// outside it or Esc.
const hub = defineWindow({
  id: "hub", title: "axidps", session: true, bare: true, modal: true, fixed: true,
  at: ([sw, sh]) => [Math.max(0, Math.round((sw - 760) / 2)), Math.max(0, Math.round((sh - 500) / 2))],
  mount(w) {
    w.tab = "windows";
    w.body.innerHTML = `<div class="hub"><nav>` +
      `<div class="brand">${icon("chart", 18, "accent")}<div><b>axidps</b><small>build ${esc(BUILD)}</small></div></div>` +
      TABS.map(([id, label, gl]) => `<div class="nav" data-tab="${id}">${icon(gl, 14)}${label}` +
        `${id === "plugins" ? '<span class="n"></span>' : ""}</div>`).join("") +
      `<div class="grow"></div>` +
      `<button data-act="layout" data-tip="move windows (Esc or Done to finish)">${icon("maximize-2", 12, "text")}Arrange windows</button>` +
      `<div class="kbd">Alt+Shift+M hides axidps</div></nav>` +
      `<main><header><div><h2></h2><p></p></div>` +
      `<button class="x" data-act="close" data-tip="close (Esc)">${icon("x", 12)}</button></header>` +
      // Windows
      `<section data-tab="windows"><div class="cards"></div><div class="acts"><button data-act="clear" data-tip="blank every window until the next fight">` +
      `${icon("trash-2", 12, "text")}Clear fight</button></div><p class="note win-note"></p>` +
      `<div class="plugs"><h3>Plugins</h3><div class="pcards"></div></div></section>` +
      // Tables
      `<section data-tab="tables"><div class="tbls"></div><div class="acts">` +
      `<button data-act="addtable" data-tip="a new table window with columns you pick">${icon("plus", 12, "text")}Add a table</button></div>` +
      `<p class="note">Heal columns are complete only for you: the game reports no one else's heals, so ` +
      `for others they show what passed between them and you (dimmed).</p></section>` +
      // Appearance
      `<section data-tab="appearance"><h3>Theme</h3><div class="themes">${THEMES.map(themeCard).join("")}</div>` +
      `<h3>Look</h3><div class="look"><span>Opacity</span><div class="seg" data-look="opacity">` +
      OPACITY.map(o => `<button data-v="${o}">${Math.round(o * 100)}%</button>`).join("") + `</div>` +
      `<span>Blur</span><div class="seg" data-look="blur">` +
      BLUR.map((b, i) => `<button data-v="${i}">${b}</button>`).join("") + `</div></div>` +
      `<p class="note">Blur softens the game behind see-through windows. Picking a theme brings its own look.</p></section>` +
      // Settings
      `<section data-tab="settings"><h3>Windows</h3>` +
      `<div class="opt"><div>Rows shown<small>Every party or squad member up to this many; scroll for the rest.</small></div>` +
      `<div class="step">${[-5, -1].map(d => `<button data-rows="${d}">${d}</button>`).join("")}` +
      `<b class="rows-n"></b>${[1, 5].map(d => `<button data-rows="${d}">+${d}</button>`).join("")}</div></div>` +
      `<div class="opt" data-compact><div>Compact<small>Smaller rows, text and columns, so each window takes less room.</small></div>` +
      `<span class="tog"><i></i></span></div>` +
      `<h3>Combat</h3>` +
      CORE_OPTS.map(([k, label, detail]) => `<div class="opt" data-opt="${k}"><div>${esc(label)}<small>${esc(detail)}</small></div>` +
        `<span class="tog"><i></i></span></div>`).join("") +
      `<h3>Plugins</h3><div class="opt" data-dev><div>Developer mode<small>Reload plugins when their files ` +
      `change, count their errors and add Reload to their cards. The overlay restarts to switch developer ` +
      `mode.</small></div><span class="tog"><i></i></span></div>` +
      `<p class="note dev-note">Developer tools: open about:debugging in Firefox, add 127.0.0.1:6080</p></section>` +
      // Plugins: the same cards as the Windows tab
      `<section data-tab="plugins"><div class="blank pblank"><b>No plugins installed</b>` +
      `Plugins will add their own windows and tools here, next to the built-in ones.</div>` +
      `<div class="pcards"></div></section>` +
      // Updates
      `<section data-tab="updates"><div class="kv"><span>AxiDPS</span><b>${esc(BUILD)}</b>` +
      `<span class="upd-label">Update</span><b class="upd-text"></b>` +
      `<span>Core</span><b class="core-state"></b></div>` +
      `<div class="opt" data-autoupdate><div>Install updates automatically` +
      `<small>New versions install when you close the game.</small></div>` +
      `<span class="tog"><i></i></span></div></section>` +
      `</main></div>`;
    const q = sel => w.body.querySelector(sel);
    q("nav").onclick = e => {
      const t = e.target.closest("[data-tab]");
      if (t) { w.tab = t.dataset.tab; w.def.render(w); paintIcons(w.el); scheduleReport(); }
    };
    q('[data-act="layout"]').onclick = () => { ui.session.hub = false; requestEdit(true); renderAll(); };
    q('[data-act="close"]').onclick = () => setOpen(w, false);
    q('[data-act="clear"]').onclick = clearFight;
    q('[data-act="addtable"]').onclick = addTable;
    q(".tbls").onclick = e => {
      const b = e.target.closest("[data-tact]"), box = e.target.closest("[data-t]");
      const t = box && wins.find(x => x.id === box.dataset.t);
      if (!b || !t || b.disabled) return;
      const act = b.dataset.tact, k = b.closest("[data-k]") && b.closest("[data-k]").dataset.k;
      if (act === "del") return removeTable(t.id);
      if (act === "grp") { const c = t.cfg; c.groupOnly = !c.groupOnly; t.offset = 0; save(); renderAll(); return; }
      const cols = colsOf(t).slice(), i = cols.indexOf(k);
      if (act === "reset") delete t.cfg.cols;
      else {
        if (act === "add" && i < 0) cols.push(k);
        else if (act === "rm" && cols.length > 1) cols.splice(i, 1);
        else if ((act === "left" && i > 0) || (act === "right" && i >= 0 && i < cols.length - 1)) {
          const j = act === "left" ? i - 1 : i + 1;
          [cols[i], cols[j]] = [cols[j], cols[i]];
        }
        t.cfg.cols = cols;
      }
      t.offset = 0; save(); renderAll();
    };
    q(".cards").onclick = e => {
      const t = e.target.closest("[data-w]");
      const x = t && wins.find(v => v.id === t.dataset.w);
      if (x) setOpen(x, !isOpen(x));
    };
    w.body.querySelectorAll(".pcards").forEach(pc => pc.onclick = e => {
      const b = e.target.closest("[data-pact]"), c = e.target.closest("[data-p]");
      if (!b || !c) return;
      if (b.dataset.pact === "enable") pluginHost.enable(c.dataset.p);
      else if (b.dataset.pact === "disable") pluginHost.disable(c.dataset.p);
      else if (b.dataset.pact === "reload") pluginHost.reload(c.dataset.p);
    });
    q(".themes").onclick = e => { const t = e.target.closest("[data-theme]"); if (t) pickTheme(t.dataset.theme); };
    w.body.querySelectorAll(".seg").forEach(sg => sg.onclick = e => {
      const b = e.target.closest("[data-v]");
      if (!b) return;
      saved[sg.dataset.look] = Number(b.dataset.v);
      applyTheme(); save(); renderAll();
    });
    q('section[data-tab="updates"]').onclick = e => {
      if (!e.target.closest("[data-autoupdate]")) return;
      saved.app = Object.assign({}, saved.app, {autoUpdate: !autoUpdate()});
      save(); renderAll();
    };
    q('section[data-tab="settings"]').onclick = e => {
      const st = e.target.closest("[data-rows]");
      if (st) { saved.rows = Math.max(1, Math.min(MAX_ROWS, maxRows() + Number(st.dataset.rows))); save(); renderAll(); return; }
      if (e.target.closest("[data-compact]")) { saved.compact = !saved.compact; save(); renderAll(); return; }
      if (e.target.closest("[data-dev]")) { setDev(!saved.dev); return; }
      const o = e.target.closest("[data-opt]");
      if (!o) return;
      saved.core = Object.assign({}, saved.core, {[o.dataset.opt]: !coreOpt(o.dataset.opt)});
      save(); renderAll();
    };
  },
  render(w) {
    const tab = TABS.find(t => t[0] === w.tab);
    w.body.querySelectorAll(".nav").forEach(n => {
      const on = n.dataset.tab === w.tab;
      n.classList.toggle("on", on);
      n.querySelector("canvas").dataset.ink = on ? "accent" : "dim";
    });
    w.body.querySelectorAll("section").forEach(sc => sc.classList.toggle("on", sc.dataset.tab === w.tab));
    w.body.querySelector("h2").textContent = tab[1];
    w.body.querySelector("header p").textContent = tab[3];
    const cardKey = wins.filter(x => x.def.blurb).map(x => x.id + x.def.blurb[0]).join();
    if (w.cardKey !== cardKey) {
      w.cardKey = cardKey;
      w.body.querySelector(".cards").innerHTML = wins.filter(x => x.def.blurb).map(x =>
        `<div class="wc" data-w="${x.id}">${icon(x.def.blurb[2], 20)}<span class="tog"><i></i></span>` +
        `<b>${esc(x.def.blurb[0])}</b><small>${esc(x.def.blurb[1])}</small></div>`).join("");
    }
    // Plugin cards (the host's list), in the Windows and Plugins tabs: name, label, author, version,
    // what it asks for, and why it can't be enabled or its Enable/Disable (and developer mode's
    // Reload) buttons.
    const plist = pluginHost.list.filter(p => p && typeof p === "object");
    const models = plist.map(p => [String(p.id), pluginHost.cardModel(p, saved)]);
    const plugKey = JSON.stringify(models);
    if (w.plugKey !== plugKey) {
      w.plugKey = plugKey;
      w.body.querySelector(".plugs").style.display = models.length ? "" : "none";
      w.body.querySelector(".pblank").style.display = models.length ? "none" : "";
      w.body.querySelector('.nav[data-tab="plugins"] .n').textContent = models.length;
      const cards = models.map(([id, m]) =>
        `<div class="pc" data-p="${esc(id)}"><div class="pc-h"><b>${esc(m.title)}</b>` +
        `${m.label ? `<span class="tag">${esc(m.label)}</span>` : ""}<span class="grow"></span>` +
        `${m.reload ? `<button data-pact="reload" data-tip="reload its windows">Reload</button>` : ""}` +
        `${m.action ? `<button data-pact="${m.action}"${m.action === "enable" ? ' class="primary"' : ""}>` +
          `${m.action === "enable" ? "Enable" : "Disable"}</button>` : ""}</div>` +
        `<small>${esc([m.version && "v" + m.version, m.author && "by " + m.author].filter(Boolean).join(" · "))}</small>` +
        `${m.lines.map(l => `<span class="pl">${esc(l)}</span>`).join("")}` +
        `${m.reason ? `<span class="pl bad">${esc(m.reason)}</span>` : ""}</div>`).join("");
      w.body.querySelectorAll(".pcards").forEach(pc => pc.innerHTML = cards);
    }
    if (w.tab === "tables") {
      w.body.querySelector(".tbls").innerHTML = wins.filter(x => x.def.table).map(t => {
        const cols = colsOf(t);
        return `<div class="tbl" data-t="${t.id}"><div class="tbl-h"><b>${esc(t.def.blurb[0])}</b>` +
          `<span class="grow"></span><button data-tact="grp" class="${t.cfg.groupOnly ? "on" : ""}" ` +
          `data-tip="show only your subgroup">My subgroup</button>` +
          `${t.cfg.cols ? `<button data-tact="reset">Defaults</button>` : ""}` +
          `${t.def.custom ? `<button data-tact="del">Remove</button>` : ""}</div><div class="chips">` +
          cols.map((k, i) => `<span class="chip on" data-k="${k}" data-tip="${esc(COLS[k].tip)}">` +
            `<button data-tact="left"${i ? "" : " disabled"}>&lsaquo;</button>${esc(COLS[k].label)}` +
            `<button data-tact="right"${i < cols.length - 1 ? "" : " disabled"}>&rsaquo;</button>` +
            `<button data-tact="rm"${cols.length > 1 ? "" : " disabled"}>&times;</button></span>`).join("") +
          Object.keys(COLS).filter(k => !cols.includes(k)).map(k =>
            `<span class="chip" data-k="${k}" data-tip="${esc(COLS[k].tip)}"><button data-tact="add">+ ${esc(COLS[k].label)}</button></span>`)
            .join("") + `</div></div>`;
      }).join("");
      w.body.querySelector('[data-act="addtable"]').disabled = saved.tables.length >= MAX_TABLES;
    }
    w.body.querySelectorAll(".wc").forEach(t => {
      const on = isOpen(wins.find(x => x.id === t.dataset.w));
      t.classList.toggle("on", on);
      t.querySelector(".tog").classList.toggle("on", on);
      t.querySelector("canvas").dataset.ink = on ? "accent" : "faint";
    });
    const note = w.body.querySelector(".win-note");
    note.textContent = ui.notice;
    note.style.display = ui.notice ? "" : "none";
    w.body.querySelectorAll(".th").forEach(t => t.classList.toggle("on", t.dataset.theme === theme.id));
    // A theme's own opacity may sit between the buttons (Midnight's 94%): light the nearest.
    const op = Number(saved.opacity) || 1;
    const near = OPACITY.reduce((x, y) => Math.abs(y - op) < Math.abs(x - op) ? y : x);
    w.body.querySelectorAll('.seg[data-look="opacity"] button').forEach(b =>
      b.classList.toggle("on", Number(b.dataset.v) === near));
    w.body.querySelectorAll('.seg[data-look="blur"] button').forEach(b =>
      b.classList.toggle("on", Number(b.dataset.v) === (saved.blur | 0)));
    w.body.querySelectorAll("[data-opt]").forEach(o =>
      o.querySelector(".tog").classList.toggle("on", coreOpt(o.dataset.opt)));
    w.body.querySelector(".rows-n").textContent = maxRows();
    w.body.querySelector("[data-compact] .tog").classList.toggle("on", !!saved.compact);
    w.body.querySelector("[data-dev] .tog").classList.toggle("on", saved.dev === true);
    w.body.querySelector(".dev-note").style.display = saved.dev === true ? "" : "none";
    w.body.querySelector("[data-autoupdate] .tog").classList.toggle("on", autoUpdate());
    const ut = updateText(ui.update);
    w.body.querySelector(".upd-text").textContent = ut;
    w.body.querySelector(".upd-label").style.display = w.body.querySelector(".upd-text").style.display = ut ? "" : "none";
    w.body.querySelector(".core-state").textContent = ui.live ? "running" : "not connected";
  },
});

// ---- host calls ----------------------------------------------------------------------------

applyTheme();

window.axidps = {
  update(d) {
    if (ui.cleared && d && d.state === "fight" && !(ui.live && ui.live.state === "fight")) ui.cleared = false;
    ui.live = d;
    renderAll();
  },
  restore(s) {
    let migrated = false;
    if (s && s.v === 1) {
      // Fight, Timeline and Healing were built in: their saved keys move to the plugins' full ids.
      const before = JSON.stringify(s);
      pluginHost.migrateSaved(s);
      migrated = JSON.stringify(s) !== before;
      const t = THEMES.find(x => x.id === s.theme) || THEMES[0];
      saved = {v: 1, pos: s.pos || {}, open: s.open || {}, cfg: s.cfg || {}, theme: t.id,
        opacity: s.opacity ?? t.look[0], blur: s.blur ?? t.look[1], core: s.core || {}, app: s.app || {},
        tables: Array.isArray(s.tables) ? s.tables : [], hideZones: !!s.hideZones, compact: !!s.compact,
        plugins: s.plugins && typeof s.plugins === "object" && !Array.isArray(s.plugins) ? s.plugins : {},
        dev: s.dev === true,
        rows: s.rows ?? Math.max(15, ...Object.values(s.cfg || {}).map(c => (c && c.maxRows) || 0))};
    }
    syncTables();
    applyTheme();
    pluginHost.refresh();
    renderAll();
    if (migrated) save();
  },
  // The plugin list (host, after restore and after each `{"plugins":"scan"}`).
  plugins(list) {
    refit();
    pluginHost.setList(list);
  },
  // A player plugin's files changed (host, developer mode): reload its frames.
  pluginChanged(id) {
    pluginHost.changed(String(id));
  },
  // Where the game's own UI is on screen (host, from MumbleLink; empty when not known).
  // The launcher's self-update status (host, from update-status.json; null when there is none).
  updateStatus(u) {
    ui.update = u && typeof u === "object" ? u : null;
    renderAll();
  },
  gameui(z) {
    ui.zones = Array.isArray(z) ? z : [];
    renderAll();
  },
  screen(w, h) {
    if (w > 0 && h > 0) ui.screen = [w, h];
    renderAll();
  },
  edit(on) {
    ui.editing = !!on;
    if ((ui.editSeq & 1) !== (on ? 1 : 0)) ui.editSeq++; // follow the DLL (Esc, hotkey)
    renderAll();
  },
  // `name` is the window's id (a plugin window's alias); its position is saved under its key.
  moved(name, x, y) {
    const w = wins.find(v => v.id === name);
    saved.pos[w ? w.key : name] = [Math.round(x), Math.round(y)];
    save();
    renderAll();
  },
  // A click outside the hub, or Esc (the DLL's modal handling).
  dismiss() {
    if (ui.session.hub) setOpen(hub, false);
  },
  // The v1 meter position from the DLL's ini, used only if the layout has none yet.
  legacy(x, y) {
    if (!saved.pos.meter) { saved.pos.meter = [Math.round(x), Math.round(y)]; save(); renderAll(); }
  },
};
pluginHost.init({hosted: HOSTED, test: TEST, defineWindow, removeWindow, wins, saved: () => saved, ui, toHost, besideMeter,
  posOf, save, saveNow, renderAll, scheduleReport, look: lookOf, feed: feedView, isOpen});
renderAll();

// index.html?demo: sample data, for working on the page without a game (?demo&edit: edit mode).
const DEMO = window.AXIDPS_DEMO || location.search;
if (DEMO.includes("demo")) {
  if (DEMO.includes("edit")) axidps.edit(true);
  const dt = /theme=(\w+)/.exec(DEMO);
  if (dt) pickTheme(dt[1]);
  if (DEMO.includes("hub")) ui.session.hub = true;
  const tb = /tab=(\w+)/.exec(DEMO);
  if (tb) hub.tab = tb[1];
  for (const k of ["support", "defense", "axidps.fight/fight", "axidps.timeline/timeline", "axidps.healing/healing"]) {
    saved.open[k] = true;
  }
  // The host's zones for 2560x1440 at normal interface size, compass at the bottom right.
  axidps.gameui([{id: "minimap", label: "Minimap", x: 2200, y: 1064, w: 360, h: 376},
    {id: "skills", label: "Skill bar", x: 735, y: 1265, w: 1090, h: 175},
    {id: "chat", label: "Chat (default size)", x: 0, y: 1150, w: 520, h: 290},
    {id: "menu", label: "Menu bar", x: 0, y: 0, w: 330, h: 42},
    {id: "party", label: "Party", x: 0, y: 44, w: 250, h: 330},
    {id: "target", label: "Target", x: 1000, y: 0, w: 560, h: 120},
    {id: "tracker", label: "Event tracker", x: 2240, y: 190, w: 320, h: 360}]);
  const names = ["Sentinel Vael", "Darkpaw", "Ironwake", "Morrow Kestrel", "Brannoc", "Ysolde Tam",
    "Oren", "Lys Harrow", "Quill", "Thessaly", "Corvin Ash", "Mira", "Dunmore", "Saffi", "Rook",
    "Talon", "Wren"];
  const SPECS = ["Firebrand", "Spellbreaker", "Scrapper", "Druid", "Deadeye", "Tempest", "Chronomancer",
    "Scourge", "Vindicator"];
  const ps = names.map((n, i) => ({name: n, spec: SPECS[i % 9], account: n.split(" ")[0] + "." + (1000 + i),
    prof: 1 + i % 9, elite: 0, group: 1 + (i % 3), me: i === 3, power: 0, condi: 0, damage: 0, dps: 0,
    downs: 0, kills: 0, deaths: 0, downed: 0, taken: 0, strips: 0, cleanses: 0, down_contrib: 0,
    healing: 0, barrier: 0, hps: 0, healing_in: 0, barrier_in: 0, heal_full: i === 3}));
  let ms = 0;
  const tl = {out: [], in: []};
  const step = () => {
    ms += 500;
    ps.forEach((p, i) => {
      p.damage += Math.round((9000 - i * 400) * (0.5 + Math.random()));
      p.dps = p.damage / (ms / 1000);
      p.taken += Math.round(3000 * Math.random());
      if (Math.random() < .08) p.down_contrib += Math.round(20000 * Math.random());
      if (Math.random() < .01) p.downs++;
      if (Math.random() < .05) p.strips++;
      if (Math.random() < .04) p.cleanses++;
      if (Math.random() < .006) p.kills++;
      if (Math.random() < .004) p.downed++;
      if (Math.random() < .002) p.deaths++;
      if (p.me || Math.random() < .2) {
        const hv = Math.round((p.me ? 2400 : 300) * Math.random());
        p.healing += hv; p.barrier += Math.round(hv * 0.3 * Math.random());
        p.hps = (p.healing + p.barrier) / (ms / 1000);
        p.healing_in += Math.round(500 * Math.random());
      }
    });
    const me = ps.find(p => p.me);
    const heal = {out: ps.slice(0, 8).map((p, i) => ({name: p.name, prof: p.prof, me: p.me,
      healing: Math.round(me.healing / (i + 2)), barrier: Math.round(me.barrier / (i + 3))})),
    in: ps.filter(p => p.healing_in).slice(0, 6).map(p => ({name: p.name, prof: p.prof, me: p.me,
      healing: p.healing, barrier: p.barrier}))};
    tl.out.length = tl.in.length = Math.ceil(ms / 1000);
    const k = tl.out.length - 1;
    tl.out[k] = (tl.out[k] || 0) + Math.round(60000 * (1 + Math.sin(k / 9)) * (0.6 + Math.random() * 0.8));
    tl.in[k] = (tl.in[k] || 0) + Math.round(40000 * (1 + Math.cos(k / 13)) * (0.6 + Math.random() * 0.8));
    axidps.update({v: 1, t: Date.now(), state: "fight", fight: {duration_ms: ms, map: 38}, players: ps,
      timeline: {bucket_ms: 1000, out: tl.out.map(v => v || 0), in: tl.in.map(v => v || 0)}, healing: heal});
  };
  for (let i = 0; i < 240; i++) step();
  setInterval(step, 500);
}
