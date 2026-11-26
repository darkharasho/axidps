// Fight: squad totals (duration, damage, downs, strips, ...) for the current fight.
"use strict";

const card = document.querySelector(".card");
const sumOf = (ps, k) => ps.reduce((s, p) => s + (p[k] || 0), 0);

axidps.on("fight", feed => {
  const st = axidps.fightStatus(feed);
  axidps.window.setStatus(st.text, {live: st.live});
  const ps = feed && feed.players || [];
  const dur = feed && !feed.cleared && feed.fight ? feed.fight.duration_ms : 0;
  const me = ps.find(p => p.me) || {};
  const rows = [["Duration", axidps.clock(dur)], ["Squad", ps.length],
    ["Damage", axidps.fmt(sumOf(ps, "damage"))], ["Squad DPS", axidps.fmt(sumOf(ps, "dps"))],
    ["Down contribution", axidps.fmt(sumOf(ps, "down_contrib"))],
    ["Enemies downed", sumOf(ps, "downs")], ["Kills", sumOf(ps, "kills")],
    ["Strips", sumOf(ps, "strips")], ["Cleanses", sumOf(ps, "cleanses")],
    ["Your healing", axidps.fmt((me.healing || 0) + (me.barrier || 0))],
    ["Damage taken", axidps.fmt(sumOf(ps, "taken"))], ["Downed", sumOf(ps, "downed")],
    ["Deaths", sumOf(ps, "deaths")]];
  card.innerHTML = rows.map(([k, v]) => `<span>${axidps.esc(k)}</span><b>${axidps.esc(v)}</b>`).join("");
});
