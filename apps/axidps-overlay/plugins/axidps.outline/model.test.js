const test = require("node:test");
const assert = require("node:assert");
const m = require("./model.js");

test("normalize fills defaults and clamps", () => {
  assert.deepStrictEqual(m.normalize({}), { on: false, color: [0.2, 1, 0.9], thickness: 3, fill: 0.15,
    fillColor: [0.2, 1, 0.9], pve: true, wvw: true });
  const n = m.normalize({ on: true, thickness: 20, fill: -1, color: [2, "x", 0.5] });
  assert.strictEqual(n.thickness, 8); assert.strictEqual(n.fill, 0);
  assert.deepStrictEqual(n.color, [0.2, 1, 0.9]);   // a bad colour takes the default whole
});
test("normalize clamps colour channels and ignores wrong types", () => {
  const n = m.normalize({ color: [2, -1, 0.5], on: "yes", pve: 0, wvw: false, thickness: "5", fill: 9 });
  assert.deepStrictEqual(n.color, [1, 0, 0.5]);
  assert.strictEqual(n.on, false); assert.strictEqual(n.pve, true); assert.strictEqual(n.wvw, false);
  assert.strictEqual(n.thickness, 3); assert.strictEqual(n.fill, 0.6);
  assert.deepStrictEqual(m.normalize(null).color, [0.2, 1, 0.9]);
  assert.deepStrictEqual(m.normalize({ color: [1, 1] }).color, [0.2, 1, 0.9]);
});
test("color(h, l) is HSL at full saturation", () => {
  assert.deepStrictEqual(m.color(0, 0.5), [1, 0, 0]);
  assert.deepStrictEqual(m.color(120, 0.5), [0, 1, 0]);
  assert.deepStrictEqual(m.color(0, 1), [1, 1, 1]);
});
test("first swatch is the default colour", () => {
  assert.deepStrictEqual(m.SWATCHES[0], [0.2, 1, 0.9]);
  assert.strictEqual(m.SWATCHES.length, 8);
});
