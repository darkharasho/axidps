// The hub's pure rules: tab names, size, the window list (ordering, hidden split, joined groups),
// the Show button's reason and the held-back notice, reordering, selection fallback, the hub's fit in the
// atlas and the drawn scroll bars' geometry. No DOM here, so it is testable.
// Plain browser JS; a global in the page, module.exports in Node.
const axidpsHub = (() => {
  const TABS = [["windows", "Windows"], ["plugins", "Plugins"], ["options", "Options"]];
  const TAB_OF = {windows: "windows", tables: "windows", store: "plugins", plugins: "plugins",
    appearance: "options", settings: "options", updates: "options", options: "options"};
  const KINDS = ["builtin", "custom", "plugin"];

  const tabFor = name => Object.prototype.hasOwnProperty.call(TAB_OF, name) ? TAB_OF[name] : "windows";

  const HUB_MIN_H = 360;
  const hubSize = ([sw, sh]) => [Math.max(480, Math.min(1040, sw - 80)), Math.max(HUB_MIN_H, Math.min(680, sh - 80))];
  // The hub packs last in the atlas, so it never pushes another window out: it takes what height is
  // left below where it lands (`room`), down to its smallest; below that a window has to give way.
  const hubFit = ([w, h], room) => [w, Math.max(HUB_MIN_H, Math.min(h, Math.floor(room)))];
  // Where the hub goes on a screen [sw, sh] at size [w, h]: centred, or where it was dragged to
  // (`moved`, this session), kept on screen.
  const hubAt = ([sw, sh], [w, h], moved) => {
    const clamp = (v, max) => Math.max(0, Math.min(Math.round(v), Math.max(0, max)));
    if (!moved) return [clamp((sw - w) / 2, sw - w), clamp((sh - h) / 2, sh - h)];
    return [clamp(moved[0], sw - w), clamp(moved[1], sh - h)];
  };

  // A plugin page popped out (session only): a narrow window over the game, first at the right edge,
  // vertically centred, as tall as its page up to most of the screen. popSize gives [width, max height].
  const POP_W = 400, POP_EDGE = 24;
  const popSize = ([sw, sh]) => [Math.min(POP_W, sw - 16), Math.max(HUB_MIN_H, Math.round(sh * 0.85))];
  const popAt = ([sw, sh], [w, h], moved) => moved ? hubAt([sw, sh], [w, h], moved)
    : hubAt([sw, sh], [w, h], [sw - w - POP_EDGE, (sh - h) / 2]);
  // Where the popped hub goes this report. Until it is dragged it keeps the top it first got on this
  // screen (`anchor`), so a page that grows or shrinks doesn't move its title bar under the cursor.
  const popPlace = (sc, size, moved, anchor) => {
    if (moved) return {at: popAt(sc, size, moved), anchor};
    if (anchor && anchor.sc[0] === sc[0] && anchor.sc[1] === sc[1])
      return {at: hubAt(sc, size, [sc[0] - size[0] - POP_EDGE, anchor.y]), anchor};
    const at = popAt(sc, size, null);
    return {at, anchor: {sc: [sc[0], sc[1]], y: at[1]}};
  };
  // The page a popped hub shows, while it is still listed, on and asking for Pop out; else null.
  const popPage = (pages, fullId) =>
    (Array.isArray(pages) ? pages : []).find(p => p && p.fullId === fullId && p.on && p.popout) || null;

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

  // The line under the Hidden list: why windows the player wants open are held back, or "" once
  // nothing is (overCap: how many are over the cap; refused: a Show was just turned down at the cap).
  function notice({overCap, overAtlas, refused, cap}) {
    if (overAtlas) return "No room for more windows; close one first";
    if (overCap > 0 || refused) return `axidps can show ${cap} windows at once. Hide one first.`;
    return "";
  }

  // A merged window's head: its cell in the lowest row, then the lowest column.
  const groupHead = cells => (cells || []).slice().sort((a, b) => a.r - b.r || a.c - b.c)[0] || null;

  // Drawn scroll bars (the overlay's engine paints none). view/content: the area's and its
  // content's heights; top: scrollTop; track: the bar's length. The thumb is in proportion, at
  // least `min` px; null when everything fits.
  function scrollThumb({view, content, top, track, min = 40}) {
    if (!(content > view + 0.5) || !(track > 0)) return null;
    const size = Math.round(Math.min(track, Math.max(min, track * view / content)));
    const room = track - size, max = content - view;
    return {size, pos: Math.round(room * Math.max(0, Math.min(1, top / max)))};
  }
  // The scrollTop for a thumb of `size` dragged to `pos` px down the track.
  function scrollForThumb({view, content, track, size, pos}) {
    const room = track - size, max = Math.max(0, content - view);
    return room > 0 ? Math.round(Math.max(0, Math.min(max, pos / room * max))) : 0;
  }
  // A press on the track at `at` px, outside the thumb: one page (less 40px of overlap) toward it.
  function pageScroll({view, content, top, at, thumb}) {
    const max = Math.max(0, content - view), step = Math.max(1, view - 40);
    if (at < thumb.pos) return Math.max(0, top - step);
    if (at >= thumb.pos + thumb.size) return Math.min(max, top + step);
    return top;
  }

  function moveItem(arr, from, to) {
    const out = arr.slice();
    if (!(from >= 0 && from < out.length && to >= 0 && to < out.length)) return out;
    out.splice(to, 0, out.splice(from, 1)[0]);
    return out;
  }

  // A search box's match: every word of the query appears in one of the texts (any case).
  function textMatch(query, texts) {
    const words = String(query || "").toLowerCase().split(/\s+/).filter(Boolean);
    const hay = texts.map(t => String(t || "").toLowerCase());
    return words.every(w => hay.some(h => h.includes(w)));
  }

  // Where a dragged row lands in its list: `mids` are the rows' midpoints where they sat before
  // the drag, `from` the dragged row's index, `y` its centre now. It takes the place of every
  // other row whose midpoint it has passed.
  function dropSlot(mids, from, y) {
    let n = 0;
    mids.forEach((m, i) => { if (i !== from && m < y) n++; });
    return n;
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
        // Installed: on this computer, or installed from the store, whether or not it can be turned on.
        installed: !!i || c.action === "update" || c.action === "installed" || c.action === "revoked",
        note: c.note, bad: c.bad, busy: c.busy, unchecked: c.trust === "unverified"}, base,
      k ? {toggle: k.action, reason: k.reason, reload: k.reload} : {});
    });
    const locals = (installed || []).filter(i => !inStore.has(i.id)).map(i => {
      const k = i.card, author = i.bundled ? "axidps" : k.author;
      return Object.assign({id: i.id, title: k.title, author, local: !i.bundled && storeReady !== false, description: "",
        version: k.version, trust: "", uses: (k.lines || []).map(text => ({text, deep: false})), store: null,
        remove: false, note: "", bad: false, busy: false, unchecked: false, installed: true}, base,
      {toggle: k.action, reason: k.reason, reload: k.reload});
    }).filter(r => !q || [r.title, r.author].some(v => String(v || "").toLowerCase().includes(q)));
    const all = fromStore.concat(locals);
    const counts = {all: all.length, installed: all.filter(r => r.installed).length,
      updates: all.filter(r => r.store === "update").length};
    const rows = filter === "installed" ? all.filter(r => r.installed)
      : filter === "updates" ? all.filter(r => r.store === "update") : all;
    return {rows, counts};
  }

  // The Options tab's left list: the built-in pages, then one per plugin hub page (hubPages) under
  // "Plugins". sel: the chosen id if it is still listed, else Look.
  const OPTION_PAGES = [["look", "Look"], ["windows", "Windows"], ["counted", "What gets counted"], ["updates", "Updates"]];
  function optionNav(pages, sel) {
    const items = OPTION_PAGES.map(([id, label]) => ({id, label}))
      .concat((Array.isArray(pages) ? pages : []).map(p => ({id: "p:" + p.fullId, label: p.title, group: "Plugins",
        icon: p.icon, on: !!p.on})));
    return {items, sel: items.some(i => i.id === sel) ? sel : "look"};
  }

  return {TABS, tabFor, hubSize, hubFit, hubAt, windowList, showBlock, notice, groupHead, scrollThumb, scrollForThumb,
    pageScroll, moveItem, textMatch, dropSlot, selected, pluginRows, optionNav, popSize, popAt, popPlace, popPage};
})();
if (typeof module !== "undefined" && module.exports) module.exports = axidpsHub;
