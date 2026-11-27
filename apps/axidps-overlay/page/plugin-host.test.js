const test = require("node:test");
const assert = require("node:assert");
const ph = require("./plugin-host.js");

const win = (id, extra) => Object.assign({ id, title: id, entry: id + ".html", description: "", icon: "chart",
  open: false, place: "beside-meter" }, extra);
const plugin = (id, extra) => Object.assign({ id, name: id, version: "1.0.0", author: "a", api: 1,
  permissions: ["fight"], network: [], windows: [win("w")], bundled: false }, extra);

test("aliases: windows get p1, p2 in order, bundled or not", () => {
  const list = [
    plugin("axidps.perf", { bundled: true, windows: [win("perf")] }),
    plugin("someone.long-plugin-name-for-testing", { windows: [win("a-window-with-a-long-id"), win("b")] }),
  ];
  const ws = ph.validManifestWindows(list);
  assert.deepStrictEqual(ws.map(w => w.alias), ["p1", "p2", "p3"]);
  assert.deepStrictEqual(ws.map(w => w.fullId), ["axidps.perf/perf",
    "someone.long-plugin-name-for-testing/a-window-with-a-long-id", "someone.long-plugin-name-for-testing/b"]);
  for (const w of ws) assert.ok(Buffer.byteLength(w.alias) <= 16, w.alias);
  assert.strictEqual(ws[1].plugin.id, "someone.long-plugin-name-for-testing");
  assert.strictEqual(ws[1].win.id, "a-window-with-a-long-id");
});

test("validManifestWindows skips plugins with errors and disabled ones, aliases stay put", () => {
  const list = [plugin("a.one"), plugin("b.two", { error: "needs verification" }), plugin("c.three"), plugin("d.four")];
  const ws = ph.validManifestWindows(list, p => p.id !== "c.three");
  assert.deepStrictEqual(ws.map(w => [w.plugin.id, w.alias]), [["a.one", "p1"], ["d.four", "p3"]]);
});

test("validManifestWindows drops bad window ids and duplicates", () => {
  const ws = ph.validManifestWindows([plugin("a.one", { windows: [win("ok"), win("Bad/Id"), win("ok")] }),
    null, "junk", { id: "x" }]);
  assert.deepStrictEqual(ws.map(w => w.fullId), ["a.one/ok"]);
});

test("isEnabled", () => {
  const b = plugin("axidps.fight", { bundled: true });
  assert.strictEqual(ph.isEnabled(b, {}), true, "bundled default on");
  assert.strictEqual(ph.isEnabled(b, { "axidps.fight": { enabled: false } }), false);
  const p = plugin("x.y");
  assert.strictEqual(ph.isEnabled(p, {}), false, "player plugin default off");
  assert.strictEqual(ph.isEnabled(p, undefined), false);
  const saved = { "x.y": { enabled: true, granted: ["fight"] } };
  assert.strictEqual(ph.isEnabled(p, saved), true, "enabled with every grant");
  const more = plugin("x.y", { network: ["x.com"] });
  assert.strictEqual(ph.isEnabled(more, saved), false, "the manifest gained net:x.com");
  assert.strictEqual(ph.isEnabled(plugin("x.y", { error: "needs verification" }), saved), false);
});

test("grantsOf and permissionLines", () => {
  const p = plugin("x.y", { network: ["api.example.com", "*.cdn.example.com"] });
  assert.deepStrictEqual(ph.grantsOf(p), ["fight", "net:api.example.com", "net:*.cdn.example.com"]);
  assert.deepStrictEqual(ph.permissionLines(p),
    ["Reads your fight data", "Talks to api.example.com", "Talks to *.cdn.example.com"]);
  assert.deepStrictEqual(ph.permissionLines(plugin("x.y", { permissions: ["fight", "world", "position", "squad", "enemies"] })),
    ["Reads your fight data", "Draws over the game world", "Reads your position and camera",
      "Reads your squad and party", "Reads enemy players seen in combat"]);
});

test("migrateSaved moves the plugin windows' position and open state back to the built-in ids, once", () => {
  const s = { pos: { fight: [1, 2], "axidps.fight/fight": [5, 6], meter: [3, 4] },
    open: { "axidps.healing/healing": true }, cfg: { "axidps.timeline/timeline": { a: 1 }, timeline: { b: 2 } } };
  const want = { pos: { fight: [5, 6], meter: [3, 4] }, open: { healing: true }, cfg: { timeline: { b: 2 } } };
  const once = ph.migrateSaved(s);
  assert.deepStrictEqual(once, want);
  // Nothing left to move: a window moved since stays where it is.
  once.pos.fight = [9, 9];
  assert.deepStrictEqual(ph.migrateSaved(once).pos.fight, [9, 9]);
  assert.deepStrictEqual(ph.migrateSaved({}), {});
});

test("clampSize", () => {
  assert.deepStrictEqual(ph.clampSize(5, 5000), [40, 1000]);
  assert.deepStrictEqual(ph.clampSize(2000, 3), [1000, 24]);
  assert.deepStrictEqual(ph.clampSize(100.2, 50.5), [100.2, 50.5]);
  assert.deepStrictEqual(ph.clampSize(39.5, 23.9), [40, 24]);
  assert.deepStrictEqual(ph.clampSize(NaN, "x"), [240, 120]);
});

test("accept", () => {
  const ok = [
    { axidps: 1, type: "hello" },
    { axidps: 1, type: "resize", w: 100, h: 50 },
    { axidps: 1, type: "title", text: "Hi" },
    { axidps: 1, type: "status", text: "in combat 1:00", live: true },
    { axidps: 1, type: "status", text: "" },
    { axidps: 1, type: "settings", value: { a: 1 } },
    { axidps: 1, type: "log", level: "warn", msg: "x" },
    { axidps: 1, type: "log", level: "error", msg: "x" },
    { axidps: 1, type: "pong", n: 3 },
  ];
  for (const m of ok) assert.strictEqual(ph.accept(m), true, JSON.stringify(m));
  const cyclic = {}; cyclic.self = cyclic;
  const bad = [
    { axidps: 2, type: "hello" },
    { axidps: 1, type: "nope" },
    { axidps: 1, type: "settings", value: { s: "x".repeat(20 * 1024) } },
    { axidps: 1, type: "settings", value: cyclic },
    { axidps: 1, type: "settings" },
    { axidps: 1, type: "title", text: "x".repeat(81) },
    { axidps: 1, type: "status", text: "x".repeat(81) },
    { axidps: 1, type: "status", text: "a", live: "yes" },
    { axidps: 1, type: "title", text: 5 },
    { axidps: 1, type: "resize", w: "100", h: 50 },
    { axidps: 1, type: "resize", w: Infinity, h: 50 },
    { axidps: 1, type: "log", level: "info", msg: "x" },
    { axidps: 1, type: "log", level: "warn", msg: 5 },
    { axidps: 1, type: "pong", n: "1" },
    null, undefined, "hello", 1, [], ["axidps", 1],
  ];
  bad.forEach((m, i) => assert.strictEqual(ph.accept(m), false, "bad #" + i));
  assert.strictEqual(ph.accept({ axidps: 1, type: "title", text: "x".repeat(80) }), true);
  assert.strictEqual(ph.accept({ axidps: 1, type: "settings", value: { s: "x".repeat(16000) } }), true);
});

test("logGate: 20 lines a second, the rest counted", () => {
  const gate = ph.logGate();
  let pass = 0, last = null;
  for (let i = 0; i < 25; i++) { last = gate(1000 + i * 10); if (last.pass) pass++; }
  assert.strictEqual(pass, 20);
  assert.strictEqual(last.dropped, 5);
  const next = gate(2100);
  assert.strictEqual(next.pass, true, "a new second lets lines through again");
  assert.strictEqual(next.dropped, 0);
});

test("resizeGate: a size change at once, then at most one per 250 ms", () => {
  const g = ph.resizeGate(250);
  assert.equal(g(1000), 0);
  assert.equal(g(1005), 245);
  assert.equal(g(1200), 50);
  assert.equal(g(1250), 0);
  assert.equal(g(1260), 240);
  assert.equal(g(2000), 0);
});

test("route: the sender is event.source only, never the body", () => {
  const a = {}, b = {}, entA = { plugin: { id: "a.one" } }, entB = { plugin: { id: "b.two" } };
  const map = new Map([[a, entA], [b, entB]]);
  const ev = { source: a, data: { axidps: 1, type: "log", level: "warn", msg: "x", plugin: "b.two", window: "w" } };
  assert.strictEqual(ph.route(map, ev), entA);
  assert.strictEqual(ph.route(map, { source: {}, data: ev.data }), null);
  assert.strictEqual(ph.route(map, { source: null, data: ev.data }), null);
});

test("overCap: windows past the cap, in order, count as closed", () => {
  const ws = [1, 2, 3, 4, 5, 6].map(i => ({ id: "w" + i, open: i !== 2 }));
  assert.deepStrictEqual(ph.overCap(ws, w => w.open, 3).map(w => w.id), ["w5", "w6"]);
  assert.deepStrictEqual(ph.overCap(ws, w => w.open, 14), []);
  // 16 table slots minus the hub and Arrange: 14 windows open by default, a 15th from a manifest.
  const many = Array.from({ length: 15 }, (_, i) => ({ id: "p" + i }));
  assert.deepStrictEqual(ph.overCap(many, () => true, ph.MAX_OPEN - 2).map(w => w.id), ["p14"]);
});

test("nameTaken: an alias or a full id that another window already uses is refused", () => {
  const wins = [{ id: "meter", key: "meter" }, { id: "fight", key: "axidps.fight/fight" }];
  assert.match(ph.nameTaken(wins, { alias: "meter", fullId: "x.y/meter" }), /name meter is taken/);
  // A player plugin `axidps.fight` (alias p1) would share the built-in Fight's saved state.
  assert.match(ph.nameTaken(wins, { alias: "p1", fullId: "axidps.fight/fight" }), /axidps.fight\/fight is taken/);
  assert.strictEqual(ph.nameTaken(wins, { alias: "p2", fullId: "x.y/card" }), "");
});

// A host for enable/disable: pluginHost with a fake env that records saves and host messages.
function fakeHost(list, saved) {
  const sent = [];
  let saves = 0;
  const h = Object.assign(Object.create(ph.host), {list, want: [], entries: [], sources: new Map()});
  // saveNow sends at once, as ui.js's does: the save lands in `sent` ahead of what follows it.
  h.env = {saved: () => saved, save: () => { saves++; }, saveNow: () => { saves++; sent.push(JSON.stringify({save: saved})); },
    toHost: t => sent.push(t), hosted: true,
    renderAll() {}, wins: [], defineWindow: () => ({el: {querySelector: () => null}}), removeWindow() {}};
  return {h, sent, saves: () => saves};
}

test("enable_then_isEnabled", () => {
  const p = plugin("x.y", { network: ["api.example.com"] });
  const saved = { plugins: {}, pos: {}, open: {}, cfg: {} };
  const { h, sent, saves } = fakeHost([p], saved);
  assert.strictEqual(ph.isEnabled(p, saved.plugins), false);
  h.enable("x.y");
  assert.deepStrictEqual(saved.plugins["x.y"], { enabled: true, granted: ["fight", "net:api.example.com"] });
  assert.strictEqual(ph.isEnabled(p, saved.plugins), true);
  assert.ok(saves() >= 1, "saved");
  // Saved at once, before the rescan: the host serves only what the saved enables say.
  assert.strictEqual(sent.length, 2);
  assert.deepStrictEqual(JSON.parse(sent[0]).save.plugins["x.y"], { enabled: true, granted: ["fight", "net:api.example.com"] });
  assert.strictEqual(sent[1], '{"plugins":"scan"}');
  // Hosted, the window waits for the list the rescan sends back.
  assert.deepStrictEqual(h.entries, []);
});

test("disable_keeps_pos", () => {
  const p = plugin("x.y", { windows: [win("a"), win("b")] });
  const saved = { plugins: { "x.y": { enabled: true, granted: ["fight"] } },
    pos: { "x.y/a": [1, 2], "x.y/b": [3, 4] }, open: { "x.y/a": true }, cfg: { "x.y/a": { big: true } } };
  const before = JSON.stringify({ pos: saved.pos, cfg: saved.cfg });
  const { h, sent } = fakeHost([p], saved);
  h.disable("x.y");
  assert.deepStrictEqual(saved.plugins["x.y"], { enabled: false, granted: ["fight"] });
  assert.strictEqual(ph.isEnabled(p, saved.plugins), false);
  assert.strictEqual(JSON.stringify({ pos: saved.pos, cfg: saved.cfg }), before);
  assert.strictEqual(sent.length, 2);
  assert.deepStrictEqual(JSON.parse(sent[0]).save.plugins["x.y"], { enabled: false, granted: ["fight"] });
  assert.strictEqual(sent[1], '{"plugins":"scan"}');
});

test("enable and disable ignore ids that aren't in the list", () => {
  const saved = { plugins: {}, pos: {}, open: {}, cfg: {} };
  const { h, sent } = fakeHost([plugin("x.y")], saved);
  h.enable("nope.z");
  h.disable("__proto__");
  assert.deepStrictEqual(saved.plugins, {});
  assert.deepStrictEqual(sent, []);
});

test("invalid_plugin_card_text", () => {
  const p = plugin("x.y", { name: "Big card", author: "Ann", version: "1.2.3", error: "windows: 5 given, at most 4" });
  const c = ph.cardModel(p, { plugins: {} });
  assert.strictEqual(c.title, "Big card");
  assert.strictEqual(c.author, "Ann");
  assert.strictEqual(c.version, "1.2.3");
  assert.strictEqual(c.reason, "windows: 5 given, at most 4");
  assert.strictEqual(c.action, null, "no Enable button");
  assert.strictEqual(c.reload, false);
});

test("deeper_card", () => {
  const p = plugin("x.y", { permissions: ["fight", "world"], error: "needs verification" });
  const c = ph.cardModel(p, { plugins: { "x.y": { enabled: true, granted: ["fight", "world"] } } });
  assert.strictEqual(c.reason, "Needs verification");
  assert.ok(c.lines.includes("Reads your fight data"));
  assert.ok(c.lines.includes("Draws over the game world"));
  assert.strictEqual(c.action, null);
  assert.strictEqual(ph.cardModel(plugin("x.y", { api: 2, error: "needs a newer axidps" }), {}).reason,
    "needs a newer axidps");
});

test("labels", () => {
  assert.strictEqual(ph.cardModel(plugin("axidps.fight", { bundled: true }), {}).label, null);
  assert.strictEqual(ph.cardModel(plugin("x.y"), {}).label, "Unverified");
  assert.strictEqual(ph.cardModel(plugin("x.y", { error: "needs verification" }), {}).label, "Unverified");
});

test("card actions: enable, disable, and the title falls back to the id", () => {
  const b = plugin("axidps.fight", { bundled: true, name: "" });
  assert.strictEqual(ph.cardModel(b, { plugins: {} }).action, "disable", "bundled default on");
  assert.strictEqual(ph.cardModel(b, { plugins: {} }).title, "axidps.fight");
  assert.strictEqual(ph.cardModel(b, { plugins: { "axidps.fight": { enabled: false } } }).action, "enable");
  const p = plugin("x.y");
  assert.strictEqual(ph.cardModel(p, {}).action, "enable");
  assert.strictEqual(ph.cardModel(p, { plugins: { "x.y": { enabled: true, granted: ["fight"] } } }).action, "disable");
  assert.strictEqual(ph.cardModel(p, {}).reason, "");
  assert.deepStrictEqual(ph.cardModel(p, {}).lines, ["Reads your fight data"]);
});

test("reload_only_in_dev", () => {
  const on = { "x.y": { enabled: true, granted: ["fight"] } };
  const p = plugin("x.y");
  assert.strictEqual(ph.cardModel(p, { dev: true, plugins: on }).reload, true);
  assert.strictEqual(ph.cardModel(p, { dev: false, plugins: on }).reload, false);
  assert.strictEqual(ph.cardModel(p, { plugins: on }).reload, false);
  assert.strictEqual(ph.cardModel(p, { dev: true, plugins: {} }).reload, false, "not enabled");
  assert.strictEqual(ph.cardModel(plugin("axidps.fight", { bundled: true }), { dev: true, plugins: {} }).reload, false,
    "bundled");
  assert.strictEqual(ph.cardModel(plugin("x.y", { error: "needs verification" }), { dev: true, plugins: on }).reload,
    false);
});

test("errorBadge: errors count, the last 5 kept, a reload resets, warnings don't count", () => {
  const err = msg => ({ type: "log", level: "error", msg });
  let s = ph.errorBadge(undefined, { type: "load" });
  assert.deepStrictEqual(s, { count: 0, last: [] });
  for (let i = 1; i <= 3; i++) s = ph.errorBadge(s, err("e" + i));
  assert.strictEqual(s.count, 3);
  assert.deepStrictEqual(s.last, ["e1", "e2", "e3"]);
  for (let i = 4; i <= 7; i++) s = ph.errorBadge(s, err("e" + i));
  assert.strictEqual(s.count, 7);
  assert.strictEqual(s.last.length, 5);
  assert.deepStrictEqual(s.last, ["e3", "e4", "e5", "e6", "e7"], "the newest");
  const before = JSON.stringify(s);
  const w = ph.errorBadge(s, { type: "log", level: "warn", msg: "w" });
  assert.strictEqual(JSON.stringify(w), before, "warn doesn't count");
  assert.strictEqual(JSON.stringify(s), before, "pure: the old state is untouched");
  assert.deepStrictEqual(ph.errorBadge(s, { type: "load" }), { count: 0, last: [] });
});

test("changed_keeps_state: only that plugin's frames reload, saved pos/open/cfg stay", () => {
  const saved = { plugins: {}, pos: { "x.y/a": [1, 2], "o.p/a": [3, 4] }, open: { "x.y/a": true, "o.p/a": true },
    cfg: { "x.y/a": { n: 1 }, "o.p/a": { n: 2 } } };
  const before = JSON.stringify(saved);
  const { h } = fakeHost([], saved);
  const frame = () => {
    const f = { loads: 0, _src: "" };
    Object.defineProperty(f, "src", { get() { return f._src; }, set(v) { f._src = v; f.loads++; } });
    return f;
  };
  const ent = (pid, wid) => ({ plugin: { id: pid }, win: { id: wid }, fullId: pid + "/" + wid, alias: "p",
    src: `axidps-plugin://${pid}/${wid}.html`, frame: frame(), w: null, hello: true, helloAt: 5, seen: 0,
    stalled: true, status: { text: "x", live: true }, badge: { count: 2, last: ["a", "b"] }, title: wid });
  h.entries = [ent("x.y", "a"), ent("o.p", "a"), ent("x.y", "b")];
  h.changed("x.y");
  assert.deepStrictEqual(h.entries.map(e => e.frame.loads), [1, 0, 1]);
  assert.strictEqual(h.entries[0].frame.src, "axidps-plugin://x.y/a.html");
  assert.strictEqual(h.entries[0].hello, false);
  assert.deepStrictEqual(h.entries[0].badge, { count: 0, last: [] }, "a reload resets the error count");
  assert.deepStrictEqual(h.entries[1].badge, { count: 2, last: ["a", "b"] }, "another plugin keeps its count");
  assert.strictEqual(JSON.stringify(saved), before);
});

test("reload: asks the host to rescan, then reloads that plugin's frames; unknown ids do nothing", () => {
  const { h, sent } = fakeHost([plugin("x.y")], { plugins: {}, pos: {}, open: {}, cfg: {} });
  const calls = [];
  h.changed = id => calls.push(id);
  h.reload("x.y");
  assert.deepStrictEqual(sent, ['{"plugins":"scan"}']);
  assert.deepStrictEqual(calls, ["x.y"]);
  h.reload("nope.z");
  assert.deepStrictEqual(sent, ['{"plugins":"scan"}']);
  assert.deepStrictEqual(calls, ["x.y"]);
});

test("mount: the frame's sandbox is exactly allow-scripts, and nothing else loosens it", () => {
  const attrs = {}, frame = { style: {}, contentWindow: {} };
  frame.setAttribute = (k, v) => { attrs[k] = String(v); };
  const made = [];
  const before = global.document;
  global.document = { createElement: tag => { made.push(tag); return frame; } };
  try {
    const { h } = fakeHost([], { plugins: {}, pos: {}, open: {}, cfg: {} });
    h.env.ui = { editing: false };
    const body = { children: [], appendChild(c) { this.children.push(c); } };
    const ent = { size: [240, 120], src: "axidps-plugin://x.y/a.html" };
    h.mount(ent, { body });
    assert.deepStrictEqual(made, ["iframe"]);
    assert.deepStrictEqual(attrs, { sandbox: "allow-scripts" });
    for (const k of ["sandbox", "allow", "allowFullscreen", "srcdoc", "credentialless"]) {
      assert.ok(!Object.prototype.hasOwnProperty.call(frame, k), k + " set as a property");
    }
    assert.strictEqual(frame.src, "axidps-plugin://x.y/a.html");
    assert.deepStrictEqual(body.children, [frame]);
    assert.strictEqual(h.sources.get(frame.contentWindow), ent);
  } finally {
    global.document = before;
  }
});

// Frame entries that record what is posted to them, on a host whose env has a feed, a look and
// open/closed windows.
function feedHost() {
  const { h } = fakeHost([], { plugins: {}, pos: {}, open: {}, cfg: {} });
  const state = { feed: { n: 1 }, look: { theme: "a" }, closed: new Set() };
  Object.assign(h.env, { ui: { editing: false }, feed: () => state.feed, look: () => state.look,
    isOpen: w => !state.closed.has(w) });
  const ent = (id, perms) => {
    const posted = [];
    const e = { plugin: { id, permissions: perms }, win: { id: "w" }, fullId: id + "/w", w: { id },
      cw: { postMessage: m => posted.push(m) }, hello: true, helloAt: 0, seen: 0, stalled: false,
      status: { text: "", live: false }, badge: { count: 0, last: [] }, gate: ph.logGate(), posted };
    h.entries.push(e);
    return e;
  };
  return { h, state, ent };
}
const kinds = e => e.posted.map(m => m.type);

test("fight and look go to answering, open frames only", () => {
  const { h, ent } = feedHost();
  const live = ent("a.live", ["fight"]), plain = ent("b.plain", []), early = ent("c.early", ["fight"]);
  early.hello = false;
  h.fight({ n: 2 });
  h.look({ theme: "b" });
  assert.deepStrictEqual(kinds(live), ["fight", "look"]);
  assert.deepStrictEqual(live.posted[0].feed, { n: 2 });
  assert.deepStrictEqual(kinds(plain), ["look"], "no fight permission, no feed");
  assert.deepStrictEqual(kinds(early), [], "before hello, nothing (hello gets the current state)");
});

test("a stalled frame gets nothing until it answers, then only the latest feed and look", () => {
  const { h, state, ent } = feedHost();
  const hung = ent("a.hung", ["fight"]);
  hung.stalled = true;
  for (let i = 0; i < 50; i++) { state.feed = { n: i }; h.fight(state.feed); h.look({ theme: "t" + i }); }
  assert.deepStrictEqual(kinds(hung), [], "nothing piles up in a hung frame");
  state.look = { theme: "latest" };
  state.feed = { n: 99 };
  h.handle(hung, { axidps: 1, type: "pong", n: 1 });
  assert.strictEqual(hung.stalled, false);
  assert.deepStrictEqual(kinds(hung), ["look", "fight"]);
  assert.deepStrictEqual(hung.posted[0], { axidps: 1, type: "look", theme: "latest" });
  assert.deepStrictEqual(hung.posted[1].feed, { n: 99 });
  h.handle(hung, { axidps: 1, type: "pong", n: 2 });
  assert.strictEqual(hung.posted.length, 2, "caught up once");
  h.fight({ n: 100 });
  assert.deepStrictEqual(kinds(hung), ["look", "fight", "fight"], "fed as usual again");
});

test("a closed window's frame catches up when it opens", () => {
  const { h, state, ent } = feedHost();
  const e = ent("a.closed", ["fight"]);
  state.closed.add(e.w);
  h.fight({ n: 5 });
  h.look({ theme: "x" });
  h.sync();
  assert.deepStrictEqual(kinds(e), []);
  state.closed.delete(e.w);
  state.feed = { n: 6 };
  h.sync();
  assert.deepStrictEqual(kinds(e), ["look", "fight"]);
  assert.deepStrictEqual(e.posted[1].feed, { n: 6 });
  h.sync();
  assert.strictEqual(e.posted.length, 2);
});

test("a frame that stalls while closed and never changed gets nothing extra", () => {
  const { h, ent } = feedHost();
  const e = ent("a.quiet", ["fight"]);
  e.stalled = true;
  h.handle(e, { axidps: 1, type: "pong", n: 1 });
  assert.deepStrictEqual(kinds(e), [], "nothing missed, nothing resent");
});

test("accept: textures rules are a bounded list", () => {
  assert.strictEqual(ph.accept({ axidps: 1, type: "textures", rules: [] }), true);
  assert.strictEqual(ph.accept({ axidps: 1, type: "textures", rules: [{ target: "mentor" }] }), true);
  assert.strictEqual(ph.accept({ axidps: 1, type: "textures", rules: {} }), false);
  assert.strictEqual(ph.accept({ axidps: 1, type: "textures", rules: Array(65).fill({}) }), false);
  assert.strictEqual(ph.accept({ axidps: 1, type: "textures", rules: [{ s: "x".repeat(17000) }] }), false);
});

test("mergeTextures: plugins in id order, junk dropped", () => {
  const merged = ph.mergeTextures({ "z.b": [{ target: "mentor", color: "#fff" }],
    "a.a": [{ target: "mentor", color: "#000" }, 5, null, { color: "#123" }] });
  assert.deepStrictEqual(merged, [{ target: "mentor", color: "#000" }, { target: "mentor", color: "#fff" }]);
});

test("textures: only plugins with the permission reach the host, and their rules go with them", () => {
  const { h, ent } = feedHost();
  const sent = [];
  h.env.toHost = t => sent.push(t);
  const styler = ent("a.tags", ["textures"]), other = ent("b.meter", ["fight"]);
  h.handle(other, { axidps: 1, type: "textures", rules: [{ target: "target" }] });
  assert.strictEqual(sent.length, 1);
  assert.match(sent[0], /without the textures permission/);
  h.handle(styler, { axidps: 1, type: "textures", rules: [{ target: "commander.red", color: "#0072b2" }] });
  assert.deepStrictEqual(JSON.parse(sent[1]), { textures: [{ target: "commander.red", color: "#0072b2" }] });
  // The same rules again: nothing new sent.
  h.handle(styler, { axidps: 1, type: "textures", rules: [{ target: "commander.red", color: "#0072b2" }] });
  assert.strictEqual(sent.length, 2);
  // The plugin goes away: its rules are cleared on the host.
  h.drop(styler);
  assert.deepStrictEqual(JSON.parse(sent[2]), { textures: [] });
});

const T = (key, extra) => Object.assign({ key, type: "toggle", label: key }, extra);
const N = (key, extra) => Object.assign({ key, type: "number", label: key, min: 1, max: 20 }, extra);
const C = (key, extra) => Object.assign({ key, type: "choice", label: key, options: ["ms", "s"] }, extra);

test("settingsModel drops bad entries, fills defaults, caps at 16", () => {
  const bad = [
    T("Big"), T("dup"), T("dup"), T("e", { label: "" }), T("long", { label: "x".repeat(61) }),
    { key: "t", type: "text", label: "t" }, { key: "nt", label: "nt" },
    C("c0", { options: [] }), C("c13", { options: Array.from({ length: 13 }, (_, i) => "o" + i) }),
    C("c41", { options: ["o".repeat(41)] }), C("cd", { default: "h" }), T("ty", { default: "yes" }),
    N("mm", { min: 5, max: 1 }), N("nd", { default: 30 }), N("st", { step: 0 }), "junk", null,
  ];
  const m = ph.settingsModel({ settings: bad.concat([T("ok")]) });
  // the first "dup" is kept; its twin is dropped
  assert.deepStrictEqual(m.map(f => f.key), ["dup", "ok"]);
  assert.deepStrictEqual(m[1], { key: "ok", type: "toggle", label: "ok", default: false });
  const many = Array.from({ length: 20 }, (_, i) => T("k" + i));
  assert.strictEqual(ph.settingsModel({ settings: many }).length, 16);
  assert.deepStrictEqual(ph.settingsModel({}), []);
  assert.deepStrictEqual(ph.settingsModel({ settings: "x" }), []);
  const [c, n] = ph.settingsModel({ settings: [C("u"), N("r")] });
  assert.strictEqual(c.default, "ms");
  assert.deepStrictEqual([n.min, n.max, n.step, n.default], [1, 20, 1, 1]);
});

test("settingValues uses stored values that fit, defaults otherwise", () => {
  const m = ph.settingsModel({ settings: [T("big"), N("rows"), C("unit")] });
  assert.deepStrictEqual(ph.settingValues(m, { big: "yes", rows: 50, unit: "s", old: 1 }),
    { big: false, rows: 1, unit: "s" });
  assert.deepStrictEqual(ph.settingValues(m, { big: true, rows: 7 }), { big: true, rows: 7, unit: "ms" });
  assert.deepStrictEqual(ph.settingValues(m, null), { big: false, rows: 1, unit: "ms" });
});

test("windowOpts defaults and reads pwin", () => {
  assert.deepStrictEqual(ph.windowOpts({}, "x.y/a"), { bar: true, clear: false });
  assert.deepStrictEqual(ph.windowOpts({ pwin: { "x.y/a": { bar: false } } }, "x.y/a"), { bar: false, clear: false });
  assert.deepStrictEqual(ph.windowOpts({ pwin: { "x.y/a": { bar: 0, clear: "yes" } } }, "x.y/a"), { bar: true, clear: false });
  assert.deepStrictEqual(ph.windowOpts({ pwin: { "x.y/a": { clear: true } } }, "x.y/a"), { bar: true, clear: true });
  assert.deepStrictEqual(ph.windowOpts({ pwin: {} }, "constructor"), { bar: true, clear: false });
});

test("panelModel: one section per window, values filled", () => {
  const p = plugin("x.y", { windows: [win("a", { title: "A".repeat(90) }), win("b", { settings: [T("big")] })] });
  const saved = { cfg: { "x.y/b": { big: true } }, pwin: { "x.y/a": { clear: true } } };
  const pm = ph.panelModel(p, saved);
  assert.strictEqual(pm.length, 2);
  assert.deepStrictEqual(pm.map(s => s.fullId), ["x.y/a", "x.y/b"]);
  assert.strictEqual(pm[0].title.length, 80);
  assert.deepStrictEqual(pm[0].opts, { bar: true, clear: true });
  assert.deepStrictEqual(pm[0].fields, []);
  assert.deepStrictEqual(pm[1].fields, [{ key: "big", type: "toggle", label: "big", default: false, value: true }]);
});

test("fitSetting snaps and clamps numbers, refuses the wrong type", () => {
  const [n] = ph.settingsModel({ settings: [N("r", { step: 2 })] });
  assert.deepStrictEqual(ph.fitSetting(n, 4.2), { ok: true, value: 5 });
  assert.deepStrictEqual(ph.fitSetting(n, 99), { ok: true, value: 19 });
  assert.deepStrictEqual(ph.fitSetting(n, -3), { ok: true, value: 1 });
  assert.strictEqual(ph.fitSetting(n, NaN).ok, false);
  assert.strictEqual(ph.fitSetting(n, "3").ok, false);
  const [t, c] = ph.settingsModel({ settings: [T("t"), C("c")] });
  assert.strictEqual(ph.fitSetting(t, "x").ok, false);
  assert.deepStrictEqual(ph.fitSetting(t, true), { ok: true, value: true });
  assert.deepStrictEqual(ph.fitSetting(c, "s"), { ok: true, value: "s" });
  assert.strictEqual(ph.fitSetting(c, "h").ok, false);
});

// A host with one plugin window "x.y/a" added and said hello: its posts and class toggles recorded.
function settingsHost(saved, settings) {
  const p = plugin("x.y", { windows: [win("a", { settings: settings || [T("big"), N("rows")] })] });
  const { h, sent } = fakeHost([p], saved);
  const toggles = [];
  let reports = 0;
  Object.assign(h.env, {
    scheduleReport: () => { reports++; }, look: () => ({}), feed: () => null, ui: { editing: false },
    defineWindow: () => ({ el: { querySelector: () => null,
      classList: { toggle: (c, on) => toggles.push([c, on]) } } }),
  });
  h.add(ph.validManifestWindows([p])[0]);
  const ent = h.entries[0];
  ent.posted = [];
  ent.cw = { postMessage: m => ent.posted.push(JSON.parse(JSON.stringify(m))) };
  return { h, ent, sent, toggles, reports: () => reports };
}

test("setSetting merges, saves, and sends the whole store", () => {
  const saved = { cfg: { "x.y/a": { mine: 1 } } };
  const { h, ent } = settingsHost(saved);
  ent.hello = true;
  assert.strictEqual(h.setSetting("x.y/a", "big", true), true);
  assert.deepStrictEqual(saved.cfg["x.y/a"], { mine: 1, big: true });
  assert.deepStrictEqual(ent.posted.at(-1), { axidps: 1, type: "settings", value: { big: true, rows: 1, mine: 1 } });
});

test("setSetting refuses unknown keys, wrong types, unknown windows and __proto__", () => {
  const saved = { cfg: { "x.y/a": { mine: 1 } } };
  const { h, ent } = settingsHost(saved);
  ent.hello = true;
  const before = JSON.stringify(saved);
  for (const [id, k, v] of [["x.y/a", "nope", true], ["x.y/a", "big", "yes"], ["x.y/b", "big", true],
    ["x.y/a", "__proto__", 1], ["__proto__", "big", true], ["x.y/a", "rows", "3"]]) {
    assert.strictEqual(h.setSetting(id, k, v), false, `${id} ${k}`);
  }
  assert.strictEqual(JSON.stringify(saved), before);
  assert.deepStrictEqual(ent.posted, []);
  assert.strictEqual(Object.prototype.big, undefined);
});

test("after a plugin's own settings.set drops hub keys, the next change merges into it", () => {
  const saved = { cfg: { "x.y/a": { big: true } } };
  const { h, ent } = settingsHost(saved);
  h.handle(ent, { axidps: 1, type: "settings", value: { mine: 2 } });
  assert.strictEqual(h.setSetting("x.y/a", "rows", 3), true);
  assert.deepStrictEqual(saved.cfg["x.y/a"], { mine: 2, rows: 3 });
});

test("setSetting with no stored object starts a fresh one", () => {
  const saved = { cfg: { "x.y/a": "junk" } };
  const { h } = settingsHost(saved);
  assert.strictEqual(h.setSetting("x.y/a", "rows", 4.6), true);
  assert.deepStrictEqual(saved.cfg["x.y/a"], { rows: 5 });
});

test("hello sends declared defaults under the stored values", () => {
  const saved = { cfg: { "x.y/a": { mine: 1, rows: 4 } } };
  const { h, ent } = settingsHost(saved);
  h.handle(ent, { axidps: 1, type: "hello" });
  const s = ent.posted.find(m => m.type === "settings");
  assert.deepStrictEqual(s.value, { big: false, rows: 4, mine: 1 });
});

test("setWindowOpt saves pwin and applies the classes; the frame never hears of it", () => {
  const saved = { cfg: {} };
  const { h, ent, toggles, reports } = settingsHost(saved);
  ent.hello = true;
  toggles.length = 0;
  assert.strictEqual(h.setWindowOpt("x.y/a", "bar", false), true);
  assert.deepStrictEqual(saved.pwin["x.y/a"], { bar: false, clear: false });
  assert.deepStrictEqual(toggles, [["nobar", true], ["clear", false]]);
  assert.strictEqual(ent.w.clear, false);
  assert.ok(reports() >= 1);
  assert.strictEqual(h.setWindowOpt("x.y/a", "clear", true), true);
  assert.deepStrictEqual(saved.pwin["x.y/a"], { bar: false, clear: true });
  assert.strictEqual(ent.w.clear, true);
  assert.strictEqual(h.setWindowOpt("x.y/a", "nope", true), false);
  assert.strictEqual(h.setWindowOpt("x.y/a", "bar", "no"), false);
  assert.strictEqual(h.setWindowOpt("x.y/b", "bar", false), false);
  assert.ok(!JSON.stringify(ent.posted).includes("pwin"));
  assert.deepStrictEqual(ent.posted, []);
});

test("add applies saved window options", () => {
  const { toggles, ent } = settingsHost({ cfg: {}, pwin: { "x.y/a": { clear: true } } });
  assert.deepStrictEqual(toggles, [["nobar", false], ["clear", true]]);
  assert.strictEqual(ent.w.clear, true);
});

test("refresh applies window options restored after the window was added", () => {
  const saved = { cfg: {}, plugins: { "x.y": { enabled: true, granted: ["fight"] } }, open: {} };
  const { h, ent, toggles } = settingsHost(saved);
  h.env.renderAll = () => {};
  toggles.length = 0;
  saved.pwin = { "x.y/a": { bar: false, clear: true } };
  h.refresh();
  assert.strictEqual(h.entries[0], ent, "kept, not re-added");
  assert.deepStrictEqual(toggles, [["nobar", true], ["clear", true]]);
  assert.strictEqual(ent.w.clear, true);
});
