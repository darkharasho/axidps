const test = require("node:test");
const assert = require("node:assert");
const { kit, makeSdk } = require("./axidps.js");

test("fightStatus texts", () => {
  assert.deepStrictEqual(kit.fightStatus(null), { text: "waiting for core", live: false });
  assert.deepStrictEqual(kit.fightStatus({ state: "fight", cleared: true }), { text: "cleared", live: false });
  assert.deepStrictEqual(kit.fightStatus({ state: "fight", fight: { duration_ms: 83000 } }),
    { text: "in combat 1:23", live: true });
  assert.deepStrictEqual(kit.fightStatus({ state: "last", fight: { duration_ms: 5000 } }),
    { text: "last fight 0:05", live: false });
  assert.deepStrictEqual(kit.fightStatus({ state: "idle" }), { text: "out of combat", live: false });
});

test("emptyText texts", () => {
  assert.strictEqual(kit.emptyText(null), "Waiting for axidps core.");
  assert.strictEqual(kit.emptyText({ state: "fight", cleared: true }), "Cleared. The next fight starts fresh.");
  assert.strictEqual(kit.emptyText({ state: "idle" }), "No fight yet.");
  assert.strictEqual(kit.emptyText({ state: "fight" }), "No squad data for this fight yet.");
});

test("kit helpers", () => {
  assert.strictEqual(kit.fmt(12345), "12.3k");
  assert.strictEqual(kit.fmt(2500000), "2.50M");
  assert.strictEqual(kit.clock(61000), "1:01");
  assert.strictEqual(kit.esc('<a href="x">&'), "&lt;a href=&quot;x&quot;&gt;&amp;");
  assert.strictEqual(kit.profColor(1), "#72c1d9");
  assert.strictEqual(kit.profColor(99), kit.PROF[0]);
});

// A frame's window as far as the SDK needs one, with no DOM.
function fakeWin() {
  const posted = [], listeners = {};
  const parent = { postMessage: (m, origin) => posted.push({ m, origin }) };
  const w = {
    parent, posted,
    console: { log() {}, warn() {}, error() {} },
    addEventListener: (t, f) => (listeners[t] = listeners[t] || []).push(f),
    fire: (t, e) => (listeners[t] || []).forEach(f => f(e)),
    fromPage: data => w.fire("message", { source: parent, data }),
  };
  return w;
}
const msgs = (w, type) => w.posted.map(p => p.m).filter(m => m.type === type);

test("sdk says hello, then ready and fight reach the plugin", () => {
  const w = fakeWin();
  const sdk = makeSdk(w);
  assert.strictEqual(msgs(w, "hello").length, 1);
  assert.strictEqual(w.posted[0].origin, "*");
  let info = null, feed = null;
  sdk.ready(i => { info = i; });
  sdk.on("fight", f => { feed = f; });
  w.fromPage({ axidps: 1, type: "ready", plugin: "x.y", window: "card", api: 1, permissions: ["fight"] });
  assert.deepStrictEqual(info, { plugin: "x.y", window: "card", api: 1, permissions: ["fight"] });
  w.fromPage({ axidps: 1, type: "fight", feed: { state: "fight", players: [] } });
  assert.deepStrictEqual(feed, { state: "fight", players: [] });
  // ready after the fact still calls back
  let late = null;
  sdk.ready(i => { late = i; });
  assert.strictEqual(late.plugin, "x.y");
});

test("sdk perf reaches the plugin, and a late handler gets the latest", () => {
  const w = fakeWin();
  const sdk = makeSdk(w);
  let got = null;
  sdk.on("perf", p => { got = p; });
  w.fromPage({ axidps: 1, type: "perf", perf: { fps: 60, ping: 80, tick_rate: 25 } });
  assert.deepStrictEqual(got, { fps: 60, ping: 80, tick_rate: 25 });
  let late = null;
  sdk.on("perf", p => { late = p; });
  assert.deepStrictEqual(late, { fps: 60, ping: 80, tick_rate: 25 });
});

test("sdk specIcon", () => {
  const sdk = makeSdk(fakeWin());
  assert.strictEqual(sdk.specIcon("Firebrand"), "/_sdk/icons/Firebrand.png");
  assert.strictEqual(sdk.specIcon("Ü x"), "/_sdk/icons/%C3%9C%20x.png");
  assert.strictEqual(sdk.specIcon(""), "");
  assert.strictEqual(sdk.specIcon(null), "");
});

test("sdk ignores messages not from the page", () => {
  const w = fakeWin();
  const sdk = makeSdk(w);
  let feed = null;
  sdk.on("fight", f => { feed = f; });
  w.fire("message", { source: {}, data: { axidps: 1, type: "fight", feed: { state: "fight" } } });
  w.fromPage({ axidps: 2, type: "fight", feed: { state: "fight" } });
  assert.strictEqual(feed, null);
});

test("sdk settings", () => {
  const w = fakeWin();
  const sdk = makeSdk(w);
  let got = null;
  sdk.on("settings", v => { got = v; });
  w.fromPage({ axidps: 1, type: "settings", value: { big: true } });
  assert.deepStrictEqual(got, { big: true });
  assert.deepStrictEqual(sdk.settings.get(), { big: true });
  sdk.settings.set({ big: false });
  assert.deepStrictEqual(msgs(w, "settings").pop(), { axidps: 1, type: "settings", value: { big: false } });
  assert.deepStrictEqual(sdk.settings.get(), { big: false });
});

test("sdk settings.set over 16 KB logs an error and keeps the old value", () => {
  const w = fakeWin();
  const sdk = makeSdk(w);
  sdk.settings.set({ a: 1 });
  const before = msgs(w, "settings").length;
  sdk.settings.set({ s: "x".repeat(17000) });
  assert.strictEqual(msgs(w, "settings").length, before, "not sent");
  assert.deepStrictEqual(sdk.settings.get(), { a: 1 }, "the local value stays");
  const err = msgs(w, "log").pop();
  assert.strictEqual(err.level, "error");
  assert.match(err.msg, /settings\.set: \d+ bytes as JSON, at most 16384; not saved/);
  // Multi-byte text counts in bytes, as the page counts it.
  sdk.settings.set({ s: "é".repeat(8190) });
  assert.deepStrictEqual(sdk.settings.get(), { a: 1 });
  const cyclic = {}; cyclic.self = cyclic;
  sdk.settings.set(cyclic);
  assert.match(msgs(w, "log").pop().msg, /can't be saved as JSON/);
  sdk.settings.set({ s: "x".repeat(16000) });
  assert.strictEqual(sdk.settings.get().s.length, 16000, "just under the limit is saved");
});

test("sdk window calls", () => {
  const w = fakeWin();
  const sdk = makeSdk(w);
  sdk.window.setTitle("Hi");
  sdk.window.setStatus("in combat", { live: true });
  sdk.window.resize(100.2, 50.7);
  assert.deepStrictEqual(msgs(w, "title").pop(), { axidps: 1, type: "title", text: "Hi" });
  assert.deepStrictEqual(msgs(w, "status").pop(), { axidps: 1, type: "status", text: "in combat", live: true });
  assert.deepStrictEqual(msgs(w, "resize").pop(), { axidps: 1, type: "resize", w: 101, h: 51 });
  const el = { dataset: {} };
  sdk.window.tip(el, "a tip");
  assert.strictEqual(el.dataset.tip, "a tip");
});

test("sdk answers pings", () => {
  const w = fakeWin();
  makeSdk(w);
  w.fromPage({ axidps: 1, type: "ping", n: 7 });
  assert.deepStrictEqual(msgs(w, "pong").pop(), { axidps: 1, type: "pong", n: 7 });
});

test("console.warn/error and errors become log messages", () => {
  const w = fakeWin();
  let orig = 0;
  w.console.error = () => { orig++; };
  makeSdk(w);
  w.console.error("x");
  assert.deepStrictEqual(msgs(w, "log").pop(), { axidps: 1, type: "log", level: "error", msg: "x" });
  assert.strictEqual(orig, 1, "the original console.error still runs");
  w.console.warn("a", 1, { b: 2 });
  assert.deepStrictEqual(msgs(w, "log").pop(), { axidps: 1, type: "log", level: "warn", msg: 'a 1 {"b":2}' });
  w.fire("error", { message: "boom", filename: "axidps-plugin://x.y/a.js", lineno: 3 });
  assert.match(msgs(w, "log").pop().msg, /boom/);
  w.fire("unhandledrejection", { reason: new Error("nope") });
  assert.match(msgs(w, "log").pop().msg, /nope/);
  w.console.log("quiet");
  assert.ok(!msgs(w, "log").some(m => m.msg === "quiet"));
});

test("tipPos keeps the tooltip inside an autosized 150x50 frame", () => {
  const { tipPos } = require("./axidps.js");
  const box = { left: 0, top: 0, width: 150, height: 50, bottom: 50 }; // the whole body
  // No room below or above: kept inside, overlapping the element (unclamped: y = -22).
  assert.deepStrictEqual(tipPos(box, 40, 20, 150, 50), [55, 2]);
  // A wide tip near the right edge stays in the frame.
  assert.deepStrictEqual(tipPos({ left: 120, top: 5, width: 30, height: 10, bottom: 15 }, 100, 20, 150, 50), [48, 17]);
  // Room below: under the element, centred.
  assert.deepStrictEqual(tipPos({ left: 50, top: 5, width: 50, height: 10, bottom: 15 }, 30, 20, 150, 200), [60, 17]);
  // No room below but room above: above it.
  assert.deepStrictEqual(tipPos({ left: 50, top: 150, width: 50, height: 40, bottom: 190 }, 30, 20, 150, 200), [60, 128]);
});

test("sdk textures", () => {
  const w = fakeWin();
  const sdk = makeSdk(w);
  assert.ok(sdk.textures.targets.includes("commander.red"));
  sdk.textures.set([{ target: "commander.red", color: "#d55e00" }, { target: "nope" }]);
  const last = () => w.posted[w.posted.length - 1].m;
  assert.deepStrictEqual(last(), { axidps: 1, type: "textures", rules: [{ target: "commander.red", color: "#d55e00" }] });
  sdk.textures.clear();
  assert.deepStrictEqual(last(), { axidps: 1, type: "textures", rules: [] });
  const n = w.posted.length;
  sdk.textures.set("nope");
  assert.strictEqual(w.posted[w.posted.length - 1].m.type, "log", "a bad call only logs");
  assert.strictEqual(w.posted.length, n + 1);
});

test("outline.set posts a copy, clear posts null, bad input logs", () => {
  const w = fakeWin();
  const sdk = makeSdk(w);
  const last = () => w.posted[w.posted.length - 1].m;
  const obj = { on: true };
  sdk.outline.set(obj);
  assert.deepStrictEqual(last(), { axidps: 1, type: "outline", settings: { on: true } });
  assert.notStrictEqual(last().settings, obj);
  sdk.outline.clear();
  assert.deepStrictEqual(last(), { axidps: 1, type: "outline", settings: null });
  const n = w.posted.length;
  sdk.outline.set([1]);
  assert.strictEqual(last().type, "log");
  assert.match(last().msg, /outline.set needs an object/);
  assert.strictEqual(w.posted.length, n + 1);
});

test("sdk marks the body for a see-through window or a hub page", () => {
  const w = fakeWin();
  const cls = new Set();
  w.document = { documentElement: { style: { setProperty() {} } },
    body: { getBoundingClientRect: () => ({ width: 300, height: 40 }),
      classList: { toggle: (c, on) => { if (on) cls.add(c); else cls.delete(c); } } } };
  const sdk = makeSdk(w);
  let got = null;
  sdk.on("frame", f => { got = f; });
  w.fromPage({ axidps: 1, type: "frame", clear: true, page: false });
  assert.deepStrictEqual([...cls].sort(), ["axidps-clear"]);
  assert.deepStrictEqual(got, { clear: true, page: false });
  w.fromPage({ axidps: 1, type: "frame", clear: false, page: true });
  assert.deepStrictEqual([...cls].sort(), ["axidps-page"]);
});
