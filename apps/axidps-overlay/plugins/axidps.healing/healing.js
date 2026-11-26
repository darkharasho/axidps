// Healing: each player's total outgoing healing and barrier, like the arcdps healing addon's peers
// outgoing: you, and squadmates sharing their heals. Anyone else is left out, since for them only
// what passed between you is known.
"use strict";

const box = document.querySelector(".heal");
const list = document.querySelector(".hl");
const sum = document.querySelector(".sum");
const {fmt, esc} = axidps;
let feed = null, look = {compact: false, rows: 15};

function render() {
  const st = axidps.fightStatus(feed);
  axidps.window.setStatus(st.text, {live: st.live});
  const k = look.compact ? 0.8 : 1, px = n => Math.round(n * k);
  box.style.width = px(320) + "px";
  const ps = feed && !feed.cleared ? feed.players || [] : [];
  const rows = ps.filter(p => p.heal_full && (p.me || p.healing + p.barrier > 0))
    .sort((a, b) => (b.healing + b.barrier) - (a.healing + a.barrier));
  const top = Math.max(1, ...rows.map(r => r.healing + r.barrier));
  list.innerHTML = rows.length
    ? `<table><colgroup><col><col style="width:${px(60)}px"><col style="width:${px(56)}px">` +
      `<col style="width:${px(50)}px"></colgroup><thead><tr><th class="l">Name</th>` +
      `<th data-tip="healing given">Heal</th><th data-tip="barrier given">Barr</th>` +
      `<th data-tip="healing and barrier per second">HPS</th></tr></thead><tbody>` +
      rows.slice(0, Math.max(1, look.rows | 0)).map(r => {
        const col = axidps.profColor(r.prof), f = ((r.healing + r.barrier) / top * 100).toFixed(1);
        return `<tr class="p${r.me ? " me" : ""}"><td class="l name" style="border-left-color:${col};` +
          `background:linear-gradient(to right, ${col}38 ${f}%, transparent ${f}%)">` +
          `${esc(r.me ? r.name + " (you)" : r.name || "unknown")}</td>` +
          `<td>${fmt(r.healing)}</td><td>${fmt(r.barrier)}</td><td>${fmt(r.hps)}</td></tr>`;
      }).join("") + `</tbody></table>`
    : `<div class="empty" style="display:block">${esc(axidps.emptyText(feed))}</div>`;
  const tot = rows.reduce((a, r) => [a[0] + r.healing, a[1] + r.barrier], [0, 0]);
  sum.textContent = rows.length ? `${fmt(tot[0])} healing · ${fmt(tot[1])} barrier · ${rows.length} players` : "";
}

axidps.on("fight", f => { feed = f; render(); });
axidps.on("look", l => { look = l; render(); });
