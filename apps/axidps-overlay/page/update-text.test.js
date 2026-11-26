const test = require("node:test");
const assert = require("node:assert");
const { updateText } = require("./update-text.js");

const cases = [
  [{ state: "up_to_date", latest: null }, "Up to date"],
  [{ state: "available", latest: "0.2.1" }, "Version 0.2.1 available"],
  [{ state: "waiting_for_game", latest: "0.2.1" }, "Version 0.2.1 installs when you close the game"],
  [{ state: "managed_by_axiom", latest: "0.2.1" }, "Version 0.2.1 available in AxiOM"],
  [{ state: "check_failed", latest: null }, "Couldn't check for updates"],
  [{ state: "verify_failed", latest: "0.2.1" }, "Download failed its check; will retry"],
  [{ state: "applying", latest: "0.2.1" }, "Installing 0.2.1…"],
  [null, ""],
  [{ state: "bogus" }, ""],
];
for (const [s, want] of cases) {
  test(`updateText ${JSON.stringify(s)}`, () => assert.strictEqual(updateText(s), want));
}
