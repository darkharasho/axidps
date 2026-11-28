const test = require("node:test");
const assert = require("node:assert");
const { kit, makeSdk, text } = require("./axidps.js");

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

test("players_reach_handlers_and_late_handlers_get_latest", () => {
  const w = fakeWin();
  const sdk = makeSdk(w);
  let got = null;
  sdk.on("players", p => { got = p; });
  w.fromPage({ axidps: 1, type: "players", players: { x: 1 } });
  assert.deepStrictEqual(got, { x: 1 });
  let late = null;
  sdk.on("players", p => { late = p; });
  assert.deepStrictEqual(late, { x: 1 });
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

// ---- text size (Settings) ----

test("text size steps: labels and values, Normal the default", () => {
  assert.deepStrictEqual(text.SIZES, [[80, "Smallest"], [90, "Small"], [100, "Normal"], [115, "Large"],
    [130, "Larger"], [150, "Largest"]]);
});

test("a stored text size round-trips; unknown or missing falls back to 100", () => {
  for (const [v] of text.SIZES) {
    // saved to ui.json and read back
    assert.strictEqual(text.size(JSON.parse(JSON.stringify({textSize: v})).textSize), v);
  }
  for (const bad of [undefined, null, 0, 1, 1.15, 95, 200, "big", "", "115", true, {}, [], NaN, -80]) {
    assert.strictEqual(text.size(bad), 100, String(bad));
  }
});

const MID = {fs: 12, rh: 21, bh: 32};
test("Normal text keeps today's sizes, compact or not", () => {
  // Without compact: the theme's rows and bar.
  assert.deepStrictEqual(text.metrics(MID, 1, false), {rh: 21, bh: 32, hh: 22, fh: 22, tbh: 22});
  assert.deepStrictEqual(text.metrics({fs: 11.5, rh: 18, bh: 26}, 1, false), {rh: 18, bh: 26, hh: 22, fh: 22, tbh: 22});
  // Compact: today's 16 px rows, 22 px bar, 16 px header and footer.
  assert.deepStrictEqual(text.metrics(MID, 1, true), {rh: 16, bh: 22, hh: 16, fh: 16, tbh: 22});
  assert.strictEqual(text.col(78, 1, false), 78);
  assert.strictEqual(text.box(250, 1, false), 250);
  assert.strictEqual(text.box(250, 1, true), 200);
});

// A line of f px text is f * 1.3 tall (the page's line-height).
const fits = (m, s, fs) => {
  assert.ok(m.rh >= Math.ceil(fs * s * 1.3 - 1e-6), `row ${m.rh} for ${fs * s}px`);
  assert.ok(m.hh >= Math.ceil(10.5 * s * 1.3 - 1e-6), `header ${m.hh}`);
  assert.ok(m.fh >= Math.ceil(11 * s * 1.3 - 1e-6), `footer ${m.fh}`);
  assert.ok(m.bh >= Math.ceil(fs * s * 1.3 - 1e-6) && m.bh >= m.tbh, `bar ${m.bh}`);
};
test("rows at the smallest and largest steps, compact off and on", () => {
  const small = text.metrics(MID, 0.8, false), large = text.metrics(MID, 1.5, false);
  const smallC = text.metrics(MID, 0.8, true), largeC = text.metrics(MID, 1.5, true);
  assert.deepStrictEqual(small, {rh: 18, bh: 29, hh: 19, fh: 19, tbh: 18});
  assert.deepStrictEqual(large, {rh: 29, bh: 40, hh: 29, fh: 29, tbh: 33});
  assert.deepStrictEqual(smallC, {rh: 13, bh: 19, hh: 13, fh: 13, tbh: 18});
  assert.deepStrictEqual(largeC, {rh: 24, bh: 33, hh: 23, fh: 23, tbh: 33});
  fits(small, 0.8, 12); fits(large, 1.5, 12); fits(smallC, 0.8, 12); fits(largeC, 1.5, 12);
  // Compact is always tighter, never taller.
  for (const k of ["rh", "bh", "hh", "fh"]) {
    assert.ok(smallC[k] < small[k] && largeC[k] < large[k], k);
  }
  // Every step and theme size fits its text.
  for (const [v] of text.SIZES) for (const st of [MID, {fs: 11.5, rh: 18, bh: 26}, {fs: 13, rh: 21, bh: 30}]) {
    fits(text.metrics(st, v / 100, false), v / 100, st.fs);
    fits(text.metrics(st, v / 100, true), v / 100, st.fs);
  }
});

test("columns at the smallest and largest steps, compact off and on", () => {
  // A 78 px column: 66 px of text and 6 px padding a side (4 in compact).
  assert.strictEqual(text.col(78, 0.8, false), 65);
  assert.strictEqual(text.col(78, 1.5, false), 111);
  assert.strictEqual(text.col(78, 0.8, true), 61);
  assert.strictEqual(text.col(78, 1.5, true), 107);
  assert.strictEqual(text.col(78, 1, true), 74);
  // The text part always scales with the text.
  for (const [v] of text.SIZES) for (const c of [false, true]) {
    assert.ok(text.col(78, v / 100, c) - (c ? 8 : 12) >= Math.floor(66 * v / 100), `${v} ${c}`);
  }
  // Boxes (names, bars, charts): with the text, compact 80%.
  assert.strictEqual(text.box(170, 0.8, false), 136);
  assert.strictEqual(text.box(170, 1.5, false), 255);
  assert.strictEqual(text.box(170, 0.8, true), 109);
  assert.strictEqual(text.box(170, 1.5, true), 204);
});

// A frame's document that records its CSS variables and body classes.
function lookDoc(w) {
  const vars = {}, cls = new Set();
  w.document = { documentElement: { style: { setProperty: (k, v) => { vars[k] = v; } } },
    body: { getBoundingClientRect: () => ({ width: 300, height: 40 }),
      classList: { toggle: (c, on) => { if (on) cls.add(c); else cls.delete(c); } } } };
  return { vars, cls };
}
const LOOK = { axidps: 1, type: "look", theme: "midnight", tokens: {}, style: { fs: 12, rh: 21, bh: 32 },
  opacity: 1, compact: false, rows: 15 };

test("the text size reaches plugins in look, as textScale and the CSS variables", () => {
  const w = fakeWin();
  const { vars, cls } = lookDoc(w);
  const sdk = makeSdk(w);
  let got = null;
  sdk.on("look", l => { got = l; });
  w.fromPage(Object.assign({}, LOOK, { textScale: 1.5 }));
  assert.strictEqual(got.textScale, 1.5);
  assert.strictEqual(got.compact, false);
  assert.strictEqual(vars["--text-scale"], "1.5");
  assert.strictEqual(vars["--fs"], "18px");
  assert.strictEqual(vars["--rh"], "29px");
  assert.strictEqual(vars["--bh"], "40px");
  w.fromPage(Object.assign({}, LOOK, { compact: true, textScale: 0.8 }));
  assert.strictEqual(got.textScale, 0.8);
  assert.ok(cls.has("compact"));
  assert.strictEqual(vars["--fs"], "9.6px");
  assert.strictEqual(vars["--rh"], "13px");
  assert.strictEqual(vars["--bh"], "19px");
});

test("a look without textScale (an older page) is Normal, and compact keeps the normal text size", () => {
  const w = fakeWin();
  const { vars } = lookDoc(w);
  const sdk = makeSdk(w);
  let got = null;
  sdk.on("look", l => { got = l; });
  w.fromPage(LOOK);
  assert.strictEqual(got.textScale, 1);
  assert.strictEqual(vars["--fs"], "12px");
  assert.strictEqual(vars["--rh"], "21px");
  w.fromPage(Object.assign({}, LOOK, { compact: true, textScale: 7 }));
  assert.strictEqual(got.textScale, 1, "not a step: Normal");
  assert.strictEqual(vars["--fs"], "12px");
  assert.strictEqual(vars["--rh"], "16px");
  assert.strictEqual(vars["--bh"], "22px");
});

test("a page in the hub's Options keeps the hub's text size", () => {
  const w = fakeWin();
  const { vars } = lookDoc(w);
  const sdk = makeSdk(w);
  let got = null;
  sdk.on("look", l => { got = l; });
  w.fromPage({ axidps: 1, type: "frame", clear: false, page: true });
  w.fromPage(Object.assign({}, LOOK, { textScale: 1.3 }));
  assert.strictEqual(got.textScale, 1.3, "the plugin still hears the setting");
  assert.strictEqual(vars["--text-scale"], "1");
  assert.strictEqual(vars["--fs"], "12px");
  // Back in a window: the meter size.
  w.fromPage({ axidps: 1, type: "frame", clear: false, page: false });
  assert.strictEqual(vars["--fs"], "15.6px");
});
