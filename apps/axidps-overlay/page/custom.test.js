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
