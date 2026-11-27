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
  session: {},         // open state of windows that always start closed (the hub, Details)
  detailFrom: "",      // the window whose player click opened Details (it sits beside it)
  update: null,        // the launcher's self-update status (host, from update-status.json)
  zones: [],           // the game's own UI (host, from MumbleLink): [{id, label, x, y, w, h}]
  notice: "",          // the hub's Windows tab note (a window that could not open)
  text: null,          // the hub's focused name field: {id, value, caret, orig}; keys come from the DLL
};
// Persisted by the host in ~/.config/axidps/ui.json.
// pos/open/cfg are keyed by window key (w.key: a plugin window's full id `<plugin>/<window>`,
// else its id); plugins: {<plugin id>: {enabled, granted}}; dev: developer mode (the host reads it at
// start).
let saved = {v: 1, pos: {}, open: {}, cfg: {}, theme: "midnight", opacity: 0.94, blur: 1, rows: 15, core: {}, app: {},
  tables: [], custom: [], hideZones: false, compact: false, plugins: {}, dev: false, storeAutoUpdate: true};
const wins = [];
// The live feed's history, for custom windows' spans other than "fight" and their charts
// (custom.js). Not `history`: that is the page's window.history.
const spanHistory = axidpsCustom.createHistory();
// Bumped when the chart history starts over (Clear fight, a new fight): charts compute again.
let chartGen = 0;

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
  if (ui.text && !isOpen(hub)) endText(true); // the hub closed: keep what was typed
  applyCap();
  document.body.classList.toggle("editing", ui.editing);
  document.body.classList.toggle("compact", !!saved.compact);
  trackCharts();
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
  const msg = JSON.stringify({windows, edit_seq: ui.editSeq, hide_zones: !!saved.hideZones,
    text_focus: !!ui.text && isOpen(hub)});
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
  placeTip(winEl, tip, e.clientX, e.clientY);
});
// Above the pointer, so the cursor never covers it; below only when the window has no room above.
function placeTip(winEl, tip, mx, my) {
  const wr = winEl.getBoundingClientRect();
  const tw = tip.offsetWidth, h = tip.offsetHeight;
  let x = mx - wr.left - tw / 2, y = my - wr.top - h - 10;
  x = Math.max(2, Math.min(x, wr.width - tw - 4));
  if (y < 2) y = Math.min(my - wr.top + 20, wr.height - h - 2);
  tip.style.left = x + "px"; tip.style.top = Math.max(2, y) + "px";
}
addEventListener("mousemove", e => {
  const winEl = e.target.closest && e.target.closest(".win");
  const tip = winEl && winEl.querySelector(".tip");
  if (tip && tip.style.display === "block") placeTip(winEl, tip, e.clientX, e.clientY);
});
addEventListener("wheel", e => {
  const winEl = e.target.closest && e.target.closest(".win");
  const w = wins.find(w => w.el === winEl);
  if (!w || e.target.closest(".dside")) return;
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
function clearFight() {
  ui.cleared = true;
  spanHistory.clear(); chartGen++;
  for (const x of wins) x.offset = 0;
  renderAll();
}
const sumOf = (ps, k) => ps.reduce((s, p) => s + (p[k] || 0), 0);

// Every column a player table can show (Tables tab), the custom window catalog (custom.js): label,
// tip, width (px); count: whole numbers. heal: the game reports heals only for you, so anyone
// else's value is just what passed between you.
const STATS = axidpsCustom.STATS;
// A player's value of catalog stat `st` is incomplete (shown dimmed): heals the game reports only
// for you, or condition damage the model estimated (nobody shares this player's exact ticks).
const partial = (st, p) => !!st && ((st.heal && !p.heal_full) || (st.condi && (p.condi_est || 0) > 0));
// A table cell's text for a catalog stat's value.
const statText = (k, v) => (STATS[k].count ? int : fmt)(v);
// The columns a table shows: the player's choice from the old Tables tab (cfg.cols; the hub can
// only reset it now), else its defaults.
function colsOf(w) {
  const c = (w.cfg.cols || []).filter(k => STATS[k]);
  return c.length ? c : w.def.cols;
}

// A row's bar behind the name, `frac` percent long in its profession colour (theme rows style).
const rowBar = (col, frac) => theme.s.rows === "line"
  ? `linear-gradient(to right, ${col} ${frac}%, transparent ${frac}%) left bottom / 100% 2px no-repeat`
  : `linear-gradient(to right, ${col}38 ${frac}%, transparent ${frac}%)`;
// A table's name cell: the profession colour bar, the spec icon and the name.
function nameCell(p, frac) {
  const col = PROF[p.prof] || PROF[0];
  return `<td class="l name" data-tip="${esc(p.account)}" style="border-left-color:${col};background:${rowBar(col, frac)}">` +
    `${p.spec ? `<img src="icons/${encodeURIComponent(p.spec)}.png" alt="">` : ""}${esc(p.name)}</td>`;
}

// What a table's layout depends on: its columns and compact mode.
const tableKey = w => colsOf(w).join() + (saved.compact ? "/c" : "");
// def: {cols: default column keys, nameW: name column width, sum(ps): footer, rows(ps): which
// players it lists (default all)}; the rank and
// name columns are added automatically. "My subgroup only" (cfg.groupOnly) was set in the old
// Tables tab; the hub's Defaults clears it.
function tableWindow(def) {
  const build = w => {
    // Compact (Settings) narrows every column with the smaller type.
    const k = saved.compact ? 0.8 : 1, px = n => Math.round(n * k);
    const cols = colsOf(w).map(c => Object.assign({k: c}, STATS[c], {w: px(STATS[c].w)}));
    w.colKey = tableKey(w);
    w.body.style.width = (Math.round(def.nameW * (saved.compact ? 0.72 : 1)) + px(28) +
      cols.reduce((n, c) => n + c.w, 0)) + "px";
    w.body.innerHTML = `<table><colgroup><col style="width:${px(28)}px"><col>` +
      cols.map(c => `<col style="width:${c.w}px">`).join("") + `</colgroup><thead><tr><th>#</th>` +
      `<th class="l" data-k="name">Name</th>` +
      cols.map(c => `<th data-k="${c.k}" data-tip="${esc(c.tip)}">${esc(c.label)}</th>`).join("") +
      `</tr></thead><tbody></tbody></table><div class="empty"></div>` +
      `<div class="foot"><span class="sum"></span><span class="scroll"></span></div>`;
    // A click on a player opens their breakdown beside this window (again: closes it).
    w.body.querySelector("tbody").onclick = e => {
      const tr = e.target.closest("tr[data-acct]");
      if (tr && !ui.editing) openDetails(tr.dataset.acct, w, statOfClick(e, w));
    };
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
      const cols = colsOf(w).map(k => Object.assign({k}, STATS[k]));
      const c = w.cfg;
      const k = c.sortKey === "name" || cols.some(x => x.k === c.sortKey) ? c.sortKey : cols[0].k;
      fightState(w);
      let ps = def.rows ? def.rows(players()) : players();
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
        const frac = (typeof p[k] === "number" ? Math.max(0, p[k]) / top * 100 : 0).toFixed(1);
        return `<tr class="p${p.me ? " me" : ""}" data-acct="${esc(p.account)}"><td>${w.offset + i + 1}</td>${nameCell(p, frac)}` +
          cols.map(cc => `<td data-k="${cc.k}"${partial(STATS[cc.k], p) ? ' class="part"' : ""}>${statText(cc.k, p[cc.k])}</td>`)
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

// A chart's per-second values: each bucket averaged with the two before and after it, so a peak
// stays where it happened. Before the fight there was nothing (zeros); after the last bucket
// nothing is known yet, so the live edge averages what there is. The Timeline and Details charts
// share it, so the same damage peaks at the same height in both.
const SMOOTH_HALF = 1;
function smoothRate(series, bucketMs) {
  const n = series.length;
  return series.map((_, i) => {
    const end = Math.min(n, i + SMOOTH_HALF + 1);
    let sum = 0;
    for (let j = Math.max(0, i - SMOOTH_HALF); j < end; j++) sum += Number(series[j]) || 0;
    return sum / (end - (i - SMOOTH_HALF)) * 1000 / (bucketMs || 1000);
  });
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

// Healing: you and squadmates sharing their heals, per fight like damage. For anyone else only
// what passed between you is known, so they are left out.
tableWindow({
  id: "healing", blurb: ["Healing", "healing and barrier by player", "plus"], title: "Healing", nameW: 170,
  cfg: {sortKey: "healing", sortAsc: false, groupOnly: false},
  at: () => besideMeter(["support", "fight"], 0),
  cols: ["healing", "barrier", "hps"],
  rows: ps => ps.filter(p => p.heal_full && (p.me || (p.healing || 0) + (p.barrier || 0) > 0)),
  sum: ps => `${fmt(sumOf(ps, "healing"))} healing · ${fmt(sumOf(ps, "barrier"))} barrier · ${ps.length} player${ps.length === 1 ? "" : "s"}`,
});

// Fight: squad totals for the current fight.
defineWindow({
  id: "fight", blurb: ["Fight", "squad totals", "swords"], title: "Fight",
  at: () => besideMeter(["support"], 0),
  mount(w) { w.body.innerHTML = `<div class="fcard"></div>`; },
  render(w) {
    fightState(w);
    const ps = players();
    const dur = ui.live && !ui.cleared && ui.live.fight ? ui.live.fight.duration_ms : 0;
    const me = ps.find(p => p.me) || {};
    const rows = [["Duration", axidpsKit.clock(dur)], ["Squad", ps.length],
      ["Damage", fmt(sumOf(ps, "damage"))], ["Squad DPS", fmt(sumOf(ps, "dps"))],
      ["Down contribution", fmt(sumOf(ps, "down_contrib"))],
      ["Enemies downed", sumOf(ps, "downs")], ["Kills", sumOf(ps, "kills")],
      ["Strips", sumOf(ps, "strips")], ["Cleanses", sumOf(ps, "cleanses")],
      ["Your healing", fmt((me.healing || 0) + (me.barrier || 0))],
      ["Damage taken", fmt(sumOf(ps, "taken"))], ["Downed", sumOf(ps, "downed")],
      ["Deaths", sumOf(ps, "deaths")]];
    w.body.firstChild.innerHTML = rows.map(([k, v]) => `<span>${esc(k)}</span><b>${esc(String(v))}</b>`).join("");
  },
});

// Timeline: squad damage per second over the fight, dealt and taken, as a 5 s rolling average.
defineWindow({
  id: "timeline", blurb: ["Timeline", "damage over time", "activity"], title: "Timeline",
  // Under the meter.
  at: ([sw, sh]) => {
    const m = wins.find(v => v.id === "meter"), [mx, my] = posOf(m);
    return [Math.max(0, Math.min(mx, sw - 430)), Math.max(0, Math.min(my + m.el.offsetHeight + 6, sh - 200))];
  },
  mount(w) {
    const px = n => Math.round(n * (saved.compact ? 0.8 : 1));
    w.body.innerHTML = `<canvas class="chart" width="${px(420)}" height="${px(150)}"></canvas>` +
      `<div class="foot"><span><b class="good">dealt</b> · <b class="bad">taken</b> per second</span><span class="peak"></span></div>`;
    w.compact = saved.compact;
  },
  render(w) {
    if (w.compact !== saved.compact) w.def.mount(w);
    fightState(w);
    const tl = ui.live && !ui.cleared ? ui.live.timeline : null;
    const key = `${JSON.stringify(tl)}|${theme.id}|${theme.s.font}|${saved.compact}|${ui.cleared}`;
    if (w.drawn === key) return;
    w.drawn = key;
    const cv = w.body.querySelector("canvas"), g = cv.getContext("2d");
    const W = cv.width, H = cv.height, padL = 40, padB = 16, padT = 6, padR = 6;
    g.clearRect(0, 0, W, H);
    const bucket = (tl && tl.bucket_ms) || 1000;
    const out = smoothRate((tl && tl.out) || [], bucket), inc = smoothRate((tl && tl.in) || [], bucket);
    const n = Math.max(out.length, inc.length);
    const top = Math.max(1, ...out, ...inc);
    const x = i => padL + (n <= 1 ? 0 : i / (n - 1) * (W - padL - padR));
    const y = v => H - padB - v / top * (H - padB - padT);
    g.strokeStyle = T("line"); g.lineWidth = 1;
    g.font = "10px " + theme.s.font; g.fillStyle = T("text");
    for (const f of [0, 0.5, 1]) {
      const yy = Math.round(y(top * f)) + 0.5;
      g.beginPath(); g.moveTo(padL, yy); g.lineTo(W - padR, yy); g.stroke();
      if (n > 1) g.fillText(fmt(top * f), 2, yy + 3);
    }
    if (n > 1) {
      g.fillText("0:00", padL, H - 3);
      const end = axidpsKit.clock(n * bucket);
      g.fillText(end, W - padR - g.measureText(end).width, H - 3);
    }
    const line = (a, col) => {
      if (a.length < 2) return;
      g.strokeStyle = col; g.lineWidth = 1.5; g.beginPath();
      a.forEach((v, i) => i ? g.lineTo(x(i), y(v)) : g.moveTo(x(i), y(v)));
      g.stroke();
    };
    line(inc, T("bad"));
    line(out, T("good"));
    if (n < 2) {
      g.fillStyle = T("dim"); g.textAlign = "center";
      g.fillText(emptyText(), W / 2, H / 2); g.textAlign = "start";
    }
    w.body.querySelector(".peak").textContent = n > 1 ? `peak ${fmt(Math.max(...out))} dealt` : "";
  },
});

// ---- details: one player's breakdown, opened by clicking a row ------------------------------

// Not a window of its own: a click on a player's number in any window opens the breakdown of that
// stat (damage, healing, strips, boons, ...) beside that window; the same click again, or its ✕,
// closes it. It is never saved open and has no hub card. The core adds the breakdown of
// saved.detail (an account) to the feed; it reads the choice from ui.json, so the first numbers
// follow a second or two after the click.
const DETAIL_VIEWS = [
  ["damage", "Damage"], ["power", "Power"], ["condi", "Condi"], ["taken", "Taken"],
  ["healing", "Healing"], ["healing_in", "Heal in"], ["strips", "Strips"], ["cleanses", "Cleanses"], ["boons", "Boons"],
];
// Which view a stat opens (boon stats: "boons"; anything else: damage).
const STAT_VIEW = {damage: "damage", dps: "damage", down_contrib: "damage", downs: "damage", kills: "damage",
  power: "power", condi: "condi", taken: "taken", deaths: "taken", downed: "taken",
  healing: "healing", barrier: "healing", hps: "healing", healing_in: "healing_in", barrier_in: "healing_in",
  strips: "strips", cleanses: "cleanses"};
function viewOfStat(k) {
  if (STAT_VIEW[k]) return STAT_VIEW[k];
  const st = STATS[k];
  return st && (st.kind === "boon" || st.kind === "boons") ? "boons" : "damage";
}
// The stat a click names: the cell's data-k, else the window's sort or first column.
function statOfClick(e, w) {
  const c = e.target.closest("[data-k]");
  if (c && c.dataset.k) return c.dataset.k;
  if (w.def.spec) return w.def.spec.sort && STATS[w.def.spec.sort.key] ? w.def.spec.sort.key : w.def.spec.stats[0].key;
  return (w.cfg.sortKey && STATS[w.cfg.sortKey]) ? w.cfg.sortKey : (w.def.cols || ["damage"])[0];
}
function openDetails(account, from, stat) {
  const w = wins.find(v => v.id === "details");
  if (!account) return;
  const view = viewOfStat(stat || "damage");
  if (isOpen(w) && saved.detail === account && ui.detailFrom === (from && from.id) && w.cfg.view === view) {
    closeDetails();
    return;
  }
  ui.detailFrom = from ? from.id : "";
  w.cfg.view = view;
  w.cfg.by = "skill";
  w.sort = null;
  saved.detail = account;
  saved.detail_skill = 0;
  saveNow();
  w.offset = 0;
  w.drawn = "";
  if (!isOpen(w)) setOpen(w, true); else renderAll();
}
function closeDetails() {
  const w = wins.find(v => v.id === "details");
  saved.detail = "";
  saved.detail_skill = 0;
  saveNow();
  setOpen(w, false);
}
// Beside the window it was opened from: to its right if it fits on screen, else to its left;
// level with its top, kept on screen. The middle of the screen when that window is gone.
function besideSource([sw, sh], el) {
  const src = wins.find(v => v.id === ui.detailFrom);
  const dw = el.offsetWidth || 600, dh = el.offsetHeight || 480;
  if (!src || !isOpen(src)) return [Math.max(0, Math.round(sw / 2 - dw / 2)), Math.max(0, Math.round(sh / 2 - dh / 2))];
  const [x, y] = posOf(src), gap = 6;
  const right = x + src.el.offsetWidth + gap;
  const left = x - dw - gap;
  const nx = right + dw <= sw || left < 0 ? right : left;
  return [Math.max(0, Math.min(Math.round(nx), sw - dw)), Math.max(0, Math.min(Math.round(y), sh - dh))];
}

// Skill and buff icons, from ArenaNet's API (the host lets the page reach only api.guildwars2.com
// for this, and render.guildwars2.com for the images). Conditions and boons are not skills there:
// their icons are listed here (as the API's skill facts give them).
const RENDER = "https://render.guildwars2.com/file/";
const BUFF_ICONS = {
  720: "09770136BB76FD0DBE1CC4267DEED54774CB20F6/102837.png", 721: "070325E519C178D502A8160523766070D30C0C19/102838.png",
  722: "28C4EC547A3516AF0242E826772DA43A5EAC3DF3/102839.png", 723: "559B0AF9FB5E1243D2649FAAE660CCB338AACC19/102840.png",
  727: "397A613651BFCA2832B6469CE34735580A2C120E/102844.png", 736: "79FF0046A5F9ADA3B4C4EC19ADB4CB124D5F0021/102848.png",
  737: "B47BF5803FED2718D7474EAF9617629AD068EE10/102849.png", 738: "3A394C1A0A3257EB27A44842DDEEF0DF000E1241/102850.png",
  742: "6CB0E64AF9AA292E332A38C1770CE577E2CDE0E8/102853.png", 791: "30307A6E766D74B6EB09EDA12A4A2DE50E4D76F4/102869.png",
  861: "289AA0A4644F0E044DED3D3F39CED958E1DDFF53/102880.png", 19426: "10BABF2708CA3575730AC662A2E72EC292565B08/598887.png",
  26766: "F60D1EF5271D7B9319610855676D320CD25F01C6/961397.png", 27705: "02EED459AD65FAF7DF32A260E479C625070841B9/1228472.png",
  740: "2FA9DF9D6BC17839BBEA14723F1C53D645DDB5E1/102852.png", 725: "96D90DF84CAFE008233DD1C2606A12C1A0E68048/102842.png",
  1187: "D4AB6401A6D6917C3D4F230764452BCCE1035B0D/1012835.png", 30328: "4FDAC2113B500104121753EF7E026E45C141E94D/1938787.png",
  717: "CD77D1FAB7B270223538A8F8ECDA1CFB044D65F4/102834.png", 718: "F69996772B9E18FD18AD0AABAB25D7E3FC42F261/102835.png",
  726: "58E92EBAF0DB4DA7C4AC04D9B22BCA5ECF0100DE/102843.png", 743: "DFB4D1B50AE4D6A275B349E15B179261EE3EB0AF/102854.png",
  1122: "3D3A1C2D6D791C05179AB871902D28782C65C244/415959.png", 719: "20CFC14967E67F7A3FD4A4B8722B4CF5B8565E11/102836.png",
  26980: "50BAC1B8E10CFAB9E749A5D910D4A9DCF29EBB7C/961398.png", 873: "D104A6B9344A2E2096424A3C300E46BC2926E4D7/2440718.png",
  5974: "E04392C3D8ED985125973AEB99D8460C483263F9/103458.png",
};
// id -> icon URL, or "" (none known); missing: not asked yet. Asked in batches.
const skillIcons = new Map(Object.entries(BUFF_ICONS).map(([id, f]) => [Number(id), RENDER + f]));
const iconWanted = new Set();
let iconTimer = 0, iconFails = 0;
function skillIcon(id) {
  id = Number(id);
  if (!(id > 0)) return "";
  if (skillIcons.has(id)) return skillIcons.get(id);
  if (iconFails < 3) {
    iconWanted.add(id);
    if (!iconTimer) iconTimer = setTimeout(fetchIcons, 150);
  }
  return "";
}
async function fetchIcons() {
  iconTimer = 0;
  const ids = [...iconWanted].slice(0, 200);
  for (const id of ids) iconWanted.delete(id);
  if (!ids.length) return;
  try {
    const r = await fetch(`https://api.guildwars2.com/v2/skills?ids=${ids.join(",")}`);
    const list = r.ok || r.status === 206 ? await r.json() : r.status === 404 ? [] : null;
    if (!Array.isArray(list)) throw new Error(`status ${r.status}`);
    for (const id of ids) skillIcons.set(id, "");
    for (const s of list) {
      if (s && Number(s.id) > 0 && typeof s.icon === "string" && s.icon.startsWith(RENDER)) skillIcons.set(Number(s.id), s.icon);
    }
    iconFails = 0;
  } catch (e) {
    // Offline or refused: ask again later, up to three times in a row.
    iconFails += 1;
    for (const id of ids) iconWanted.add(id);
  }
  const w = wins.find(v => v.id === "details");
  if (w && isOpen(w)) { w.drawnTable = ""; w.def.render(w); }
  if (iconWanted.size && iconFails < 3) iconTimer = setTimeout(fetchIcons, iconFails ? 5000 : 150);
}
const iconImg = id => {
  const u = skillIcon(id);
  return u ? `<img class="sk" src="${esc(u)}" alt="">` : `<i class="sk"></i>`;
};

// One view of the feed's breakdown: the chart's series and colour, the table's columns and rows,
// the footer, and the empty text. p: the player's row (boons come from it).
// A squad member's spec by account (for a feed row without one).
function specOf(account) {
  const p = account && players().find(x => x.account === account);
  return p && typeof p.spec === "string" ? p.spec : "";
}
function detailView(view, by, d, p) {
  const sum = a => a.reduce((n, v) => n + (Number(v) || 0), 0);
  const share = (v, t) => t ? (v / t * 100).toFixed(1) + "%" : "–";
  const dmgRows = list => {
    const total = sum(list.map(s => s.damage));
    return {total, rows: list.map(s => ({icon: s.id, name: s.name || `Skill ${s.id}`, v: s.damage, stat: s,
      keys: [s.damage, s.damage, s.hits, s.hits ? s.damage / s.hits : 0],
      cells: [fmt(s.damage), share(s.damage, total), int(s.hits), fmt(s.hits ? s.damage / s.hits : 0)]}))};
  };
  const dmgHead = ["Damage", "Share", "Hits", "Per hit"];
  const minus = (a, b) => a.map((v, i) => Math.max(0, v - (b[i] || 0)));
  if (view === "damage" || view === "power" || view === "condi") {
    const all = d ? d.skills_out : [];
    const list = view === "damage" ? all : all.filter(s => !!s.condi === (view === "condi"));
    const {total, rows} = dmgRows(list);
    for (const r of rows) r.panel = {kind: "dmg", total, casts: true};
    const series = !d ? [] : view === "damage" ? d.out : view === "condi" ? (d.out_condi || []) : minus(d.out, d.out_condi || []);
    const est = view !== "power" && p && (p.condi_est || 0) > 0;
    return {series, col: T("good"), head: dmgHead, rows, pick: "out", sum: `${fmt(total)} ${view === "damage" ? "" : view + " "}dealt` +
      (est ? " · condition damage estimated" : ""), empty: `No ${view === "damage" ? "damage" : view + " damage"} dealt this fight`};
  }
  if (view === "taken") {
    const {total, rows} = dmgRows(d ? d.skills_in : []);
    for (const r of rows) r.panel = {kind: "dmg", total, casts: false};
    const downs = d ? d.states.filter(s => s[1] === "down").length : 0;
    const deaths = d ? d.states.filter(s => s[1] === "dead").length : 0;
    return {series: d ? d.in : [], col: T("bad"), head: dmgHead, rows,
      sum: `${fmt(total)} taken · ${downs} down${downs === 1 ? "" : "s"} · ${deaths} death${deaths === 1 ? "" : "s"}`,
      empty: "No damage taken this fight"};
  }
  if (view === "healing" || view === "healing_in") {
    const out = view === "healing";
    const partial = d && out && !d.heal_full;
    if (by === "player") {
      const list = d ? (out ? d.heal_targets : d.heal_sources) || [] : [];
      const total = sum(list.map(h => h.healing + h.barrier));
      return {series: d ? (out ? d.heal_out : d.heal_in) || [] : [], col: T("accent"), byToggle: true,
        head: ["Healing", "Barrier", "Share"], nameHead: out ? "Healed" : "Healed by",
        rows: list.map(h => ({prof: h.prof, spec: h.spec || specOf(h.account), name: h.name || h.account || "Unknown",
          v: h.healing + h.barrier,
          keys: [h.healing, h.barrier, h.healing + h.barrier],
          cells: [fmt(h.healing), fmt(h.barrier), share(h.healing + h.barrier, total)]})),
        sum: `${fmt(total)} ${out ? "given" : "received"}` + (partial ? " · only what passed between you" : ""),
        empty: out ? "No healing given this fight" : "No healing received this fight"};
    }
    const list = d ? (out ? d.heals : d.heals_in) || [] : [];
    const total = sum(list.map(h => h.healing + h.barrier));
    return {series: d ? (out ? d.heal_out : d.heal_in) || [] : [], col: T("accent"), byToggle: true,
      head: ["Healing", "Barrier", "Share", "Hits"], pick: out ? "heal" : null,
      rows: list.map(h => ({icon: h.id, name: h.name || `Skill ${h.id}`, v: h.healing + h.barrier, stat: h,
        panel: {kind: "heal", total, casts: out},
        keys: [h.healing, h.barrier, h.healing + h.barrier, h.hits],
        cells: [fmt(h.healing), fmt(h.barrier), share(h.healing + h.barrier, total), int(h.hits)]})),
      sum: `${fmt(total)} ${out ? "given" : "received"}` + (partial ? " · only what passed between you" : ""),
      empty: out ? "No healing given this fight" : "No healing received this fight"};
  }
  if (view === "strips" || view === "cleanses") {
    const strips = view === "strips";
    const list = d ? (strips ? d.strips : d.cleanses) || [] : [];
    const total = sum(list.map(x => x.count));
    return {series: d ? (strips ? d.strip_series : d.cleanse_series) || [] : [], col: T("accent"), perSecond: false,
      head: ["Stacks", "Share"], nameHead: strips ? "Boon" : "Condition",
      rows: list.map(x => ({icon: x.id, name: x.name || `Buff ${x.id}`, v: x.count, keys: [x.count, x.count], cells: [int(x.count), share(x.count, total)]})),
      sum: `${int(total)} stack${total === 1 ? "" : "s"} ${strips ? "stripped" : "cleansed"}`,
      empty: strips ? "No boons stripped this fight" : "No conditions cleansed this fight"};
  }
  // Boons: the row's uptime and generation, per boon.
  const C = axidpsCustom, f = ui.live && ui.live.fight, span = f ? Number(f.duration_ms) || 0 : 0;
  const rows = [];
  for (const b of C.SQUAD_BOONS) {
    const ms = p && p.boons && p.boons[b];
    if (!Array.isArray(ms) || !ms.some(v => v > 0)) continue;
    const vals = ["up", "self", "group", "squad"].map((m, i) => C.boonRate(`boon:${b}:${m}`, ms[i], span));
    rows.push({icon: b, name: STATS[`boon:${b}:up`].label, v: vals[0], keys: vals,
      cells: vals.map((v, i) => C.formatBoon(`boon:${b}:${["up", "self", "group", "squad"][i]}`, v))});
  }
  // The chart: the picked boon's uptime each second (render fills it in once the core sends it).
  return {series: [], perSecond: false, pick: "boon", col: T("accent"), head: ["Uptime", "Self", "Group", "Squad"], nameHead: "Boon", rows,
    sum: "generation per player of the subgroup and squad", empty: "No boons this fight"};
}

const DETAIL_ROWS = 12;
defineWindow({
  id: "details", title: "Details", session: true, fixed: true,
  cfg: {view: "damage", by: "skill"},
  tools: [{id: "close", tip: "Close", label: "✕"}],
  tool(w, id) {
    if (id === "close") closeDetails();
  },
  at: besideSource,
  mount(w) {
    const px = n => Math.round(n * (saved.compact ? 0.8 : 1));
    w.body.style.width = "";
    w.body.innerHTML = `<div class="dmain" style="width:${px(620)}px"><div class="dhead"><span class="who"></span><div class="seg by">` +
      `<button data-by="skill">By skill</button><button data-by="player">By player</button></div></div>` +
      `<div class="seg views">` + DETAIL_VIEWS.map(([k, label]) => `<button data-view="${k}">${label}</button>`).join("") + `</div>` +
      `<canvas class="chart" width="${px(620)}" height="${px(140)}"></canvas>` +
      `<table><colgroup></colgroup><thead><tr></tr></thead><tbody></tbody></table><div class="empty"></div>` +
      `<div class="foot"><span class="sum"></span><span class="scroll"></span></div></div>` +
      `<div class="dside" style="width:${px(210)}px"><div class="din"><div class="dtop"></div><div class="dscroll"></div></div></div>`;
    w.body.querySelectorAll("[data-view]").forEach(b => b.onclick = () => {
      w.cfg.view = b.dataset.view; w.offset = 0; w.drawn = ""; w.drawnTable = ""; w.pin = 0; w.hover = 0; pickSkill(0); save(); w.def.render(w);
    });
    // A header click sorts by that column (again: the other way); the name column sorts A-Z first.
    w.body.querySelector("thead").onclick = e => {
      const th = e.target.closest("th[data-col]");
      if (!th) return;
      const col = Number(th.dataset.col), cur = w.sort || {};
      w.sort = cur.col === col ? {col, asc: !cur.asc} : {col, asc: col < 0};
      w.offset = 0; w.drawnTable = ""; w.def.render(w);
    };
    // A skill row click marks its casts and its damage per second on the chart (again: clears).
    // The skill panel (right) also opens; clicking the same row again closes both.
    const tbody = w.body.querySelector("tbody"), side = w.body.querySelector(".dside");
    tbody.onclick = e => {
      const tr = e.target.closest("tr[data-panel], tr[data-skill]");
      if (!tr) return;
      const id = Number(tr.dataset.panel || tr.dataset.skill);
      // A boon stays charted (clicking it again keeps it).
      if (w.cfg.view === "boons") pickSkill(id);
      else if (tr.dataset.skill) pickSkill(saved.detail_skill === id ? 0 : id);
      else w.pin = w.pin === id ? 0 : id;
      w.hover = 0;
      w.drawn = ""; w.drawnTable = ""; w.def.render(w);
    };
    // Hovering a skill row shows its panel until the pointer leaves the rows (or the panel).
    tbody.onmouseover = e => {
      const tr = e.target.closest("tr[data-panel]");
      const id = tr ? Number(tr.dataset.panel) : 0;
      if (id && id !== w.hover) { w.hover = id; w.def.render(w); }
    };
    const leave = e => {
      if (e.relatedTarget && (tbody.contains(e.relatedTarget) || side.contains(e.relatedTarget))) return;
      if (w.hover) { w.hover = 0; w.def.render(w); }
    };
    tbody.onmouseleave = leave;
    side.onmouseleave = leave;
    w.body.querySelectorAll("[data-by]").forEach(b => b.onclick = () => {
      w.cfg.by = b.dataset.by; w.offset = 0; w.drawnTable = ""; w.drawn = ""; w.pin = 0; w.hover = 0; pickSkill(0); save(); w.def.render(w);
    });
    w.compact = saved.compact;
  },
  render(w) {
    if (w.compact !== saved.compact) w.def.mount(w);
    fightState(w);
    const px = n => Math.round(n * (saved.compact ? 0.8 : 1));
    const want = saved.detail || "";
    const d = ui.live && !ui.cleared && ui.live.detail && ui.live.detail.account === want ? ui.live.detail : null;
    const view = DETAIL_VIEWS.some(([k]) => k === w.cfg.view) ? w.cfg.view : "damage";
    const by = w.cfg.by === "player" ? "player" : "skill";
    w.body.querySelectorAll("[data-view]").forEach(b => b.classList.toggle("on", b.dataset.view === view));
    w.body.querySelectorAll("[data-by]").forEach(b => b.classList.toggle("on", b.dataset.by === by));
    const p = players().find(x => x.account === want);
    const name = d ? d.name : p ? p.name : want;
    const spec = d ? d.spec : p && p.spec;
    // The player's profession colour paints the name, the bars and the chart (else the view's).
    const prof = d && d.prof !== undefined ? d.prof : p ? p.prof : undefined;
    const cls = prof !== undefined && PROF[prof] ? PROF[prof] : null;
    w.body.querySelector(".who").innerHTML = (spec ? `<img src="icons/${encodeURIComponent(spec)}.png" alt="">` : "") +
      `<b${cls ? ` style="color:${cls}"` : ""}>${esc(name)}</b><small>${esc(want)}</small>`;
    const v = detailView(view, by, d, p);
    if (cls) v.col = cls;
    w.body.querySelector(".seg.by").style.visibility = v.byToggle ? "visible" : "hidden";
    // The table: columns per view (rebuilt when the view changes).
    const layout = `${view}|${by}|${saved.compact}`;
    if (w.layoutKey !== layout) {
      if (w.layoutKey && w.layoutKey.split("|")[0] !== view) w.sort = null;
      w.layoutKey = layout;
      w.body.querySelector("colgroup").innerHTML = `<col>` + v.head.map(() => `<col style="width:${px(78)}px">`).join("");
      w.body.querySelector("thead tr").innerHTML = `<th class="l" data-col="-1">${esc(v.nameHead || "Skill")}</th>` +
        v.head.map((h, i) => `<th data-col="${i}">${esc(h)}</th>`).join("");
    }
    // Sorted by the clicked column; the feed's order (largest first) until one is clicked.
    const so = w.sort;
    const rows = !so ? v.rows : v.rows.slice().sort((a, b) => {
      const d = so.col < 0 ? String(a.name).localeCompare(String(b.name))
        : (Number(a.keys && a.keys[so.col]) || 0) - (Number(b.keys && b.keys[so.col]) || 0);
      return so.asc ? d : -d;
    });
    w.body.querySelectorAll("th[data-col]").forEach(th => th.classList.toggle("sort", !!so && Number(th.dataset.col) === so.col));
    let pick = v.pick && rows.some(r => r.icon === saved.detail_skill) ? saved.detail_skill : 0;
    // Boons always chart one: the first until another is clicked.
    if (v.pick === "boon" && !pick && rows.length) { pick = rows[0].icon; pickSkill(pick); }
    const boonUp = v.pick === "boon" && d && d.boon && d.boon.id === pick ? d.boon.up || [] : null;
    if (v.pick === "boon") {
      // Uptime in % per second; Might and Stability as average stacks.
      const stacks = [740, 1122].includes(pick), bucket = (d && d.bucket_ms) || 1000;
      v.series = boonUp ? boonUp.map(ms => stacks ? ms / 1000 : Math.min(100, ms / bucket * 100)) : [];
    }
    w.offset = Math.max(0, Math.min(w.offset, rows.length - DETAIL_ROWS));
    const shown = rows.slice(w.offset, w.offset + DETAIL_ROWS);
    const top = Math.max(1, ...rows.map(r => r.v));
    const html = shown.map(r => {
      const frac = (Math.max(0, r.v) / top * 100).toFixed(1);
      const col = r.prof !== undefined ? PROF[r.prof] || PROF[0] : v.col;
      const lead = r.icon !== undefined ? iconImg(r.icon)
        : r.spec ? `<img class="sk" src="icons/${encodeURIComponent(r.spec)}.png" alt="">` : "";
      const sel = v.pick && r.icon !== undefined;
      const on = v.pick ? sel && r.icon === pick : r.panel && r.icon === w.pin;
      return `<tr class="p${on ? " sel" : ""}"${sel ? ` data-skill="${r.icon}"` : ""}${r.panel ? ` data-panel="${r.icon}"` : ""}>` +
        `<td class="l name" style="border-left-color:${col};background:${rowBar(col, frac)}">` +
        `${lead}${esc(r.name)}</td>` + r.cells.map(c => `<td>${esc(c)}</td>`).join("") + `</tr>`;
    }).join("");
    if (w.drawnTable !== html) { w.drawnTable = html; w.body.querySelector("tbody").innerHTML = html; }
    const empty = w.body.querySelector(".empty");
    empty.style.display = shown.length ? "none" : "block";
    empty.textContent = !want ? "Click a player's number in a window" : !d ? (ui.live && ui.live.state === "idle"
      ? emptyText() : "Loading…") : v.empty;
    const pickRow = pick && rows.find(r => r.icon === pick);
    const nCasts = pick && d && Array.isArray(d.casts) ? d.casts.filter(c => c[1] === pick).length : 0;
    w.body.querySelector(".sum").textContent = !d ? "" : v.pick === "boon"
      ? (pickRow ? `${pickRow.name}: ${[740, 1122].includes(pick) ? "average stacks" : "uptime %"} per second · click a boon to chart it` : v.sum)
      : pickRow
      ? `${pickRow.name}: ${nCasts ? `${int(nCasts)} cast${nCasts === 1 ? "" : "s"}` : "no casts (it comes from another skill or a trait)"}`
      : v.sum + (v.pick ? " · click a skill for its casts" : "");
    w.body.querySelector(".scroll").textContent = v.rows.length > DETAIL_ROWS
      ? `${w.offset + 1}-${Math.min(v.rows.length, w.offset + DETAIL_ROWS)} of ${v.rows.length} (wheel)` : "";
    // The skill panel: the hovered row's, else the clicked one's.
    const shownId = w.hover || (v.pick ? pick : w.pin) || 0;
    const prow = shownId && rows.find(r => r.icon === shownId && r.panel);
    const side = w.body.querySelector(".dside");
    const ph = prow ? skillPanel(prow, d) : "";
    if (w.drawnPanel !== ph) {
      w.drawnPanel = ph;
      side.classList.toggle("on", !!prow);
      if (prow) {
        side.style.setProperty("--cls", v.col);
        const [top, body] = ph.split("\u0000");
        side.querySelector(".dtop").innerHTML = top;
        side.querySelector(".dscroll").innerHTML = body;
      }
      scheduleReport();
    }
    const cv = w.body.querySelector("canvas");
    cv.style.display = v.series ? "block" : "none";
    if (v.series) drawDetail(w, d, v.series, v.col, view, v.perSecond !== false,
      pick && v.pick !== "boon" ? {id: pick, of: v.pick} : null);
  },
});
// The Details skill panel for row `r`: its header and its scrolling stats, split by a NUL.
function skillPanel(r, d) {
  const s = r.stat || {}, p = r.panel;
  const f = ui.live && ui.live.fight, secs = f ? (Number(f.duration_ms) || 0) / 1000 : 0;
  const amount = p.kind === "heal" ? (s.healing || 0) + (s.barrier || 0) : s.damage || 0;
  const pct = (n, of) => of ? (n / of * 100).toFixed(1) + "%" : "–";
  const kv = list => `<div class="kv">` + list.map(([k, v]) => `<span>${esc(k)}</span><b>${esc(v)}</b>`).join("") + `</div>`;
  const sec = t => `<div class="sec">${esc(t)}</div>`;
  const bar = (label, n, of) => `<div class="cbl">${esc(label)} ${pct(n, of)}</div>` +
    `<div class="cb"><div style="width:${of ? Math.min(100, n / of * 100).toFixed(1) : 0}%"></div></div>`;
  const top = `<h4>${iconImg(r.icon)}<span>${esc(r.name)}</span></h4><div class="dbig">${fmt(amount)}` +
    ` <small>· ${pct(amount, p.total)}${secs > 0 ? ` · ${fmt(amount / secs)}/s` : ""}</small></div>`;
  const hits = s.hits || 0;
  let body = "";
  if (p.casts && !s.condi) {
    const n = d && Array.isArray(d.casts) ? d.casts.filter(c => c[1] === r.icon).length : 0;
    body += sec("Casts") + kv(n ? [["Activations", int(n)], ["Hits per cast", (hits / n).toFixed(1)]]
      : [["Activations", "none"]]);
  }
  if (p.kind === "heal") {
    body += sec("Hits") + kv([["Hits", int(hits)], ["Healing", fmt(s.healing || 0)], ["Barrier", fmt(s.barrier || 0)],
      ["Per hit", fmt(hits ? amount / hits : 0)]]);
    return top + "\u0000" + body;
  }
  body += sec(s.condi ? "Ticks" : "Hits") + kv([[s.condi ? "Ticks" : "Hits", int(hits)], ["Min", fmt(s.min || 0)],
    ["Avg", fmt(hits ? amount / hits : 0)], ["Max", fmt(s.max || 0)]]);
  if (!s.condi) {
    body += bar("Crit", s.crit || 0, hits) + bar("Glance", s.glance || 0, hits);
    const avoided = [["Blocked", s.block], ["Evaded", s.evade], ["Missed (blind)", s.miss], ["Absorbed", s.absorb],
      ["Interrupts", s.interrupt]];
    body += sec("Avoided") + kv(avoided.map(([k, n]) => [k, int(n)]).concat([["On barrier", fmt(s.barrier || 0)]]));
  }
  return top + "\u0000" + body;
}
// The chart: the view's amount per second (smoothRate, as the Timeline), downs and deaths marked.
function drawDetail(w, d, series, col, view, perSecond, pick) {
  // The selected skill: its amount per bucket (once the core sends it) and its cast starts.
  const sk = pick && d && d.skill && d.skill.id === pick.id ? d.skill[pick.of] || [] : null;
  const casts = pick && d && Array.isArray(d.casts) ? d.casts.filter(c => c[1] === pick.id).map(c => c[0]) : [];
  const key = `${JSON.stringify(series)}|${d ? JSON.stringify(d.states) : ""}|${view}|${theme.id}|${theme.s.font}|${saved.compact}` +
    `|${pick ? pick.id : ""}|${JSON.stringify(sk)}|${casts.length}`;
  if (w.drawn === key) return;
  w.drawn = key;
  const cv = w.body.querySelector("canvas"), g = cv.getContext("2d");
  const W = cv.width, H = cv.height, padL = 40, padB = 16, padT = 8, padR = 8;
  g.clearRect(0, 0, W, H);
  const bucket = (d && d.bucket_ms) || 1000;
  const a = smoothRate(series, perSecond ? bucket : 1000);
  const bars = sk ? sk.map(v => (Number(v) || 0) * 1000 / bucket) : [];
  const n = a.length, top = Math.max(1, ...a, ...bars);
  const x = i => padL + (n <= 1 ? 0 : i / (n - 1) * (W - padL - padR));
  const y = v => H - padB - v / top * (H - padB - padT);
  g.strokeStyle = T("line"); g.lineWidth = 1;
  g.font = "10px " + theme.s.font; g.fillStyle = T("text");
  for (const f of [0, 0.5, 1]) {
    const yy = Math.round(y(top * f)) + 0.5;
    g.beginPath(); g.moveTo(padL, yy); g.lineTo(W - padR, yy); g.stroke();
    if (n > 1) g.fillText(perSecond ? fmt(top * f) : (top * f).toFixed(top < 10 ? 1 : 0), 2, yy + 3);
  }
  if (n < 2) {
    g.fillStyle = T("dim"); g.textAlign = "center";
    g.fillText(d ? "Not enough of the fight yet" : "", W / 2, H / 2); g.textAlign = "start";
    return;
  }
  g.fillText("0:00", padL, H - 3);
  const end = axidpsKit.clock(n * bucket);
  g.fillText(end, W - padR - g.measureText(end).width, H - 3);
  // Downs (dashed) and deaths (solid) across the chart.
  for (const [at, st] of d.states) {
    if (st === "up") continue;
    const xx = Math.round(x(Math.min(n - 1, at / bucket))) + 0.5;
    g.strokeStyle = st === "dead" ? T("bad") : T("dim");
    g.setLineDash(st === "dead" ? [] : [3, 3]);
    g.beginPath(); g.moveTo(xx, padT); g.lineTo(xx, H - padB); g.stroke();
  }
  g.setLineDash([]);
  g.fillStyle = alpha(col, pick ? 0.08 : 0.18);
  g.beginPath(); g.moveTo(x(0), y(0));
  a.forEach((v, i) => g.lineTo(x(i), y(v)));
  g.lineTo(x(n - 1), y(0)); g.closePath(); g.fill();
  g.strokeStyle = pick ? alpha(col, 0.45) : col; g.lineWidth = 1.5; g.beginPath();
  a.forEach((v, i) => i ? g.lineTo(x(i), y(v)) : g.moveTo(x(i), y(v)));
  g.stroke();
  if (!pick) return;
  // The skill's own amount each second, as bars (not smoothed), under its cast marks.
  const bw = Math.max(1, (W - padL - padR) / Math.max(1, n - 1) - 1);
  g.fillStyle = col;
  bars.forEach((v, i) => {
    if (v <= 0 || i >= n) return;
    const yy = y(v);
    g.fillRect(Math.round(x(i) - bw / 2), yy, Math.max(1, Math.round(bw)), H - padB - yy);
  });
  // A cast: a tick along the top.
  g.strokeStyle = T("title"); g.lineWidth = 1;
  for (const at of casts) {
    const xx = Math.round(x(Math.min(n - 1, at / bucket))) + 0.5;
    g.beginPath(); g.moveTo(xx, padT); g.lineTo(xx, padT + 7); g.stroke();
  }
}
// The skill the Details chart marks (saved: the core sends its numbers per second).
function pickSkill(id) {
  if ((saved.detail_skill || 0) === id) return;
  saved.detail_skill = id;
  saveNow();
}

// ---- custom windows (saved.custom: the player's specs; see custom.js) ------------------------

const NO_VIEW = {players: [], seconds: 0};
// The base keys a stat is counted from (dps: damage; hps: healing and barrier).
const baseOf = k => STATS[k].rate || [k];
// A custom window cell's text: a share in % ("–" when the sum is zero), counted totals as whole
// numbers, everything else with fmt.
// A boon stat reads as % (stacks for Might and Stability); key is the boon's own key in a boons
// window, whose cells are measures.
function customText(s, v, key) {
  if (STATS[s.key].kind === "boon" || STATS[s.key].kind === "boons") return axidpsCustom.formatBoon(key || s.key, v);
  if (s.as === "share") return v == null ? "–" : fmt(v) + "%";
  return STATS[s.key].count && s.as === "total" ? int(v) : fmt(v);
}
const statHead = s => STATS[s.key].label + (s.as === "share" ? " %" : s.as === "per_s" && !STATS[s.key].rate ? "/s" : "");
const statTip = s => STATS[s.key].tip + (s.as === "share" ? " (% of everyone shown)" :
  s.as === "per_s" && !STATS[s.key].rate ? " per second" : "");
// The numbers a spec's span covers: the live feed (fight), else the history; nothing after Clear fight.
function customView(spec) {
  if (spec.span === "fight") {
    const f = ui.live && ui.live.fight;
    return {players: players(), seconds: f ? (Number(f.duration_ms) || 0) / 1000 : 0, live: true};
  }
  if (ui.cleared) return NO_VIEW;
  return spanHistory.view(spec.span) || NO_VIEW;
}
// Chart line colours: the theme's accent, good and bad, then fixed inks.
const CHART_INKS = ["accent", "good", "bad", "#e0af68", "#bb9af7", "#7dcfff", "#ff9e64", "dim"];
const chartInk = i => { const c = CHART_INKS[i % CHART_INKS.length]; return c[0] === "#" ? c : T(c); };
// A chart's lines: per stat, the per-second values summed over "who", as a 5 s rolling average
// (the Timeline's). Computed again only when a new whole fight second arrives, the spec changes or
// the history starts over; series() then adds only the new seconds (w.series keeps its lines).
function chartData(w, spec) {
  const f = ui.live && ui.live.fight;
  const key = JSON.stringify(spec) + "|" + (f ? Math.floor((Number(f.duration_ms) || 0) / 1000) : -1) + "|" + chartGen;
  if (w.chartKey === key) return w.chart;
  w.series = w.series || {};
  const ser = spanHistory.series(spec.stats.map(s => baseOf(s.key)), ps => axidpsCustom.filterWho(ps, spec.who), w.series);
  const n = ser.values.length ? ser.values[0].length : 0;
  // fight and session: the whole fight; last:N the last N seconds.
  const m = /^last:(\d+)$/.exec(spec.span);
  const from = m ? Math.max(0, n - Number(m[1])) : 0;
  const smooth = a => {
    const out = new Array(n - from);
    const start = Math.max(0, from - 4);
    let sum = 0;
    for (let i = start; i < n; i++) {
      sum += a[i];
      if (i - 5 >= start) sum -= a[i - 5];
      if (i >= from) out[i - from] = sum / Math.min(i + 1, 5) * 1000 / ser.step;
    }
    return out;
  };
  const lines = ser.values.map(smooth);
  w.chartKey = key;
  w.chart = {t0: ser.t0 + from * ser.step, step: ser.step, lines};
  return w.chart;
}
// The chart look, drawn like the Timeline plugin: three grid lines with values, the start and end
// times, one 1.5 px line per stat.
function drawChart(w, spec) {
  const data = ui.cleared ? null : chartData(w, spec);
  // A stat past the history's chart stat cap has no line.
  const capped = spec.stats.some(s => baseOf(s.key).some(k => !spanHistory.isTracked(k)));
  const drawKey = `${w.chartKey}|${ui.cleared}|${theme.id}|${theme.s.font}|${capped}`;
  if (w.drawn === drawKey) return;
  w.drawn = drawKey;
  const cv = w.body.querySelector("canvas"), g = cv.getContext("2d");
  const W = cv.width, H = cv.height, padL = 40, padB = 16, padT = 6, padR = 6;
  g.clearRect(0, 0, W, H);
  const lines = data ? data.lines : [];
  const n = lines.length ? lines[0].length : 0;
  let top = 1;
  for (const a of lines) for (const v of a) if (v > top) top = v;
  const x = i => padL + (n <= 1 ? 0 : i / (n - 1) * (W - padL - padR));
  const y = v => H - padB - v / top * (H - padB - padT);
  g.strokeStyle = T("line"); g.lineWidth = 1;
  g.font = "10px " + theme.s.font; g.fillStyle = T("text");
  for (const fr of [0, 0.5, 1]) {
    const yy = Math.round(y(top * fr)) + 0.5;
    g.beginPath(); g.moveTo(padL, yy); g.lineTo(W - padR, yy); g.stroke();
    if (n > 1) g.fillText(fmt(top * fr), 2, yy + 3);
  }
  if (n > 1) {
    g.fillText(axidpsKit.clock(data.t0), padL, H - 3);
    const end = axidpsKit.clock(data.t0 + n * data.step);
    g.fillText(end, W - padR - g.measureText(end).width, H - 3);
  }
  // The first stat on top.
  for (let k = lines.length - 1; k >= 0; k--) {
    const a = lines[k];
    if (a.length < 2) continue;
    g.strokeStyle = chartInk(k); g.lineWidth = 1.5; g.beginPath();
    a.forEach((v, i) => i ? g.lineTo(x(i), y(v)) : g.moveTo(x(i), y(v)));
    g.stroke();
  }
  if (n < 2) {
    g.fillStyle = T("dim"); g.textAlign = "center";
    g.fillText(emptyText(), W / 2, H / 2);
    g.textAlign = "start";
  }
  const st = STATS[spec.stats[0].key];
  w.body.querySelector(".peak").textContent = capped ? `chart stat limit (${axidpsCustom.CHART_KEYS}) reached`
    : n > 1 ? `peak ${fmt(Math.max(...lines[0]))} ${st.label.toLowerCase()}${st.rate ? "" : "/s"}` : "";
}

// A custom window's body for its look (again whenever the spec or compact mode changes).
function buildCustom(w) {
  const spec = w.def.spec;
  // A click on a player (not the Total row) opens their breakdown beside this window.
  w.body.onclick = e => {
    const r = e.target.closest("[data-acct]");
    if (r && r.dataset.acct && !ui.editing) openDetails(r.dataset.acct, w, statOfClick(e, w));
  };
  const k = saved.compact ? 0.8 : 1, px = n => Math.round(n * k);
  w.layout = JSON.stringify(spec) + (saved.compact ? "/c" : "");
  w.drawn = "";
  const foot = `<div class="foot"><span class="sum"></span><span class="scroll"></span></div>`;
  if (spec.look === "table") {
    const cols = spec.stats.map(s => ({s, w: px(STATS[s.key].w + (s.as === "total" ? 0 : 8))}));
    w.body.style.width = (Math.round(170 * (saved.compact ? 0.72 : 1)) + px(28) + cols.reduce((n, c) => n + c.w, 0)) + "px";
    w.body.innerHTML = `<table><colgroup><col style="width:${px(28)}px"><col>` +
      cols.map(c => `<col style="width:${c.w}px">`).join("") + `</colgroup><thead><tr><th>#</th>` +
      `<th class="l" data-k="name">Name</th>` +
      cols.map(c => `<th data-k="${c.s.key}" data-tip="${esc(statTip(c.s))}">${esc(statHead(c.s))}</th>`).join("") +
      `</tr></thead><tbody></tbody></table><div class="empty"></div>` + foot;
    w.body.querySelectorAll("th[data-k]").forEach(th => th.onclick = () => {
      const cur = w.def.spec.sort, key = th.dataset.k;
      updateSpec(w.id, {sort: cur.key === key ? {key, asc: !cur.asc} : {key, asc: key === "name"}});
    });
  } else if (spec.look === "bars") {
    w.body.style.width = px(250) + "px";
    w.body.innerHTML = `<div class="cbars"></div><div class="empty"></div>` + foot;
  } else if (spec.look === "number") {
    w.body.style.width = px(180) + "px";
    w.body.innerHTML = `<div class="big"><b></b><small></small></div>`;
  } else {
    w.body.style.width = "";
    w.body.innerHTML = `<canvas class="chart" width="${px(420)}" height="${px(150)}"></canvas>` +
      `<div class="foot"><span>${spec.stats.map((s, i) =>
        `<b style="color:${esc(chartInk(i))}">${esc(STATS[s.key].label)}</b>`).join(" · ")} per second</span>` +
      `<span class="peak"></span></div>`;
  }
}
const SPAN_TEXT = {fight: "this fight", session: "this session"};
const spanText = sp => SPAN_TEXT[sp] || `last ${sp.slice(5)} s`;

function renderCustom(w) {
  const spec = w.def.spec;
  if (w.layout !== JSON.stringify(spec) + (saved.compact ? "/c" : "")) buildCustom(w);
  fightState(w);
  if (spec.look === "chart") return drawChart(w, spec);
  const view = customView(spec);
  if (spec.look === "number") {
    const s = spec.stats[0];
    const any = axidpsCustom.filterWho(view.players, spec.who).length > 0;
    const {rows} = axidpsCustom.computeRows(Object.assign({}, spec, {rows: "total"}), view, fmt, 0);
    w.body.querySelector(".big b").textContent = any ? customText(s, rows[0].values[0]) : "–";
    w.body.querySelector(".big small").textContent = any ? `${statHead(s)} · ${spanText(spec.span)}` : emptyText();
    return;
  }
  // limit: a fixed number of rows; else Settings' Rows shown, the rest by wheel.
  const {rows, footer} = axidpsCustom.computeRows(spec, view, fmt, 0);
  const page = spec.limit || maxRows();
  w.offset = Math.max(0, Math.min(w.offset, rows.length - page));
  const shown = rows.slice(w.offset, w.offset + page);
  // The name bar follows the sorted stat (the first one when sorted by name).
  const si = Math.max(0, spec.stats.findIndex(s => s.key === spec.sort.key));
  const bi = spec.look === "bars" ? 0 : si;
  let top = 1;
  for (const r of rows) if ((r.values[bi] || 0) > top) top = r.values[bi];
  const frac = r => (Math.max(0, r.values[bi] || 0) / top * 100).toFixed(1);
  const part = (s, p) => partial(STATS[s.key], p);
  if (spec.look === "bars") {
    const s = spec.stats[0];
    w.body.querySelector(".cbars").innerHTML = shown.map(r => {
      const p = r.p, col = PROF[p.prof] || PROF[0];
      return `<div class="bl${p.me ? " me" : ""}" data-acct="${esc(p.account)}" data-tip="${esc(p.account)}" style="border-left-color:${col};` +
        `background:${rowBar(col, frac(r))}"><span class="n">` +
        `${p.spec ? `<img src="icons/${encodeURIComponent(p.spec)}.png" alt="">` : ""}${esc(p.name)}</span>` +
        `<span class="v${part(s, p) ? " part" : ""}">${esc(customText(s, r.values[0]))}</span></div>`;
    }).join("");
  } else {
    w.body.querySelector("tbody").innerHTML = shown.map((r, i) => {
      const p = r.p;
      // A boons window's rows are boons: the name only (the page has no boon icons), one cell per measure.
      if (spec.rows === "boons") {
        return `<tr class="p"><td>${w.offset + i + 1}</td><td class="l name">${esc(p.name)}</td>` +
          spec.stats.map((s, j) => `<td>${esc(customText(s, r.values[j], r.keys[j]))}</td>`).join("") + `</tr>`;
      }
      return `<tr class="p${p.me ? " me" : ""}" data-acct="${esc(p.account)}"><td>${w.offset + i + 1}</td>${nameCell(p, frac(r))}` +
        spec.stats.map((s, j) => `<td data-k="${esc(s.key)}"${part(s, p) ? ' class="part"' : ""}>${esc(customText(s, r.values[j]))}</td>`).join("") +
        `</tr>`;
    }).join("");
    w.body.querySelectorAll("th[data-k]").forEach(th => th.classList.toggle("sort", th.dataset.k === spec.sort.key));
  }
  const empty = w.body.querySelector(".empty");
  empty.style.display = shown.length ? "none" : "block";
  empty.textContent = emptyText();
  w.body.querySelector(".sum").textContent = rows.length ? footer : "";
  w.body.querySelector(".scroll").textContent = rows.length > page
    ? `${w.offset + 1}-${Math.min(rows.length, w.offset + page)} of ${rows.length} (wheel)` : "";
}

// A custom window for a spec: w.def.spec is the spec it draws (syncCustom swaps it).
function customWindow(spec) {
  const n = wins.filter(x => x.def.custom).length;
  return defineWindow({
    id: spec.id, title: spec.name, blurb: [spec.name, "your window", "columns"], custom: true, spec,
    open: true,
    at: () => besideMeter(["support"], 140 + 26 * n),
    mount: buildCustom,
    render: renderCustom,
  });
}
// Make the windows match saved.custom (after a restore, an add, an edit or a removal): new ones
// are made, a renamed one's title changes in place, a changed spec rebuilds its body.
function syncCustom() {
  const list = saved.custom;
  for (const spec of list) {
    const w = wins.find(x => x.def.custom && x.id === spec.id);
    if (!w) { customWindow(spec); continue; }
    if (w.def.title !== spec.name) {
      w.def.title = spec.name; w.def.blurb[0] = spec.name;
      w.el.querySelector(".bar b").textContent = spec.name;
      w.el.querySelector(".edit b").textContent = spec.name;
    }
    w.def.spec = spec;
  }
  for (const w of wins.filter(x => x.def.custom && !list.some(s => s.id === x.id))) removeWindow(w);
}
// A new custom window: blank (from null), a copy of a built-in table, or a copy of a custom one.
function addCustom(from) {
  const list = saved.custom, C = axidpsCustom;
  if (list.length >= C.MAX_CUSTOM) return;
  let id = "";
  for (let i = 1; i <= C.MAX_CUSTOM && !id; i++) if (!list.some(s => s.id === "c" + i)) id = "c" + i;
  let spec;
  if (from && from.def.custom) {
    const src = JSON.parse(JSON.stringify(from.def.spec));
    spec = C.normalizeSpec(Object.assign(src, {name: ("Copy of " + src.name).slice(0, C.NAME_MAX)}), id);
  } else if (from && from.def.table) {
    spec = C.specFromBuiltin({title: from.def.blurb[0], cols: from.def.cols, cfg: from.cfg}, id);
  } else spec = C.blankSpec(id);
  delete saved.pos[id]; delete saved.cfg[id]; saved.open[id] = true;
  list.push(spec);
  refit(); syncCustom(); save(); renderAll();
}
function removeCustom(id) {
  saved.custom = saved.custom.filter(s => s.id !== id);
  delete saved.pos[id]; delete saved.cfg[id]; delete saved.open[id];
  refit(); syncCustom(); save(); renderAll();
}
// Change a custom window's spec: patch is merged, then checked like a saved one.
function updateSpec(id, patch) {
  const i = saved.custom.findIndex(s => s.id === id);
  if (i < 0) return;
  const next = axidpsCustom.normalizeSpec(Object.assign({}, saved.custom[i], patch), id);
  if (!next) return;
  saved.custom[i] = next;
  syncCustom(); save(); renderAll();
  return next;
}
// The hub's name field: focus enters on a click in it (caret at the end) and leaves on Enter
// (commit), Esc (cancel), a click elsewhere or the hub closing (commit).
// The Store tab's search box uses the same field (id STORE_SEARCH): it filters as you type, Esc
// puts back what was there, and Enter or a click elsewhere keeps it.
const STORE_SEARCH = "store-search";
function startText(id) {
  if (id === STORE_SEARCH) {
    if (ui.text && ui.text.id === id) return;
    if (ui.text) endText(true);
    const v = hub.storeQuery || "";
    ui.text = {id, value: v, caret: v.length, orig: v};
    renderAll();
    return;
  }
  const spec = saved.custom.find(s => s.id === id);
  if (!spec || (ui.text && ui.text.id === id)) return;
  if (ui.text) endText(true);
  ui.text = {id, value: spec.name, caret: spec.name.length, orig: spec.name};
  renderAll();
}
function endText(commit) {
  const t = ui.text;
  if (!t) return;
  ui.text = null;
  if (t.id === STORE_SEARCH) {
    if (!commit) hub.storeQuery = t.orig;
    renderAll();
    return;
  }
  if (commit && t.value !== t.orig) updateSpec(t.id, {name: t.value}); // empty: the default name
  else renderAll();
}
function textKey(k) {
  if (!ui.text) return;
  const r = axidpsCustom.applyKey(ui.text, k);
  if (r.done) { endText(r.done === "commit"); return; }
  ui.text = Object.assign({}, ui.text, {value: r.value, caret: r.caret});
  if (ui.text.id === STORE_SEARCH) hub.storeQuery = r.value;
  renderAll();
}
// The hub's question before something that can't be taken back: `text`, a `ok` button that runs
// `yes`, and Cancel (or Esc, or a click outside the question).
function hubConfirm(text, ok, yes) {
  hub.ask = {text: String(text), ok: String(ok), yes};
  renderAll();
}
const nameField = spec => {
  const t = ui.text && ui.text.id === spec.id ? ui.text : null;
  if (!t) return `<span class="field" data-cact="name">${esc(spec.name)}</span>`;
  return `<span class="field focus" data-cact="name">${esc(t.value.slice(0, t.caret))}<i class="caret"></i>` +
    `${esc(t.value.slice(t.caret))}</span>`;
};
// The history keeps chart series only for the stats an open chart window draws, up to its cap
// (axidpsCustom.CHART_KEYS base keys): stats past it are not tracked and their windows say so.
function trackCharts() {
  const keys = new Set();
  for (const w of wins) {
    if (!w.def.custom || w.def.spec.look !== "chart" || !isOpen(w)) continue;
    for (const s of w.def.spec.stats) for (const b of baseOf(s.key)) keys.add(b);
  }
  spanHistory.track(keys);
}

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
// How opaque the button's glyph is at rest (GW2's own menu icons are dimmed until hovered).
const LAUNCHER_REST_ALPHA = 0.6;
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
  // At rest the glyph is as faint as the game's menu icons; hovered (or the hub open) it lights
  // up as they do. The cut below and the live dot ignore this.
  const glyph = hot ? 1 : LAUNCHER_REST_ALPHA;
  g.globalAlpha = glyph;
  meter(1, 1.5); g.fillStyle = "rgba(0,0,0,.5)"; g.fill();
  meter(0, 0); g.lineWidth = 2; g.strokeStyle = "rgba(24,20,14,.85)"; g.stroke();
  g.fillStyle = light; g.fill();
  // Cut the meter away around the sword so the two read apart.
  g.save(); g.globalCompositeOperation = "destination-out"; g.globalAlpha = 1;
  sword(0, 0); g.lineWidth = 5; g.lineJoin = "round"; g.stroke(); g.fill();
  g.restore();
  sword(1, 1.5); g.fillStyle = "rgba(0,0,0,.5)"; g.fill();
  sword(0, 0); g.lineWidth = 2; g.lineJoin = "miter"; g.strokeStyle = "rgba(24,20,14,.85)"; g.stroke();
  g.fillStyle = light; g.fill();
  g.globalAlpha = 1;
  if (live) {
    g.beginPath(); g.arc(27, 6, 3.5, 0, Math.PI * 2);
    g.fillStyle = "#16130e"; g.fill();
    g.beginPath(); g.arc(27, 6, 2.5, 0, Math.PI * 2);
    g.fillStyle = "#8fdc5c"; g.fill();
  }
}
const launcher = defineWindow({
  id: "launcher", title: "axidps button", open: true, bare: true,
  // The slot after the game's last menu icon: its icons are 32 wide at the top edge at "normal"
  // interface size, so the menu zone (320 wide at normal) ends where the next slot starts.
  // Blish HUD puts its own icons in that slot, so with it running the button goes just right of
  // the target bar at the top centre, where Blish's row never reaches.
  at: () => {
    const m = ui.zones.find(z => z.id === "menu");
    const k = m ? m.w / 320 : 1, pad = (32 * k - 32) / 2;
    const t = ui.zones.find(z => z.id === "target");
    if (t && ui.zones.some(z => z.id === "blish")) return [Math.round(t.x + t.w + 8 * k), Math.max(0, Math.round(pad))];
    return [Math.round((m ? m.w : 320) + pad), Math.max(0, Math.round(pad))];
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
  ["tables", "Custom windows", "columns", "Build your own windows from what axidps counts."],
  ["appearance", "Appearance", "layers", "Themes, opacity and blur for every window."],
  ["settings", "Settings", "sliders", "What axidps counts, and how."],
  ["store", "Store", "plus", "Find plugins and install them."],
  ["plugins", "Plugins", "plug", "Add windows and tools made by others."],
  ["updates", "Updates", "download", "Versions of the axidps parts on this machine."],
];
// Core settings (saved.core, read by axidps core from ui.json): [key, label, detail, default].
const CORE_OPTS = [
  ["wvw_gadgets", "WvW: count siege, gates and walls", "Damage to gadgets in WvW and Armistice Bastion shows in the meter.", true],
  ["wvw_npcs", "WvW: count NPCs", "Guards, lords, supervisors and other NPCs in WvW, and the golems in Armistice Bastion.", true],
  ["relay", "Share healing and condition damage with my squad", "Sends your heals and your exact condition damage to squad and party members running axidps and shows theirs, so everyone sees the group's healing and condition damage that any axidps user sees.", true],
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

// ---- the Custom windows tab (the builder): a card per window --------------------------------

// Profession names for the "+ spec…" filters (ids 1-9, the kit's PROF colour order).
const PROF_NAMES = ["", "Guardian", "Warrior", "Engineer", "Ranger", "Thief", "Elementalist", "Mesmer",
  "Necromancer", "Revenant"];
const FILTER_MAX = 9;
const filterLabel = f => f.startsWith("prof:") ? (PROF_NAMES[Number(f.slice(5))] || f) : f.slice(5);
const SEG_TEXT = {
  rows: [["players", "players"], ["total", "total"], ["boons", "boons"]],
  who: [["squad", "squad"], ["group", "my subgroup"], ["me", "me"]],
  span: [["fight", "fight"], ["last:10", "10s"], ["last:30", "30s"], ["last:60", "60s"], ["session", "session"]],
  look: [["table", "table"], ["bars", "bars"], ["number", "number"], ["chart", "chart"]],
};
const AS_TEXT = {total: "total", per_s: "/s", share: "%"};
// The as toggle's next value: total, /s, % (no % for a number: it would always read 100%).
const nextAs = (as, look) => {
  const order = look === "number" ? ["total", "per_s"] : ["total", "per_s", "share"];
  return order[(order.indexOf(as) + 1) % order.length];
};
// The filters a card offers: every profession, the specs in the current feed, and its own.
function filterChoices(spec) {
  const out = PROF_NAMES.slice(1).map((_, i) => "prof:" + (i + 1));
  const specs = [...new Set(players().map(p => p.spec).filter(x => typeof x === "string" && /^[A-Za-z ]{1,24}$/.test(x)))].sort();
  for (const f of specs.map(x => "spec:" + x).concat(spec.who.filters)) if (!out.includes(f)) out.push(f);
  return out;
}
// only: the values offered (default: all of them).
const seg = (f, cur, only) => `<div class="seg" data-f="${f}">` + SEG_TEXT[f].filter(([v]) => !only || only.includes(v)).map(([v, t]) =>
  `<button data-cact="set" data-v="${v}"${v === cur ? ' class="on"' : ""}>${t}</button>`).join("") + `</div>`;
const crow = (label, body) => `<div class="crow"><span>${label}</span><div>${body}</div></div>`;

// Column or subgroup choices a built-in table kept from before custom windows (they can no longer
// be made, only reset).
const builtinChanged = t => { const c = t.cfg; return Array.isArray(c.cols) || !!c.groupOnly; };
// A built-in table's card: its name, Copy and, while it has old choices, Defaults (they are not
// editable).
function builtinCard(t, full) {
  return `<div class="tbl" data-c="${t.id}"><div class="tbl-h"><b>${esc(t.def.blurb[0])}</b><span class="tag">built in</span>` +
    `<span class="grow"></span>${builtinChanged(t) ? `<button data-cact="defaults" data-tip="its default columns, for the whole squad">Defaults</button>` : ""}` +
    `<button data-cact="copy"${full ? " disabled" : ""} data-tip="a custom window with its columns">Copy</button></div></div>`;
}
// A custom window's card: every part of its spec.
function customCard(spec, full, filterOpen, boonOpen, note) {
  const C = axidpsCustom, n = spec.stats.length, chart = spec.look === "chart", boons = spec.rows === "boons";
  const chips = spec.stats.map((s, i) => {
    const as = STATS[s.key].rate || STATS[s.key].noAs || chart ? "" :
      `<button data-cact="as" class="as" data-tip="total, per second or % of everyone shown">${AS_TEXT[s.as]}</button>`;
    return `<span class="chip on" data-k="${s.key}" data-i="${i}" data-tip="${esc(statTip(s))}">` +
      `<button data-cact="left"${i ? "" : " disabled"}>&lsaquo;</button>${esc(STATS[s.key].label)}` +
      `<button data-cact="right"${i < n - 1 ? "" : " disabled"}>&rsaquo;</button>${as}` +
      `<button data-cact="rm"${n > 1 ? "" : " disabled"}>&times;</button></span>`;
  }).join("");
  const addChip = k => `<span class="chip" data-k="${k}" data-tip="${esc(STATS[k].tip)}"><button data-cact="add"` +
    `${n >= C.MAX_STATS ? " disabled" : ""}>+ ${esc(STATS[k].label.replace(/ \(.*\)$/, ""))}</button></span>`;
  const free = k => !spec.stats.some(s => s.key === k);
  const group = (name, body) => `<span class="grp">${name}</span>${body ? `<div class="chips">${body}</div>` : ""}`;
  let picker;
  if (boons) {
    // A boons window picks measures only: its rows are the boons.
    picker = Object.keys(STATS).filter(k => STATS[k].kind === "boons" && free(k)).map(addChip).join("");
  } else {
    picker = group("Fight", Object.keys(STATS).filter(k => !STATS[k].kind && free(k)).map(addChip).join(""));
    // Boon stats are not for charts (summing uptime over players over time means nothing).
    if (!chart) {
      const names = C.SQUAD_BOONS.map(b => {
        const k = `boon:${b}:up`;
        return `<span class="chip${boonOpen === b ? " on" : ""}"><button data-cact="boonopen" data-v="${b}">` +
          `${esc(STATS[k].label)}${boonOpen === b ? " &times;" : ""}</button></span>`;
      }).join("");
      const measures = boonOpen ? ["up", "self", "group", "squad"].map(m => `boon:${boonOpen}:${m}`).filter(free).map(k =>
        `<span class="chip" data-k="${k}" data-tip="${esc(STATS[k].tip)}"><button data-cact="add"` +
        `${n >= C.MAX_STATS ? " disabled" : ""}>+ ${esc(({up: "Uptime", self: "Self", group: "Group", squad: "Squad"})[STATS[k].measure])}` +
        `</button></span>`).join("") : "";
      picker += group("Boons", names) + (boonOpen ? group(`${esc(STATS[`boon:${boonOpen}:up`].label)}: pick a measure`, measures || "") : "");
    }
  }
  const chipsHtml = `<div class="chips">${chips}</div>` + picker;
  const fl = spec.who.filters;
  const filters = (filterOpen ? filterChoices(spec) : fl).map(f => {
    const on = fl.includes(f);
    return `<span class="chip${on ? " on" : ""}"><button data-cact="filter" data-v="${esc(f)}"` +
      `${!on && fl.length >= FILTER_MAX ? " disabled" : ""}>${on ? "" : "+ "}${esc(filterLabel(f))}` +
      `${on ? " &times;" : ""}</button></span>`;
  }).join("") + `<span class="chip"><button data-cact="filters"${filterOpen ? ' class="on"' : ""}>` +
    `${filterOpen ? "done" : "+ spec&hellip;"}</button></span>`;
  const sortName = spec.sort.key === "name" ? "Name" : spec.sort.key ? STATS[spec.sort.key].label : "Default";
  return `<div class="tbl cw" data-c="${spec.id}"><div class="tbl-h"><b>${esc(spec.name)}</b><span class="grow"></span>` +
    `<button data-cact="copy"${full ? " disabled" : ""}>Copy</button><button data-cact="del">Remove</button></div>` +
    crow("Name", nameField(spec)) +
    crow("Rows", seg("rows", spec.rows)) +
    crow("Who", seg("who", spec.who.base) + `<div class="chips">${filters}</div>`) +
    crow("Span", seg("span", spec.span)) +
    // A boons window is a table.
    crow("Look", seg("look", spec.look, boons ? ["table"] : null)) +
    crow("Stats", chipsHtml) +
    (note ? `<div class="cnote">${esc(note)}</div>` : "") +
    crow("Sort", `<button data-cact="sortkey" data-tip="sort by the name or one of its stats">${esc(sortName)}</button>` +
      `<button data-cact="sortdir" data-tip="${spec.sort.asc ? "lowest" : "highest"} first">${spec.sort.asc ? "&uarr;" : "&darr;"}</button>` +
      `<span class="lbl">Rows</span><div class="step"><button data-cact="limit-">&minus;</button>` +
      `<b>${spec.limit || "auto"}</b><button data-cact="limit+">+</button></div>`) +
    `</div>`;
}
// The tab: a card per built-in table and per custom window, and the Add a window menu. Drawn
// again only when something on it changes.
function renderBuilder(w) {
  const C = axidpsCustom, full = saved.custom.length >= C.MAX_CUSTOM;
  const tables = wins.filter(x => x.def.table);
  if (full) w.addMenu = false;
  const feedSpecs = w.filterOpen ? players().map(p => p.spec).join() : "";
  const key = JSON.stringify([saved.custom, tables.map(t => t.id + t.def.blurb[0] + builtinChanged(t)), w.filterOpen || "", !!w.addMenu, feedSpecs,
    ui.text, w.boonOpen || "", w.cardNote || ""]);
  if (w.builderKey === key) return;
  w.builderKey = key;
  w.body.querySelector(".tbls").innerHTML = tables.map(t => builtinCard(t, full)).join("") +
    saved.custom.map(s => customCard(s, full, w.filterOpen === s.id,
      w.boonOpen && w.boonOpen.id === s.id ? w.boonOpen.boon : 0, w.cardNote && w.cardNote.id === s.id ? w.cardNote.text : "")).join("");
  const add = w.body.querySelector('[data-act="addtable"]');
  add.disabled = full;
  add.classList.toggle("on", !!w.addMenu);
  const menu = w.body.querySelector(".addmenu");
  menu.style.display = w.addMenu ? "" : "none";
  menu.innerHTML = w.addMenu ? `<button data-cact="addblank">Blank</button>` +
    tables.map(t => `<button data-cact="addfrom" data-v="${t.id}">Copy of ${esc(t.def.blurb[0])}</button>`).join("") +
    saved.custom.map(s => `<button data-cact="addfrom" data-v="${s.id}">Copy of ${esc(s.name)}</button>`).join("") : "";
}
// A click on a card's control (data-cact) or the Add a window menu.
function builderAct(hw, b) {
  const act = b.dataset.cact, v = b.dataset.v;
  if (act === "addblank" || act === "addfrom") {
    hw.addMenu = false;
    const from = act === "addfrom" ? wins.find(x => x.id === v) : null;
    if (act === "addfrom" && !from) return;
    addCustom(from);
    return;
  }
  const card = b.closest("[data-c]");
  const t = card && wins.find(x => x.id === card.dataset.c);
  if (!t) return;
  hw.cardNote = null;
  if (act === "copy") { addCustom(t); return; }
  if (act === "defaults" && t.def.table) {
    const c = t.cfg;
    delete c.cols; delete c.groupOnly;
    t.offset = 0; save(); renderAll();
    return;
  }
  if (!t.def.custom) return;
  const spec = t.def.spec, id = spec.id;
  const stats = spec.stats.map(s => Object.assign({}, s));
  const note = text => { hw.cardNote = {id, text}; hw.builderKey = ""; hw.def.render(hw); };
  const chip = b.closest("[data-i]"), i = chip ? Number(chip.dataset.i) : -1;
  switch (act) {
    case "name": return startText(id);
    case "del":
      if (hw.filterOpen === id) hw.filterOpen = "";
      if (hw.boonOpen && hw.boonOpen.id === id) hw.boonOpen = null;
      return removeCustom(id);
    case "set": {
      const f = b.closest("[data-f]").dataset.f;
      // A boons window or a chart has no per-boon picker to leave open.
      if (hw.boonOpen && hw.boonOpen.id === id && (f === "rows" && v === "boons" || f === "look" && v === "chart")) hw.boonOpen = null;
      if (f === "who") return updateSpec(id, {who: {base: v, filters: spec.who.filters}});
      const next = updateSpec(id, {[f]: v});
      // Say what the change took away.
      const lost = next && spec.stats.length && spec.stats.some(s => !next.stats.some(x => x.key === s.key));
      if (next && lost && f === "rows" && v === "boons") note("Boons windows show only Uptime, Self, Group and Squad; your other stats were dropped.");
      else if (next && lost && f === "rows" && spec.rows === "boons") note("Those measures are for boons windows only, so they were dropped.");
      else if (next && lost && f === "look" && v === "chart") note("Charts do not show boon stats (uptime summed over players means nothing), so they were dropped.");
      return;
    }
    case "boonopen":
      hw.boonOpen = hw.boonOpen && hw.boonOpen.id === id && hw.boonOpen.boon === Number(v) ? null : {id, boon: Number(v)};
      return hw.def.render(hw);
    case "filters":
      hw.filterOpen = hw.filterOpen === id ? "" : id;
      return hw.def.render(hw);
    case "filter": {
      const fl = spec.who.filters;
      const next = fl.includes(v) ? fl.filter(x => x !== v) : fl.length < FILTER_MAX ? fl.concat(v) : fl;
      return updateSpec(id, {who: {base: spec.who.base, filters: next}});
    }
    case "add": {
      const k = b.closest("[data-k]").dataset.k;
      if (stats.length >= axidpsCustom.MAX_STATS) return;
      return updateSpec(id, {stats: stats.concat({key: k, as: "total"})});
    }
    case "rm":
      if (stats.length < 2 || i < 0) return;
      stats.splice(i, 1);
      return updateSpec(id, {stats});
    case "left": case "right": {
      const j = act === "left" ? i - 1 : i + 1;
      if (i < 0 || j < 0 || j >= stats.length) return;
      [stats[i], stats[j]] = [stats[j], stats[i]];
      return updateSpec(id, {stats});
    }
    case "as":
      if (i < 0) return;
      stats[i].as = nextAs(stats[i].as, spec.look);
      return updateSpec(id, {stats});
    case "sortkey": {
      const keys = ["name", ...new Set(spec.stats.map(s => s.key))];
      const key = keys[(keys.indexOf(spec.sort.key) + 1) % keys.length];
      return updateSpec(id, {sort: {key, asc: key === "name"}});
    }
    case "sortdir": return updateSpec(id, {sort: {key: spec.sort.key, asc: !spec.sort.asc}});
    // From auto, the steps start at Settings' Rows shown; below 1 is auto again.
    case "limit+": return updateSpec(id, {limit: Math.min(MAX_ROWS, (spec.limit || maxRows()) + 1)});
    case "limit-": {
      const cur = spec.limit || maxRows();
      return updateSpec(id, {limit: spec.limit === 1 || cur <= 1 ? null : cur - 1});
    }
  }
}

// A trust badge: Verified, Unverified, DEV or Revoked.
function badge(label) {
  const cls = {Verified: "v", Unverified: "u", DEV: "dev", Revoked: "r"}[label] || "u";
  return `<span class="badge ${cls}">${esc(label)}</span>`;
}
const STORE_BUTTON = {install: "Install", update: "Update", replace: "Replace", uninstall: "Uninstall"};
// The Store tab: the search box, the store's note and a card per plugin.
function renderStore(w) {
  const t = ui.text && ui.text.id === STORE_SEARCH ? ui.text : null;
  const query = t ? t.value : (w.storeQuery || "");
  const m = pluginHost.storeModel(pluginHost.store, query, pluginHost.list);
  w.body.querySelector("[data-storeauto] .tog").classList.toggle("on", saved.storeAutoUpdate !== false);
  const key = JSON.stringify([m, t]);
  if (w.storeKey === key) return;
  w.storeKey = key;
  w.body.querySelector(".sfield").innerHTML = t
    ? `<span class="field focus search" data-search>${esc(t.value.slice(0, t.caret))}<i class="caret"></i>` +
      `${esc(t.value.slice(t.caret))}</span>`
    : `<span class="field search${query ? "" : " empty"}" data-search>${esc(query || "Search plugins")}</span>`;
  const msg = w.body.querySelector(".store-msg");
  msg.textContent = m.message;
  msg.style.display = m.message ? "" : "none";
  const btn = (act, primary) => `<button data-sact="${act}"${primary ? ' class="primary"' : ""}>${STORE_BUTTON[act]}</button>`;
  w.body.querySelector(".scards").innerHTML = m.cards.map(c => {
    const acts = c.busy ? `<button disabled>Working…</button>`
      : c.action === "install" ? btn("install", true)
      : c.action === "replace" ? btn("replace", false)
      : c.action === "update" ? btn("uninstall", false) + btn("update", true)
      : c.action === "installed" ? `<span class="have">Installed</span>` + btn("uninstall", false) : "";
    return `<div class="pc" data-s="${esc(c.id)}"><div class="pc-h"><b>${esc(c.title)}</b>${badge(c.badge)}</div>` +
      `<small>${esc([c.version && "v" + c.version, c.author && "by " + c.author].filter(Boolean).join(" · "))}</small>` +
      `${c.description ? `<span class="pd">${esc(c.description)}</span>` : ""}` +
      `${c.lines.map(l => `<span class="pl">${esc(l)}</span>`).join("")}` +
      `${c.note ? `<span class="pl${c.bad ? " bad" : " hint"}">${esc(c.note)}</span>` : ""}` +
      `${acts ? `<div class="pc-a">${acts}</div>` : ""}</div>`;
  }).join("");
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
      `${icon("trash-2", 12, "text")}Clear fight</button><button data-act="clearsession" ` +
      `data-tip="start the session span of every custom window over">${icon("trash-2", 12, "text")}Clear session</button></div><p class="note win-note"></p>` +
      `<div class="plugs"><h3>Plugins</h3><div class="pcards"></div></div></section>` +
      // Tables
      `<section data-tab="tables"><div class="tbls"></div><div class="acts">` +
      `<button data-act="addtable" data-tip="a blank window, or a copy of one you have">${icon("plus", 12, "text")}Add a window</button>` +
      `</div><div class="addmenu"></div>` +
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
      // Store: the registry's plugins, a search box, and whether store plugins update by themselves
      `<section data-tab="store"><div class="store-top"><span class="sfield"></span><span class="grow"></span>` +
      `<button data-sact="refresh" data-tip="check the store for new plugins and versions">Refresh</button></div>` +
      `<p class="note store-msg"></p><div class="scards"></div>` +
      `<div class="opt" data-storeauto><div>Update automatically<small>Store plugins update by themselves ` +
      `when the new version asks for nothing new. Anything else waits for you to press Update.</small></div>` +
      `<span class="tog"><i></i></span></div></section>` +
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
      `<div class="ask"><div><p></p><div class="acts"><button data-ask="no">Cancel</button>` +
      `<button data-ask="yes" class="primary"></button></div></div></div>` +
      `</main></div>`;
    const q = sel => w.body.querySelector(sel);
    // A click anywhere in the hub but the focused name field ends the typing (keeps it) first.
    w.body.addEventListener("click", e => {
      const f = ui.text && e.target.closest && e.target.closest('[data-cact="name"]');
      const card = f && f.closest("[data-c]");
      const search = ui.text && ui.text.id === STORE_SEARCH && e.target.closest && e.target.closest("[data-search]");
      if (ui.text && !search && !(card && card.dataset.c === ui.text.id)) endText(true);
    }, true);
    q("nav").onclick = e => {
      const t = e.target.closest("[data-tab]");
      if (!t) return;
      w.tab = t.dataset.tab;
      if (w.tab === "store") pluginHost.storeOpen();
      w.def.render(w); paintIcons(w.el); scheduleReport();
    };
    q(".ask").onclick = e => {
      const b = e.target.closest("[data-ask]");
      if (!b && e.target.closest(".ask > div")) return;
      const a = w.ask;
      w.ask = null;
      renderAll();
      if (a && b && b.dataset.ask === "yes") a.yes();
    };
    q('section[data-tab="store"]').onclick = e => {
      if (e.target.closest("[data-search]")) { startText(STORE_SEARCH); return; }
      if (e.target.closest("[data-storeauto]")) { saved.storeAutoUpdate = saved.storeAutoUpdate === false; save(); renderAll(); return; }
      const b = e.target.closest("[data-sact]");
      if (!b || b.disabled) return;
      if (b.dataset.sact === "refresh") { pluginHost.storeRefresh(); return; }
      const c = b.closest("[data-s]");
      if (c) pluginHost.storeAct(c.dataset.s, b.dataset.sact);
    };
    q('[data-act="layout"]').onclick = () => { ui.session.hub = false; requestEdit(true); renderAll(); };
    q('[data-act="close"]').onclick = () => setOpen(w, false);
    q('[data-act="clear"]').onclick = clearFight;
    q('[data-act="clearsession"]').onclick = () => { spanHistory.clearSession(); renderAll(); };
    q('[data-act="addtable"]').onclick = () => { w.addMenu = !w.addMenu; w.def.render(w); };
    q('section[data-tab="tables"]').onclick = e => {
      const b = e.target.closest("[data-cact]");
      if (b && !b.disabled) builderAct(w, b);
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
        `${m.label ? badge(m.label) : ""}<span class="grow"></span>` +
        `${m.reload ? `<button data-pact="reload" data-tip="reload its windows">Reload</button>` : ""}` +
        `${m.action ? `<button data-pact="${m.action}"${m.action === "enable" ? ' class="primary"' : ""}>` +
          `${m.action === "enable" ? "Enable" : "Disable"}</button>` : ""}</div>` +
        `<small>${esc([m.version && "v" + m.version, m.author && "by " + m.author].filter(Boolean).join(" · "))}</small>` +
        `${m.lines.map(l => `<span class="pl">${esc(l)}</span>`).join("")}` +
        `${m.reason ? `<span class="pl bad">${esc(m.reason)}</span>` : ""}</div>`).join("");
      w.body.querySelectorAll(".pcards").forEach(pc => pc.innerHTML = cards);
    }
    if (w.tab === "tables") renderBuilder(w);
    renderStore(w);
    const ask = w.body.querySelector(".ask");
    ask.classList.toggle("on", !!w.ask);
    if (w.ask) {
      ask.querySelector("p").textContent = w.ask.text;
      ask.querySelector('[data-ask="yes"]').textContent = w.ask.ok;
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
    if (d && d.state === "fight" && !(ui.live && ui.live.state === "fight")) chartGen++;
    spanHistory.push(d);
    if (ui.cleared && d && d.state === "fight" && !(ui.live && ui.live.state === "fight")) ui.cleared = false;
    ui.live = d;
    renderAll();
  },
  restore(s) {
    let migrated = false;
    if (s && s.v === 1) {
      // Fight, Timeline and Healing were bundled plugins: their saved keys move back to these ids.
      const before = JSON.stringify(s);
      pluginHost.migrateSaved(s);
      migrated = JSON.stringify(s) !== before;
      const t = THEMES.find(x => x.id === s.theme) || THEMES[0];
      saved = {v: 1, pos: s.pos || {}, open: s.open || {}, cfg: s.cfg || {}, theme: t.id,
        opacity: s.opacity ?? t.look[0], blur: s.blur ?? t.look[1], core: s.core || {}, app: s.app || {},
        tables: Array.isArray(s.tables) ? s.tables : [], custom: s.custom, hideZones: !!s.hideZones, compact: !!s.compact,
        plugins: s.plugins && typeof s.plugins === "object" && !Array.isArray(s.plugins) ? s.plugins : {},
        dev: s.dev === true, storeAutoUpdate: s.storeAutoUpdate !== false,
        rows: s.rows ?? Math.max(15, ...Object.values(s.cfg || {}).map(c => (c && c.maxRows) || 0))};
    }
    // Details was a window of its own, saved open and placed: now it opens only from a click.
    if (saved.open.details !== undefined || saved.pos.details) {
      delete saved.open.details; delete saved.pos.details; migrated = true;
    }
    // The old fixed tables become custom windows once (saved.custom absent).
    if (saved.custom === undefined && axidpsCustom.migrateTables(saved)) migrated = true;
    saved.custom = axidpsCustom.normalizeAll(saved.custom);
    syncCustom();
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
  // The Store tab's list and state (host: on start, after each fetch, install or uninstall).
  store(payload) {
    pluginHost.setStore(payload);
    renderAll();
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
  // The game's frame rate, ping and server tick rate over the last second (host, from the DLL).
  perf(p) {
    if (!p || typeof p !== "object") return;
    const n = v => (Number.isFinite(v) && v >= 0 ? Math.round(v) : 0);
    pluginHost.perf({fps: n(p.fps), ping: n(p.ping), tick_rate: n(p.tick_rate)});
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
  // A click outside the hub, or Esc (the DLL's modal handling). While the name field has focus Esc
  // arrives as a key instead; a click outside then only ends the typing.
  dismiss() {
    if (ui.text) { endText(true); return; }
    if (hub.ask) { hub.ask = null; renderAll(); return; }
    if (ui.session.hub) setOpen(hub, false);
  },
  // A key for the focused name field (DLL, while the page reports text_focus).
  key(k) {
    textKey(k);
  },
  // The v1 meter position from the DLL's ini, used only if the layout has none yet.
  legacy(x, y) {
    if (!saved.pos.meter) { saved.pos.meter = [Math.round(x), Math.round(y)]; save(); renderAll(); }
  },
};
pluginHost.init({hosted: HOSTED, test: TEST, defineWindow, removeWindow, wins, saved: () => saved, ui, toHost, besideMeter,
  posOf, save, saveNow, renderAll, scheduleReport, look: lookOf, feed: feedView, isOpen, confirm: hubConfirm});
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
  // ?demo&...&tab=store: the Store tab with a canned list (pluginHost.sampleStore).
  if (DEMO.includes("store")) axidps.store(pluginHost.sampleStore());
  for (const k of ["support", "defense", "fight", "timeline", "healing"]) {
    saved.open[k] = true;
  }
  // ?demo&custom: three custom windows (a table, bars and a chart); custom=many: eight of them, all
  // open (the window cap's note); custom=xss: the first one named like markup (drawn as text).
  if (DEMO.includes("custom")) {
    const specs = [
      {id: "c1", name: "Damage share", look: "table", span: "fight",
        stats: [{key: "damage"}, {key: "damage", as: "share"}, {key: "dps"}, {key: "downs"}]},
      {id: "c2", name: "Last 30 s", look: "bars", span: "last:30", stats: [{key: "damage", as: "per_s"}], limit: 8},
      {id: "c3", name: "Session", look: "chart", span: "session", stats: [{key: "dps"}, {key: "hps"}, {key: "taken"}]},
    ];
    if (DEMO.includes("many")) {
      specs.push({id: "c4", name: "My DPS", look: "number", span: "last:10", stats: [{key: "dps"}], who: {base: "me"}},
        {id: "c5", name: "Group heals", look: "table", span: "session", who: {base: "group"},
          stats: [{key: "healing"}, {key: "barrier"}, {key: "hps"}]},
        {id: "c6", name: "Strips", look: "bars", span: "last:60", stats: [{key: "strips"}]},
        {id: "c7", name: "Squad total", look: "number", span: "fight", stats: [{key: "damage"}]},
        {id: "c8", name: "Firebrands", look: "table", span: "fight", who: {base: "squad", filters: ["spec:Firebrand"]},
          stats: [{key: "cleanses", as: "per_s"}, {key: "taken", as: "share"}]});
    }
    // custom=boons: a Boons window, a boon in the bars look and a boon in the number look.
    if (DEMO.includes("boons")) {
      specs.length = 0;
      specs.push({id: "c1", name: "Boons", rows: "boons", who: {base: "group"}, stats: [{key: "boons:up"}, {key: "boons:group"}]},
        {id: "c2", name: "Quickness", look: "bars", stats: [{key: "boon:1187:group"}]},
        {id: "c3", name: "Alacrity", rows: "total", look: "number", stats: [{key: "boon:30328:up"}]});
    }
    if (DEMO.includes("xss")) specs[0].name = "<img src=x onerror=alert(1)>";
    saved.custom = axidpsCustom.normalizeAll(specs);
    for (const s of saved.custom) saved.open[s.id] = true;
    syncCustom();
  }
  // The host's zones for 2560x1440 at normal interface size, compass at the bottom right.
  axidps.gameui([{id: "minimap", label: "Minimap", x: 2200, y: 1064, w: 360, h: 376},
    {id: "skills", label: "Skill bar", x: 880, y: 1265, w: 800, h: 175},
    {id: "chat", label: "Chat (default size)", x: 0, y: 1150, w: 550, h: 290},
    {id: "menu", label: "Menu bar", x: 0, y: 0, w: 320, h: 42},
    {id: "party", label: "Party / squad", x: 0, y: 44, w: 400, h: 330},
    {id: "target", label: "Target", x: 1000, y: 0, w: 560, h: 120},
    {id: "tracker", label: "Event tracker", x: 2240, y: 0, w: 320, h: 560}]);
  const names = ["Sentinel Vael", "Darkpaw", "Ironwake", "Morrow Kestrel", "Brannoc", "Ysolde Tam",
    "Oren", "Lys Harrow", "Quill", "Thessaly", "Corvin Ash", "Mira", "Dunmore", "Saffi", "Rook",
    "Talon", "Wren"];
  const SPECS = ["Firebrand", "Spellbreaker", "Scrapper", "Druid", "Deadeye", "Tempest", "Chronomancer",
    "Scourge", "Vindicator"];
  const ps = names.map((n, i) => ({name: n, spec: SPECS[i % 9], account: n.split(" ")[0] + "." + (1000 + i),
    prof: 1 + i % 9, elite: 0, group: 1 + (i % 3), me: i === 3, power: 0, condi: 0, damage: 0, dps: 0,
    downs: 0, kills: 0, deaths: 0, downed: 0, taken: 0, strips: 0, cleanses: 0, down_contrib: 0,
    healing: 0, barrier: 0, hps: 0, healing_in: 0, barrier_in: 0, heal_full: i === 3, boons: {}}));
  let ms = 0;
  const tl = {out: [], in: []};
  const step = () => {
    ms += 500;
    ps.forEach((p, i) => {
      // Boon ms (stack-ms for Might and Stability): each player holds a different share of the time.
      axidpsCustom.SQUAD_BOONS.forEach((b, j) => {
        if ((i + j) % 7 === 0) return;
        const f = 0.2 + ((i * 3 + j * 5) % 8) / 10, stacks = b === 740 ? 12 : b === 1122 ? 1.6 : 1;
        const a = p.boons[b] = p.boons[b] || [0, 0, 0, 0];
        a[0] += 500 * f * stacks; a[1] += 500 * f * 0.4 * stacks; a[2] += 500 * f * 0.7 * stacks; a[3] += 500 * f * 0.5 * stacks;
      });
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
    // The Details window's player (the core sends only the one asked for).
    const dp = ps.find(p => p.account === saved.detail);
    // Real skill ids, so the icons show; ids under 1000 and Torment are conditions.
    const sk = (list, scale) => list.map(([id, n], i) => {
      const condi = [736, 737, 723, 861, 19426].includes(id), hits = Math.max(1, Math.round(ms / 900 / (i + 1)));
      const damage = Math.round(scale * ms / (i + 1.5)), avg = damage / hits;
      return Object.assign({id, name: n, condi, damage, hits, min: Math.round(avg * 0.3), max: Math.round(avg * 1.4)},
        condi ? {} : {crit: Math.round(hits * 0.9), glance: i % 4 ? 0 : 2, block: i % 3 ? 0 : 1, evade: i % 2, barrier: Math.round(damage / 40)});
    });
    const hl = list => list.map(([id, n], i) => ({id, name: n, healing: Math.round(3 * ms / (i + 1.5)),
      barrier: i % 3 ? 0 : Math.round(ms / (i + 2)), hits: Math.round(ms / 1500 / (i + 1))}));
    const peers = ps.slice(0, 8).map((p, i) => ({name: p.name, account: p.account, prof: p.prof, spec: p.spec,
      healing: Math.round(2 * ms / (i + 1.5)), barrier: Math.round(ms / (i + 3))}));
    const detail = dp && {account: dp.account, name: dp.name, prof: dp.prof, spec: dp.spec, bucket_ms: 1000,
      out: tl.out.map(v => Math.round((v || 0) / 9)), in: tl.in.map(v => Math.round((v || 0) / 14)),
      out_condi: tl.out.map(v => Math.round((v || 0) / 30)),
      skills_out: sk([[5690, "Earthquake"], [737, "Burning"], [5671, "Static Field"], [736, "Bleeding"], [5681, "Geyser"],
        [5675, "Phoenix"], [5683, "Unsteady Ground"], [19426, "Torment"], [5661, "Murky Water"], [5687, "Updraft"]], 5),
      skills_in: sk([[10545, "Well of Corruption"], [737, "Burning"], [9140, "Holy Strike"], [736, "Bleeding"],
        [723, "Poisoned"], [861, "Confusion"]], 3),
      states: [[42000, "down"], [47000, "up"], [96000, "down"], [99000, "dead"]],
      heal_full: true, heal_out: tl.out.map(v => Math.round((v || 0) / 20)), heal_in: tl.in.map(v => Math.round((v || 0) / 25)),
      heals: hl([[9083, "\"Receive the Light!\""], [718, "Regeneration"], [9158, "Signet of Resolve"], [10213, "Mantra of Recovery"]]),
      heals_in: hl([[718, "Regeneration"], [10176, "Ether Feast"]]),
      heal_targets: peers, heal_sources: peers.slice(2, 6),
      strips: [{id: 740, name: "Might", count: 41}, {id: 1187, name: "Quickness", count: 17}, {id: 717, name: "Protection", count: 12},
        {id: 1122, name: "Stability", count: 9}],
      cleanses: [{id: 736, name: "Bleeding", count: 55}, {id: 737, name: "Burning", count: 23}, {id: 721, name: "Crippled", count: 7}],
      casts: [5690, 5671, 5681, 5675, 5683, 5661, 5687, 9083, 9158].flatMap((id, j) =>
        tl.out.map((_, i) => i).filter(i => (i + j * 3) % (6 + j) === 0).map(i => [i * 1000 + 200, id])),
      skill: saved.detail_skill ? {id: saved.detail_skill,
        out: tl.out.map((_, i) => (i % 6) < 2 ? Math.round(9000 + 4000 * Math.sin(i)) : 0),
        heal: tl.out.map((_, i) => (i % 5) < 2 ? 1500 : 0)} : null,
      boon: axidpsCustom.SQUAD_BOONS.includes(saved.detail_skill) ? {id: saved.detail_skill,
        up: tl.out.map((_, i) => [740, 1122].includes(saved.detail_skill)
          ? Math.round(1000 * (12 + 10 * Math.sin(i / 7))) : (i % 40) < 28 ? 1000 : (i % 40) < 31 ? 500 : 0)} : null,
      strip_series: tl.out.map((_, i) => (i * 7) % 5 === 0 ? 3 : i % 4 === 0 ? 1 : 0),
      cleanse_series: tl.out.map((_, i) => (i * 3) % 7 === 0 ? 4 : i % 5 === 0 ? 2 : 0)};
    axidps.update({v: 1, t: Date.now(), state: "fight", fight: {duration_ms: ms, map: 38}, players: ps,
      timeline: {bucket_ms: 1000, out: tl.out.map(v => v || 0), in: tl.in.map(v => v || 0)}, healing: heal,
      detail: detail || null});
  };
  // ?demo&details: the Details window open on the fourth player.
  if (DEMO.includes("details")) { saved.detail = ps[3].account; ui.session.details = true; }
  for (let i = 0; i < 240; i++) step();
  setInterval(step, 500);
  setInterval(() => axidps.perf({fps: 55 + Math.round(Math.random() * 10), ping: 90 + Math.round(Math.random() * 40),
    tick_rate: 24 + Math.round(Math.random() * 2)}), 1000);
}
