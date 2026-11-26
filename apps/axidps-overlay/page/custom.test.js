const test = require("node:test");
const assert = require("node:assert");
const C = require("./custom.js");
const { normalizeSpec, normalizeAll, migrateTables, specFromBuiltin, blankSpec } = C;

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
