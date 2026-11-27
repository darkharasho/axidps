// Fills the line from axidps's "perf" event (once a second; 0 = not known yet).
(function () {
  "use strict";
  const el = c => document.querySelector("." + c);
  const show = (c, v, bad) => {
    const e = el(c);
    e.textContent = v > 0 ? String(v) : "–";
    e.classList.toggle("warn", v > 0 && bad(v));
  };
  // "Large numbers", from the hub's settings panel.
  axidps.on("settings", s => document.body.classList.toggle("big", s.big === true));
  axidps.on("perf", p => {
    show("fps", p.fps, v => v < 30);
    show("ping", p.ping, v => v > 250);
    // The server runs at 25 ticks a second; far fewer means it is lagging.
    show("tick", p.tick_rate, v => v < 20);
  });
})();
