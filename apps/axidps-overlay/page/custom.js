// User-defined overlay windows ("custom windows"): the stat catalog, spec checks, migration from
// the old fixed tables, and copying. Plain browser JS; a global in the page, module.exports in Node.
const axidpsCustom = (() => {
  // Every stat a custom window can show: labels, tips and widths (px). The built-in tables read them too (ui.js).
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
  // The fight stats' summed keys (charts and the old tables use these).
  const FIGHT_KEYS = Object.keys(STATS).filter(k => !STATS[k].rate);

  // Boons: the feed's p.boons = {"<id>": [up, self, group, squad]}, ms cumulative over the fight
  // (stack-ms for intensity boons). Group and squad come per target already, averaged by the core
  // over this fight's other squad players as axilog does. Order as the core's SQUAD_BOONS.
  const SQUAD_BOONS = [740, 725, 1187, 30328, 717, 718, 726, 743, 1122, 719, 26980, 873, 5974];
  const INTENSITY = [740, 1122];
  const BOON_NAMES = {740: "Might", 725: "Fury", 1187: "Quickness", 30328: "Alacrity", 717: "Protection",
    718: "Regeneration", 726: "Vigor", 743: "Aegis", 1122: "Stability", 719: "Swiftness", 26980: "Resistance",
    873: "Resolution", 5974: "Superspeed"};
  const MEASURES = ["up", "self", "group", "squad"];
  const MEASURE_TIP = {up: "uptime", self: "generation on themself", group: "generation on their subgroup, per player",
    squad: "generation on the squad, per player"};
  // Boon stats: kind "boon", one per boon and measure. noAs: always "total" (no per second, no share).
  for (const b of SQUAD_BOONS) for (const m of MEASURES) {
    const intensity = INTENSITY.includes(b);
    STATS[`boon:${b}:${m}`] = {kind: "boon", boon: b, measure: m, intensity, noAs: true,
      label: BOON_NAMES[b] + (m === "up" ? "" : ` (${m})`),
      tip: `${BOON_NAMES[b]} ${MEASURE_TIP[m]}${intensity ? ", in stacks" : ""}`, w: 58};
  }
  const isBoon = k => !!STATS[k] && STATS[k].kind === "boon";
  // Summed keys, fight stats first: the history keeps and differences every one of them.
  const BASE_KEYS = Object.keys(STATS).filter(k => !STATS[k].rate);
  const MAX_CUSTOM = 8, MAX_STATS = 8, NAME_MAX = 24, MAX_LIMIT = 50, MAX_FILTERS = 9;
  const SPANS = ["fight", "last:10", "last:30", "last:60", "session"];
  const LOOKS = ["table", "bars", "number", "chart"];
  const FILTER_RE = /^(spec:[A-Za-z ]{1,24}|prof:[1-9])$/;
  const AS = ["total", "per_s", "share"];
  const BASES = ["squad", "group", "me"];
  const ROWS = ["players", "total", "boons"];
  // A boons window with no boon stat left.
  const BOON_DEFAULT = "boon:1187:up";
  const ID_RE = /^c[1-8]$/;

  const isObj = v => v !== null && typeof v === "object" && !Array.isArray(v);
  const idNum = id => String(id).replace(/\D/g, "") || "1";

  // boons: a boons window keeps only boon stats.
  function normStats(raw, boons) {
    const out = [];
    for (const s of Array.isArray(raw) ? raw : []) {
      if (out.length >= MAX_STATS) break;
      if (!isObj(s) || !Object.prototype.hasOwnProperty.call(STATS, s.key)) continue;
      if (boons && !isBoon(s.key)) continue;
      const st = STATS[s.key];
      const as = st.noAs ? "total" : st.rate ? "per_s" : (AS.includes(s.as) ? s.as : "total");
      out.push({key: s.key, as});
    }
    return out.length ? out : [{key: boons ? BOON_DEFAULT : "damage", as: "total"}];
  }

  function normalizeSpec(raw, id) {
    if (!isObj(raw)) return null;
    let name = typeof raw.name === "string" ? raw.name.replace(/[\u0000-\u001f\u007f-\u009f]/g, "").trim().slice(0, NAME_MAX).trim() : "";
    if (!name) name = `Window ${idNum(id)}`;
    const rows = ROWS.includes(raw.rows) ? raw.rows : "players";
    const stats = normStats(raw.stats, rows === "boons");
    const who = isObj(raw.who) ? raw.who : {};
    const filters = (Array.isArray(who.filters) ? who.filters : [])
      .filter(f => typeof f === "string" && FILTER_RE.test(f)).slice(0, MAX_FILTERS);
    const sort = isObj(raw.sort) ? raw.sort : {};
    const sortKey = sort.key === "name" || stats.some(s => s.key === sort.key) ? sort.key : stats[0].key;
    const lim = Math.floor(Number(raw.limit));
    return {
      id, name, rows, stats,
      who: {base: BASES.includes(who.base) ? who.base : "squad", filters},
      span: SPANS.includes(raw.span) ? raw.span : "fight",
      // A boons window is a table (its rows are boons, not players).
      look: rows === "boons" ? "table" : LOOKS.includes(raw.look) ? raw.look : "table",
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

  // A player's ms for a boon stat: the flattened key (history views), else the feed's p.boons.
  function boonMs(p, key) {
    if (typeof p[key] === "number") return p[key];
    const st = STATS[key], b = p.boons && p.boons[st.boon];
    return Array.isArray(b) ? Number(b[MEASURES.indexOf(st.measure)]) || 0 : 0;
  }

  // The feed's p.boons as flat keys ("boon:<id>:<measure>", ms; 0 for every boon not held), so
  // a span differences them like any summed stat. Returns a new object without p.boons.
  function flattenPlayer(p) {
    const out = Object.assign({}, p);
    delete out.boons;
    for (const k of BASE_KEYS) if (isBoon(k)) out[k] = boonMs(p, k);
    return out;
  }

  // A boon stat's value over a span of spanMs: % of the span for duration boons, average stacks
  // for intensity boons. targets: how many players the ms are spread over; 0 gives null (no
  // targets, shown as "–"). The feed's group and squad are already per target, so callers pass 1
  // for every measure. Never negative, never NaN.
  function boonRate(key, ms, spanMs, targets) {
    if (targets === 0) return null;
    const st = STATS[key];
    const n = Number(ms), span = Number(spanMs), t = Number(targets) || 1;
    if (!st || !(span > 0) || !(n > 0)) return 0;
    const v = n / t / span;
    return st.intensity ? v : v * 100;
  }

  // A boon value's text: "–" for null, stacks with 1 decimal, else % with 0 decimals.
  function formatBoon(key, v) {
    if (v == null || !Number.isFinite(v)) return "\u2013";
    return STATS[key] && STATS[key].intensity ? v.toFixed(1) : `${Math.round(v)}%`;
  }

  // A player's value for a catalog key. Totals for base keys; rates are per second; boons as a
  // rate of the span (boonRate).
  // live (fight span): dps/hps come from the feed. Otherwise rate keys summed over seconds.
  function statValue(p, key, seconds, live) {
    const st = STATS[key];
    if (!st) return 0;
    if (st.kind === "boon") return boonRate(key, boonMs(p, key), seconds * 1000, 1);
    if (!st.rate) return Number(p[key]) || 0;
    if (live) return Number(p[key]) || 0;
    if (!(seconds > 0)) return 0;
    return st.rate.reduce((a, k) => a + (Number(p[k]) || 0), 0) / seconds;
  }

  function totalPlayer(list) {
    // Heal stats are full only if every player's are.
    const t = {name: "Total", account: "", prof: 0, heal_full: list.length > 0 && list.every(p => p.heal_full)};
    for (const k of Object.keys(STATS)) if (!isBoon(k)) t[k] = list.reduce((a, p) => a + (Number(p[k]) || 0), 0);
    return t;
  }

  // A boon stat over several players: uptime averaged, generation summed.
  function boonOver(players, key, seconds) {
    let sum = 0;
    for (const p of players) sum += statValue(p, key, seconds, false) || 0;
    return STATS[key].measure === "up" ? (players.length ? sum / players.length : 0) : sum;
  }

  // rows "boons": one row per boon picked (SQUAD_BOONS order), one column per measure picked (in
  // the order picked). Rows whose values are all zero are left out.
  function boonRows(spec, view) {
    const matched = filterWho(view.players, spec.who);
    const picked = new Set(), cols = [];
    for (const s of spec.stats) {
      if (!isBoon(s.key)) continue;
      picked.add(STATS[s.key].boon);
      if (!cols.includes(STATS[s.key].measure)) cols.push(STATS[s.key].measure);
    }
    const rows = [];
    for (const b of SQUAD_BOONS) {
      if (!picked.has(b)) continue;
      const values = cols.map(m => boonOver(matched, `boon:${b}:${m}`, view.seconds));
      if (values.every(v => !v)) continue;
      rows.push({p: {name: BOON_NAMES[b], boon: b, intensity: INTENSITY.includes(b)}, values});
    }
    return {rows, cols, footer: `${rows.length} boons`};
  }

  // Rows and footer for a spec over a view ({players, seconds, live}). limit null -> maxRows.
  // rows "boons" also returns cols (see boonRows).
  function computeRows(spec, view, fmt, maxRows) {
    if (spec.rows === "boons") return boonRows(spec, view);
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
      const r = build(t);
      // Boons over the players: uptime averaged, generation summed (the ms sum is not a rate).
      spec.stats.forEach((s, i) => { if (isBoon(s.key)) r.values[i] = boonOver(matched, s.key, seconds); });
      rows = [r];
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
      if (shown >= 2 || STATS[s.key].count || isBoon(s.key)) return;
      shown++;
      parts.push(`${fmt(sums[i])} ${STATS[s.key].label.toLowerCase()}`);
    });
    return {rows, footer: parts.join(" \u00b7 ")};
  }

  // History of the live feed for spans other than "fight": a 60 s ring of snapshots (last:N), the
  // session (summed finished fights) and 1 s chart series. The clock is fight.duration_ms, never
  // wall time. Players are keyed by account || name; only identity fields and BASE_KEYS are kept
  // (boons flattened, see flattenPlayer). A player with no boons keeps only the fight keys (NF):
  // missing keys read as 0. Charts track at most CHART_KEYS base keys at once: 12 keys x 50
  // players x 3600 s x 4 bytes is about 8.6 MB.
  const IDENT = ["name", "account", "prof", "elite", "spec", "group", "me", "heal_full"];
  const RING_MS = 60000, SERIES_CAP = 3600, NK = BASE_KEYS.length, NF = FIGHT_KEYS.length, CHART_KEYS = 12;
  const at = (v, i) => i < v.length ? v[i] : 0;
  const hasBoons = p => isObj(p.boons) ? Object.keys(p.boons).length > 0
    : BASE_KEYS.some((k, i) => i >= NF && typeof p[k] === "number");
  const NO_VALS = new Map();
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
    // Series: one sample per whole fight second. sample.vals: Map<key, Float32Array by slot>, each
    // player's gain since the previous sample (0 when absent in either). prev: the previous
    // sample's totals per key (Float64Array by slot, NaN = absent). gen: bumped when past samples
    // change (start over, a key dropped), so cached series are computed again.
    let samples = [], slots = new Map(), slotId = [], prev = new Map(), gen = 0;

    const snapOf = (f) => {
      const map = new Map();
      for (const p of Array.isArray(f.players) ? f.players : []) {
        if (!p) continue;
        const k = pkey(p);
        if (!k) continue;
        const id = {};
        for (const n of IDENT) id[n] = p[n];
        const v = new Float64Array(hasBoons(p) ? NK : NF);
        for (let i = 0; i < NF; i++) v[i] = Number(p[BASE_KEYS[i]]) || 0;
        for (let i = NF; i < v.length; i++) v[i] = boonMs(p, BASE_KEYS[i]);
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
        for (let i = 0; i < NK; i++) s.v[i] += Math.max(0, at(e.v, i) - (b ? at(b.v, i) : 0));
      }
      sessionMs += Math.max(0, cur.ms - (base ? base.ms : 0));
      curAdded = true;
    }

    function clearSeries() { samples = []; slots = new Map(); slotId = []; prev = new Map(); gen++; }

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
          const now = new Float64Array(slotId.length).fill(NaN);
          for (const [k, e] of snap.map) now[slots.get(k)] = at(e.v, ki);
          const was = last ? prev.get(key) : null;
          if (was) {
            const d = new Float32Array(slotId.length);
            for (let j = 0; j < was.length; j++) {
              const x = now[j] - was[j];
              if (x > 0) d[j] = x;
            }
            vals.set(key, d);
          }
          prev.set(key, now);
        }
      }
      // Seconds the feed skipped gain nothing (the gain lands on this second), so there is one
      // sample per second.
      if (last) for (let s = Math.max(last.s + 1, sec - SERIES_CAP); s < sec; s++) samples.push({s, vals: NO_VALS});
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
        for (let i = 0; i < NK; i++) d[i] = at(e.v, i) - at(r.v, i);
        out.push(player(e.id, d));
      }
      // No snapshot N s back (the ring started mid-fight, or the fight is younger): the time the
      // ring covers.
      return withRates(out, ref ? n : Math.min(n, (now.ms - ring[0].ms) / 1000));
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
        for (let i = 0; i < NK; i++) a.v[i] += Math.max(0, at(e.v, i) - (b ? at(b.v, i) : 0));
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

    // The base keys open charts use, in order. Keys already tracked stay; new ones are added up
    // to CHART_KEYS, the rest are not tracked. Returns the tracked set.
    function track(keys) {
      const want = [...new Set([...(keys || [])].filter(k => BASE_KEYS.includes(k)))];
      const next = new Set(want.filter(k => tracked.has(k)));
      for (const k of want) if (next.size < CHART_KEYS) next.add(k);
      for (const k of tracked) if (!next.has(k)) {
        prev.delete(k);
        for (const s of samples) s.vals.delete(k);
        gen++;
      }
      tracked = next;
      return tracked;
    }

    // Per-second lines over "who" (whoFn picks from the players seen). keys: one entry per line,
    // a base key or an array of them (summed). memo: an object the caller keeps per window; while
    // keys, "who" and the history stay the same, only new seconds are added. The returned arrays
    // belong to memo: don't mutate them.
    function series(keys, whoFn, memo) {
      const groups = (keys || []).map(k => Array.isArray(k) ? k : [k]);
      const players = slotId.map(id => Object.assign({}, id));
      const kept = new Set(typeof whoFn === "function" ? whoFn(players) : players);
      const idx = [];
      players.forEach((p, j) => { if (kept.has(p)) idx.push(j); });
      const n = Math.max(0, samples.length - 1);
      const first = samples.length ? samples[0].s : 0, end = first + n;
      // Line value i: the gain at samples[i + 1], second first + i + 1.
      const at = (i, g) => {
        const vals = samples[i + 1].vals;
        let sum = 0;
        for (const key of g) {
          const d = vals.get(key);
          if (d) for (const j of idx) { const x = d[j]; if (x > 0) sum += x; }
        }
        return sum;
      };
      const m = memo && typeof memo === "object" ? memo : {};
      const sig = JSON.stringify(groups) + "|" + idx.join();
      if (m.values && m.gen === gen && m.sig === sig && m.first <= first && m.end <= end) {
        const drop = Math.min(first - m.first, m.end - m.first);
        if (drop > 0) for (const a of m.values) a.splice(0, drop);
        for (let s = Math.max(m.end, first) + 1; s <= end; s++) {
          const i = s - first - 1;
          m.values.forEach((a, g) => a.push(at(i, groups[g])));
        }
      } else {
        m.values = groups.map(g => {
          const a = new Array(n);
          for (let i = 0; i < n; i++) a[i] = at(i, g);
          return a;
        });
      }
      m.gen = gen; m.sig = sig; m.first = first; m.end = end;
      return {t0: first * 1000, step: 1000, values: m.values};
    }

    function bytes() {
      let n = 0;
      for (const s of ring) for (const e of s.map.values()) n += e.v.byteLength;
      n += session.size * NK * 8;
      for (const a of prev.values()) n += a.byteLength;
      for (const s of samples) for (const a of s.vals.values()) n += a.byteLength;
      return n;
    }

    return {
      push, view, track, series, bytes,
      isTracked: k => tracked.has(k),
      clear() { ring = []; clearSeries(); cache = new Map(); },
      clearSession() {
        cache = new Map();
        session = new Map(); sessionMs = 0;
        base = cur && !curAdded ? cur : null;
      },
    };
  }

  const blankSpec = id => normalizeSpec({}, id);

  // One key in a name field (the DLL forwards them while the field has focus): a character to
  // insert at the caret, or an editing key. done: "commit" (Enter), "cancel" (Escape) or null.
  // Characters are single UTF-16 units (the DLL drops the rest); the value stays <= NAME_MAX.
  const KEY_RE = /^[^\u0000-\u001f\u007f-\u009f\ud800-\udfff]$/;
  function applyKey(field, key) {
    const value = String(field.value);
    let caret = Math.max(0, Math.min(value.length, Math.floor(Number(field.caret)) || 0));
    const out = (v, c, done = null) => ({value: v, caret: c, done});
    switch (key) {
      case "Backspace": return caret ? out(value.slice(0, caret - 1) + value.slice(caret), caret - 1) : out(value, caret);
      case "Delete": return out(value.slice(0, caret) + value.slice(caret + 1), caret);
      case "ArrowLeft": return out(value, Math.max(0, caret - 1));
      case "ArrowRight": return out(value, Math.min(value.length, caret + 1));
      case "Home": return out(value, 0);
      case "End": return out(value, value.length);
      case "Enter": return out(value, caret, "commit");
      case "Escape": return out(value, caret, "cancel");
    }
    if (typeof key !== "string" || !KEY_RE.test(key) || value.length >= NAME_MAX) return out(value, caret);
    return out(value.slice(0, caret) + key + value.slice(caret), caret + 1);
  }

  return {STATS, BASE_KEYS, FIGHT_KEYS, CHART_KEYS, MAX_CUSTOM, MAX_STATS, NAME_MAX, SPANS, LOOKS, SQUAD_BOONS,
    filterWho, statValue, computeRows, normalizeSpec, normalizeAll, migrateTables, specFromBuiltin, blankSpec, createHistory, applyKey,
    flattenPlayer, boonRate, formatBoon};
})();
if (typeof module !== "undefined" && module.exports) module.exports = axidpsCustom;
