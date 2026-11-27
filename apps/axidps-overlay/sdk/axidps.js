// axidps plugin SDK (API version 1). Plugins load it with <script src="/_sdk/axidps.js"></script>.
//
// Two modes:
// - the overlay page itself (top level): only sets `axidpsKit`, the helpers the built-in windows
//   use (fmt, clock, esc, PROF, profColor, fightStatus, emptyText);
// - a plugin frame: also sets `window.axidps`, the plugin API, which talks to the page with
//   postMessage ({axidps: 1, type, ...}; see docs/plugins/README.md for the wire protocol).
// Frames are sandboxed (opaque origin): the page is reached only through window.parent.
(function () {
  "use strict";

  // Profession colours (ids 1-9: Guardian, Warrior, Engineer, Ranger, Thief, Elementalist,
  // Mesmer, Necromancer, Revenant).
  const PROF = ["#888", "#72c1d9", "#ffd166", "#d09c59", "#8cdc82", "#c08f95", "#f68a87",
    "#b679d5", "#52a76f", "#d16e5a"];

  function fmt(n) {
    n = Number(n) || 0;
    const a = Math.abs(n);
    if (a >= 1e6) return (n / 1e6).toFixed(a >= 1e7 ? 1 : 2) + "M";
    if (a >= 1e4) return (n / 1e3).toFixed(a >= 1e5 ? 0 : 1) + "k";
    return Math.round(n).toLocaleString();
  }
  function clock(ms) {
    const s = Math.floor((ms || 0) / 1000);
    return Math.floor(s / 60) + ":" + String(s % 60).padStart(2, "0");
  }
  function esc(s) {
    return String(s ?? "").replace(/[&<>"]/g, c => ({"&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;"}[c]));
  }
  function profColor(prof) { return PROF[prof] || PROF[0]; }
  // The title bar's fight state for a feed (null: no core yet; `cleared`: Clear fight applied).
  function fightStatus(feed) {
    if (!feed) return {text: "waiting for core", live: false};
    if (feed.cleared) return {text: "cleared", live: false};
    const dur = clock(feed.fight && feed.fight.duration_ms);
    if (feed.state === "fight") return {text: "in combat " + dur, live: true};
    if (feed.state === "last") return {text: "last fight " + dur, live: false};
    return {text: "out of combat", live: false};
  }
  // What an empty window says for a feed.
  function emptyText(feed) {
    if (!feed) return "Waiting for axidps core.";
    if (feed.cleared) return "Cleared. The next fight starts fresh.";
    if (feed.state === "idle") return "No fight yet.";
    return "No squad data for this fight yet.";
  }
  const kit = {fmt, clock, esc, PROF, profColor, fightStatus, emptyText};

  // Where a tooltip goes inside a frame of W x H for an element at rect r: centred under it, above
  // it when there is no room below, and always kept inside the frame (an autosized frame is only
  // as big as its content, so the tip may overlap the element rather than be cut off).
  function tipPos(r, tw, h, W, H) {
    let x = r.left + (r.width - tw) / 2, y = r.bottom + 2;
    if (y + h > H - 2) y = r.top - h - 2;
    x = Math.max(2, Math.min(x, W - tw - 2));
    y = Math.max(2, Math.min(y, H - h - 2));
    return [x, y];
  }

  // The page's fonts and base look, so plain HTML in a frame matches the built-in windows.
  const FONTS = [["IBM Plex Sans", "IBMPlexSans-Regular", "400"], ["IBM Plex Sans", "IBMPlexSans-SemiBold", "600 700"],
    ["Fira Code", "FiraCode-Regular", "400"], ["Fira Code", "FiraCode-SemiBold", "600 700"],
    ["Lato", "Lato-Regular", "400"], ["Lato", "Lato-Bold", "600 700"]];
  const BASE_CSS = FONTS.map(([f, file, wt]) =>
    `@font-face { font-family: "${f}"; src: url(/_sdk/fonts/${file}.ttf); font-weight: ${wt}; }`).join("\n") + `
:root { --bg: #12151b; --bar: #12151b; --raise: #1a1e26; --raise-hi: #222833; --btn: #232a35;
  --btn-hi: #2e3746; --line: #242a34; --line-hi: #343c49; --text: #e8eaee; --title: #f2f4f7;
  --dim: #8d96a5; --faint: #646d7a; --accent: #7aa2f7; --accent-hi: #a9c1ff; --accent-line: #34456a;
  --accent-soft: rgba(122,162,247,.13); --good: #9ece6a; --bad: #f7768e; --tip: #0b0d11;
  --font: "IBM Plex Sans", system-ui, sans-serif; --fs: 12px; --r: 10px; --rs: 6px; --bw: 1px;
  --rh: 21px; --bh: 32px; }
* { user-select: none; -webkit-user-select: none; cursor: default; box-sizing: border-box; }
html, body { margin: 0; background: transparent; overflow: hidden; color: var(--text); font: var(--fs)/1.3 var(--font); }
body { display: inline-block; width: max-content; vertical-align: top; }
body.compact { --rh: 16px; --bh: 22px; --fs: 10.5px; font-size: var(--fs); }
b, strong { font-weight: 600; }
.axidps-tip { position: fixed; display: none; padding: 3px 8px; background: var(--tip); color: var(--text);
  border: 1px solid var(--line-hi); border-radius: var(--rs); white-space: nowrap; pointer-events: none;
  font-size: 11px; z-index: 2147483647; }`;

  const TYPES = ["fight", "look", "edit", "settings", "perf"];
  const clone = v => v === undefined ? undefined : JSON.parse(JSON.stringify(v));
  function str(v) {
    if (typeof v === "string") return v;
    if (v instanceof Error) return v.stack || String(v);
    try { const j = JSON.stringify(v); return j === undefined ? String(v) : j; } catch (e) { return String(v); }
  }

  // The game icons a `textures` rule can name (the wire list, axidps_overlay_shm::texstyle).
  const TEXTURE_TARGETS = ["commander.red", "commander.orange", "commander.yellow", "commander.green",
    "commander.cyan", "commander.blue", "commander.purple", "commander.magenta", "commander.white",
    "squad.commander", "squad.lieutenant", "mentor", "marker.arrow", "marker.circle", "marker.heart",
    "marker.square", "marker.star", "marker.spiral", "marker.triangle", "marker.cross",
    "selection.orange", "selection.blue", "selection.green", "selection.red", "target",
    "target.personal", "target.combined", "target.attack", "target.attack-selected"];
  const MAX_TEXTURE_RULES = 64;
  // The largest settings object the page saves, in bytes of JSON (the page's MAX_SETTINGS).
  const MAX_SETTINGS = 16384;
  function utf8Len(s) {
    if (typeof TextEncoder === "function") return new TextEncoder().encode(s).length;
    return unescape(encodeURIComponent(s)).length;
  }

  // The plugin API for a frame's window `win` (its parent is the overlay page).
  function makeSdk(win) {
    const parent = win.parent;
    const post = (type, body) => {
      try { parent.postMessage(Object.assign({axidps: 1, type}, body || {}), "*"); } catch (e) { /* page gone */ }
    };
    const log = (level, msg) => post("log", {level, msg: String(msg).slice(0, 1000)});
    const handlers = {fight: [], look: [], edit: [], settings: [], perf: []};
    const last = {};
    let info = null;
    const readyQ = [];
    let settings = {};
    let auto = true, lastSize = "";
    const call = (fn, v) => { try { fn(v); } catch (e) { log("error", str(e)); } };
    const emit = (type, v) => { last[type] = {v}; for (const fn of handlers[type].slice()) call(fn, clone(v)); };

    const doc = win.document;
    function applyLook(l) {
      if (!doc || !doc.documentElement) return;
      const root = doc.documentElement.style;
      for (const [k, v] of Object.entries(l.tokens || {})) {
        if (/^[a-z-]+$/.test(k) && typeof v === "string") root.setProperty("--" + k, v);
      }
      const st = l.style || {};
      if (typeof st.font === "string") root.setProperty("--font", st.font);
      for (const k of ["fs", "r", "rs", "bw", "rh", "bh"]) if (typeof st[k] === "number") root.setProperty("--" + k, st[k] + "px");
      if (doc.body) {
        doc.body.classList.toggle("compact", !!l.compact);
        doc.body.classList.toggle("caps", !!st.caps);
      }
    }
    function measure() {
      if (!auto || !doc || !doc.body) return;
      // The exact size, fractions included: the window around the frame then lays out as the
      // same content would in the page itself (rounded up, it could be a pixel taller).
      const r = doc.body.getBoundingClientRect();
      const key = r.width + "x" + r.height;
      if (key === lastSize) return;
      lastSize = key;
      post("resize", {w: r.width, h: r.height});
    }

    win.addEventListener("message", e => {
      if (e.source !== parent) return;
      const m = e.data;
      if (!m || typeof m !== "object" || m.axidps !== 1) return;
      switch (m.type) {
        case "ready":
          info = {plugin: m.plugin, window: m.window, api: m.api, permissions: clone(m.permissions) || []};
          lastSize = ""; measure();
          for (const fn of readyQ.splice(0)) call(fn, clone(info));
          break;
        case "fight": emit("fight", m.feed); break;
        case "look": {
          const l = {theme: m.theme, tokens: m.tokens, style: m.style, opacity: m.opacity, compact: m.compact, rows: m.rows};
          applyLook(l);
          emit("look", l);
          break;
        }
        case "edit": emit("edit", !!m.on); break;
        case "perf": emit("perf", m.perf); break;
        case "settings":
          settings = m.value && typeof m.value === "object" ? m.value : {};
          emit("settings", settings);
          break;
        case "ping": post("pong", {n: m.n}); break;
      }
    });

    // console.warn / console.error and uncaught errors reach the axidps log as `plugin <id>: ...`.
    const con = win.console;
    if (con) for (const level of ["warn", "error"]) {
      const orig = con[level];
      con[level] = function (...a) {
        log(level, a.map(str).join(" "));
        if (typeof orig === "function") return orig.apply(con, a);
      };
    }
    win.addEventListener("error", e => {
      log("error", (e && e.message || "error") + (e && e.filename ? ` (${e.filename}:${e.lineno || 0})` : ""));
    });
    win.addEventListener("unhandledrejection", e => log("error", "unhandled rejection: " + str(e && e.reason)));

    // The frame's DOM: base styles now, then the size reports and tooltips once the body exists.
    if (doc && doc.createElement) {
      const style = doc.createElement("style");
      style.textContent = BASE_CSS;
      (doc.head || doc.documentElement).appendChild(style);
      const start = () => {
        const tip = doc.createElement("div");
        tip.className = "axidps-tip";
        doc.body.appendChild(tip);
        // Tooltips for [data-tip] (placed by tipPos).
        win.addEventListener("mouseover", e => {
          const el = e.target && e.target.closest && e.target.closest("[data-tip]");
          if (!el || !el.dataset.tip) { tip.style.display = "none"; return; }
          tip.textContent = el.dataset.tip;
          tip.style.display = "block";
          const W = doc.documentElement.clientWidth, H = doc.documentElement.clientHeight;
          const [x, y] = tipPos(el.getBoundingClientRect(), tip.offsetWidth, tip.offsetHeight, W, H);
          tip.style.left = x + "px"; tip.style.top = y + "px";
        });
        win.addEventListener("mouseout", e => { if (!e.relatedTarget) tip.style.display = "none"; });
        win.addEventListener("wheel", () => { tip.style.display = "none"; }, {passive: true});
        if (typeof win.ResizeObserver === "function") new win.ResizeObserver(measure).observe(doc.body);
        else win.setInterval(measure, 250);
        measure();
        if (last.look) applyLook(last.look.v);
      };
      if (doc.readyState === "loading") doc.addEventListener("DOMContentLoaded", start);
      else if (doc.body) start();
    }

    const sdk = Object.assign({}, kit, {
      api: 1,
      // The URL of a specialization's icon (the feed's `spec`, like "Firebrand"), the same icons
      // the built-in tables show; "" for none.
      specIcon(spec) { return typeof spec === "string" && spec ? "/_sdk/icons/" + encodeURIComponent(spec) + ".png" : ""; },
      ready(fn) {
        if (typeof fn !== "function") return;
        if (info) call(fn, clone(info)); else readyQ.push(fn);
      },
      // on(type, fn): "fight" (the live feed), "look" (theme and display), "edit" (bool),
      // "settings" (this window's settings), "perf" ({fps, ping, tick_rate}, once a second;
      // 0 = unknown). A handler added late gets the latest value at once.
      on(type, fn) {
        if (!TYPES.includes(type) || typeof fn !== "function") return;
        handlers[type].push(fn);
        if (last[type]) call(fn, clone(last[type].v));
      },
      settings: {
        get() { return clone(settings); },
        // An object, at most 16 KB as JSON. A larger one (or one that isn't JSON) is not saved:
        // an error is logged and the settings stay as they were.
        set(obj) {
          if (!obj || typeof obj !== "object" || Array.isArray(obj)) { log("error", "settings.set needs an object"); return; }
          let size = -1;
          try { const j = JSON.stringify(obj); if (typeof j === "string") size = utf8Len(j); } catch (e) { /* cyclic */ }
          if (size < 0) { log("error", "settings.set: the object can't be saved as JSON; not saved"); return; }
          if (size > MAX_SETTINGS) {
            log("error", `settings.set: ${size} bytes as JSON, at most ${MAX_SETTINGS}; not saved`);
            return;
          }
          settings = clone(obj);
          post("settings", {value: clone(settings)});
        },
      },
      // Restyling the game's own icons. Needs the `textures` permission; without it the page
      // ignores these. set(rules) replaces this plugin's rules (see docs/plugins/README.md for a
      // rule's fields); clear() removes them. Rules stay while the plugin is enabled.
      textures: {
        targets: TEXTURE_TARGETS.slice(),
        set(rules) {
          if (!Array.isArray(rules)) { log("error", "textures.set needs an array of rules"); return; }
          const ok = rules.filter(r => r && typeof r === "object" && TEXTURE_TARGETS.includes(r.target));
          if (ok.length !== rules.length) log("warn", `textures.set: ${rules.length - ok.length} rules without a known target dropped`);
          if (ok.length > MAX_TEXTURE_RULES) log("warn", `textures.set: only the first ${MAX_TEXTURE_RULES} rules are used`);
          let copy;
          try { copy = JSON.parse(JSON.stringify(ok.slice(0, MAX_TEXTURE_RULES))); } catch (e) {
            log("error", "textures.set: the rules can't be sent as JSON"); return;
          }
          post("textures", {rules: copy});
        },
        clear() { post("textures", {rules: []}); },
      },
      window: {
        setTitle(text) { post("title", {text: String(text ?? "").slice(0, 80)}); },
        setStatus(text, opts) {
          post("status", {text: String(text ?? "").slice(0, 80), live: !!(opts && opts.live)});
        },
        resize(w, h) { post("resize", {w: Math.ceil(Number(w) || 0), h: Math.ceil(Number(h) || 0)}); },
        autoSize(on) { auto = on !== false; if (auto) { lastSize = ""; measure(); } },
        tip(el, text) { if (el && el.dataset) el.dataset.tip = String(text ?? ""); },
      },
    });
    post("hello");
    return sdk;
  }

  const root = typeof globalThis !== "undefined" ? globalThis : this;
  root.axidpsKit = kit;
  if (typeof window !== "undefined" && window.parent && window.parent !== window) window.axidps = makeSdk(window);
  if (typeof module !== "undefined" && module.exports) module.exports = {kit, makeSdk, tipPos};
})();
