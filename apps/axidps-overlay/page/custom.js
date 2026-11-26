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

  const blankSpec = id => normalizeSpec({}, id);

  return {STATS, BASE_KEYS, MAX_CUSTOM, MAX_STATS, NAME_MAX, SPANS, LOOKS,
    normalizeSpec, normalizeAll, migrateTables, specFromBuiltin, blankSpec};
})();
if (typeof module !== "undefined" && module.exports) module.exports = axidpsCustom;
