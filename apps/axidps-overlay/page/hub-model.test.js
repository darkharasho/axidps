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

const sc = (id, action, extra) => Object.assign({id, title: id, author: "au", version: "1.0.0", description: "d",
  trust: "verified", uses: [{text: "Reads fights.", deep: false}], action, note: "", bad: false, busy: false}, extra);
const inst = (id, bundled, extra) => ({id, bundled: !!bundled, card: Object.assign({title: id, author: "", version: "0.1.0",
  lines: ["Reads fights."], action: "disable", reason: "", label: "", reload: false}, extra)});

test("pluginRows merges an installed store plugin into one row", () => {
  const {rows} = H.pluginRows([sc("a.b", "installed")], [inst("a.b")], "all", "");
  assert.strictEqual(rows.length, 1);
  assert.strictEqual(rows[0].toggle, "disable");
  assert.strictEqual(rows[0].store, "installed");
  assert.strictEqual(rows[0].remove, true);
  assert.strictEqual(rows[0].local, false);
});

test("a local plugin gets no Remove", () => {
  const {rows} = H.pluginRows([], [inst("mine")], "all", "");
  assert.strictEqual(rows[0].local, true);
  assert.strictEqual(rows[0].remove, false);
  assert.deepStrictEqual(rows[0].uses, [{text: "Reads fights.", deep: false}]);
});

test("a store plugin not installed has no toggle", () => {
  const {rows} = H.pluginRows([sc("a.b", "install")], [], "all", "");
  assert.strictEqual(rows[0].toggle, null);
  assert.strictEqual(rows[0].store, "install");
  assert.strictEqual(rows[0].remove, false);
});

test("counts match the rows under each filter", () => {
  const cards = [sc("up", "update"), sc("in", "installed"), sc("new", "install")];
  const list = [inst("up"), inst("in"), inst("mine")];
  const r = H.pluginRows(cards, list, "all", "");
  assert.deepStrictEqual(r.counts, {all: 4, installed: 3, updates: 1});
  assert.deepStrictEqual(r.rows.map(x => x.id), ["up", "in", "new", "mine"]);
  assert.strictEqual(H.pluginRows(cards, list, "installed", "").rows.length, 3);
  assert.deepStrictEqual(H.pluginRows(cards, list, "updates", "").rows.map(x => x.id), ["up"]);
  assert.deepStrictEqual(H.pluginRows(cards, list, "updates", "").counts, r.counts);
});

test("query filters local rows by name", () => {
  const list = [inst("alpha", false, {title: "Alpha"}), inst("beta", false, {title: "Beta"})];
  assert.deepStrictEqual(H.pluginRows([], list, "all", "alp").rows.map(x => x.id), ["alpha"]);
});

test("unverified store rows are unchecked", () => {
  const {rows} = H.pluginRows([sc("x", "install", {trust: "unverified"}), sc("y", "install")], [], "all", "");
  assert.strictEqual(rows[0].unchecked, true);
  assert.strictEqual(rows[1].unchecked, false);
});

test("bundled plugins are not local and credit axidps", () => {
  const {rows} = H.pluginRows([], [inst("core", true)], "all", "");
  assert.strictEqual(rows[0].local, false);
  assert.strictEqual(rows[0].author, "axidps");
  assert.strictEqual(rows[0].remove, false);
});

test("an offline store does not mark installed plugins local", () => {
  const {rows} = H.pluginRows([], [inst("a.b")], "all", "", false);
  assert.strictEqual(rows[0].local, false);
  assert.strictEqual(rows[0].remove, false);
  assert.strictEqual(rows[0].store, null);
  assert.strictEqual(H.pluginRows([], [inst("a.b")], "all", "", true).rows[0].local, true);
});
