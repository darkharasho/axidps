const test = require("node:test");
const assert = require("node:assert");
const M = require("./merge.js");

const cell = (k, r, c) => ({ k, r, c });
const sorted = cells => cells.map(x => `${x.k}@${x.r}${x.c}`).sort();

test("compact squeezes out empty rows and columns", () => {
  assert.deepStrictEqual(sorted(M.compact([cell("a", 0, 0), cell("b", 2, 3)])), ["a@00", "b@11"]);
});

test("place beside a cell uses free space as it is", () => {
  const two = [cell("a", 0, 0), cell("b", 0, 1)];
  // below b: row 1 is free there
  assert.deepStrictEqual(sorted(M.place(two, 0, 1, "bottom", [cell("x", 0, 0)])), ["a@00", "b@01", "x@11"]);
  assert.deepStrictEqual(sorted(M.place(two, 0, 1, "right", [cell("x", 0, 0)])), ["a@00", "b@01", "x@02"]);
});

test("place opens a whole column or row when the spot is taken", () => {
  const two = [cell("a", 0, 0), cell("b", 0, 1)];
  assert.deepStrictEqual(sorted(M.place(two, 0, 0, "right", [cell("x", 0, 0)])), ["a@00", "b@02", "x@01"]);
  assert.deepStrictEqual(sorted(M.place(two, 0, 0, "left", [cell("x", 0, 0)])), ["a@01", "b@02", "x@00"]);
  const col = [cell("a", 0, 0), cell("b", 1, 0)];
  assert.deepStrictEqual(sorted(M.place(col, 0, 0, "top", [cell("x", 0, 0)])), ["a@10", "b@20", "x@00"]);
  assert.deepStrictEqual(sorted(M.place(col, 0, 0, "bottom", [cell("x", 0, 0)])), ["a@00", "b@20", "x@10"]);
});

test("a window of several tables places as one block", () => {
  const one = [cell("a", 0, 0)];
  const block = [cell("x", 0, 0), cell("y", 0, 1)];
  assert.deepStrictEqual(sorted(M.place(one, 0, 0, "bottom", block)), ["a@00", "x@10", "y@11"]);
});

test("fill takes an empty cell and refuses a taken one", () => {
  const three = [cell("a", 0, 0), cell("b", 0, 1), cell("c", 1, 1)];
  assert.deepStrictEqual(sorted(M.place(three, 1, 0, "fill", [cell("x", 0, 0)])), ["a@00", "b@01", "c@11", "x@10"]);
  assert.strictEqual(M.place(three, 0, 0, "fill", [cell("x", 0, 0)]), null);
});

test("removeKey drops the cell and squeezes the grid", () => {
  const grid = [cell("a", 0, 0), cell("b", 0, 1), cell("c", 1, 1)];
  assert.deepStrictEqual(sorted(M.removeKey(grid, "b")), ["a@00", "c@11"]);
  assert.deepStrictEqual(sorted(M.removeKey([cell("a", 0, 0), cell("b", 0, 1)], "a")), ["b@00"]);
});

// A target window at (100, 100): tables a (200x100) and b (200x100) side by side.
const target = {
  id: "g1", cells: [cell("a", 0, 0), cell("b", 0, 1)],
  rects: { a: { x: 100, y: 120, w: 200, h: 100 }, b: { x: 300, y: 120, w: 200, h: 100 } },
  grid: { x: 100, y: 120, cols: [200, 200], rows: [100] },
};

test("dockAt over a table picks its nearest edge", () => {
  assert.deepStrictEqual(M.dockAt(110, 170, [target], 36), { id: "g1", k: "a", side: "left" });
  assert.deepStrictEqual(M.dockAt(400, 215, [target], 36), { id: "g1", k: "b", side: "bottom" });
});

test("dockAt just outside an edge snaps to it, within the band only", () => {
  assert.deepStrictEqual(M.dockAt(520, 170, [target], 36), { id: "g1", k: "b", side: "right" });
  assert.deepStrictEqual(M.dockAt(200, 240, [target], 36), { id: "g1", k: "a", side: "bottom" });
  assert.strictEqual(M.dockAt(560, 170, [target], 36), null);
  assert.strictEqual(M.dockAt(1000, 1000, [target], 36), null);
});

test("dockAt in an empty grid cell fills it", () => {
  const t = Object.assign({}, target, {
    cells: [cell("a", 0, 0), cell("b", 0, 1), cell("c", 1, 1)],
    rects: Object.assign({}, target.rects, { c: { x: 300, y: 220, w: 200, h: 100 } }),
    grid: { x: 100, y: 120, cols: [200, 200], rows: [100, 100] },
  });
  assert.deepStrictEqual(M.dockAt(150, 260, [t], 36), { id: "g1", r: 1, c: 0, side: "fill" });
});

test("normalizeGroups keeps whole, known, unique groups of two or more", () => {
  const ok = k => ["a", "b", "c", "d"].includes(k);
  const out = M.normalizeGroups([
    { id: 1, cells: [cell("a", 0, 0), cell("gone", 0, 1), cell("b", 0, 2)] },
    { id: 2, cells: [cell("b", 0, 0), cell("c", 0, 1)] }, // b is taken: c alone, dropped
    { id: 3, cells: [cell("c", 5, 5), cell("d", 6, 5)] },
    { id: "x", cells: "junk" }, null, { id: 4, cells: [cell("a", 0, 0)] },
  ], ok);
  assert.deepStrictEqual(out.map(g => [g.id, sorted(g.cells)]), [[1, ["a@00", "b@01"]], [3, ["c@00", "d@10"]]]);
  assert.deepStrictEqual(M.normalizeGroups("junk", ok), []);
});

test("the spring overshoots a little and settles; the glide never does", () => {
  const p = M.SPR.map(f => f.p);
  const peak = Math.max(...p);
  assert.ok(peak > 1.05 && peak < 1.15, String(peak));
  assert.ok(Math.abs(p.at(-1) - 1) < 0.01, String(p.at(-1)));
  const g = M.GL.map(f => f.p);
  assert.strictEqual(g[0], 0);
  assert.strictEqual(g.at(-1), 1);
  assert.ok(g.every((v, i) => i === 0 || v >= g[i - 1]));
});

// The dragged window's own rectangle: it joins when one of its edges comes near a table's edge,
// wherever the pointer is on it.
const T1 = { id: "a", cells: [{ k: "a", r: 0, c: 0 }], rects: { a: { x: 100, y: 100, w: 300, h: 200 } }, grid: null };
test("dockAt joins by the dragged window's edge, not the pointer", () => {
  // Its left edge 10px right of the table, the pointer far inside it (its title bar).
  const rect = { x: 410, y: 120, w: 300, h: 200 };
  assert.deepStrictEqual(M.dockAt(560, 130, [T1], M.BAND, rect), { id: "a", k: "a", side: "right" });
  // Overlapping the table by 40px still joins on that side.
  assert.deepStrictEqual(M.dockAt(560, 130, [T1], M.BAND, { x: 360, y: 120, w: 300, h: 200 }).side, "right");
  // Below it, level enough: the bottom.
  assert.deepStrictEqual(M.dockAt(250, 320, [T1], M.BAND, { x: 150, y: 305, w: 200, h: 150 }).side, "bottom");
  // Above and to its left: the left and top edges by its right and bottom.
  assert.deepStrictEqual(M.dockAt(0, 0, [T1], M.BAND, { x: -205, y: 150, w: 300, h: 100 }).side, "left");
  assert.deepStrictEqual(M.dockAt(0, 0, [T1], M.BAND, { x: 150, y: -95, w: 200, h: 190 }).side, "top");
});
test("dockAt by edge needs the windows level with each other and close", () => {
  // Too far.
  assert.strictEqual(M.dockAt(900, 900, [T1], M.BAND, { x: 460, y: 120, w: 300, h: 200 }), null);
  // Only a corner touching (20px of 200 level): no.
  assert.strictEqual(M.dockAt(900, 900, [T1], M.BAND, { x: 405, y: 280, w: 300, h: 200 }), null);
  // Lying mostly over it: not an edge; the pointer decides as before.
  assert.deepStrictEqual(M.dockAt(390, 200, [T1], M.BAND, { x: 150, y: 120, w: 300, h: 200 }), { id: "a", k: "a", side: "right" });
});
test("dockAt by edge picks the nearest edge of several tables", () => {
  const T2 = { id: "b", cells: [{ k: "b", r: 0, c: 0 }], rects: { b: { x: 100, y: 330, w: 300, h: 200 } }, grid: null };
  // Lying over a's lower half, its bottom edge 5px above b's top: b's top.
  assert.deepStrictEqual(M.dockAt(900, 900, [T1, T2], M.BAND, { x: 110, y: 160, w: 280, h: 165 }), { id: "b", k: "b", side: "top" });
});
