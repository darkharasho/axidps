// The outline settings: defaults and limits. Loads under node (tests) and in the frame (window.outlineModel).
(function (root) {
  "use strict";
  const DEFAULT = [0.2, 1, 0.9];
  const SWATCHES = [DEFAULT, [1, 1, 1], [1, 1, 0.2], [1, 0.55, 0.1], [1, 0.2, 0.2], [1, 0.2, 1], [0.25, 0.4, 1], [0.2, 1, 0.3]];
  const num = v => typeof v === "number" && Number.isFinite(v);
  const clamp = (v, lo, hi, d) => (num(v) ? Math.min(hi, Math.max(lo, v)) : d);
  // Three numbers, each clamped to 0-1; anything else takes the default whole.
  const colour = v => (Array.isArray(v) && v.length === 3 && v.every(num) ? v.map(c => clamp(c, 0, 1, 0)) : DEFAULT.slice());
  const flag = (v, d) => (typeof v === "boolean" ? v : d);

  // `on` is always true: the plugin's switch in the hub turns the outline on and off (switched off,
  // this page doesn't run). An `on` saved before that is ignored.
  function normalize(o) {
    o = o && typeof o === "object" && !Array.isArray(o) ? o : {};
    return {
      on: true, color: colour(o.color), thickness: clamp(o.thickness, 1, 8, 3),
      fill: clamp(o.fill, 0, 0.6, 0.15), fillColor: colour(o.fillColor), pve: flag(o.pve, true), wvw: flag(o.wvw, true),
    };
  }

  // HSL with saturation 1: h in degrees (0-360), l 0-1. Channels rounded to 3 decimals.
  function color(h, l) {
    const f = n => {
      const k = (n + h / 30) % 12;
      const v = l - (1 - Math.abs(2 * l - 1)) / 2 * Math.max(-1, Math.min(k - 3, 9 - k, 1));
      return Math.round(Math.min(1, Math.max(0, v)) * 1000) / 1000 + 0;
    };
    return [f(0), f(8), f(4)];
  }

  // What gets saved: the settings without `on`.
  function toSave(o) {
    const out = Object.assign({}, o);
    delete out.on;
    return out;
  }

  const api = { normalize, toSave, color, SWATCHES };
  if (typeof module !== "undefined" && module.exports) module.exports = api; else root.outlineModel = api;
})(typeof window !== "undefined" ? window : this);
