// The overlay page's side of plugins: which plugin windows exist, their sandboxed frames, and the
// postMessage protocol with them ({axidps: 1, type, ...}; sdk/axidps.js is the frame side).
// Pure functions first (tested with node), then `pluginHost`, which ui.js wires in with init().
// Security: frames are <iframe sandbox="allow-scripts"> (opaque origin, never allow-same-origin);
// a message's sender is only ever its event.source, never ids in its body; nothing secret or
// page-global is posted to a frame.
(function () {
  "use strict";

  const API = 1;
  const MAX_OPEN = 16;
  const MAX_TEXT = 80;
  const MAX_SETTINGS = 16384;
  const PING_MS = 2000, STALL_MS = 5000;
  const DEFAULT_SIZE = [240, 120];
  // The bundled windows that keep the names the DLL and saved layouts already know.
  const BUNDLED_ALIAS = {"axidps.fight/fight": "fight", "axidps.timeline/timeline": "timeline",
    "axidps.healing/healing": "healing"};
  // Saved keys of the windows that were built in before they became bundled plugins.
  const MIGRATE = {fight: "axidps.fight/fight", timeline: "axidps.timeline/timeline", healing: "axidps.healing/healing"};
  const ICONS = ["chart", "users", "shield", "swords", "activity", "x", "grid", "layers", "sliders", "plug",
    "download", "columns", "plus"];
  const PERMISSION_TEXT = {fight: "Reads your fight data", world: "Draws over the game world",
    position: "Reads your position and camera", squad: "Reads your squad and party",
    enemies: "Reads enemy players seen in combat"};
  const PLUGIN_ID = /^[a-z0-9.-]{3,48}$/, WINDOW_ID = /^[a-z0-9-]+$/;
  const own = (o, k) => !!o && typeof o === "object" && Object.prototype.hasOwnProperty.call(o, k);

  // The windows of `list` (the host's plugin list) to show: valid plugins only, those `enabled`
  // says yes to. Aliases (the DLL's 16-byte window names) are p1, p2, ... over every valid window
  // in list order, whether shown or not, so enabling one plugin doesn't rename another's windows;
  // the bundled Fight, Timeline and Healing keep fight, timeline and healing.
  function validManifestWindows(list, enabled = () => true) {
    const out = [], seen = new Set();
    let n = 0;
    for (const p of Array.isArray(list) ? list : []) {
      if (!p || typeof p !== "object" || p.error || typeof p.id !== "string" || !PLUGIN_ID.test(p.id)
        || !Array.isArray(p.windows)) continue;
      const on = !!enabled(p);
      for (const win of p.windows.slice(0, 4)) {
        if (!win || typeof win.id !== "string" || !WINDOW_ID.test(win.id) || typeof win.entry !== "string") continue;
        const fullId = p.id + "/" + win.id;
        if (seen.has(fullId)) continue;
        seen.add(fullId);
        const alias = (p.bundled === true && BUNDLED_ALIAS[fullId]) || "p" + ++n;
        if (on) out.push({plugin: p, win, fullId, alias});
      }
    }
    return out;
  }

  // What the player grants a plugin by enabling it: its permissions and one "net:<host>" per
  // network entry.
  function grantsOf(p) {
    const perms = Array.isArray(p && p.permissions) ? p.permissions : [];
    const net = Array.isArray(p && p.network) ? p.network : [];
    return perms.filter(x => typeof x === "string").concat(net.filter(x => typeof x === "string").map(h => "net:" + h));
  }

  // Bundled plugins are on unless switched off. A player's plugin is on only once enabled, and
  // only while the manifest asks for nothing beyond what was granted then.
  function isEnabled(p, savedPlugins) {
    if (!p || typeof p !== "object" || p.error) return false;
    const s = own(savedPlugins, p.id) ? savedPlugins[p.id] : null;
    if (p.bundled === true) return !(s && s.enabled === false);
    if (!s || s.enabled !== true || !Array.isArray(s.granted)) return false;
    return grantsOf(p).every(g => s.granted.includes(g));
  }

  // What a plugin asks for, in the player's words.
  function permissionLines(p) {
    const perms = Array.isArray(p && p.permissions) ? p.permissions : [];
    const net = Array.isArray(p && p.network) ? p.network : [];
    return perms.filter(x => own(PERMISSION_TEXT, x)).map(x => PERMISSION_TEXT[x])
      .concat(net.filter(x => typeof x === "string").map(h => "Talks to " + h));
  }

  // A plugin's hub card: what it is, what it asks for, and what the player can do with it.
  // action: "enable" | "disable" | null (an invalid plugin, or one that needs verification);
  // reason: why it can't be enabled ("" if it can); label: "Unverified" for every player plugin;
  // reload: developer mode's Reload, for an enabled player plugin.
  function cardModel(p, saved) {
    const s = saved && typeof saved === "object" ? saved : {};
    const str = v => typeof v === "string" ? v.slice(0, MAX_TEXT) : "";
    const err = p && p.error ? String(p.error) : "";
    const on = isEnabled(p, s.plugins);
    const bundled = !!p && p.bundled === true;
    return {
      title: str(p && p.name) || str(p && p.id),
      author: str(p && p.author),
      version: str(p && p.version),
      lines: permissionLines(p),
      action: err ? null : on ? "disable" : "enable",
      reason: err === "needs verification" ? "Needs verification" : err,
      label: bundled ? null : "Unverified",
      reload: !err && on && !bundled && s.dev === true,
    };
  }

  // Copy the saved pos/open/cfg of the windows that used to be built in (fight, timeline,
  // healing) to their full plugin ids, unless those exist already. The old keys stay, so an older
  // build (which reads them) still finds its windows where they were; nothing here reads them.
  // Safe to run again: the full-id key, once there, wins.
  function migrateSaved(saved) {
    if (!saved || typeof saved !== "object") return saved;
    for (const m of ["pos", "open", "cfg"]) {
      const o = saved[m];
      if (!o || typeof o !== "object") continue;
      for (const [old, full] of Object.entries(MIGRATE)) {
        if (own(o, old) && !own(o, full)) o[full] = o[old];
      }
    }
    return saved;
  }

  // A frame's size, kept so a plugin can't break the atlas: 40-1000 x 24-1000. Fractions stay (an
  // autosized body's exact size), so a window is as tall as with the same content in the page.
  function clampSize(w, h) {
    const c = (v, lo, d) => {
      v = Number(v);
      return Number.isFinite(v) ? Math.max(lo, Math.min(1000, v)) : d;
    };
    return [c(w, 40, DEFAULT_SIZE[0]), c(h, 24, DEFAULT_SIZE[1])];
  }

  function utf8Len(s) {
    if (typeof TextEncoder === "function") return new TextEncoder().encode(s).length;
    return unescape(encodeURIComponent(s)).length;
  }
  // Whether a frame -> page message is well formed: a known type with fields of the right types.
  function accept(m) {
    if (!m || typeof m !== "object" || Array.isArray(m) || m.axidps !== API) return false;
    const text = v => typeof v === "string" && v.length <= MAX_TEXT;
    const num = v => typeof v === "number" && Number.isFinite(v);
    switch (m.type) {
      case "hello": return true;
      case "resize": return num(m.w) && num(m.h);
      case "title": return text(m.text);
      case "status": return text(m.text) && (m.live === undefined || typeof m.live === "boolean");
      case "settings": {
        if (!m.value || typeof m.value !== "object" || Array.isArray(m.value)) return false;
        try {
          const j = JSON.stringify(m.value);
          return typeof j === "string" && utf8Len(j) <= MAX_SETTINGS;
        } catch (e) {
          return false;
        }
      }
      case "log": return (m.level === "warn" || m.level === "error") && typeof m.msg === "string";
      case "pong": return num(m.n);
      default: return false;
    }
  }

  // A log limiter for one plugin: `limit` lines per `span` ms. gate(now) -> {pass, dropped (this
  // span so far), resetIn (ms left in this span)}.
  function logGate(limit = 20, span = 1000) {
    let start = -Infinity, count = 0, dropped = 0;
    return (now = Date.now()) => {
      if (now - start >= span) { start = now; count = 0; dropped = 0; }
      const resetIn = start + span - now;
      if (count < limit) { count++; return {pass: true, dropped, resetIn}; }
      dropped++;
      return {pass: false, dropped, resetIn};
    };
  }

  // When a frame's new size may be applied: at most once per `span` ms. gate(now) -> ms to wait
  // (0: now). A size changes where every later window sits in the atlas, and the DLL aims clicks
  // with the window table it last read, so a frame resizing non-stop would make clicks into the
  // windows after it land off target; held to a few changes a second, the table stays true
  // nearly all the time.
  function resizeGate(span = 250) {
    let last = -Infinity;
    return (now = Date.now()) => {
      const wait = Math.max(0, last + span - now);
      if (wait === 0) last = now;
      return wait;
    };
  }

  // The windows (in `wins` order) that `isWanted` says are open beyond the first `cap`: they are
  // shown as closed.
  function overCap(wins, isWanted, cap) {
    let n = 0;
    const over = [];
    for (const w of wins) {
      if (!isWanted(w)) continue;
      if (n < cap) n++; else over.push(w);
    }
    return over;
  }

  // Why plugin window `w` ({alias, fullId}) can't be defined beside `wins`, or "" if it can: its
  // alias is another window's name, or its full id is another window's saved key (a player's
  // `axidps.fight` must not share the built-in Fight's position and settings).
  function nameTaken(wins, w) {
    if (wins.some(x => x.id === w.alias)) return `the name ${w.alias} is taken`;
    if (wins.some(x => x.key === w.fullId)) return `${w.fullId} is taken`;
    return "";
  }

  // A frame's error count (developer mode's title bar badge): {count, last: the newest 5 messages}.
  // A `log` event with level "error" adds one; a frame (re)load ({type: "load"}) starts over.
  const BADGE_LAST = 5, BADGE_MSG = 200;
  function errorBadge(state, event) {
    const s = state && typeof state === "object" ? state : {count: 0, last: []};
    if (!event || event.type === "load") return {count: 0, last: []};
    if (event.type !== "log" || event.level !== "error") return s;
    const msg = String(event.msg == null ? "" : event.msg).slice(0, BADGE_MSG);
    return {count: s.count + 1, last: s.last.concat([msg]).slice(-BADGE_LAST)};
  }

  // The frame entry a message came from, by its event.source alone; null for anything else.
  function route(sourceMap, event) {
    if (!event || !event.source || !sourceMap) return null;
    return sourceMap.get(event.source) || null;
  }

  // ---- the page: frames for the enabled plugin windows -----------------------------------------

  // env (from ui.js): {hosted, test (?test: send debug reports), defineWindow(def), removeWindow(w), wins,
  //   saved(), ui, toHost(text), besideMeter(after, dy), posOf(w), save(), saveNow(), renderAll(), scheduleReport(),
  //   look(), feed(), isOpen(w)}
  const host = {
    env: null,
    list: [],
    want: [],
    entries: [],
    sources: new Map(),

    init(env) {
      this.env = env;
      addEventListener("message", e => {
        const ent = route(this.sources, e);
        if (ent && accept(e.data)) this.handle(ent, e.data);
      });
      setInterval(() => this.pingAll(Date.now()), PING_MS);
      // Demo mode (no host): the preview server (`axidps plugin preview`) lists the plugins and
      // says when one changes.
      if (!env.hosted && typeof fetch === "function" && /^https?:$/.test(location.protocol)) {
        this.fetchList().then(ok => {
          if (!ok || typeof EventSource !== "function") return;
          new EventSource("/_events").addEventListener("changed", e => {
            const id = String(e.data || "");
            this.fetchList().then(() => this.changed(id));
          });
        });
      }
    },
    fetchList() {
      return fetch("/plugins.json").then(r => r.ok ? r.json() : null).then(list => {
        if (!Array.isArray(list)) return false;
        this.setList(list);
        return true;
      }).catch(() => false);
    },

    // The host's plugin list (axidps.plugins): define the windows of enabled plugins, remove those
    // of plugins that are gone or disabled.
    setList(list) {
      this.list = Array.isArray(list) ? list : [];
      this.refresh();
    },
    refresh() {
      const env = this.env;
      if (!env) return;
      const saved = env.saved();
      const sp = saved.plugins || {};
      const on = p => isEnabled(p, sp) || (!env.hosted && p.preview === true && !p.error);
      this.want = validManifestWindows(this.list, on);
      for (const ent of this.entries.slice()) {
        const w = this.want.find(x => x.fullId === ent.fullId);
        if (!w || w.alias !== ent.alias || w.win.entry !== ent.win.entry) this.drop(ent);
        else { ent.plugin = w.plugin; ent.win = w.win; }
      }
      for (const w of this.want) {
        if (this.entries.some(e => e.fullId === w.fullId)) continue;
        // In the browser demo the plugin being previewed starts open.
        if (!env.hosted && w.plugin.preview === true && !own(saved.open, w.fullId)) saved.open[w.fullId] = true;
        this.add(w);
      }
      env.renderAll();
      this.debug();
    },
    // The player's OK in the hub: enabled for exactly what the plugin asks for now. Disabling keeps
    // the grants and the windows' saved positions and settings. Either saves at once (the host
    // serves only enabled plugins, by the saved enables) and then asks the host to rescan.
    // Hosted, an enabled plugin's windows appear with the list the rescan sends back: framed
    // before the host has the save, their files would not be served yet.
    enable(id) { this.choose(id, true); },
    disable(id) { this.choose(id, false); },
    choose(id, enabled) {
      const env = this.env;
      const p = this.list.find(x => x && x.id === id);
      if (!env || !p || (enabled && p.error)) return;
      const saved = env.saved();
      if (!saved.plugins || typeof saved.plugins !== "object") saved.plugins = {};
      saved.plugins[id] = {enabled, granted: grantsOf(p)};
      env.saveNow();
      env.toHost(JSON.stringify({plugins: "scan"}));
      if (enabled && env.hosted) env.renderAll();
      else this.refresh();
    },
    // The hub card's Reload (developer mode): rescan, then reload the plugin's frames.
    reload(id) {
      if (!this.env || !this.list.some(x => x && x.id === id)) return;
      this.env.toHost(JSON.stringify({plugins: "scan"}));
      this.changed(id);
    },
    srcOf(w) {
      const path = w.win.entry.split("/").map(encodeURIComponent).join("/");
      return this.env.hosted ? `axidps-plugin://${w.plugin.id}/${path}` : `/plugins/${w.plugin.id}/${path}`;
    },
    // Default spot: beside-meter windows chain right of Support in list order; below-meter ones
    // sit under the meter (as Timeline does).
    at(ent) {
      const env = this.env;
      if (ent.win.place === "below-meter") {
        return ([sw, sh]) => {
          const m = env.wins.find(v => v.id === "meter"), [mx, my] = env.posOf(m);
          return [Math.max(0, Math.min(mx, sw - 430)), Math.max(0, Math.min(my + m.el.offsetHeight + 6, sh - 200))];
        };
      }
      return () => {
        const before = [];
        for (const w of this.want) {
          if (w.fullId === ent.fullId) break;
          if (w.win.place !== "below-meter") before.push(w.alias);
        }
        return env.besideMeter(["support"].concat(before), 0);
      };
    },
    add(w) {
      const env = this.env;
      const taken = nameTaken(env.wins, w);
      if (taken) {
        console.warn(`plugin window ${w.fullId}: ${taken}; not shown`);
        return;
      }
      const ent = {plugin: w.plugin, win: w.win, fullId: w.fullId, alias: w.alias, src: this.srcOf(w),
        frame: null, cw: null, w: null, size: DEFAULT_SIZE.slice(), hello: false, helloAt: 0,
        seen: Date.now(), n: 0, stalled: false, status: {text: "", live: false}, gate: logGate(), dropped: 0,
        sizeGate: resizeGate(), wantSize: null, sizeTimer: 0,
        badge: errorBadge(null, {type: "load"})};
      const title = String(w.win.title || w.plugin.name || w.plugin.id).slice(0, MAX_TEXT);
      const glyph = ICONS.includes(w.win.icon) ? w.win.icon : "plug";
      ent.title = title;
      ent.w = env.defineWindow({
        id: w.alias, key: w.fullId, title, plugin: true, open: w.win.open === true,
        blurb: [title, String(w.win.description || "").slice(0, MAX_TEXT), glyph],
        at: this.at(ent),
        mount: win => this.mount(ent, win),
      });
      if (w.plugin.bundled !== true) {
        const b = ent.w.el.querySelector(".bar b");
        if (b) b.dataset.tip = `${String(w.plugin.name || w.plugin.id).slice(0, MAX_TEXT)} · Unverified`;
      }
      this.entries.push(ent);
      this.debug();
    },
    mount(ent, win) {
      const f = document.createElement("iframe");
      f.setAttribute("sandbox", "allow-scripts");
      f.className = "plugin";
      f.style.width = ent.size[0] + "px";
      f.style.height = ent.size[1] + "px";
      f.style.pointerEvents = this.env.ui.editing ? "none" : "";
      f.src = ent.src;
      win.body.appendChild(f);
      ent.frame = f;
      ent.cw = f.contentWindow;
      if (ent.cw) this.sources.set(ent.cw, ent);
    },
    drop(ent) {
      if (ent.cw) this.sources.delete(ent.cw);
      if (ent.w) this.env.removeWindow(ent.w);
      const i = this.entries.indexOf(ent);
      if (i >= 0) this.entries.splice(i, 1);
      this.debug();
    },
    post(ent, type, body) {
      try {
        ent.cw.postMessage(Object.assign({axidps: API, type}, body || {}), "*");
      } catch (e) { /* frame gone */ }
    },
    hasFight(ent) { return Array.isArray(ent.plugin.permissions) && ent.plugin.permissions.includes("fight"); },
    showStatus(ent) {
      const st = ent.w && ent.w.state;
      if (!st) return;
      const live = !ent.stalled && ent.status.live;
      st.className = "state" + (live ? " live" : "");
      st.textContent = ent.stalled ? "not responding" : ent.status.text;
      this.debug();
    },
    // Developer mode's error count in the title bar, in --bad, the last messages in its tooltip.
    showBadge(ent) {
      const bar = ent.w && ent.w.bar;
      if (bar && typeof bar.querySelector === "function") {
        let el = bar.querySelector(".errs");
        const on = this.env.saved().dev === true && ent.badge.count > 0;
        if (on && !el) {
          el = document.createElement("span");
          el.className = "errs";
          bar.insertBefore(el, bar.querySelector(".tools"));
        }
        if (el) {
          el.style.display = on ? "" : "none";
          el.textContent = String(ent.badge.count);
          // One line per message, short enough to stay inside a small window.
          el.dataset.tip = ent.badge.last.map(x => x.length > 60 ? x.slice(0, 59) + "…" : x).join("\n");
        }
      }
      this.debug();
    },
    // `?test` only: each plugin window's status text and error count, every listed plugin's id
    // and error, and the hub's window note, to the host log (the harness reads it). Sent when it
    // changes.
    debug() {
      const env = this.env;
      if (!env || !env.test || this.debugTimer) return;
      this.debugTimer = setTimeout(() => {
        this.debugTimer = 0;
        const windows = this.entries.map(e => ({window: e.fullId, alias: e.alias,
          status: e.stalled ? "not responding" : e.status.text, errors: e.badge.count}));
        const plugins = this.list.filter(p => p && typeof p === "object")
          .map(p => ({id: String(p.id), error: p.error ? String(p.error) : ""}));
        const notice = String((env.ui && env.ui.notice) || "");
        const msg = JSON.stringify({debug: {windows, plugins, notice}});
        if (msg !== this.debugSent) { this.debugSent = msg; env.toHost(msg); }
      }, 0);
    },
    handle(ent, m) {
      const env = this.env, now = Date.now();
      ent.seen = now;
      if (ent.stalled) { ent.stalled = false; this.showStatus(ent); this.catchUp(ent); }
      switch (m.type) {
        case "hello": {
          if (now - ent.helloAt < 250) return; // a frame repeating hello gets one answer
          ent.helloAt = now;
          ent.hello = true;
          // The SDK says hello once per document: a (re)load, the frame's own included.
          ent.badge = errorBadge(ent.badge, {type: "load"});
          this.showBadge(ent);
          const saved = env.saved();
          this.post(ent, "ready", {plugin: ent.plugin.id, window: ent.win.id, api: API,
            permissions: (ent.plugin.permissions || []).slice()});
          const cfg = own(saved.cfg, ent.fullId) && saved.cfg[ent.fullId] && typeof saved.cfg[ent.fullId] === "object"
            ? saved.cfg[ent.fullId] : {};
          this.post(ent, "settings", {value: cfg});
          this.post(ent, "look", env.look());
          this.post(ent, "edit", {on: !!env.ui.editing});
          if (this.hasFight(ent)) this.post(ent, "fight", {feed: env.feed()});
          if (this.lastPerf) this.post(ent, "perf", {perf: this.lastPerf});
          ent.lookStale = ent.feedStale = ent.perfStale = false;
          break;
        }
        case "resize": {
          // The latest size wins; it is applied now or, after a recent change, once resizeGate allows.
          ent.wantSize = clampSize(m.w, m.h);
          if (ent.sizeTimer) return;
          const apply = () => {
            ent.sizeTimer = 0;
            const s = ent.wantSize;
            if (!s || (s[0] === ent.size[0] && s[1] === ent.size[1])) return;
            const wait = ent.sizeGate(Date.now());
            if (wait > 0) { ent.sizeTimer = setTimeout(apply, wait); return; }
            ent.size = s;
            if (ent.frame) { ent.frame.style.width = s[0] + "px"; ent.frame.style.height = s[1] + "px"; }
            env.scheduleReport();
          };
          apply();
          break;
        }
        case "title": {
          const b = ent.w && ent.w.el.querySelector(".bar b");
          if (b) b.textContent = m.text || ent.title;
          break;
        }
        case "status":
          ent.status = {text: m.text, live: m.live === true};
          this.showStatus(ent);
          break;
        case "settings":
          env.saved().cfg[ent.fullId] = m.value;
          env.save();
          break;
        case "log": {
          if (m.level === "error") { ent.badge = errorBadge(ent.badge, m); this.showBadge(ent); }
          const g = ent.gate(now);
          const send = (level, msg) => env.toHost(JSON.stringify({log: {plugin: ent.plugin.id, level, msg}}));
          if (g.pass) send(m.level, m.msg.slice(0, 500));
          else {
            ent.dropped = g.dropped;
            if (g.dropped === 1) setTimeout(() => send("warn", `… ${ent.dropped} lines dropped`), Math.max(0, g.resetIn));
          }
          break;
        }
        case "pong":
          break;
      }
    },
    pingAll(now) {
      for (const ent of this.entries) {
        if (!ent.cw) continue;
        if (!ent.stalled && now - ent.seen > STALL_MS) { ent.stalled = true; this.showStatus(ent); }
        this.post(ent, "ping", {n: ++ent.n});
      }
      this.debug(); // the hub's note can change without a plugin event
    },
    // Whether a frame is fed the live feed and look as they change: it said hello, answers, and
    // its window is open. A hung frame never drains its messages, and a closed one shows nothing,
    // so they get only what is current once they answer again or open (`catchUp`).
    fed(ent) {
      const isOpen = this.env && this.env.isOpen;
      return ent.hello && !ent.stalled && (!isOpen || !ent.w || isOpen(ent.w));
    },
    catchUp(ent) {
      if (!this.fed(ent)) return;
      if (ent.lookStale) this.post(ent, "look", this.env.look());
      if (ent.feedStale && this.hasFight(ent)) this.post(ent, "fight", {feed: this.env.feed()});
      if (ent.perfStale && this.lastPerf) this.post(ent, "perf", {perf: this.lastPerf});
      ent.lookStale = ent.feedStale = ent.perfStale = false;
    },
    // A window opened or closed (ui.js, after each render): frames that are fed again catch up.
    sync() { for (const ent of this.entries) if (ent.lookStale || ent.feedStale || ent.perfStale) this.catchUp(ent); },
    // Page state the frames follow (ui.js calls these when it changes).
    fight(feed) {
      for (const ent of this.entries) {
        if (!ent.hello || !this.hasFight(ent)) continue;
        if (this.fed(ent)) this.post(ent, "fight", {feed}); else ent.feedStale = true;
      }
    },
    // The game's frame rate, ping and server tick rate, once a second: no permission needed.
    perf(p) {
      this.lastPerf = p;
      for (const ent of this.entries) {
        if (!ent.hello) continue;
        if (this.fed(ent)) this.post(ent, "perf", {perf: p}); else ent.perfStale = true;
      }
    },
    look(l) {
      for (const ent of this.entries) {
        if (!ent.hello) continue;
        if (this.fed(ent)) this.post(ent, "look", l); else ent.lookStale = true;
      }
    },
    edit(on) {
      for (const ent of this.entries) {
        if (ent.frame) ent.frame.style.pointerEvents = on ? "none" : "";
        if (ent.hello) this.post(ent, "edit", {on: !!on});
      }
    },
    // Reload a plugin's frames (developer mode, preview): position, open state and settings stay.
    changed(id) {
      for (const ent of this.entries) {
        if (ent.plugin.id !== id || !ent.frame) continue;
        ent.hello = false;
        ent.helloAt = 0;
        ent.seen = Date.now();
        ent.stalled = false;
        ent.status = {text: "", live: false};
        ent.badge = errorBadge(ent.badge, {type: "load"});
        this.showStatus(ent);
        this.showBadge(ent);
        const b = ent.w && ent.w.el.querySelector(".bar b");
        if (b) b.textContent = ent.title;
        ent.frame.src = ent.src;
      }
    },
  };

  const pure = {validManifestWindows, isEnabled, grantsOf, permissionLines, cardModel, migrateSaved, clampSize, accept,
    logGate, resizeGate, route, overCap, nameTaken, errorBadge, MAX_OPEN};
  Object.assign(host, pure);
  if (typeof window !== "undefined") window.pluginHost = host;
  if (typeof module !== "undefined" && module.exports) module.exports = Object.assign({host}, pure);
})();
