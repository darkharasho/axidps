// Merged windows: the grid a merged window keeps its tables in, where a dragged window would dock,
// and the motion curves. Plain browser JS; a global in the page, module.exports in Node.
//
// A merged window's cells are [{k, r, c}]: k is a member window's key, r/c its row and column.
// Empty rows and columns are always squeezed out, so the grid stays tidy (2 top, 2 bottom).
const axidpsMerge = (() => {
  const at = (cells, r, c) => cells.find(x => x.r === r && x.c === c);
  const uniq = v => [...new Set(v)].sort((a, b) => a - b);

  function compact(cells) {
    const rs = uniq(cells.map(x => x.r)), cs = uniq(cells.map(x => x.c));
    return cells.map(x => Object.assign({}, x, {r: rs.indexOf(x.r), c: cs.indexOf(x.c)}));
  }

  // `block` (a window's own cells) beside cell (r, c) on `side`, or into the empty cell (r, c) for
  // "fill". Free space is used as it is; otherwise whole rows or columns open, so the grid stays
  // aligned. Null if "fill" finds the cell taken.
  function place(cells, r, c, side, block) {
    block = compact(block);
    const bw = Math.max(...block.map(x => x.c)) + 1, bh = Math.max(...block.map(x => x.r)) + 1;
    let ar = r, ac = c;
    if (side === "right") ac = c + 1;
    if (side === "left") ac = c - bw;
    if (side === "bottom") ar = r + 1;
    if (side === "top") ar = r - bh;
    const free = block.every(b => !at(cells, ar + b.r, ac + b.c));
    let out = cells.map(x => Object.assign({}, x));
    if (!free) {
      if (side === "fill") return null;
      if (side === "right") { out.forEach(x => { if (x.c > c) x.c += bw; }); ac = c + 1; }
      if (side === "left") { out.forEach(x => { if (x.c >= c) x.c += bw; }); ac = c; }
      if (side === "bottom") { out.forEach(x => { if (x.r > r) x.r += bh; }); ar = r + 1; }
      if (side === "top") { out.forEach(x => { if (x.r >= r) x.r += bh; }); ar = r; }
    }
    out = out.concat(block.map(b => Object.assign({}, b, {r: ar + b.r, c: ac + b.c})));
    const mr = Math.min(...out.map(x => x.r)), mc = Math.min(...out.map(x => x.c));
    out.forEach(x => { x.r -= mr; x.c -= mc; });
    return compact(out);
  }

  const removeKey = (cells, k) => compact(cells.filter(x => x.k !== k));

  const inside = (px, py, r) => px >= r.x && px <= r.x + r.w && py >= r.y && py <= r.y + r.h;
  // Which track of `sizes` (starting at `o`) holds p, or -1.
  const track = (sizes, o, p) => {
    let a = o;
    for (let i = 0; i < sizes.length; i++) { if (p < a + sizes[i]) return p >= o ? i : -1; a += sizes[i]; }
    return -1;
  };

  // Where a window dragged with its pointer at (px, py) would dock, in screen space. targets:
  // [{id, cells, rects: {k: {x, y, w, h}}, grid: {x, y, cols: [w], rows: [h]}}]. Over a table: its
  // nearest edge. In an empty cell of a target's grid: that cell. Within `band` outside a table's
  // edge (and level with it): that edge. Otherwise null.
  function dockAt(px, py, targets, band) {
    for (const t of targets) {
      for (const x of t.cells) {
        const r = t.rects[x.k];
        if (!r || !inside(px, py, r)) continue;
        const fx = (px - r.x) / r.w, fy = (py - r.y) / r.h;
        const d = {left: fx, right: 1 - fx, top: fy, bottom: 1 - fy};
        return {id: t.id, k: x.k, side: Object.keys(d).sort((a, b) => d[a] - d[b])[0]};
      }
      const g = t.grid;
      if (g) {
        const c = track(g.cols, g.x, px), r = track(g.rows, g.y, py);
        if (c >= 0 && r >= 0 && !at(t.cells, r, c)) return {id: t.id, r, c, side: "fill"};
      }
      for (const x of t.cells) {
        const r = t.rects[x.k];
        if (!r) continue;
        const d = {left: r.x - px, right: px - (r.x + r.w), top: r.y - py, bottom: py - (r.y + r.h)};
        const inX = px > r.x && px < r.x + r.w, inY = py > r.y && py < r.y + r.h;
        for (const s of Object.keys(d)) {
          if (d[s] > 0 && d[s] < band && (s === "left" || s === "right" ? inY : inX)) return {id: t.id, k: x.k, side: s};
        }
      }
    }
    return null;
  }

  // Saved groups made safe: known keys (`ok(k)`), each key in one group only (the first), whole
  // numbers, two or more members, the grid squeezed.
  function normalizeGroups(groups, ok) {
    if (!Array.isArray(groups)) return [];
    const seen = new Set(), out = [];
    for (const g of groups) {
      if (!g || !Number.isInteger(g.id) || !Array.isArray(g.cells)) continue;
      const cells = [];
      for (const x of g.cells) {
        if (!x || typeof x.k !== "string" || !ok(x.k) || seen.has(x.k)) continue;
        if (!Number.isInteger(x.r) || !Number.isInteger(x.c) || x.r < 0 || x.c < 0) continue;
        if (at(cells, x.r, x.c)) continue;
        cells.push({k: x.k, r: x.r, c: x.c});
      }
      if (cells.length < 2) continue;
      cells.forEach(x => seen.add(x.k));
      out.push({id: g.id, cells: compact(cells)});
    }
    return out;
  }

  // Motion. Attach and detach: a damped spring, about 9% overshoot, settled in ~0.6 s; `v` is the
  // normalised velocity, for the squash. Everything else: a 180 ms ease-out with no overshoot.
  const SPR = (() => {
    let x = 0, v = 0;
    const p = [0], vs = [0], dt = 1 / 60;
    for (let i = 0; i < 36; i++) { v += (-260 * (x - 1) - 18 * v) * dt; x += v * dt; p.push(x); vs.push(v); }
    const m = Math.max(...vs.map(Math.abs));
    return p.map((x, i) => ({p: x, v: vs[i] / m}));
  })();
  const SPR_MS = SPR.length * 1000 / 60;
  const GL = Array.from({length: 13}, (_, i) => ({p: 1 - Math.pow(1 - i / 12, 3), v: 0}));
  const GL_MS = 180;
  const squish = (horiz, v, amt) => {
    const s = 1 + amt * v, c = 1 - amt * 0.7 * v;
    return horiz ? `scale(${s},${c})` : `scale(${c},${s})`;
  };

  return {compact, place, removeKey, dockAt, normalizeGroups, SPR, SPR_MS, GL, GL_MS, squish,
    TEAR: 70, BAND: 36};
})();
if (typeof module !== "undefined" && module.exports) module.exports = axidpsMerge;
