// Timeline: squad damage per second over the fight, dealt (green) and taken (red), as a 5 s
// rolling average.
"use strict";

const canvas = document.querySelector("canvas");
const peak = document.querySelector(".peak");
let feed = null, font = "";
// A theme colour, from the CSS variables the SDK sets.
const ink = k => getComputedStyle(document.documentElement).getPropertyValue("--" + k).trim();

function render() {
  const st = axidps.fightStatus(feed);
  axidps.window.setStatus(st.text, {live: st.live});
  const g = canvas.getContext("2d");
  const W = canvas.width, H = canvas.height, padL = 40, padB = 16, padT = 6, padR = 6;
  g.clearRect(0, 0, W, H);
  const tl = feed && !feed.cleared ? feed.timeline : null;
  const bucket = (tl && tl.bucket_ms) || 1000;
  const smooth = a => a.map((_, i) => {
    const s = a.slice(Math.max(0, i - 4), i + 1);
    return s.reduce((x, y) => x + y, 0) / s.length * 1000 / bucket;
  });
  const out = smooth((tl && tl.out) || []), inc = smooth((tl && tl.in) || []);
  const n = Math.max(out.length, inc.length);
  const top = Math.max(1, ...out, ...inc);
  const x = i => padL + (n <= 1 ? 0 : i / (n - 1) * (W - padL - padR));
  const y = v => H - padB - v / top * (H - padB - padT);
  g.strokeStyle = ink("line"); g.lineWidth = 1;
  g.font = "10px " + font; g.fillStyle = ink("faint");
  for (const f of [0, 0.5, 1]) {
    const yy = Math.round(y(top * f)) + 0.5;
    g.beginPath(); g.moveTo(padL, yy); g.lineTo(W - padR, yy); g.stroke();
    if (n > 1) g.fillText(axidps.fmt(top * f), 2, yy + 3);
  }
  if (n > 1) {
    g.fillText("0:00", padL, H - 3);
    const end = axidps.clock(n * bucket);
    g.fillText(end, W - padR - g.measureText(end).width, H - 3);
  }
  const line = (a, col) => {
    if (a.length < 2) return;
    g.strokeStyle = col; g.lineWidth = 1.5; g.beginPath();
    a.forEach((v, i) => i ? g.lineTo(x(i), y(v)) : g.moveTo(x(i), y(v)));
    g.stroke();
  };
  line(inc, ink("bad"));
  line(out, ink("good"));
  if (n < 2) {
    g.fillStyle = ink("dim"); g.textAlign = "center";
    g.fillText(axidps.emptyText(feed), W / 2, H / 2); g.textAlign = "start";
  }
  peak.textContent = n > 1 ? `peak ${axidps.fmt(Math.max(...out))} dealt` : "";
}

axidps.on("fight", f => { feed = f; render(); });
axidps.on("look", l => {
  font = l.style && l.style.font || "";
  render();
  // The canvas text needs the theme's font: draw again once it has loaded.
  setTimeout(render, 300);
  setTimeout(render, 1500);
});
