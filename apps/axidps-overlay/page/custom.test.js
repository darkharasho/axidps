const test = require("node:test");
const assert = require("node:assert");
const C = require("./custom.js");
const { filterWho, computeRows, statValue, normalizeSpec, normalizeAll, migrateTables, specFromBuiltin, blankSpec } = C;

test("catalog shape", () => {
  // 17 fight stats, 13 boons x 4 measures, and the boons windows' 4 measures.
  assert.strictEqual(Object.keys(C.STATS).length, 17 + 52 + 4);
  assert.strictEqual(C.BASE_KEYS.length, 15 + 52);
  assert.strictEqual(C.FIGHT_KEYS.length, 15);
  assert.ok(!C.BASE_KEYS.includes("dps") && !C.BASE_KEYS.includes("hps"));
  assert.deepStrictEqual(C.STATS.hps.rate, ["healing", "barrier"]);
  assert.deepStrictEqual(C.STATS.dps.rate, ["damage"]);
  assert.strictEqual(C.STATS.cleanses.count, true);
  assert.strictEqual(C.MAX_CUSTOM, 8);
});

test("normalizeSpec drops unknown stats and keeps order", () => {
  const s = normalizeSpec({ stats: [{ key: "cleanses" }, { key: "gear" }, { key: "strips", as: "share" }] }, "c1");
  assert.deepStrictEqual(s.stats.map(x => x.key), ["cleanses", "strips"]);
  assert.deepStrictEqual(s.stats.map(x => x.as), ["total", "share"]);
});

test("normalizeSpec fixes dps and hps to per_s", () => {
  assert.strictEqual(normalizeSpec({ stats: [{ key: "dps", as: "share" }] }, "c1").stats[0].as, "per_s");
});

test("normalizeSpec defaults", () => {
  assert.deepStrictEqual(normalizeSpec({}, "c1"), {
    id: "c1", name: "Window 1", rows: "players", stats: [{ key: "damage", as: "total" }],
    who: { base: "squad", filters: [] }, span: "fight", look: "table",
    sort: { key: "damage", asc: false }, limit: null,
  });
  assert.deepStrictEqual(blankSpec("c3"), normalizeSpec({}, "c3"));
});

test("normalizeSpec clamps", () => {
  assert.strictEqual(normalizeSpec({ name: "x".repeat(500) }, "c1").name.length, 24);
  assert.strictEqual(normalizeSpec({ name: "a\u0000b\u001fc" }, "c1").name, "abc");
  assert.strictEqual(normalizeSpec({ name: "   " }, "c4").name, "Window 4");
  const many = Array.from({ length: 12 }, () => ({ key: "damage" }));
  assert.strictEqual(normalizeSpec({ stats: many }, "c1").stats.length, 8);
  assert.strictEqual(normalizeSpec({ limit: 0 }, "c1").limit, null);
  assert.strictEqual(normalizeSpec({ limit: 99 }, "c1").limit, 50);
  assert.strictEqual(normalizeSpec({ span: "last:45" }, "c1").span, "fight");
  assert.strictEqual(normalizeSpec({ span: "last:30" }, "c1").span, "last:30");
  assert.strictEqual(normalizeSpec({ look: "pie" }, "c1").look, "table");
  const f = ["spec:Firebrand", "prof:3", "prof:0", "spec:", "spec:a1", "x"];
  assert.deepStrictEqual(normalizeSpec({ who: { filters: f } }, "c1").who.filters, ["spec:Firebrand", "prof:3"]);
  const nine = Array.from({ length: 12 }, () => "prof:1");
  assert.strictEqual(normalizeSpec({ who: { filters: nine } }, "c1").who.filters.length, 9);
  assert.strictEqual(normalizeSpec({ sort: { key: "name", asc: true } }, "c1").sort.key, "name");
  assert.strictEqual(normalizeSpec({ stats: [{ key: "kills" }], sort: { key: "damage" } }, "c1").sort.key, "kills");
});

test("normalizeSpec rejects non-objects", () => {
  assert.strictEqual(normalizeSpec(null, "c1"), null);
  assert.strictEqual(normalizeSpec("x", "c1"), null);
  assert.strictEqual(normalizeSpec([], "c1"), null);
});

test("normalizeAll", () => {
  assert.deepStrictEqual(normalizeAll("x"), []);
  const dup = normalizeAll([{ id: "c2", name: "A" }, { id: "c2", name: "B" }]);
  assert.strictEqual(dup.length, 1);
  assert.strictEqual(dup[0].name, "A");
  const ten = Array.from({ length: 10 }, (_, i) => ({ id: "c" + (i + 1) }));
  assert.strictEqual(normalizeAll(ten).length, 8);
  assert.deepStrictEqual(normalizeAll([{ id: "t1" }]), []);
});

test("migrateTables", () => {
  const saved = {
    tables: [{ id: "t1", title: "Stab" }],
    cfg: { t1: { cols: ["cleanses", "nope"], groupOnly: true, sortKey: "cleanses", sortAsc: true } },
    pos: { t1: [5, 6] }, open: { t1: false },
  };
  assert.strictEqual(migrateTables(saved), true);
  assert.deepStrictEqual(saved.custom, [{
    id: "c1", name: "Stab", rows: "players", stats: [{ key: "cleanses", as: "total" }],
    who: { base: "group", filters: [] }, span: "fight", look: "table",
    sort: { key: "cleanses", asc: true }, limit: null,
  }]);
  assert.deepStrictEqual(saved.pos.c1, [5, 6]);
  assert.strictEqual(saved.open.c1, false);
  assert.deepStrictEqual(saved.tables, [{ id: "t1", title: "Stab" }]);
  assert.deepStrictEqual(saved.cfg.t1, { cols: ["cleanses", "nope"], groupOnly: true, sortKey: "cleanses", sortAsc: true });
  const before = JSON.stringify(saved);
  assert.strictEqual(migrateTables(saved), false);
  assert.strictEqual(JSON.stringify(saved), before);
  const none = {};
  assert.strictEqual(migrateTables(none), true);
  assert.deepStrictEqual(none.custom, []);
});

test("specFromBuiltin", () => {
  const s = specFromBuiltin({ title: "Support", cols: ["cleanses", "strips"], cfg: { groupOnly: false, sortKey: "cleanses", sortAsc: false } }, "c2");
  assert.strictEqual(s.name, "Copy of Support");
  assert.deepStrictEqual(s.stats.map(x => x.key), ["cleanses", "strips"]);
  assert.strictEqual(s.who.base, "squad");
  assert.strictEqual(s.look, "table");
  assert.strictEqual(s.span, "fight");
  const o = specFromBuiltin({ title: "S", cols: ["cleanses"], cfg: { cols: ["strips"] } }, "c2");
  assert.deepStrictEqual(o.stats.map(x => x.key), ["strips"]);
});

test("normalizeSpec accepts every schema value", () => {
  for (const b of ["squad", "group", "me"]) assert.strictEqual(normalizeSpec({ who: { base: b } }, "c1").who.base, b);
  assert.strictEqual(normalizeSpec({ who: { base: "all" } }, "c1").who.base, "squad");
  for (const r of ["players", "total"]) assert.strictEqual(normalizeSpec({ rows: r }, "c1").rows, r);
  assert.strictEqual(normalizeSpec({ rows: "x" }, "c1").rows, "players");
  for (const a of ["total", "per_s", "share"]) assert.strictEqual(normalizeSpec({ stats: [{ key: "damage", as: a }] }, "c1").stats[0].as, a);
  assert.strictEqual(normalizeSpec({ stats: [{ key: "damage", as: "x" }] }, "c1").stats[0].as, "total");
  assert.strictEqual(normalizeSpec({ stats: [{ key: "hps", as: "total" }] }, "c1").stats[0].as, "per_s");
});

test("migrateTables gives default columns when none are valid", () => {
  for (const cfg of [{}, { cols: ["nope"] }]) {
    const saved = { tables: [{ id: "t2", title: "X" }], cfg: { t2: cfg } };
    migrateTables(saved);
    assert.deepStrictEqual(saved.custom[0].stats, [{ key: "damage", as: "total" }, { key: "dps", as: "per_s" }]);
  }
});

const P = (o) => Object.assign({ account: "", name: "", prof: 0, spec: "", group: 0, me: false, heal_full: false,
  damage: 0, downs: 0, taken: 0, healing: 0, barrier: 0 }, o);
const fixture = () => [
  P({ account: "a", name: "A", group: 1, me: true, spec: "Firebrand", prof: 1, damage: 100 }),
  P({ account: "b", name: "B", group: 1, spec: "Scrapper", prof: 3, damage: 300 }),
  P({ account: "c", name: "C", group: 2, spec: "Firebrand", damage: 0 }),
  P({ account: "d", name: "D", group: 2, spec: "Druid", prof: 4, damage: 600 }),
];
const names = l => l.map(p => p.name).join("");
const mk = (o) => normalizeSpec(o, "c1");

test("filterWho group", () => assert.strictEqual(names(filterWho(fixture(), { base: "group" })), "AB"));
test("filterWho me", () => assert.strictEqual(names(filterWho(fixture(), { base: "me" })), "A"));
test("filterWho without me", () => {
  const ps = fixture(); ps[0].me = false;
  assert.strictEqual(filterWho(ps, { base: "group" }).length, 4);
  assert.deepStrictEqual(filterWho(ps, { base: "me" }), []);
});
test("filterWho filters are any-of", () => {
  assert.strictEqual(names(filterWho(fixture(), { base: "squad", filters: ["spec:Firebrand", "prof:4"] })), "ACD");
});

test("share before limit", () => {
  const r = computeRows(mk({ stats: [{ key: "damage", as: "share" }], sort: { key: "damage", asc: false }, limit: 2 }),
    { players: fixture(), seconds: 10 }, String, 5);
  assert.deepStrictEqual(r.rows.map(x => x.p.name), ["D", "B"]);
  assert.deepStrictEqual(r.rows.map(x => x.values[0]), [60, 30]);
});
test("share of zero", () => {
  const ps = fixture().map(p => ({ ...p, damage: 0 }));
  const r = computeRows(mk({ stats: [{ key: "damage", as: "share" }] }), { players: ps, seconds: 10 }, String, 5);
  assert.ok(r.rows.every(x => x.values[0] === null));
});
test("per_s", () => {
  const s = mk({ stats: [{ key: "damage", as: "per_s" }] });
  assert.strictEqual(computeRows(s, { players: fixture(), seconds: 10 }, String, 5).rows[0].values[0], 60);
  assert.strictEqual(computeRows(s, { players: fixture(), seconds: 0 }, String, 5).rows[0].values[0], 0);
});
test("dps live vs span", () => {
  assert.strictEqual(statValue(P({ dps: 7, hps: 3 }), "dps", 10, true), 7);
  assert.strictEqual(statValue(P({ dps: 7, hps: 3 }), "hps", 10, true), 3);
  assert.strictEqual(statValue(P({ damage: 600 }), "dps", 10, false), 60);
  assert.strictEqual(statValue(P({ healing: 30, barrier: 20 }), "hps", 10, false), 5);
  assert.strictEqual(statValue(P({ damage: 600 }), "dps", 0, false), 0);
});
test("rows total", () => {
  const v = { players: fixture(), seconds: 10 };
  const r = computeRows(mk({ rows: "total", stats: [{ key: "damage" }] }), v, String, 5);
  assert.strictEqual(r.rows.length, 1);
  assert.strictEqual(r.rows[0].p.name, "Total");
  assert.strictEqual(r.rows[0].values[0], 1000);
  const s = computeRows(mk({ rows: "total", stats: [{ key: "damage", as: "share" }] }), v, String, 5);
  assert.strictEqual(s.rows[0].values[0], 100);
});
test("rows total heal_full only when everyone's is", () => {
  const spec = mk({ rows: "total", stats: [{ key: "healing" }] });
  const total = ps => computeRows(spec, { players: ps, seconds: 1 }, String, 5).rows[0].p.heal_full;
  const ps = fixture();
  ps[0].heal_full = true;
  assert.strictEqual(total(ps), false);
  assert.strictEqual(total(ps.map(p => ({ ...p, heal_full: true }))), true);
  assert.strictEqual(total([]), false);
});
test("sort by name asc", () => {
  const ps = fixture().reverse();
  const r = computeRows(mk({ stats: [{ key: "damage" }], sort: { key: "name", asc: true } }), { players: ps, seconds: 1 }, String, 5);
  assert.strictEqual(names(r.rows.map(x => x.p)), "ABCD");
  const m = fixture(); delete m[3].damage;
  const r2 = computeRows(mk({ stats: [{ key: "damage" }], sort: { key: "damage", asc: true } }), { players: m, seconds: 1 }, String, 5);
  assert.deepStrictEqual(r2.rows.map(x => x.values[0]), [0, 0, 100, 300]);
});
test("footer", () => {
  const v = { players: fixture(), seconds: 10 };
  const r = computeRows(mk({ stats: [{ key: "downs" }, { key: "damage" }, { key: "taken" }] }), v, String, 5);
  assert.strictEqual(r.footer, "4 players \u00b7 1000 dmg \u00b7 0 taken");
  assert.strictEqual(computeRows(mk({ stats: [{ key: "downs" }] }), v, String, 5).footer, "4 players");
});
test("limit falls back to maxRows", () => {
  const r = computeRows(mk({ stats: [{ key: "damage" }] }), { players: fixture(), seconds: 1 }, String, 3);
  assert.strictEqual(r.rows.length, 3);
});

// ---- history ----
const { createHistory } = C;
const feed = (state, ms, players) => ({
  v: 1, t: ms, state, fight: state === "idle" ? null : { duration_ms: ms },
  players: players.map(p => Object.assign({ account: p.name.toLowerCase(), prof: 1, elite: 0, spec: "Firebrand",
    group: 1, me: false, heal_full: false }, p)),
  timeline: [], healing: null,
});
const byName = (v, n) => v.players.find(p => p.name === n);
const fightTo = (h, end, players, state = "fight") => {
  for (let ms = 0; ms <= end; ms += 500) h.push(feed(state, ms, players(ms)));
};

test("history last:10", () => {
  const h = createHistory();
  fightTo(h, 30000, ms => [{ name: "A", damage: ms / 10 }]);
  const v = h.view("last:10");
  assert.strictEqual(byName(v, "A").damage, 1000);
  assert.strictEqual(v.seconds, 10);
  assert.strictEqual(byName(v, "A").account, "a");
  assert.strictEqual(byName(v, "A").spec, "Firebrand");
  assert.strictEqual(h.view("fight"), null);
});
test("history last:N early", () => {
  const h = createHistory();
  fightTo(h, 4000, ms => [{ name: "A", damage: ms / 10 }]);
  const v = h.view("last:10");
  assert.strictEqual(v.seconds, 4);
  assert.strictEqual(byName(v, "A").damage, 400);
});
test("history last:N when the ring starts mid-fight", () => {
  const h = createHistory();
  for (let ms = 20000; ms <= 30000; ms += 500) h.push(feed("fight", ms, [{ name: "A", damage: ms / 10 }]));
  const v = h.view("last:60");
  assert.strictEqual(v.seconds, 10);
  assert.strictEqual(byName(v, "A").damage, 1000);
  assert.strictEqual(h.view("last:10").seconds, 10);
});
test("history after the fight", () => {
  const h = createHistory();
  fightTo(h, 30000, ms => [{ name: "A", damage: ms / 10 }]);
  for (let i = 0; i < 4; i++) h.push(feed("last", 30000, [{ name: "A", damage: 3000 }]));
  assert.strictEqual(byName(h.view("last:10"), "A").damage, 1000);
});
test("history joiner counts from first snapshot", () => {
  const h = createHistory();
  fightTo(h, 30000, ms => ms < 25000 ? [{ name: "A", damage: ms }]
    : [{ name: "A", damage: ms }, { name: "B", damage: 50 + (ms - 25000) * 40 / 5000 }]);
  assert.strictEqual(byName(h.view("last:10"), "B").damage, 40);
});
test("history leaver dropped", () => {
  const h = createHistory();
  fightTo(h, 30000, ms => ms <= 20000 ? [{ name: "A", damage: ms }, { name: "B", damage: ms }] : [{ name: "A", damage: ms }]);
  assert.strictEqual(byName(h.view("last:10"), "B"), undefined);
  assert.ok(byName(h.view("last:10"), "A"));
});
test("history no negatives", () => {
  const h = createHistory();
  fightTo(h, 30000, ms => [{ name: "A", damage: ms <= 20000 ? 900 : 100 }]);
  assert.strictEqual(byName(h.view("last:10"), "A").damage, 0);
});
test("history new fight clears ring", () => {
  const h = createHistory();
  fightTo(h, 30000, ms => [{ name: "A", damage: ms }, { name: "B", damage: ms }]);
  h.push(feed("last", 30000, [{ name: "A", damage: 30000 }, { name: "B", damage: 30000 }]));
  h.push(feed("fight", 0, [{ name: "A", damage: 0 }]));
  h.push(feed("fight", 500, [{ name: "A", damage: 7 }]));
  const v = h.view("last:60");
  assert.strictEqual(v.players.length, 1);
  assert.strictEqual(byName(v, "A").damage, 7);
  assert.strictEqual(v.seconds, 0.5);
});
test("history session", () => {
  const h = createHistory();
  fightTo(h, 30000, ms => [{ name: "A", damage: ms / 30 }]);
  for (let i = 0; i < 3; i++) h.push(feed("last", 30000, [{ name: "A", damage: 1000 }]));
  h.push(feed("idle", 0, []));
  fightTo(h, 5000, ms => [{ name: "A", damage: ms / 25 }]);
  const v = h.view("session");
  assert.strictEqual(byName(v, "A").damage, 1200);
  assert.strictEqual(v.seconds, 35);
  assert.strictEqual(byName(v, "A").name, "A");
});
test("history session without a clean end", () => {
  const h = createHistory();
  fightTo(h, 30000, ms => [{ name: "A", damage: ms / 30 }]);
  h.push(null);
  h.push(feed("fight", 500, [{ name: "A", damage: 0 }]));
  const v = h.view("session");
  assert.strictEqual(byName(v, "A").damage, 1000);
  assert.strictEqual(v.seconds, 30.5);
});
test("history session restart", () => {
  const h = createHistory();
  fightTo(h, 30000, ms => [{ name: "A", damage: ms / 30 }]);
  h.push(feed("fight", 1000, [{ name: "A", damage: 10 }]));
  h.push(feed("fight", 2000, [{ name: "A", damage: 20 }]));
  const v = h.view("session");
  assert.strictEqual(byName(v, "A").damage, 1020);
  assert.strictEqual(v.seconds, 32);
});
test("history clearSession and clear", () => {
  const h = createHistory();
  fightTo(h, 30000, ms => [{ name: "A", damage: ms / 30 }]);
  h.push(feed("last", 30000, [{ name: "A", damage: 1000 }]));
  h.clear();
  assert.strictEqual(h.view("last:10").players.length, 0);
  assert.strictEqual(byName(h.view("session"), "A").damage, 1000);
  h.clearSession();
  const v = h.view("session");
  assert.strictEqual(v.seconds, 0);
  assert.deepStrictEqual(v.players, []);
});
test("history clearSession mid-fight counts from now", () => {
  const h = createHistory();
  fightTo(h, 10000, ms => [{ name: "A", damage: ms / 10 }]);
  h.clearSession();
  for (let ms = 10500; ms <= 20000; ms += 500) h.push(feed("fight", ms, [{ name: "A", damage: ms / 10 }]));
  h.push(feed("last", 20000, [{ name: "A", damage: 2000 }]));
  const v = h.view("session");
  assert.strictEqual(byName(v, "A").damage, 1000);
  assert.strictEqual(v.seconds, 10);
});
test("history series", () => {
  const h = createHistory();
  h.track(new Set(["damage"]));
  fightTo(h, 3000, ms => [{ name: "A", damage: ms / 10 }, { name: "B", damage: ms / 20 }]);
  const s = h.series(["damage"], ps => ps);
  assert.strictEqual(s.step, 1000);
  assert.strictEqual(s.t0, 0);
  assert.deepStrictEqual(s.values[0], [150, 150, 150]);
  assert.deepStrictEqual(h.series(["damage"], ps => ps.filter(p => p.name === "A")).values[0], [100, 100, 100]);
  assert.deepStrictEqual(h.series(["damage"], ps => filterWho(ps, { base: "squad", filters: ["spec:Firebrand"] })).values[0], [150, 150, 150]);
  assert.deepStrictEqual(h.series(["healing"], ps => ps).values[0], [0, 0, 0]);
  h.track(new Set());
  assert.deepStrictEqual(h.series(["damage"], ps => ps).values[0], [0, 0, 0]);
});
test("history series cap", () => {
  const h = createHistory();
  h.track(new Set(["damage"]));
  for (let s = 0; s < 4000; s++) h.push(feed("fight", s * 1000, [{ name: "A", damage: s }]));
  const r = h.series(["damage"], ps => ps);
  assert.strictEqual(r.values[0].length, 3600);
  assert.strictEqual(r.t0, 399000);
  assert.ok(r.values[0].every(x => x === 1));
});
// The render budget is for a player's machine (spec: 8 windows under 1 ms per render). Shared CI
// runners are a few times slower, so CI only checks for a gross regression.
const RENDER_MS = process.env.CI ? 3 : 1;

test("history budget", () => {
  const h = createHistory();
  h.track(new Set(["damage", "healing"]));
  const keys = C.FIGHT_KEYS;
  // Every player holds and gives all 13 boons, as the feed sends them.
  const players = ms => Array.from({ length: 50 }, (_, i) => {
    const p = { name: "P" + i, group: 1 + (i % 10), me: i === 0, boons: {} };
    keys.forEach((k, j) => { p[k] = (ms / 1000) * (i + j + 1); });
    C.SQUAD_BOONS.forEach((b, j) => { p.boons[b] = [ms * 0.9, ms * 0.3, ms * 0.2, ms * 0.1 + j]; });
    return p;
  });
  for (let ms = 0; ms <= 3600000; ms += 500) h.push(feed("fight", ms, players(ms)));
  assert.ok(h.bytes() < 10e6, `bytes ${h.bytes()}`);
  assert.ok(h.bytes() > 0);
  const all = Object.keys(C.STATS);
  const specs = Array.from({ length: 8 }, (_, i) => normalizeSpec({
    stats: all.slice(i, i + 8).concat(all).slice(0, 8).map(key => ({ key, as: i % 2 ? "share" : "total" })),
    span: "last:60",
  }, "c" + (i + 1)));
  // Each render follows a fresh feed (as in the overlay), so the span views are computed again.
  let ms = 3600000;
  const timed = (render) => {
    const step = () => { ms += 500; const f = feed("fight", ms, players(ms)); h.push(f); return f.players; };
    for (let i = 0; i < 20; i++) render(step());
    let spent = 0;
    for (let i = 0; i < 200; i++) {
      const ps = step();
      const t = performance.now();
      render(ps);
      spent += performance.now() - t;
    }
    return spent / 200;
  };
  const mean = timed(() => { for (const s of specs) computeRows(s, h.view(s.span), String, 50); });
  assert.ok(mean < RENDER_MS, `mean ${mean} ms`);
  // 8 windows of boon stats: players windows of 8 boon stats, boons windows of the 4 measures,
  // over spans and the live fight.
  const boonKeys = Object.keys(C.STATS).filter(k => C.STATS[k].kind === "boon");
  const bspecs = Array.from({ length: 8 }, (_, i) => normalizeSpec(i % 2 ? {
    rows: "boons", stats: C.BOON_MEASURE_KEYS.map(key => ({ key })),
    who: { base: i % 4 < 2 ? "squad" : "group" }, span: ["fight", "last:10", "last:60", "session"][i % 4],
  } : {
    rows: "players", stats: boonKeys.slice(i * 6, i * 6 + 8).map(key => ({ key })),
    who: { base: i % 4 < 2 ? "squad" : "group" }, span: ["fight", "last:10", "last:60", "session"][i % 4],
  }, "c" + (i + 1)));
  assert.ok(bspecs.every((s, i) => s.stats.length === (i % 2 ? 4 : 8)));
  // Each cell formatted as the windows show it (formatBoon), as well.
  let cells = 0;
  const bmean = timed(ps => {
    const live = { players: ps, seconds: ms / 1000, live: true };
    for (const s of bspecs) {
      const r = computeRows(s, s.span === "fight" ? live : h.view(s.span), String, 50);
      for (const row of r.rows) row.values.forEach((v, i) => { cells += C.formatBoon(row.keys ? row.keys[i] : s.stats[i].key, v).length; });
    }
  });
  assert.ok(cells > 0);
  assert.ok(bmean < RENDER_MS, `boon mean ${bmean} ms`);
  assert.ok(h.bytes() < 10e6, `bytes ${h.bytes()}`);
  console.log(`budget: fight ${mean.toFixed(3)} ms, boons ${bmean.toFixed(3)} ms per render, ${(h.bytes() / 1e6).toFixed(2)} MB`);
});
test("history chart key cap", () => {
  const h = createHistory();
  const keys = C.FIGHT_KEYS;
  assert.strictEqual(C.CHART_KEYS, 12);
  const t = h.track(keys);
  assert.deepStrictEqual([...t], keys.slice(0, 12));
  assert.ok(!h.isTracked(keys[12]) && !h.isTracked(keys[14]));
  // Keys already tracked keep their place; a new key gets a free one.
  h.track(keys.slice(1).concat(["gear", "dps"]));
  assert.deepStrictEqual([...h.track(keys.slice(1))].sort(), keys.slice(1, 13).sort());
  assert.ok(h.isTracked(keys[12]) && !h.isTracked(keys[0]));
  // An untracked key has no series.
  fightTo(h, 3000, ms => [Object.fromEntries([["name", "A"], ...keys.map(k => [k, ms])])]);
  const r = h.series([keys[0], keys[1], keys[14]], ps => ps);
  assert.deepStrictEqual(r.values, [[0, 0, 0], [1000, 1000, 1000], [0, 0, 0]]);
});
test("history series grouped keys", () => {
  const h = createHistory();
  h.track(["healing", "barrier"]);
  fightTo(h, 3000, ms => [{ name: "A", healing: ms / 10, barrier: ms / 20 }]);
  assert.deepStrictEqual(h.series([["healing", "barrier"], "barrier"], ps => ps).values, [[150, 150, 150], [50, 50, 50]]);
});
test("history series incremental equals a full recompute", () => {
  const h = createHistory();
  h.track(["damage", "healing"]);
  const memo = {}, keys = ["damage", ["damage", "healing"]];
  let who = ps => ps.filter(p => p.group === 1);
  const check = at => {
    const inc = h.series(keys, who, memo), full = h.series(keys, who);
    assert.deepStrictEqual(inc, full, `at ${at}`);
  };
  // B joins, leaves and comes back; C changes group (who changes); the feed skips seconds; the
  // series passes its cap; a key is dropped and added again.
  const pl = ms => {
    const out = [{ name: "A", damage: ms / 3, healing: ms / 7 }];
    if (ms > 5000 && (ms < 9000 || ms > 12000)) out.push({ name: "B", damage: ms / 5, healing: 3 });
    out.push({ name: "C", damage: ms / 11, group: ms > 20000 ? 2 : 1 });
    return out;
  };
  let ms = 0;
  const step = (to, d) => { for (; ms <= to; ms += d) { h.push(feed("fight", ms, pl(ms))); check(ms); } };
  step(30000, 500);
  step(60000, 3700);
  who = ps => ps;
  step(80000, 1000);
  h.track(["damage"]); check("drop");
  step(90000, 1000);
  h.track(["damage", "healing"]); check("add");
  step(100000, 1000);
  for (let i = 0; i < 3700; i++) { ms += 1000; h.push(feed("fight", ms, pl(ms))); if (i % 97 === 0) check(ms); }
  check("cap");
  assert.strictEqual(h.series(keys, who, memo).values[0].length, 3600);
  ms += 5000000; h.push(feed("fight", ms, pl(ms))); check("long gap");
  h.clear(); check("clear");
  ms = 0; step(5000, 500);
  // The memo's lines are extended in place, not computed again (B joined at 5 s: who changed).
  ms += 1000; h.push(feed("fight", ms, pl(ms)));
  const before = h.series(keys, who, memo).values[0];
  ms += 1000; h.push(feed("fight", ms, pl(ms)));
  assert.strictEqual(h.series(keys, who, memo).values[0], before);
  check("extended");
});
test("history chart budget: 50 players, 1 h, 8 charts of 8 stats", () => {
  const h = createHistory();
  const keys = C.FIGHT_KEYS;
  // Charts at the cap of tracked keys, and every player holding and giving all 13 boons.
  assert.strictEqual(h.track(keys).size, C.CHART_KEYS);
  const players = ms => Array.from({ length: 50 }, (_, i) => {
    const p = { name: "P" + i, group: 1 + (i % 10), me: i === 0, boons: {} };
    keys.forEach((k, j) => { p[k] = (ms / 1000) * (i + j + 1); });
    C.SQUAD_BOONS.forEach((b, j) => { p.boons[b] = [ms * 0.9, ms * 0.3, ms * 0.2, ms * 0.1 + j]; });
    return p;
  });
  const all = Object.keys(C.STATS).filter(k => !C.STATS[k].kind);
  const charts = Array.from({ length: 8 }, (_, i) => ({
    keys: all.slice(i, i + 8).concat(all).slice(0, 8).map(k => C.STATS[k].rate || k),
    who: ps => filterWho(ps, { base: i % 2 ? "group" : "squad", filters: [] }),
    memo: {},
  }));
  const draw = () => { for (const c of charts) h.series(c.keys, c.who, c.memo); };
  let ms = 0;
  for (; ms <= 3600000; ms += 500) h.push(feed("fight", ms, players(ms)));
  assert.ok(h.bytes() < 10e6, `bytes ${h.bytes()}`);
  console.log(`chart budget: ${(h.bytes() / 1e6).toFixed(2)} MB`);
  draw();
  let spent = 0;
  for (let i = 0; i < 30; i++) {
    ms += 1000; h.push(feed("fight", ms, players(ms)));
    const t = performance.now();
    draw();
    spent += performance.now() - t;
  }
  assert.ok(spent / 30 < 1, `mean ${spent / 30} ms`);
});
test("history returning player counts from inside the span", () => {
  const h = createHistory();
  fightTo(h, 30000, ms => ms >= 19000 && ms <= 21000 ? [{ name: "A", damage: ms }]
    : [{ name: "A", damage: ms }, { name: "B", damage: ms }]);
  const v = h.view("last:10");
  assert.strictEqual(byName(v, "A").damage, 10000);
  // B is back at 21.5 s; the ring keeps each second's first snapshot, so B counts from 22 s.
  assert.strictEqual(byName(v, "B").damage, 8000);
});

test("applyKey inserts at the caret", () => {
  assert.deepStrictEqual(C.applyKey({ value: "xy", caret: 1 }, "a"), { value: "xay", caret: 2, done: null });
  assert.deepStrictEqual(C.applyKey({ value: "", caret: 0 }, "é"), { value: "é", caret: 1, done: null });
});

test("applyKey Backspace and Delete", () => {
  assert.deepStrictEqual(C.applyKey({ value: "xy", caret: 0 }, "Backspace"), { value: "xy", caret: 0, done: null });
  assert.deepStrictEqual(C.applyKey({ value: "xy", caret: 2 }, "Backspace"), { value: "x", caret: 1, done: null });
  assert.deepStrictEqual(C.applyKey({ value: "xy", caret: 2 }, "Delete"), { value: "xy", caret: 2, done: null });
  assert.deepStrictEqual(C.applyKey({ value: "xy", caret: 0 }, "Delete"), { value: "y", caret: 0, done: null });
});

test("applyKey moves and clamps the caret", () => {
  const k = (caret, key) => C.applyKey({ value: "abc", caret }, key).caret;
  assert.strictEqual(k(1, "ArrowLeft"), 0);
  assert.strictEqual(k(0, "ArrowLeft"), 0);
  assert.strictEqual(k(1, "ArrowRight"), 2);
  assert.strictEqual(k(3, "ArrowRight"), 3);
  assert.strictEqual(k(2, "Home"), 0);
  assert.strictEqual(k(1, "End"), 3);
  assert.strictEqual(k(9, "ArrowLeft"), 2, "an out-of-range caret is clamped first");
});

test("applyKey Enter commits and Escape cancels", () => {
  assert.deepStrictEqual(C.applyKey({ value: "ab", caret: 1 }, "Enter"), { value: "ab", caret: 1, done: "commit" });
  assert.deepStrictEqual(C.applyKey({ value: "ab", caret: 1 }, "Escape"), { value: "ab", caret: 1, done: "cancel" });
});

test("applyKey refuses past NAME_MAX", () => {
  const full = "x".repeat(C.NAME_MAX);
  assert.deepStrictEqual(C.applyKey({ value: full, caret: 3 }, "y"), { value: full, caret: 3, done: null });
  assert.strictEqual(C.applyKey({ value: full.slice(1), caret: 0 }, "y").value.length, C.NAME_MAX);
});

test("applyKey ignores unknown keys and strings", () => {
  for (const key of ["F1", "Tab", "ab", "\u0007", "\u007f", "", null, 5]) {
    assert.deepStrictEqual(C.applyKey({ value: "ab", caret: 1 }, key), { value: "ab", caret: 1, done: null }, String(key));
  }
});

// ---- boons ----
const B = (boons, o) => P(Object.assign({ boons }, o));

test("boon_stats_flatten_and_difference", () => {
  // The history keeps the feed's nested boons as flat keys (ms; 0 for a boon not held).
  const f = createHistory();
  fightTo(f, 2000, ms => [{ name: "A", damage: ms, boons: { 1187: [ms * 3, ms, ms / 2, ms / 4], 740: [ms * 9, 0, 0, 0] } }]);
  const fl = byName(f.view("last:10"), "A");
  assert.strictEqual(fl["boon:1187:up"], 6000);
  assert.strictEqual(fl["boon:1187:self"], 2000);
  assert.strictEqual(fl["boon:1187:group"], 1000);
  assert.strictEqual(fl["boon:1187:squad"], 500);
  assert.strictEqual(fl["boon:740:up"], 18000);
  assert.strictEqual(fl["boon:30328:up"], 0);
  assert.strictEqual(fl.damage, 2000);
  assert.ok(!("boons" in fl));
  // A span differences the cumulative ms like any summed stat; a total that goes down (the end
  // tick) gives 0, never a negative.
  const h = createHistory();
  const q = ms => ms <= 20000 ? [ms / 2, ms / 4, 0, ms / 10] : [ms / 2, 5000 - 1, 0, ms / 10];
  fightTo(h, 30000, ms => [{ name: "A", boons: { 1187: q(ms) } }]);
  const v = byName(h.view("last:10"), "A");
  assert.strictEqual(v["boon:1187:up"], 5000);
  assert.strictEqual(v["boon:1187:self"], 0);
  assert.strictEqual(v["boon:1187:squad"], 1000);
  // Quickness uptime over the last 10 s: 5000 ms of 10000.
  assert.strictEqual(statValue(v, "boon:1187:up", 10, false), 50);
  // The live fight reads the feed's nested boons directly.
  assert.strictEqual(statValue(B({ 1187: [3000, 0, 0, 0] }), "boon:1187:up", 6, true), 50);
  // The session sums fights.
  for (let i = 0; i < 3; i++) h.push(feed("last", 30000, [{ name: "A", boons: { 1187: q(30000) } }]));
  h.push(feed("idle", 0, []));
  fightTo(h, 10000, ms => [{ name: "A", boons: { 1187: [ms, 0, 0, 0] } }]);
  const s = byName(h.view("session"), "A");
  assert.strictEqual(s["boon:1187:up"], 15000 + 10000);
  assert.strictEqual(h.view("session").seconds, 40);
});

test("boon_rate_zero_and_per_target", () => {
  // Duration boons in %, intensity boons in stacks.
  assert.strictEqual(C.boonRate("boon:1187:up", 2500, 10000), 25);
  assert.strictEqual(C.boonRate("boon:740:up", 250000, 10000), 25);
  // No boon: 0. No span: 0. Never NaN.
  assert.strictEqual(C.boonRate("boon:1187:group", 0, 10000), 0);
  assert.strictEqual(C.boonRate("boon:1187:up", 500, 0), 0);
  assert.strictEqual(C.boonRate("boon:1187:up", NaN, 1000), 0);
  // Group and squad are per target in the feed already: not divided again.
  assert.strictEqual(C.boonRate("boon:1187:squad", 4000, 10000), 40);
  // Negative ms (a total that went down) is 0.
  assert.strictEqual(C.boonRate("boon:1187:up", -40, 10000), 0);
  assert.strictEqual(C.formatBoon("boon:1187:group", null), "–");
  assert.strictEqual(C.formatBoon("boon:1187:group", 0), "0%");
  // The feed is already per target: group and squad are rates like uptime, never "–".
  const r = computeRows(mk({ stats: [{ key: "boon:1187:group" }] }), { players: [B({}), B({ 1187: [0, 0, 1000, 0] })], seconds: 10 }, String, 5);
  assert.deepStrictEqual(r.rows.map(x => x.values[0]).sort(), [0, 10]);
});

test("intensity_shows_stacks", () => {
  assert.strictEqual(C.STATS["boon:740:up"].intensity, true);
  assert.strictEqual(C.STATS["boon:1122:squad"].intensity, true);
  assert.strictEqual(C.STATS["boon:1187:up"].intensity, false);
  assert.strictEqual(C.STATS["boon:1187:up"].kind, "boon");
  assert.strictEqual(C.STATS["boon:1187:up"].label, "Quickness");
  assert.strictEqual(C.STATS["boon:1187:group"].label, "Quickness (group)");
  assert.strictEqual(C.STATS["boon:5974:self"].label, "Superspeed (self)");
  // Stacks with 1 decimal; duration boons in % with 0 decimals.
  assert.strictEqual(C.formatBoon("boon:740:up", 12.345), "12.3");
  assert.strictEqual(C.formatBoon("boon:1122:self", 0.04), "0.0");
  assert.strictEqual(C.formatBoon("boon:1187:up", 45.6), "46%");
  assert.strictEqual(C.formatBoon("boon:1187:up", 99.5), "100%");
  // 8 might stacks for half of 10 s: 4 stacks on average.
  assert.strictEqual(statValue(B({ 740: [40000, 0, 0, 0] }), "boon:740:up", 10, true), 4);
});

test("rows_boons_one_row_per_boon", () => {
  const ps = [
    B({ 1187: [10000, 2000, 4000, 1000], 30328: [5000, 0, 0, 0], 740: [100000, 0, 0, 0] }, { name: "A", group: 1, me: true }),
    B({ 1187: [0, 0, 2000, 3000], 30328: [5000, 0, 0, 0] }, { name: "B", group: 1 }),
    B({ 1122: [0, 0, 0, 0] }, { name: "C", group: 2 }),
  ];
  const spec = mk({ rows: "boons", stats: [{ key: "boons:up" }, { key: "boons:group" }] });
  assert.strictEqual(spec.rows, "boons");
  assert.strictEqual(spec.sort.key, null);
  const r = computeRows(spec, { players: ps, seconds: 10 }, String, 5);
  // Every boon with a value, in SQUAD_BOONS order; Stability (all zero) and the boons nobody has
  // are left out. Columns are the measures picked.
  assert.deepStrictEqual(r.cols, ["up", "group"]);
  assert.deepStrictEqual(r.rows.map(x => x.p.name), ["Might", "Quickness", "Alacrity"]);
  assert.deepStrictEqual(r.rows.map(x => x.p.boon), [740, 1187, 30328]);
  // Each cell's own boon key, for formatting (stacks for Might, % for the rest).
  assert.deepStrictEqual(r.rows[0].keys, ["boon:740:up", "boon:740:group"]);
  assert.strictEqual(C.formatBoon(r.rows[0].keys[0], r.rows[0].values[0]), "3.3");
  assert.strictEqual(C.formatBoon(r.rows[1].keys[0], r.rows[1].values[0]), "33%");
  // Uptime averaged over the 3 players; generation summed.
  const round = v => Math.round(v * 1000) / 1000;
  assert.deepStrictEqual(r.rows[1].values.map(round), [33.333, 60]);
  assert.deepStrictEqual(r.rows[2].values.map(round), [33.333, 0]);
  assert.strictEqual(r.footer, "3 boons");
  // All 13 boons can show.
  const all = ps.slice(0, 1).map(p => B(Object.fromEntries(C.SQUAD_BOONS.map((b, i) => [b, [1000 * (i + 1), 0, 0, 0]])), { name: "A", me: true }));
  const r13 = computeRows(mk({ rows: "boons", stats: [{ key: "boons:up" }] }), { players: all, seconds: 100 }, String, 0);
  assert.deepStrictEqual(r13.rows.map(x => x.p.boon), C.SQUAD_BOONS);
  // "who" picks the players: group 1 only.
  const g = computeRows(mk({ rows: "boons", stats: [{ key: "boons:up" }], who: { base: "group" } }), { players: ps, seconds: 10 }, String, 5);
  // Might in stacks: 10 stacks on A, 0 on B.
  assert.deepStrictEqual(g.rows.map(x => [x.p.name, x.values[0]]), [["Might", 5], ["Quickness", 50], ["Alacrity", 50]]);
  // Nothing to show: no rows, no NaN.
  const e = computeRows(spec, { players: [], seconds: 0 }, String, 5);
  assert.deepStrictEqual(e.rows, []);
  assert.strictEqual(e.footer, "0 boons");
  // A players window of per-boon stats: one row per player.
  const pr = computeRows(mk({ stats: [{ key: "boon:1187:up" }], sort: { key: "boon:1187:up" } }), { players: ps, seconds: 10 }, String, 5);
  assert.deepStrictEqual(pr.rows.map(x => [x.p.name, x.values[0]]), [["A", 100], ["B", 0], ["C", 0]]);
  // The total row: uptime averaged, generation summed.
  const t = computeRows(mk({ rows: "total", stats: [{ key: "boon:1187:up" }, { key: "boon:1187:group" }, { key: "damage" }] }), { players: ps, seconds: 10 }, String, 5);
  assert.deepStrictEqual(t.rows[0].values.map(round), [33.333, 60, 0]);
});

test("rows_boons_sort_and_limit", () => {
  const ps = [B({ 740: [20000, 0, 0, 0], 725: [3000, 0, 9000, 0], 1187: [8000, 0, 1000, 0], 718: [5000, 0, 4000, 0] }, { name: "A", me: true })];
  const v = { players: ps, seconds: 10 };
  const order = o => computeRows(mk(Object.assign({ rows: "boons", stats: [{ key: "boons:up" }, { key: "boons:group" }] }, o)), v, String, 0).rows.map(x => x.p.name);
  assert.deepStrictEqual(order({}), ["Might", "Fury", "Quickness", "Regeneration"]);
  // By a measure (highest first unless asc), by name, and limited.
  assert.deepStrictEqual(order({ sort: { key: "boons:group" } }), ["Fury", "Regeneration", "Quickness", "Might"]);
  // Might's uptime is in stacks (2), so it sorts with the numbers as shown.
  assert.deepStrictEqual(order({ sort: { key: "boons:up", asc: true } }), ["Might", "Fury", "Regeneration", "Quickness"]);
  assert.deepStrictEqual(order({ sort: { key: "name", asc: true } }), ["Fury", "Might", "Quickness", "Regeneration"]);
  assert.deepStrictEqual(order({ sort: { key: "boons:group" }, limit: 2 }), ["Fury", "Regeneration"]);
  assert.deepStrictEqual(order({ limit: 1 }), ["Might"]);
  // limit null falls back to maxRows.
  assert.strictEqual(computeRows(mk({ rows: "boons", stats: [{ key: "boons:up" }] }), v, String, 3).rows.length, 3);
  // A sort key that is not one of the window's measures: SQUAD_BOONS order.
  assert.strictEqual(mk({ rows: "boons", stats: [{ key: "boons:up" }], sort: { key: "boons:self" } }).sort.key, null);
  assert.strictEqual(mk({ rows: "boons", stats: [{ key: "boons:up" }], sort: { key: "damage" } }).sort.key, null);
  assert.strictEqual(mk({ rows: "boons", stats: [{ key: "boons:up" }], sort: { key: "name" } }).sort.key, "name");
});

test("normalize_drops_unknown_boon_keys", () => {
  const s = mk({ stats: [{ key: "boon:9999:up" }, { key: "boon:740:foo" }, { key: "boon:740:up" }, { key: "boon:1187" }, { key: "damage" }] });
  assert.deepStrictEqual(s.stats.map(x => x.key), ["boon:740:up", "damage"]);
  // rows boons keeps only the measure keys (per-boon keys and fight stats go), and is a table.
  const b = mk({ rows: "boons", look: "bars", stats: [{ key: "damage" }, { key: "boon:1187:squad" }, { key: "boons:squad" }, { key: "boons:nope" }, { key: "boons:up" }] });
  assert.deepStrictEqual(b.stats.map(x => x.key), ["boons:squad", "boons:up"]);
  assert.strictEqual(b.look, "table");
  // No measure left: uptime.
  assert.deepStrictEqual(mk({ rows: "boons", stats: [{ key: "boon:1187:up" }] }).stats, [{ key: "boons:up", as: "total" }]);
  // The measure keys only belong to boons windows; per-boon keys stay valid in players and total windows.
  assert.deepStrictEqual(mk({ rows: "players", stats: [{ key: "boons:up" }, { key: "boon:30328:group" }] }).stats.map(x => x.key), ["boon:30328:group"]);
  assert.deepStrictEqual(mk({ rows: "total", stats: [{ key: "boons:group" }, { key: "boon:30328:group" }] }).stats.map(x => x.key), ["boon:30328:group"]);
  assert.deepStrictEqual(mk({ stats: [{ key: "boons:up" }] }).stats, [{ key: "damage", as: "total" }]);
  assert.deepStrictEqual(C.BOON_MEASURE_KEYS, ["boons:up", "boons:self", "boons:group", "boons:squad"]);
  assert.deepStrictEqual(C.BOON_MEASURE_KEYS.map(k => C.STATS[k].label), ["Uptime", "Self", "Group", "Squad"]);
  assert.ok(!C.BASE_KEYS.some(k => k.startsWith("boons:")));
  for (const b of C.SQUAD_BOONS) for (const m of ["up", "self", "group", "squad"]) assert.ok(C.STATS[`boon:${b}:${m}`], `${b} ${m}`);
});

test("share_not_offered_for_boons", () => {
  for (const as of ["share", "per_s", "total", "x"]) {
    assert.strictEqual(mk({ stats: [{ key: "boon:1187:group", as }] }).stats[0].as, "total", as);
    assert.strictEqual(mk({ rows: "boons", stats: [{ key: "boons:group", as }] }).stats[0].as, "total", as);
  }
  assert.strictEqual(C.STATS["boons:group"].noAs, true);
  assert.strictEqual(C.STATS["boon:1187:group"].noAs, true);
  assert.strictEqual(C.STATS.damage.noAs, undefined);
});

test("history last:N with sub-second feeds divides by the span it covers", () => {
  // A feed every 500 ms at x.25 and x.75 s: 1000 damage per second, Quickness always up.
  const h = createHistory();
  for (let ms = 250; ms <= 30750; ms += 500) h.push(feed("fight", ms, [{ name: "A", damage: ms, boons: { 1187: [ms, 0, 0, 0] } }]));
  const v = h.view("last:10");
  const a = byName(v, "A");
  assert.ok(Math.abs(a.dps - 1000) < 1e-6, `dps ${a.dps}`);
  assert.ok(Math.abs(statValue(a, "boon:1187:up", v.seconds, false) - 100) < 1e-6, `uptime ${statValue(a, "boon:1187:up", v.seconds, false)}`);
  const r = computeRows(mk({ rows: "boons", stats: [{ key: "boons:up" }] }), v, String, 0);
  assert.ok(Math.abs(r.rows[0].values[0] - 100) < 1e-6, `boons row ${r.rows[0].values[0]}`);
  // The span covered: N up to a second more, and the time since the fight started when shorter.
  assert.strictEqual(v.seconds, 10.5);
  const early = createHistory();
  for (let ms = 250; ms <= 4750; ms += 500) early.push(feed("fight", ms, [{ name: "A", damage: ms }]));
  assert.strictEqual(early.view("last:10").seconds, 4.5);
  assert.ok(Math.abs(byName(early.view("last:10"), "A").dps - 1000) < 1e-6);
});

test("chart look drops boon stats", () => {
  const s = normalizeSpec({ look: "chart", stats: [{ key: "boon:1187:up" }, { key: "dps" }, { key: "boon:740:squad" }] }, "c1");
  assert.deepStrictEqual(s.stats.map(x => x.key), ["dps"]);
  const only = normalizeSpec({ look: "chart", stats: [{ key: "boon:1187:up" }] }, "c1");
  assert.deepStrictEqual(only.stats, [{ key: "damage", as: "total" }]);
  assert.strictEqual(only.sort.key, "damage");
  // Bars and number keep them.
  for (const look of ["bars", "number", "table"]) {
    assert.deepStrictEqual(normalizeSpec({ look, stats: [{ key: "boon:1187:up" }] }, "c1").stats.map(x => x.key), ["boon:1187:up"]);
  }
});
