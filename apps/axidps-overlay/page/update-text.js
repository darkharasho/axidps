// What the hub's Updates tab says about the launcher's self-update status
// ({current, latest, state}, written by the launcher to update-status.json).
function updateText(s) {
  if (!s) return "";
  const v = s.latest;
  switch (s.state) {
    case "up_to_date": return "Up to date";
    case "available": return `Version ${v} available`;
    case "waiting_for_game": return `Version ${v} installs when you close the game`;
    case "managed_by_axiom": return `Version ${v} available in AxiOM`;
    case "check_failed": return "Couldn't check for updates";
    case "verify_failed": return "Download failed its check; will retry";
    case "applying": return `Installing ${v}…`;
    default: return "";
  }
}
if (typeof module !== "undefined" && module.exports) module.exports = { updateText };
