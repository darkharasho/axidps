// Character Outline settings: shows the saved settings, saves each change and hands the result to the outline.
(function () {
  "use strict";
  const M = window.outlineModel;
  const $ = id => document.getElementById(id);
  let cur = M.normalize({});

  const css = c => "rgb(" + c.map(v => Math.round(v * 255)).join(",") + ")";
  // Hue and lightness that give back (about) this colour, so the sliders show where a swatch sits.
  function toHL(c) {
    const mx = Math.max(...c), mn = Math.min(...c), d = mx - mn;
    let h = 0;
    if (d > 0) {
      if (mx === c[0]) h = ((c[1] - c[2]) / d + 6) % 6;
      else if (mx === c[1]) h = (c[2] - c[0]) / d + 2;
      else h = (c[0] - c[1]) / d + 4;
    }
    return [Math.round(h * 60), (mx + mn) / 2];
  }

  function apply() {
    cur = M.normalize(cur);
    axidps.outline.set(cur);
  }
  function save() {
    apply();
    axidps.settings.set(cur);
    render();
  }

  function render() {
    ["on", "pve", "wvw"].forEach(k => { $(k).checked = cur[k]; });
    $("thickness").value = cur.thickness; $("thickness-v").textContent = cur.thickness;
    $("fill").value = cur.fill; $("fill-v").textContent = cur.fill.toFixed(2);
    for (const k of ["color", "fillColor"]) {
      const [h, l] = toHL(cur[k]);
      if (document.activeElement !== $(k + "-h")) $(k + "-h").value = h;
      if (document.activeElement !== $(k + "-l")) $(k + "-l").value = l;
      $(k + "-p").style.background = css(cur[k]);
      [...$(k + "-sw").children].forEach((b, i) => b.classList.toggle("on", M.SWATCHES[i].every((v, j) => v === cur[k][j])));
    }
  }

  for (const k of ["on", "pve", "wvw"]) $(k).addEventListener("change", e => { cur[k] = e.target.checked; save(); });
  for (const k of ["thickness", "fill"]) $(k).addEventListener("input", e => { cur[k] = Number(e.target.value); save(); });
  for (const k of ["color", "fillColor"]) {
    M.SWATCHES.forEach(c => {
      const b = document.createElement("button");
      b.className = "sw"; b.style.background = css(c); b.title = css(c);
      b.addEventListener("click", () => { cur[k] = c.slice(); save(); });
      $(k + "-sw").appendChild(b);
    });
    const fromSliders = () => { cur[k] = M.color(Number($(k + "-h").value), Number($(k + "-l").value)); save(); };
    $(k + "-h").addEventListener("input", fromSliders);
    $(k + "-l").addEventListener("input", fromSliders);
  }

  // The saved settings arrive on load and whenever they change; apply them every time.
  axidps.on("settings", s => { cur = M.normalize(s); apply(); render(); });
  cur = M.normalize(axidps.settings.get());
  apply();
  render();
})();
