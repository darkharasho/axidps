const test = require("node:test");
const assert = require("node:assert");
const C = require("./custom.js");
const { filterWho, computeRows, statValue, normalizeSpec, normalizeAll, migrateTables, specFromBuiltin, blankSpec } = C;

test("catalog shape", () => {
  assert.strictEqual(Object.keys(C.STATS).length, 17);
  assert.strictEqual(C.BASE_KEYS.length, 15);
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
test("history budget", () => {
  const h = createHistory();
  h.track(new Set(["damage", "healing"]));
  const keys = C.BASE_KEYS;
  const players = ms => Array.from({ length: 50 }, (_, i) => {
    const p = { name: "P" + i, group: 1 + (i % 10), me: i === 0 };
    keys.forEach((k, j) => { p[k] = (ms / 1000) * (i + j + 1); });
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
  const render = () => { for (const s of specs) computeRows(s, h.view(s.span), String, 50); };
  for (let i = 0; i < 20; i++) render();
  const t = performance.now();
  for (let i = 0; i < 200; i++) render();
  const mean = (performance.now() - t) / 200;
  assert.ok(mean < 1, `mean ${mean} ms`);
});
test("history returning player counts from inside the span", () => {
  const h = createHistory();
  fightTo(h, 30000, ms => ms >= 19000 && ms <= 21000 ? [{ name: "A", damage: ms }]
    : [{ name: "A", damage: ms }, { name: "B", damage: ms }]);
  const v = h.view("last:10");
  assert.strictEqual(byName(v, "A").damage, 10000);
  assert.strictEqual(byName(v, "B").damage, 8500);
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
