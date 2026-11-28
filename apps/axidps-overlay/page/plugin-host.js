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
  // Fight, Timeline and Healing were bundled plugins for a while; they are built in again.
  const MIGRATE = {"axidps.fight/fight": "fight", "axidps.timeline/timeline": "timeline",
    "axidps.healing/healing": "healing"};
  const ICONS = ["chart", "users", "shield", "swords", "activity", "x", "grid", "layers", "sliders", "plug",
    "download", "columns", "plus"];
  const PERMISSION_TEXT = {fight: "Reads your fight data", world: "Draws over the game world",
    position: "Reads your position and camera", squad: "Reads your squad and party",
    enemies: "Reads enemy players seen in combat",
    textures: "Changes how the game's icons look (commander tags, markers, targets)",
    outline: "Outlines your character"};
  const MAX_TEXTURE_RULES = 64;
  const MAX_OUTLINE = 1024;
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
        const alias = "p" + ++n;
        if (on) out.push({plugin: p, win, fullId, alias});
      }
    }
    return out;
  }

  // The hub's form for one plugin window: the window's declared settings, checked again (the
  // browser demo serves the list too, so it's untrusted). A bad entry is dropped, not fatal.
  const MAX_WIN_SETTINGS = 16, MAX_LABEL = 60, MAX_OPTION = 40, MAX_OPTIONS = 12;
  const finite = x => typeof x === "number" && Number.isFinite(x);
  function settingsModel(win) {
    const list = win && Array.isArray(win.settings) ? win.settings : [];
    const out = [], keys = new Set();
    for (const s of list) {
      if (out.length >= MAX_WIN_SETTINGS) break;
      if (!s || typeof s !== "object" || typeof s.key !== "string" || !WINDOW_ID.test(s.key) || keys.has(s.key)
        || typeof s.label !== "string" || !s.label || s.label.length > MAX_LABEL) continue;
      const f = {key: s.key, type: s.type, label: s.label};
      if (s.type === "toggle") {
        if (s.default !== undefined && typeof s.default !== "boolean") continue;
        f.default = s.default === true;
      } else if (s.type === "choice") {
        const o = s.options;
        if (!Array.isArray(o) || o.length < 1 || o.length > MAX_OPTIONS
          || !o.every(x => typeof x === "string" && x && x.length <= MAX_OPTION)) continue;
        if (s.default !== undefined && !o.includes(s.default)) continue;
        f.options = o.slice();
        f.default = s.default === undefined ? o[0] : s.default;
      } else if (s.type === "number") {
        if (!finite(s.min) || !finite(s.max) || s.min > s.max) continue;
        if (s.step !== undefined && !(finite(s.step) && s.step > 0)) continue;
        if (s.default !== undefined && !(finite(s.default) && s.default >= s.min && s.default <= s.max)) continue;
        Object.assign(f, {min: s.min, max: s.max, step: s.step === undefined ? 1 : s.step,
          default: s.default === undefined ? s.min : s.default});
      } else continue;
      keys.add(s.key);
      out.push(f);
    }
    return out;
  }

  // `value` made to fit `field`: numbers snap to the step grid from min and stay in range;
  // anything of the wrong type is refused.
  function fitSetting(field, value) {
    if (field.type === "toggle") return typeof value === "boolean" ? {ok: true, value} : {ok: false};
    if (field.type === "choice") return field.options.includes(value) ? {ok: true, value} : {ok: false};
    if (field.type !== "number" || !finite(value)) return {ok: false};
    const {min, max, step} = field;
    const v = Math.min(Math.max(value, min), max);
    let k = Math.round((v - min) / step);
    if (min + k * step > max + 1e-9) k -= 1;
    return {ok: true, value: Number((min + k * step).toFixed(10))};
  }

  // What the form shows: a stored value that fits its setting, otherwise the default. Keys the
  // manifest no longer declares stay in the store but aren't shown.
  function settingValues(model, cfg) {
    const out = {};
    for (const f of model) {
      const v = own(cfg, f.key) ? cfg[f.key] : undefined;
      const fits = f.type === "toggle" ? typeof v === "boolean"
        : f.type === "choice" ? f.options.includes(v)
        : finite(v) && v >= f.min && v <= f.max;
      out[f.key] = fits ? v : f.default;
    }
    return out;
  }

  // The host's own options for a plugin window (never seen by the frame).
  function windowOpts(saved, fullId) {
    const o = own(saved && saved.pwin, fullId) ? saved.pwin[fullId] : null;
    return {bar: o && typeof o.bar === "boolean" ? o.bar : true,
      clear: o && typeof o.clear === "boolean" ? o.clear : false};
  }

  // The hub's settings panel for plugin `p`: one section per valid window.
  function panelModel(p, saved) {
    const cfg = saved && saved.cfg;
    return validManifestWindows([p]).map(({win, fullId}) => {
      const model = settingsModel(win);
      const vals = settingValues(model, own(cfg, fullId) ? cfg[fullId] : null);
      return {fullId, title: String(win.title || win.id).slice(0, MAX_TEXT), opts: windowOpts(saved, fullId),
        fields: model.map(f => Object.assign({}, f, {value: vals[f.key]}))};
    });
  }

  // What the player grants a plugin by enabling it: its permissions and one "net:<host>" per
  // network entry.
  function grantsOf(p) {
    const perms = Array.isArray(p && p.permissions) ? p.permissions : [];
    const net = Array.isArray(p && p.network) ? p.network : [];
    return perms.filter(x => typeof x === "string").concat(net.filter(x => typeof x === "string").map(h => "net:" + h));
  }

  // Bundled plugins are on unless switched off, except those in OFF_BY_DEFAULT, which are on only
  // once switched on. A player's plugin is on only once enabled, and only while the manifest asks
  // for nothing beyond what was granted then.
  const OFF_BY_DEFAULT = ["axidps.outline"];
  function isEnabled(p, savedPlugins) {
    if (!p || typeof p !== "object" || p.error) return false;
    const s = own(savedPlugins, p.id) ? savedPlugins[p.id] : null;
    if (p.bundled === true) return OFF_BY_DEFAULT.includes(p.id) ? !!(s && s.enabled === true) : !(s && s.enabled === false);
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

  // A player plugin's badge, from the host's trust level: Verified, Permissions approved (a
  // standing approval covers it), Unverified (also when the host gives none), DEV (a developer
  // pass) or Revoked; bundled plugins have none.
  const TRUST_LABEL = {verified: "Verified", approved: "Permissions approved", unverified: "Unverified",
    dev: "DEV", revoked: "Revoked"};
  function trustLabel(p) {
    if (!p || p.bundled === true || p.trust === "bundled") return null;
    return own(TRUST_LABEL, p.trust) ? TRUST_LABEL[p.trust] : "Unverified";
  }

  // A plugin's hub card: what it is, what it asks for, and what the player can do with it.
  // action: "enable" | "disable" | null (an invalid plugin, one that needs verification or a
  // revoked one); reason: why it can't be enabled ("" if it can; "Revoked: <why>" when pulled);
  // label: trustLabel; reload: developer mode's Reload, for an enabled player plugin.
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
      label: trustLabel(p),
      reload: !err && on && !bundled && s.dev === true,
    };
  }

  // Whether plugin p's windows are framed: enabled, or (browser demo only) the plugin being
  // previewed.
  const framed = (p, saved, hosted) => isEnabled(p, saved && saved.plugins) || (!hosted && !!p && p.preview === true && !p.error);

  // A window drawn as a page in the hub's Options tab instead of as an overlay window.
  const isHub = win => !!win && win.in === "hub";

  // The Options tab's plugin pages: one per hub window of every listed plugin whose manifest
  // parsed (on or off, or failed to load), in list order. on: whether it's framed (isEnabled, or
  // previewed in the browser demo when opts.hosted is false); action, reason and lines as on the
  // plugin's card.
  function hubPages(list, saved, opts) {
    const hosted = !opts || opts.hosted !== false;
    const s = saved && typeof saved === "object" ? saved : {};
    const str = v => typeof v === "string" ? v.slice(0, MAX_TEXT) : "";
    const out = [], seen = new Set();
    for (const p of Array.isArray(list) ? list : []) {
      if (!p || typeof p !== "object" || typeof p.id !== "string" || !PLUGIN_ID.test(p.id) || !Array.isArray(p.windows)) continue;
      let card = null;
      for (const win of p.windows.slice(0, 4)) {
        if (!isHub(win) || typeof win.id !== "string" || !WINDOW_ID.test(win.id) || typeof win.entry !== "string") continue;
        const fullId = p.id + "/" + win.id;
        if (seen.has(fullId)) continue;
        seen.add(fullId);
        card = card || cardModel(p, s);
        const on = framed(p, s, hosted);
        out.push({fullId, id: p.id, title: str(win.title) || win.id, description: str(win.description),
          icon: ICONS.includes(win.icon) ? win.icon : "plug", on,
          action: card.action && (on ? "disable" : "enable"), reason: card.reason, lines: card.lines});
      }
    }
    return out;
  }

  const OFFLINE = "Can't reach the plugin store. Installed plugins still work.";
  const STORE_TEXT = {
    loading: "Loading the plugin store…",
    offline: "Can't reach the plugin store right now. This is the list from last time.",
    error: "The plugin store sent a list that didn't check out. This is the last good one.",
  };
  // Permissions that need verification (`fight` is the only basic one): marked in the Store.
  const DEEPER = ["world", "position", "squad", "enemies", "textures"];
  const STORE_FILTERS = ["all", "installed", "updates"];
  const EMPTY_FILTER = {installed: "Nothing installed from the store yet.", updates: "Everything is up to date."};
  // The same, while a search is on: the filter isn't empty, the search just has no hits in it.
  const EMPTY_SEARCH = {installed: "No installed plugins match.", updates: "No updates match."};

  // "a", "a and b", "a, b and c".
  function andList(xs) {
    return xs.length < 2 ? xs.join("") : xs.slice(0, -1).join(", ") + " and " + xs[xs.length - 1];
  }

  // What a Store plugin uses, as plain sentences: one per permission (deep: true for the ones that
  // need verification), then the network hosts it talks to.
  function usesOf(x) {
    const perms = Array.isArray(x && x.permissions) ? x.permissions : [];
    const net = Array.isArray(x && x.network) ? x.network.filter(h => typeof h === "string") : [];
    const out = perms.filter(k => own(PERMISSION_TEXT, k))
      .map(k => ({text: PERMISSION_TEXT[k] + ".", deep: DEEPER.includes(k)}));
    if (net.length) out.push({text: `Talks to ${andList(net.map(h => h.slice(0, MAX_TEXT)))}.`, deep: false});
    return out;
  }

  // The Store tab from the host's `axidps.store` payload: the rows matching `query` (name, author
  // or description, any case) and `filter` ("all" | "installed" | "updates"), and how many match
  // the query under each filter. `list` (the plugin list) marks installed plugins that were
  // revoked. Row action: "install" | "update" | "installed" (Uninstall) | "replace" (over the
  // player's own folder, after asking) | "revoked" (installed but turned off; Uninstall only) |
  // null (needs a newer axidps). trust: "verified" | "approved" (a standing approval) |
  // "unverified" | "revoked". uses: usesOf plus "You have <v>." for an update and the awaiting-
  // review sentence. note: the player's own copy, the revocation or the last error (bad: true for
  // those last two).
  function storeModel(store, query, list, filter) {
    const st = store && typeof store === "object" ? store : {};
    const state = typeof st.state === "string" ? st.state : "offline";
    const f = STORE_FILTERS.includes(filter) ? filter : "all";
    const all = Array.isArray(st.plugins) ? st.plugins.filter(x => x && typeof x === "object"
      && typeof x.id === "string" && PLUGIN_ID.test(x.id)) : [];
    const str = (v, n = MAX_TEXT) => typeof v === "string" ? v.slice(0, n) : "";
    const q = String(query || "").trim().toLowerCase();
    const hit = x => !q || [x.name, x.author, x.description].some(v => typeof v === "string" && v.toLowerCase().includes(q));
    const plugins = Array.isArray(list) ? list : [];
    const rows = all.filter(hit).map(x => {
      const err = str(x.error, 200);
      const mine = plugins.find(p => p && p.id === x.id);
      const revoked = mine && mine.trust === "revoked" ? String(mine.error || "Revoked") : "";
      // A revoked install is turned off: nothing to install or update, only Uninstall.
      const action = x.handInstalled ? "replace" : x.installed ? (revoked ? "revoked" : x.update ? "update" : "installed")
        : err === "needs a newer axidps" ? null : "install";
      const title = str(x.name) || x.id;
      const uses = usesOf(x);
      if (action === "update") uses.push({text: `You have ${str(x.installed)}.`, deep: false});
      if (x.awaiting) uses.push({text: "A newer version is waiting for review.", deep: false});
      const note = revoked || err
        || (x.handInstalled ? `You added a ${title} yourself. Replacing it gets you updates; your copy is kept.` : "");
      return {
        id: x.id, title, author: str(x.author), version: str(x.update || x.installed || x.version),
        have: str(x.installed), isUpdate: action === "update",
        description: str(x.description, 200), trust: revoked ? "revoked" : x.trust === "verified" || x.trust === "approved" ? x.trust : "unverified",
        uses, action, note, bad: !!(revoked || err), busy: x.busy === true,
      };
    });
    const inFilter = {
      all: () => true,
      installed: r => r.action === "installed" || r.action === "update" || r.action === "revoked",
      updates: r => r.action === "update",
    };
    const counts = {all: rows.length, installed: rows.filter(inFilter.installed).length,
      updates: rows.filter(inFilter.updates).length};
    const cards = rows.filter(inFilter[f]);
    let message;
    if (!all.length) message = state === "loading" ? STORE_TEXT.loading : state === "ok" ? "No plugins in the store yet." : OFFLINE;
    else {
      const parts = state !== "ok" && STORE_TEXT[state] ? [STORE_TEXT[state]] : [];
      if (!cards.length) {
        parts.push(q && !rows.length ? `No plugins match “${String(query).trim()}”.` : (q ? EMPTY_SEARCH : EMPTY_FILTER)[f] || "");
      }
      message = parts.filter(Boolean).join(" ");
    }
    return {state, filter: f, counts, message, cards};
  }

  // A canned `axidps.store` payload and plugin list: the browser demo's Store tab
  // (index.html?demo&hub&tab=store). Every kind of row: verified, unverified, an update, installed,
  // the player's own copy (Replace) and a revoked install.
  function sampleStore() {
    const l = (id, name, author, description, extra) => Object.assign({id, name, author, description, icon: "", repo: "",
      version: "1.0.0", trust: "unverified", awaiting: false, permissions: ["fight"], network: [], installed: null,
      handInstalled: false, update: null, busy: false}, extra);
    return {state: "ok", fetched: Date.now(), plugins: [
      l("jo.dps-log", "Fight Log", "jo", "Uploads your fights to a log site when they end.",
        {trust: "verified", network: ["dps.report"], installed: "1.0.0", update: "1.1.0", version: "1.1.0"}),
      l("mo.tag-colors", "Tag Colours", "mo", "Commander tags in colours you can tell apart.",
        {trust: "approved", permissions: ["textures"], installed: "2.0.1", version: "2.0.1"}),
      l("kat.boon-board", "Boon Board", "kat", "Your squad's boon uptime, one row per subgroup.",
        {trust: "verified"}),
      l("rin.squad-map", "Squad Map", "rin", "Where everyone in your squad is, on a small map.",
        {permissions: ["squad", "position"], awaiting: true}),
      l("ash.timer", "Raid Timer", "ash", "Counts down to the next enrage and mechanic.",
        {version: "0.3.0", handInstalled: true}),
      l("zed.loot", "Loot Counter", "zed", "Counts what drops in a session.",
        {installed: "0.9.0", version: "0.9.0"}),
    ]};
  }
  // The plugin list entries sampleStore's rows imply (Fight Log and Tag Colours installed, Loot
  // Counter installed and then revoked).
  function sampleStoreList() {
    return [{id: "jo.dps-log", name: "Fight Log", version: "1.0.0", author: "jo", api: 1, permissions: ["fight"],
      network: ["dps.report"], windows: [], bundled: false, trust: "verified"},
    {id: "mo.tag-colors", name: "Tag Colours", version: "2.0.1", author: "mo", api: 1, permissions: ["textures"],
      network: [], windows: [], bundled: false, trust: "verified"},
    {id: "zed.loot", name: "Loot Counter", version: "0.9.0", author: "zed", api: 1, permissions: ["fight"],
      network: [], windows: [], bundled: false, trust: "revoked", reason: "sent your account name to a third party",
      error: "Revoked: sent your account name to a third party"}];
  }

  // Move the saved position and open state of Fight, Timeline and Healing from their plugin ids
  // back to the built-in windows' ids (the plugin ids were the newer ones, so they win), and drop
  // the plugin keys so this happens once. Their plugin settings don't fit the built-in windows.
  function migrateSaved(saved) {
    if (!saved || typeof saved !== "object") return saved;
    for (const m of ["pos", "open", "cfg"]) {
      const o = saved[m];
      if (!o || typeof o !== "object") continue;
      for (const [full, id] of Object.entries(MIGRATE)) {
        if (!own(o, full)) continue;
        if (m !== "cfg") o[id] = o[full];
        delete o[full];
      }
    }
    // Character Outline's own On toggle became the plugin's switch: carry it over once (an
    // explicit switch-off on its card stays off), then drop it.
    const OK = "axidps.outline", cfg = saved.cfg && saved.cfg[OK + "/settings"];
    if (cfg && typeof cfg === "object" && own(cfg, "on")) {
      if (!saved.plugins || typeof saved.plugins !== "object") saved.plugins = {};
      const cur = own(saved.plugins, OK) ? saved.plugins[OK] : null;
      if (!(cur && cur.enabled === false)) saved.plugins[OK] = {enabled: cfg.on === true, granted: []};
      delete cfg.on;
    }
    return saved;
  }

  // A hub page's line under its header: not responding, developer mode's error count (tip: the
  // last messages), or the plugin's own status; null when there's nothing to say.
  function hubStatus(ent, dev) {
    if (ent.stalled) return {text: "Not responding", bad: true, tip: ""};
    const n = ent.badge && ent.badge.count;
    if (dev && n > 0) return {text: n === 1 ? "1 error" : n + " errors", bad: true, tip: ent.badge.last.join("\n")};
    const t = ent.status && typeof ent.status.text === "string" ? ent.status.text.slice(0, MAX_TEXT) : "";
    return t ? {text: t, bad: false, tip: ""} : null;
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
      case "textures": {
        if (!Array.isArray(m.rules) || m.rules.length > MAX_TEXTURE_RULES) return false;
        try {
          const j = JSON.stringify(m.rules);
          return typeof j === "string" && utf8Len(j) <= MAX_SETTINGS;
        } catch (e) {
          return false;
        }
      }
      case "outline": {
        if (m.settings === null) return true;
        if (!m.settings || typeof m.settings !== "object" || Array.isArray(m.settings)) return false;
        try {
          const j = JSON.stringify(m.settings);
          return typeof j === "string" && utf8Len(j) <= MAX_OUTLINE;
        } catch (e) {
          return false;
        }
      }
      default: return false;
    }
  }

  // Every plugin's texture rules as one list for the host: plugins in id order, so when two name
  // the same icon the result doesn't depend on load order (the later one wins on the host).
  function mergeTextures(byPlugin) {
    const out = [];
    for (const id of Object.keys(byPlugin).sort()) {
      for (const r of byPlugin[id]) {
        if (r && typeof r === "object" && !Array.isArray(r) && typeof r.target === "string") out.push(r);
      }
    }
    return out;
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
  //   look(), feed(), isOpen(w), hubPage(fullId) (the element in the hub's Options tab that holds a hub
  //   window's frame; made once per fullId and never rebuilt)}
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
      const on = p => framed(p, saved, env.hosted);
      this.want = validManifestWindows(this.list, on);
      for (const ent of this.entries.slice()) {
        const w = this.want.find(x => x.fullId === ent.fullId);
        if (!w || w.alias !== ent.alias || w.win.entry !== ent.win.entry || isHub(w.win) !== !!ent.hub) this.drop(ent);
        else { ent.plugin = w.plugin; ent.win = w.win; this.applyOpts(ent); }
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
    // The Store tab (ui.js shows `this.store`, the host's last `axidps.store` payload).
    store: null,
    // Ids the player confirmed an uninstall for: their saved enable and settings go once the host
    // says they're gone, and stay if it refuses.
    uninstalling: null,
    // The host's `axidps.store` payload. Each pending uninstall is settled by its card: busy (still
    // working) or unchanged without an error waits; gone (no card, or neither installed nor the
    // player's) forgets the plugin; an error while it's still there was a refusal, so it's kept.
    setStore(payload) {
      this.store = payload && typeof payload === "object" ? payload : null;
      const env = this.env, pending = this.uninstalling;
      if (!env || !pending || !pending.size || !this.store || !Array.isArray(this.store.plugins)) return;
      let forgot = false;
      for (const id of [...pending]) {
        const c = this.store.plugins.find(x => x && x.id === id);
        if (c && c.busy === true) continue;
        if (!c || (!c.installed && !c.handInstalled)) {
          pending.delete(id);
          const saved = env.saved();
          if (saved.plugins && own(saved.plugins, id)) delete saved.plugins[id];
          for (const k of Object.keys(saved.cfg || {})) if (k.startsWith(id + "/")) delete saved.cfg[k];
          forgot = true;
        } else if (c.error) pending.delete(id);
      }
      if (forgot) env.saveNow();
    },
    storeOpen() { if (this.env) this.env.toHost(JSON.stringify({store: "open"})); },
    storeRefresh() { if (this.env) this.env.toHost(JSON.stringify({store: "refresh"})); },
    // A Store card's button. Replace and Uninstall ask first (env.confirm(text, ok label, onYes));
    // the host refuses anything else that would touch a folder the player added.
    storeAct(id, act) {
      const env = this.env;
      const m = storeModel(this.store, "", this.list).cards.find(c => c.id === id);
      if (!env || !m || m.busy) return;
      const go = what => env.toHost(JSON.stringify({store: {[what]: id}}));
      if (act === "install" && m.action === "install") go("install");
      else if (act === "update" && m.action === "update") go("update");
      else if (act === "replace" && m.action === "replace") {
        env.confirm(`Replace the plugin you added yourself with the store version of ${m.title}? ` +
          "Your copy is kept in the store's replaced folder.", "Replace", () => go("replace"));
      } else if (act === "uninstall" && ["installed", "update", "revoked"].includes(m.action)) {
        env.confirm(`Uninstall ${m.title}? Its windows and settings go with it.`, "Uninstall", () => {
          if (!this.uninstalling) this.uninstalling = new Set();
          this.uninstalling.add(id);
          go("uninstall");
        });
      }
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
      const hub = isHub(w.win);
      const taken = !hub && nameTaken(env.wins, w);
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
      if (hub) {
        // A hub page: its frame lives in a holder in the hub's Options tab (env.hubPage), which
        // is never rebuilt, so it keeps running while the hub is closed. No window, no bar.
        ent.hub = true;
        // No height yet (CSS fills the page), so the plugin's first resize always applies.
        ent.size = [0, 0];
        this.mount(ent, {body: env.hubPage(w.fullId)});
        this.entries.push(ent);
        this.debug();
        return;
      }
      ent.w = env.defineWindow({
        id: w.alias, key: w.fullId, title, plugin: true, open: w.win.open === true,
        blurb: [title, String(w.win.description || "").slice(0, MAX_TEXT), glyph],
        at: this.at(ent),
        mount: win => this.mount(ent, win),
      });
      this.applyOpts(ent);
      const label = trustLabel(w.plugin);
      if (label) {
        const b = ent.w.el.querySelector(".bar b");
        if (b) b.dataset.tip = `${String(w.plugin.name || w.plugin.id).slice(0, MAX_TEXT)} · ${label}`;
        // A developer pass: say so on the window itself, not only in the hub.
        const bar = ent.w.bar || ent.w.el.querySelector(".bar");
        if (label === "DEV" && bar && typeof bar.querySelector === "function" && !bar.querySelector(".badge.dev")) {
          const d = document.createElement("span");
          d.className = "badge dev";
          d.textContent = "DEV";
          d.dataset.tip = "loaded with a developer pass";
          if (b && b.nextSibling) bar.insertBefore(d, b.nextSibling); else bar.appendChild(d);
        }
      }
      this.entries.push(ent);
      this.debug();
    },
    // The window's stored settings, with each declared setting's default under them.
    settingsOf(ent) {
      const cfg = this.env.saved().cfg;
      const mine = own(cfg, ent.fullId) && cfg[ent.fullId] && typeof cfg[ent.fullId] === "object" ? cfg[ent.fullId] : {};
      return Object.assign(settingValues(settingsModel(ent.win), {}), mine);
    },
    // A change from the hub's settings panel: merged into the window's store, saved, and the
    // whole store sent to the frame. False, with nothing changed, if it doesn't fit.
    setSetting(fullId, key, value) {
      const ent = this.entries.find(e => e.fullId === fullId);
      const field = ent && settingsModel(ent.win).find(f => f.key === key);
      const fit = field ? fitSetting(field, value) : {ok: false};
      if (!fit.ok) return false;
      const saved = this.env.saved();
      if (!saved.cfg || typeof saved.cfg !== "object") saved.cfg = {};
      const cur = own(saved.cfg, fullId) ? saved.cfg[fullId] : null;
      saved.cfg[fullId] = Object.assign({}, cur && typeof cur === "object" && !Array.isArray(cur) ? cur : {},
        {[key]: fit.value});
      this.env.save();
      if (ent.hello) this.post(ent, "settings", {value: this.settingsOf(ent)});
      return true;
    },
    // The host's own window options (title bar, transparent background), kept in saved.pwin,
    // which no frame ever sees.
    setWindowOpt(fullId, name, on) {
      const ent = this.entries.find(e => e.fullId === fullId);
      if (!ent || (name !== "bar" && name !== "clear") || typeof on !== "boolean") return false;
      const saved = this.env.saved();
      if (!saved.pwin || typeof saved.pwin !== "object") saved.pwin = {};
      saved.pwin[fullId] = Object.assign(windowOpts(saved, fullId), {[name]: on});
      this.env.save();
      this.applyOpts(ent);
      this.env.scheduleReport();
      return true;
    },
    applyOpts(ent) {
      const o = windowOpts(this.env.saved(), ent.fullId);
      if (!ent.w) return;
      const cl = ent.w.el && ent.w.el.classList;
      if (cl) { cl.toggle("nobar", !o.bar); cl.toggle("clear", o.clear); }
      ent.w.clear = o.clear;
    },
    mount(ent, win) {
      const f = document.createElement("iframe");
      f.setAttribute("sandbox", "allow-scripts");
      f.className = "plugin";
      // A hub page's frame is as wide as the page and fills it until the plugin asks for a height.
      f.style.width = ent.hub ? "100%" : ent.size[0] + "px";
      f.style.height = ent.hub ? "" : ent.size[1] + "px";
      f.style.pointerEvents = this.env.ui.editing ? "none" : "";
      f.src = ent.src;
      win.body.appendChild(f);
      ent.frame = f;
      ent.cw = f.contentWindow;
      if (ent.cw) this.sources.set(ent.cw, ent);
    },
    drop(ent) {
      if (ent.cw) this.sources.delete(ent.cw);
      // The plugin's last window is gone (disabled or removed): its rules go too.
      if (this.textures && !this.entries.some(e => e !== ent && e.plugin.id === ent.plugin.id)) {
        this.setTextures(ent.plugin.id, []);
      }
      if (this.outlineOwner === ent.plugin.id && !this.entries.some(e => e !== ent && e.plugin.id === ent.plugin.id)) {
        this.setOutline(ent.plugin.id, null);
      }
      if (ent.w) this.env.removeWindow(ent.w);
      if (ent.hub && ent.frame && ent.frame.parentNode) ent.frame.parentNode.removeChild(ent.frame);
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
    // A window in trouble (not responding, or developer mode's error count) shows its title bar
    // even when the player hid it, so the warning isn't lost.
    showTrouble(ent) {
      if (ent.hub) {
        // A hub page shows this in its header (ui.js, hubStatusOf): redraw when it changes.
        const key = JSON.stringify(hubStatus(ent, this.env.saved().dev === true));
        if (key !== ent.hubShown) { ent.hubShown = key; this.env.renderAll(); }
        return;
      }
      const cl = ent.w && ent.w.el && ent.w.el.classList;
      if (cl) cl.toggle("trouble", ent.stalled || (this.env.saved().dev === true && ent.badge.count > 0));
    },
    hubStatusOf(fullId) {
      const ent = this.entries.find(e => e.hub && e.fullId === fullId);
      return ent && this.env ? hubStatus(ent, this.env.saved().dev === true) : null;
    },
    showStatus(ent) {
      this.showTrouble(ent);
      const st = ent.w && ent.w.state;
      if (!st) return;
      const live = !ent.stalled && ent.status.live;
      st.className = "state" + (live ? " live" : "");
      st.textContent = ent.stalled ? "not responding" : ent.status.text;
      this.debug();
    },
    // Developer mode's error count in the title bar, in --bad, the last messages in its tooltip.
    showBadge(ent) {
      this.showTrouble(ent);
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
          this.post(ent, "ready", {plugin: ent.plugin.id, window: ent.win.id, api: API,
            permissions: (ent.plugin.permissions || []).slice()});
          this.post(ent, "settings", {value: this.settingsOf(ent)});
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
            if (ent.frame && ent.hub) ent.frame.style.height = s[1] + "px";
            else if (ent.frame) { ent.frame.style.width = s[0] + "px"; ent.frame.style.height = s[1] + "px"; }
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
        case "textures": {
          if (!this.hasTextures(ent)) {
            env.toHost(JSON.stringify({log: {plugin: ent.plugin.id, level: "warn",
              msg: "textures.set without the textures permission; ignored"}}));
            return;
          }
          this.setTextures(ent.plugin.id, m.rules);
          break;
        }
        case "outline": {
          if (!this.hasOutline(ent)) {
            env.toHost(JSON.stringify({log: {plugin: ent.plugin.id, level: "warn",
              msg: "outline.set is only for built-in plugins; ignored"}}));
            return;
          }
          this.setOutline(ent.plugin.id, m.settings);
          break;
        }
      }
    },
    hasTextures(ent) {
      return Array.isArray(ent.plugin.permissions) && ent.plugin.permissions.includes("textures");
    },
    hasOutline(ent) {
      return ent.plugin.bundled === true && Array.isArray(ent.plugin.permissions)
        && ent.plugin.permissions.includes("outline");
    },
    // The outline plugin's settings changed (null turns the outline off): send them if they differ.
    setOutline(id, settings) {
      this.outlineOwner = id;
      const msg = JSON.stringify({outline: settings});
      if (msg === this.outlineSent) return;
      this.outlineSent = msg;
      if (this.env) this.env.toHost(msg);
    },
    // One plugin's rules changed (an empty list removes them): send the merged list if it differs.
    setTextures(id, rules) {
      if (!this.textures) this.textures = {};
      if (Array.isArray(rules) && rules.length) this.textures[id] = rules;
      else delete this.textures[id];
      const msg = JSON.stringify({textures: mergeTextures(this.textures)});
      if (msg === this.texturesSent) return;
      this.texturesSent = msg;
      if (this.env) this.env.toHost(msg);
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
        // The reloaded page sets its rules again.
        if (this.textures) this.setTextures(id, []);
        if (this.outlineOwner === id) this.setOutline(id, null);
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

  const pure = {validManifestWindows, isEnabled, grantsOf, permissionLines, cardModel, trustLabel, storeModel, usesOf, sampleStore, sampleStoreList, migrateSaved, clampSize, accept,
    logGate, resizeGate, route, mergeTextures, overCap, nameTaken, errorBadge, settingsModel, settingValues, windowOpts,
    panelModel, fitSetting, MAX_OPEN, isHub, hubPages, hubStatus, OFF_BY_DEFAULT};
  Object.assign(host, pure);
  if (typeof window !== "undefined") window.pluginHost = host;
  if (typeof module !== "undefined" && module.exports) module.exports = Object.assign({host}, pure);
})();
