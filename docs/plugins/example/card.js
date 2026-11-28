// Your DPS: your own DPS, big. Click the number to switch between big and small; the choice is
// this window's setting `big`, saved with settings.set.
"use strict";

const dps = document.querySelector(".dps");
let big = true, scale = 1, me = null;

function render() {
  const size = (big ? 32 : 18) * scale;
  dps.style.fontSize = size + "px";
  dps.textContent = axidps.fmt(me ? me.dps : 0);
  axidps.window.tip(dps, big ? "click for a small number" : "click for a big number");
}

axidps.on("fight", feed => {
  const st = axidps.fightStatus(feed);
  axidps.window.setStatus(st.text, {live: st.live});
  const players = feed && !feed.cleared ? feed.players || [] : [];
  me = players.find(p => p.me) || null;
  render();
});

axidps.on("look", look => { scale = look.textScale; render(); });

axidps.on("settings", s => { big = s.big !== false; render(); });

dps.addEventListener("click", () => {
  big = !big;
  axidps.settings.set({big});
  render();
});
