// The hub's pure rules: tab names, size, the window list (ordering, hidden split, joined groups),
// the Show button's reason, reordering and selection fallback. No DOM here, so it is testable.
// Plain browser JS; a global in the page, module.exports in Node.
const axidpsHub = (() => {
  const TABS = [["windows", "Windows"], ["plugins", "Plugins"], ["options", "Options"]];
  const TAB_OF = {windows: "windows", tables: "windows", store: "plugins", plugins: "plugins",
    appearance: "options", settings: "options", updates: "options", options: "options"};
  const KINDS = ["builtin", "custom", "plugin"];

  const tabFor = name => Object.prototype.hasOwnProperty.call(TAB_OF, name) ? TAB_OF[name] : "windows";

  const hubSize = ([sw, sh]) => [Math.max(480, Math.min(1040, sw - 80)), Math.max(360, Math.min(680, sh - 80))];

  // Stable sort by kind: builtin, custom, plugin; `wins` order kept within a kind.
  const byKind = list => KINDS.flatMap(k => list.filter(x => x.kind === k));

  const row = (it, joinedTo, depth) => ({id: it.id, name: it.name,
    note: joinedTo ? "Joined to " + joinedTo : it.note, kind: it.kind, open: it.open, joinedTo, depth});

  function windowList(items, groups) {
    const open = byKind(items.filter(x => x.open));
    const hidden = byKind(items.filter(x => !x.open)).map(x => row(x, null, 0));
    const byKey = new Map(open.map(x => [x.key, x]));
    // Per group: its open members in grid order (lowest r, then c); the first is the head.
    const chains = [];
    for (const g of groups || []) {
      const cells = (g.cells || []).filter(c => byKey.has(c.k)).sort((a, b) => a.r - b.r || a.c - b.c);
      if (cells.length > 1) chains.push(cells.map(c => byKey.get(c.k)));
    }
    const follower = new Set(), after = new Map();
    for (const ch of chains) { after.set(ch[0], ch.slice(1)); ch.slice(1).forEach(x => follower.add(x)); }
    const onScreen = [];
    for (const it of open) {
      if (follower.has(it)) continue;
      onScreen.push(row(it, null, 0));
      for (const m of after.get(it) || []) onScreen.push(row(m, it.name, 1));
    }
    return {onScreen, hidden};
  }

  function showBlock({open, openCount, cap, atlasFull}) {
    if (open) return "";
    if (openCount >= cap) return `axidps can show ${cap} windows at once. Hide one first.`;
    if (atlasFull) return "No room for more windows";
    return "";
  }

  function moveItem(arr, from, to) {
    const out = arr.slice();
    if (!(from >= 0 && from < out.length && to >= 0 && to < out.length)) return out;
    out.splice(to, 0, out.splice(from, 1)[0]);
    return out;
  }

  const selected = (id, rows) =>
    rows.onScreen.concat(rows.hidden).some(r => r.id === id) ? id : "meter";

  // One plugin list: the Store's cards (already filtered by the query) merged with the installed
  // plugins. Store rows come first, in store order, taking toggle/reason/reload from the installed
  // plugin with the same id; installed plugins with no store card follow, in `installed` order, as
  // local (or bundled) rows, filtered here by name and author. counts ignore `filter`.
  function pluginRows(storeCards, installed, filter, query, storeReady) {
    const q = String(query || "").trim().toLowerCase();
    const have = new Map((installed || []).map(i => [i.id, i]));
    const inStore = new Set((storeCards || []).map(c => c.id));
    const base = {toggle: null, reason: "", reload: false};
    const fromStore = (storeCards || []).map(c => {
      const i = have.get(c.id), k = i ? i.card : null;
      return Object.assign({id: c.id, title: c.title, author: c.author, local: false, description: c.description,
        version: c.version, trust: c.trust, uses: c.uses, store: c.action,
        remove: c.action === "update" || c.action === "installed" || c.action === "revoked",
        note: c.note, bad: c.bad, busy: c.busy, unchecked: c.trust === "unverified"}, base,
      k ? {toggle: k.action, reason: k.reason, reload: k.reload} : {});
    });
    const locals = (installed || []).filter(i => !inStore.has(i.id)).map(i => {
      const k = i.card, author = i.bundled ? "axidps" : k.author;
      return Object.assign({id: i.id, title: k.title, author, local: !i.bundled && storeReady !== false, description: "",
        version: k.version, trust: "", uses: (k.lines || []).map(text => ({text, deep: false})), store: null,
        remove: false, note: "", bad: false, busy: false, unchecked: false}, base,
      {toggle: k.action, reason: k.reason, reload: k.reload});
    }).filter(r => !q || [r.title, r.author].some(v => String(v || "").toLowerCase().includes(q)));
    const all = fromStore.concat(locals);
    const counts = {all: all.length, installed: all.filter(r => r.toggle).length,
      updates: all.filter(r => r.store === "update").length};
    const rows = filter === "installed" ? all.filter(r => r.toggle)
      : filter === "updates" ? all.filter(r => r.store === "update") : all;
    return {rows, counts};
  }

  return {TABS, tabFor, hubSize, windowList, showBlock, moveItem, selected, pluginRows};
})();
if (typeof module !== "undefined" && module.exports) module.exports = axidpsHub;
