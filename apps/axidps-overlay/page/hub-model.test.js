const test = require("node:test");
const assert = require("node:assert");
const H = require("./hub-model.js");

const item = (id, kind, open, extra) => Object.assign({id, key: id, name: id[0].toUpperCase() + id.slice(1), note: "", kind, open}, extra);

test("tabFor maps old tabs", () => {
  for (const t of ["windows", "tables"]) assert.strictEqual(H.tabFor(t), "windows");
  for (const t of ["store", "plugins"]) assert.strictEqual(H.tabFor(t), "plugins");
  for (const t of ["appearance", "settings", "updates", "options"]) assert.strictEqual(H.tabFor(t), "options");
  assert.strictEqual(H.tabFor("nope"), "windows");
});

test("hubSize clamps to the screen", () => {
  assert.deepStrictEqual(H.hubSize([2560, 1440]), [1040, 680]);
  assert.deepStrictEqual(H.hubSize([1280, 720]), [1040, 640]);
  assert.deepStrictEqual(H.hubSize([1024, 600]), [944, 520]);
  assert.deepStrictEqual(H.hubSize([400, 300]), [480, 360]);
});

test("windowList orders by kind and splits hidden", () => {
  const r = H.windowList([item("meter", "builtin", true), item("p1", "plugin", true),
    item("c1", "custom", true), item("fight", "builtin", false)], []);
  assert.deepStrictEqual(r.onScreen.map(x => x.id), ["meter", "c1", "p1"]);
  assert.deepStrictEqual(r.hidden.map(x => x.id), ["fight"]);
  assert.strictEqual(r.onScreen[0].joinedTo, null);
  assert.strictEqual(r.onScreen[0].depth, 0);
});

test("windowList nests joined windows under the group head", () => {
  const items = [item("support", "builtin", true), item("fight", "builtin", true), item("meter", "builtin", true)];
  const groups = [{id: 1, cells: [{k: "support", r: 0, c: 1}, {k: "meter", r: 0, c: 0}]}];
  const r = H.windowList(items, groups).onScreen;
  assert.deepStrictEqual(r.map(x => x.id), ["fight", "meter", "support"]);
  assert.strictEqual(r[2].depth, 1);
  assert.strictEqual(r[2].joinedTo, "Meter");
  assert.strictEqual(r[2].note, "Joined to Meter");
  assert.strictEqual(r[1].depth, 0);
});

test("showBlock gives the reason", () => {
  assert.strictEqual(H.showBlock({open: false, openCount: 14, cap: 14, atlasFull: false}),
    "axidps can show 14 windows at once. Hide one first.");
  assert.strictEqual(H.showBlock({open: false, openCount: 3, cap: 14, atlasFull: true}), "No room for more windows");
  assert.strictEqual(H.showBlock({open: true, openCount: 14, cap: 14, atlasFull: true}), "");
  assert.strictEqual(H.showBlock({open: false, openCount: 3, cap: 14, atlasFull: false}), "");
});

test("moveItem moves without touching the input", () => {
  const a = ["a", "b", "c"];
  assert.deepStrictEqual(H.moveItem(a, 0, 2), ["b", "c", "a"]);
  assert.deepStrictEqual(a, ["a", "b", "c"]);
  const b = ["a"];
  const out = H.moveItem(b, 0, 5);
  assert.deepStrictEqual(out, ["a"]);
  assert.notStrictEqual(out, b);
});

test("selected falls back to the meter", () => {
  const rows = {onScreen: [{id: "meter"}, {id: "c1"}], hidden: [{id: "fight"}]};
  assert.strictEqual(H.selected("c1", rows), "c1");
  assert.strictEqual(H.selected("fight", rows), "fight");
  assert.strictEqual(H.selected("gone", rows), "meter");
});
