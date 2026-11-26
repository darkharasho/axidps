// User-defined overlay windows ("custom windows"): the stat catalog, spec checks, migration from
// the old fixed tables, and copying. Plain browser JS; a global in the page, module.exports in Node.
const axidpsCustom = (() => {
  // Every stat a custom window can show. Labels, tips and widths (px) match COLS in ui.js.
  // count: whole-number display. heal: the game reports heals only for you.
  // rate: the base keys a per-second stat is computed from.
  const STATS = {
    damage: {label: "Dmg", tip: "damage dealt", w: 62},
    dps: {label: "DPS", tip: "damage per second", w: 52, rate: ["damage"]},
    power: {label: "Power", tip: "strike damage dealt", w: 58},
    condi: {label: "Condi", tip: "condition damage dealt", w: 52},
    down_contrib: {label: "DnC", tip: "down contribution: damage that led to enemy downs", w: 52},
    downs: {label: "Dn", tip: "enemies downed", w: 30, count: true},
    kills: {label: "K", tip: "enemies killed", w: 30, count: true},
    deaths: {label: "D", tip: "deaths", w: 26, count: true},
    downed: {label: "Dn'd", tip: "times downed", w: 40, count: true},
    taken: {label: "Taken", tip: "damage taken", w: 52},
    cleanses: {label: "Cleanse", tip: "condition stacks cleansed from allies", w: 62, count: true},
    strips: {label: "Strip", tip: "boon stacks stripped from enemies", w: 52, count: true},
    healing: {label: "Heal", tip: "healing given (others: only what they gave you)", w: 56, heal: true},
    barrier: {label: "Barr", tip: "barrier given (others: only what they gave you)", w: 52, heal: true},
    hps: {label: "HPS", tip: "healing and barrier given per second (others: only to you)", w: 50, heal: true, rate: ["healing", "barrier"]},
    healing_in: {label: "Heal in", tip: "healing received (others: only what you gave them)", w: 58, heal: true},
    barrier_in: {label: "Barr in", tip: "barrier received (others: only what you gave them)", w: 58, heal: true},
  };
  const BASE_KEYS = Object.keys(STATS).filter(k => !STATS[k].rate);
  const MAX_CUSTOM = 8, MAX_STATS = 8, NAME_MAX = 24, MAX_LIMIT = 50, MAX_FILTERS = 9;
  const SPANS = ["fight", "last:10", "last:30", "last:60", "session"];
  const LOOKS = ["table", "bars", "number", "chart"];
  const FILTER_RE = /^(spec:[A-Za-z ]{1,24}|prof:[1-9])$/;
  const AS = ["total", "per_s", "share"];
  const BASES = ["squad", "group", "me"];
  const ROWS = ["players", "total"];
  const ID_RE = /^c[1-8]$/;

  const isObj = v => v !== null && typeof v === "object" && !Array.isArray(v);
  const idNum = id => String(id).replace(/\D/g, "") || "1";

  function normStats(raw) {
    const out = [];
    for (const s of Array.isArray(raw) ? raw : []) {
      if (out.length >= MAX_STATS) break;
      if (!isObj(s) || !Object.prototype.hasOwnProperty.call(STATS, s.key)) continue;
      const as = STATS[s.key].rate ? "per_s" : (AS.includes(s.as) ? s.as : "total");
      out.push({key: s.key, as});
    }
    return out.length ? out : [{key: "damage", as: "total"}];
  }

  function normalizeSpec(raw, id) {
    if (!isObj(raw)) return null;
    let name = typeof raw.name === "string" ? raw.name.replace(/[\u0000-\u001f\u007f-\u009f]/g, "").trim().slice(0, NAME_MAX).trim() : "";
    if (!name) name = `Window ${idNum(id)}`;
    const stats = normStats(raw.stats);
    const who = isObj(raw.who) ? raw.who : {};
    const filters = (Array.isArray(who.filters) ? who.filters : [])
      .filter(f => typeof f === "string" && FILTER_RE.test(f)).slice(0, MAX_FILTERS);
    const sort = isObj(raw.sort) ? raw.sort : {};
    const sortKey = sort.key === "name" || stats.some(s => s.key === sort.key) ? sort.key : stats[0].key;
    const lim = Math.floor(Number(raw.limit));
    return {
      id, name, rows: ROWS.includes(raw.rows) ? raw.rows : "players", stats,
      who: {base: BASES.includes(who.base) ? who.base : "squad", filters},
      span: SPANS.includes(raw.span) ? raw.span : "fight",
      look: LOOKS.includes(raw.look) ? raw.look : "table",
      sort: {key: sortKey, asc: sort.asc === true},
      limit: lim >= 1 ? Math.min(lim, MAX_LIMIT) : null,
    };
  }

  function normalizeAll(list) {
    if (!Array.isArray(list)) return [];
    const seen = new Set(), out = [];
    for (const raw of list) {
      if (out.length >= MAX_CUSTOM) break;
      const id = isObj(raw) ? raw.id : null;
      if (typeof id !== "string" || !ID_RE.test(id) || seen.has(id)) continue;
      const spec = normalizeSpec(raw, id);
      if (!spec) continue;
      seen.add(id);
      out.push(spec);
    }
    return out;
  }

  // Turns the old fixed tables into custom windows, once (saved.custom absent). Mutates saved;
  // leaves tables/cfg alone. Returns whether it changed anything.
  function migrateTables(saved) {
    if (!isObj(saved) || saved.custom !== undefined) return false;
    const custom = [];
    for (const t of Array.isArray(saved.tables) ? saved.tables : []) {
      if (!isObj(t) || typeof t.id !== "string" || !/^t\d+$/.test(t.id)) continue;
      const cfg = isObj(saved.cfg) && isObj(saved.cfg[t.id]) ? saved.cfg[t.id] : {};
      const id = "c" + t.id.slice(1);
      let keys = (Array.isArray(cfg.cols) ? cfg.cols : []).filter(k => Object.prototype.hasOwnProperty.call(STATS, k));
      if (!keys.length) keys = ["damage", "dps"];
      const spec = normalizeSpec({
        name: t.title,
        stats: keys.map(key => ({key})),
        who: {base: cfg.groupOnly ? "group" : "squad"},
        sort: {key: cfg.sortKey, asc: cfg.sortAsc},
      }, id);
      if (!ID_RE.test(id) || custom.length >= MAX_CUSTOM || custom.some(c => c.id === id)) continue;
      custom.push(spec);
      if (isObj(saved.pos) && saved.pos[t.id] !== undefined) saved.pos[id] = saved.pos[t.id];
      if (isObj(saved.open) && saved.open[t.id] !== undefined) saved.open[id] = saved.open[t.id];
    }
    saved.custom = custom;
    return true;
  }

  // A custom copy of a built-in table: {title, cols, cfg}; cfg.cols (the player's choice) wins.
  function specFromBuiltin({title, cols, cfg}, id) {
    cfg = cfg || {};
    const keys = Array.isArray(cfg.cols) ? cfg.cols : (cols || []);
    return normalizeSpec({
      name: `Copy of ${title}`,
      stats: keys.map(key => ({key})),
      who: {base: cfg.groupOnly ? "group" : "squad"},
      sort: {key: cfg.sortKey, asc: cfg.sortAsc},
    }, id);
  }

  // Who a window covers: the base (squad = everyone, group = my group, me = just me), then any-of filters.
  function filterWho(players, who) {
    const list = Array.isArray(players) ? players : [];
    const base = who && who.base;
    const me = list.find(p => p.me);
    let out = list;
    if (base === "me") out = me ? [me] : [];
    else if (base === "group" && me) out = list.filter(p => p.group === me.group);
    const filters = (who && Array.isArray(who.filters)) ? who.filters : [];
    if (!filters.length) return out;
    return out.filter(p => filters.some(f => {
      const i = f.indexOf(":"), kind = f.slice(0, i), val = f.slice(i + 1);
      return kind === "spec" ? p.spec === val : kind === "prof" ? p.prof === Number(val) : false;
    }));
  }

  // A player's value for a catalog key. Totals for base keys; rates are per second.
  // live (fight span): dps/hps come from the feed. Otherwise rate keys summed over seconds.
  function statValue(p, key, seconds, live) {
    const st = STATS[key];
    if (!st) return 0;
    if (!st.rate) return Number(p[key]) || 0;
    if (live) return Number(p[key]) || 0;
    if (!(seconds > 0)) return 0;
    return st.rate.reduce((a, k) => a + (Number(p[k]) || 0), 0) / seconds;
  }

  function totalPlayer(list) {
    const t = {name: "Total", account: "", prof: 0, heal_full: list.some(p => p.heal_full)};
    for (const k of Object.keys(STATS)) t[k] = list.reduce((a, p) => a + (Number(p[k]) || 0), 0);
    return t;
  }

  // Rows and footer for a spec over a view ({players, seconds, live}). limit null -> maxRows.
  function computeRows(spec, view, fmt, maxRows) {
    const seconds = view.seconds, live = !!view.live;
    const matched = filterWho(view.players, spec.who);
    const raw = (p, key) => statValue(p, key, seconds, live);
    const sums = spec.stats.map(s => matched.reduce((a, p) => a + raw(p, s.key), 0));
    const valueOf = (p, s, i) => {
      const v = raw(p, s.key);
      if (STATS[s.key].rate || s.as === "total") return v;
      if (s.as === "per_s") return seconds > 0 ? v / seconds : 0;
      return sums[i] > 0 ? v / sums[i] * 100 : null;
    };
    const build = p => ({p, values: spec.stats.map((s, i) => valueOf(p, s, i))});
    let rows;
    if (spec.rows === "total") {
      const t = totalPlayer(matched);
      if (live) {
        t.dps = matched.reduce((a, p) => a + (Number(p.dps) || 0), 0);
        t.hps = matched.reduce((a, p) => a + (Number(p.hps) || 0), 0);
      }
      rows = [build(t)];
    } else {
      rows = matched.map(build);
      const idx = spec.stats.findIndex(s => s.key === spec.sort.key);
      const dir = spec.sort.asc ? 1 : -1;
      if (spec.sort.key === "name") rows.sort((a, b) => dir * String(a.p.name || "").localeCompare(String(b.p.name || "")));
      else if (idx >= 0) rows.sort((a, b) => dir * ((a.values[idx] || 0) - (b.values[idx] || 0)));
      const lim = spec.limit || maxRows;
      if (lim >= 1) rows = rows.slice(0, lim);
    }
    const parts = [`${matched.length} players`];
    let shown = 0;
    spec.stats.forEach((s, i) => {
      if (shown >= 2 || STATS[s.key].count) return;
      shown++;
      parts.push(`${fmt(sums[i])} ${STATS[s.key].label.toLowerCase()}`);
    });
    return {rows, footer: parts.join(" \u00b7 ")};
  }

  // History of the live feed for spans other than "fight": a 60 s ring of snapshots (last:N), the
  // session (summed finished fights) and 1 s chart series. The clock is fight.duration_ms, never
  // wall time. Players are keyed by account || name; only identity fields and BASE_KEYS are kept.
  const IDENT = ["name", "account", "prof", "elite", "spec", "group", "me", "heal_full"];
  const RING_MS = 60000, SERIES_CAP = 3600, NK = BASE_KEYS.length;
  const pkey = p => String(p.account || p.name || "");

  function createHistory() {
    let ring = [];              // [{ms, map: Map<key, {id, v: Float64Array}>}], oldest first
    let cur = null;             // newest snapshot of the current fight
    let curAdded = true;        // current fight already in the session (or none)
    let base = null;            // session baseline inside the current fight (after clearSession)
    let session = new Map(), sessionMs = 0;
    let prevState = null, prevMs = 0;
    let tracked = new Set();
    let cache = new Map();      // view per span until the next push or clear; callers must not mutate it
    // Series: one sample per whole fight second; sample.vals: Map<key, Float64Array by slot> (NaN = absent).
    let samples = [], slots = new Map(), slotId = [];

    const snapOf = (f) => {
      const map = new Map();
      for (const p of Array.isArray(f.players) ? f.players : []) {
        if (!p) continue;
        const k = pkey(p);
        if (!k) continue;
        const id = {};
        for (const n of IDENT) id[n] = p[n];
        const v = new Float64Array(NK);
        for (let i = 0; i < NK; i++) v[i] = Number(p[BASE_KEYS[i]]) || 0;
        map.set(k, {id, v});
      }
      return {ms: Number(f.fight.duration_ms) || 0, map};
    };

    function addToSession() {
      if (!cur || curAdded) return;
      for (const [k, e] of cur.map) {
        const b = base && base.map.get(k);
        let s = session.get(k);
        if (!s) session.set(k, s = {id: e.id, v: new Float64Array(NK)});
        s.id = e.id;
        for (let i = 0; i < NK; i++) s.v[i] += Math.max(0, e.v[i] - (b ? b.v[i] : 0));
      }
      sessionMs += Math.max(0, cur.ms - (base ? base.ms : 0));
      curAdded = true;
    }

    function clearSeries() { samples = []; slots = new Map(); slotId = []; }

    function sample(snap) {
      const sec = Math.floor(snap.ms / 1000);
      const last = samples[samples.length - 1];
      if (last && sec <= last.s) return;
      const vals = new Map();
      if (tracked.size) {
        for (const [k, e] of snap.map) {
          let j = slots.get(k);
          if (j === undefined) { j = slotId.length; slots.set(k, j); slotId.push(e.id); }
          else slotId[j] = e.id;
        }
        for (const key of tracked) {
          const ki = BASE_KEYS.indexOf(key);
          const a = new Float64Array(slotId.length).fill(NaN);
          for (const [k, e] of snap.map) a[slots.get(k)] = e.v[ki];
          vals.set(key, a);
        }
      }
      // Seconds the feed skipped repeat the previous sample, so values stay one per second.
      if (last) for (let s = Math.max(last.s + 1, sec - SERIES_CAP); s < sec; s++) samples.push({s, vals: last.vals});
      samples.push({s: sec, vals});
      if (samples.length > SERIES_CAP + 1) samples.splice(0, samples.length - SERIES_CAP - 1);
    }

    function push(f) {
      cache = new Map();
      const state = f ? f.state : null;
      const fight = !!(f && f.fight);
      if (!fight) {
        if (prevState === "fight") addToSession();
        prevState = state;
        return;
      }
      const snap = snapOf(f);
      if (state === "fight" && (prevState !== "fight" || snap.ms < prevMs)) {
        addToSession();
        ring = []; cur = null; base = null; curAdded = false; clearSeries();
      }
      const newest = ring[ring.length - 1];
      if (newest && newest.ms === snap.ms) ring[ring.length - 1] = snap;
      else if (!newest || snap.ms > newest.ms) ring.push(snap);
      let drop = -1;
      for (let i = 0; i < ring.length && ring[i].ms <= snap.ms - RING_MS; i++) drop = i;
      if (drop > 0) ring.splice(0, drop);
      if (state === "fight" || prevState === "fight") cur = snap;
      if (state === "fight") sample(snap);
      else if (prevState === "fight") addToSession();
      prevState = state; prevMs = snap.ms;
    }

    const player = (id, v) => {
      const p = Object.assign({}, id);
      for (let i = 0; i < NK; i++) p[BASE_KEYS[i]] = Math.max(0, v[i]);
      return p;
    };
    const withRates = (players, seconds) => {
      for (const p of players) {
        p.dps = seconds > 0 ? p.damage / seconds : 0;
        p.hps = seconds > 0 ? (p.healing + p.barrier) / seconds : 0;
      }
      return {players, seconds};
    };

    function viewLast(n) {
      const now = ring[ring.length - 1];
      if (!now) return {players: [], seconds: 0};
      const target = now.ms - n * 1000;
      let refIdx = -1;
      for (let i = ring.length - 1; i >= 0; i--) if (ring[i].ms <= target) { refIdx = i; break; }
      const ref = refIdx >= 0 ? ring[refIdx] : null;
      const out = [], d = new Float64Array(NK);
      for (const [k, e] of now.map) {
        let r = ref && ref.map.get(k);
        // Not at the reference (joined or came back): the first snapshot inside the span.
        if (!r) for (let i = refIdx + 1; i < ring.length; i++) { r = ring[i].map.get(k); if (r) break; }
        for (let i = 0; i < NK; i++) d[i] = e.v[i] - r.v[i];
        out.push(player(e.id, d));
      }
      return withRates(out, Math.min(n, now.ms / 1000));
    }

    function viewSession() {
      const live = cur && !curAdded;
      const acc = new Map();
      for (const [k, s] of session) acc.set(k, {id: s.id, v: Float64Array.from(s.v)});
      if (live) for (const [k, e] of cur.map) {
        const b = base && base.map.get(k);
        let a = acc.get(k);
        if (!a) acc.set(k, a = {id: e.id, v: new Float64Array(NK)});
        a.id = e.id;
        for (let i = 0; i < NK; i++) a.v[i] += Math.max(0, e.v[i] - (b ? b.v[i] : 0));
      }
      const ms = sessionMs + (live ? Math.max(0, cur.ms - (base ? base.ms : 0)) : 0);
      return withRates([...acc.values()].map(a => player(a.id, a.v)), ms / 1000);
    }

    function view(span) {
      if (cache.has(span)) return cache.get(span);
      const m = /^last:(\d+)$/.exec(String(span));
      const v = span === "session" ? viewSession() : m ? viewLast(Number(m[1])) : null;
      cache.set(span, v);
      return v;
    }

    function track(keys) {
      const next = new Set([...(keys || [])].filter(k => BASE_KEYS.includes(k)));
      for (const k of tracked) if (!next.has(k)) for (const s of samples) s.vals.delete(k);
      tracked = next;
    }

    function series(keys, whoFn) {
      const players = slotId.map(id => Object.assign({}, id));
      const kept = new Set(typeof whoFn === "function" ? whoFn(players) : players);
      const idx = [];
      players.forEach((p, j) => { if (kept.has(p)) idx.push(j); });
      const n = Math.max(0, samples.length - 1);
      const values = (keys || []).map(key => {
        const out = new Array(n).fill(0);
        for (let i = 0; i < n; i++) {
          const a = samples[i].vals.get(key), b = samples[i + 1].vals.get(key);
          if (!a || !b) continue;
          let sum = 0;
          for (const j of idx) {
            const d = b[j] - a[j];
            if (d > 0) sum += d;
          }
          out[i] = sum;
        }
        return out;
      });
      return {t0: samples.length ? samples[0].s * 1000 : 0, step: 1000, values};
    }

    function bytes() {
      let n = 0;
      for (const s of ring) n += s.map.size * NK;
      n += session.size * NK;
      const seen = new Set();
      for (const s of samples) for (const a of s.vals.values()) if (!seen.has(a)) { seen.add(a); n += a.length; }
      return n * 8;
    }

    return {
      push, view, track, series, bytes,
      clear() { ring = []; clearSeries(); cache = new Map(); },
      clearSession() {
        cache = new Map();
        session = new Map(); sessionMs = 0;
        base = cur && !curAdded ? cur : null;
      },
    };
  }

  const blankSpec = id => normalizeSpec({}, id);

  return {STATS, BASE_KEYS, MAX_CUSTOM, MAX_STATS, NAME_MAX, SPANS, LOOKS,
    filterWho, statValue, computeRows, normalizeSpec, normalizeAll, migrateTables, specFromBuiltin, blankSpec, createHistory};
})();
if (typeof module !== "undefined" && module.exports) module.exports = axidpsCustom;
