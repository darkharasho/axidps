# Plugin settings in the hub: implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A cog on each enabled plugin's hub card opens per-window settings: the host's *Show
title bar* and *Transparent background*, and the settings the manifest declares.

**Architecture:**
- Rust checks `windows[].settings` in the manifest and passes the normalized list to the page.
- `plugin-host.js` gets pure functions for the form model, values and window options, plus host
  methods that save a change and apply it. The values go in `saved.cfg[fullId]`, which is sent
  to the frame. The window options go in `saved.pwin[fullId]` and become classes on the window.
- `ui.js` draws the cog and the panel on the hub card.

**Tech stack:** Rust (`serde_json`) in `crates/axidps-plugins`; plain JS in the overlay page,
tested with `node --test`.

**Spec:** `docs/superpowers/specs/2026-11-27-plugin-settings-design.md`

## Global Constraints

- Types: `toggle`, `choice` (1–12 `options`, each a non-empty string ≤ 40 chars), `number`
  (`min` ≤ `max`, both finite).
- `key` matches `^[a-z0-9-]+$` and is unique within the window. `label` is a non-empty string of
  ≤ 60 chars. A window has at most 16 settings. `default` must fit its type.
- A missing `default` becomes `false`, the first option, or `min`.
- Labels and options are shown as text, never HTML: `esc()` in ui.js.
- Plugin values live in `saved.cfg[fullId]`. Window options live in
  `saved.pwin[fullId] = {bar, clear}`, which no frame ever sees. Defaults are `bar: true` and
  `clear: false`.
- No SDK change.
- cargo runs as `nice -n 19 env CARGO_BUILD_JOBS=6 cargo test -p axidps-plugins`. Don't run
  `cargo fmt`.
- JS tests: `node --test apps/axidps-overlay/page/*.test.js apps/axidps-overlay/sdk/*.test.js`.
- Commits end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Never put the
  owner's real name anywhere.

### Rulings against the spec (each is my call; tell the owner)

1. **No `text` type.** The overlay gets mouse input only: nothing in the DLL or page forwards
   keys, and the hub has no `<input>` anywhere. A `"text"` setting is reported as an unknown
   type.
2. **`number` takes an optional `step`** (> 0, default 1). It is drawn as − / value / +
   buttons, like the hub's *Rows shown*. A typed number field can't be used without keys.
3. **Title bar off uses a new `nobar` class, not `bare`.** `.win.bare` also removes the
   background, which would tie the two options together. `.win.nobar .bar { display: none }`
   leaves the `.bar` element in the DOM, so title, status and error badge code keeps working.
4. **Transparent windows tell the DLL `r: 0, blur: 0`** in `report()`, the same as built-in
   bare windows. Without this, the game's blurred backdrop would still show behind them.

## Review Focus

1. **A plugin's own `settings.set` drops the hub's keys.** The next hub change merges into
   whatever is stored, and missing keys show their defaults. Nothing throws. Test in Task 3.
2. **A stored value of the wrong type**, like `"big": "yes"`, which a plugin wrote itself.
   `settingValues` shows the default for that key. Test in Task 2.
3. **The manifest changes between versions**, so a stored key is no longer declared. The value
   stays in `cfg` (it is the plugin's data) and the form doesn't show it. Test in Task 2.
4. **The panel is open while the plugin is disabled or the list is rescanned.** The cog and
   panel go with the Disable button. The open state lives only for the session. Check by hand in
   Task 4.
5. **`__proto__` / `constructor` as a key or full id.** Keys fail `^[a-z0-9-]+$`. Full ids come
   from validated plugin and window ids, and the host looks entries up with `own()`. Test in
   Task 3: `setSetting("x.y/a", "__proto__", 1)` changes nothing.

---

### Task 1: Manifest `settings` in Rust

**Files:**
- Modify: `crates/axidps-plugins/src/manifest.rs`:
  - `WindowDef`;
  - `parse_window`;
  - a new `parse_settings`;
  - the tests.
- Modify: `crates/axidps-plugins/src/scan.rs:192-195`, to add `"settings": w.settings` to the
  window JSON.
- Modify: `crates/axidps-plugins/src/serve.rs:167`, so the test fixture gets
  `settings: Vec::new()`.
- `grep -rn "WindowDef {" crates apps private 2>/dev/null` and fix any other literal it finds.

**Interfaces:**
- Produces:
  - `WindowDef.settings: Vec<serde_json::Value>`, where each entry is a normalized object.
    - `{"key","type":"toggle","label","default":bool}`
    - `{"key","type":"choice","label","options":[..],"default":str}`
    - `{"key","type":"number","label","min","max","step","default"}`
  - The page JSON's window gets `"settings": [...]`. It is `[]` when there are none.

- [ ] **Step 1: Write the failing tests** in `manifest.rs` `mod tests`. Use the `with()` helper
  to set `v["windows"][0]["settings"]`.
  - `settings_parse_and_fill_defaults`:
    - Input: `[{"key":"big","type":"toggle","label":"Large numbers"}, {"key":"unit","type":"choice","label":"Units","options":["ms","s"]}, {"key":"rows","type":"number","label":"Rows","min":1,"max":20}]`.
    - Parse the manifest. Expect `settings[0]["default"] == false`, `settings[1]["default"] == "ms"`,
      `settings[2]["default"] == 1`, and `settings[2]["step"] == 1`.
  - `settings_absent_is_empty`: `EXAMPLE` gives `windows[0].settings.is_empty()`.
  - `rejects_bad_settings`: each of these is an `Err` whose text contains `windows[0].settings`.
    - not a list;
    - 17 toggles;
    - key `"Big"`;
    - a duplicate key;
    - an empty label;
    - a 61-char label;
    - type `"text"`;
    - type missing;
    - a choice with 0 options;
    - a choice with 13 options;
    - a 41-char option;
    - a choice whose default isn't among its options;
    - a toggle with default `"yes"`;
    - a number with `min` 5 and `max` 1;
    - a number whose default is outside `min`..`max`;
    - `step` 0.
  - `page_json_has_settings`: `to_page_json` of a parsed manifest with one toggle has
    `[0]["windows"][0]["settings"][0]["key"] == "big"`.
- [ ] **Step 2: Run the tests and check they fail.**
  Run `nice -n 19 env CARGO_BUILD_JOBS=6 cargo test -p axidps-plugins settings`. They should
  fail to compile: `WindowDef` has no `settings` field.
- [ ] **Step 3: Write `fn parse_settings(v: Option<&Value>, ctx: &str, out: &mut Vec<String>) -> Vec<Value>`.**
  - Call it from `parse_window` with `ctx = "windows[{i}].settings"`.
  - Problems look like `windows[0].settings[2].default: must be one of the options`.
  - Normalized entries keep only the known fields.
  - Validate the key with `valid_window_id`.
  - Bad settings push problems, which make `parse_window` return `None` through the existing
    `out.len() != before` check.
- [ ] **Step 4: Run the tests and check they pass.**
  Run `nice -n 19 env CARGO_BUILD_JOBS=6 cargo test -p axidps-plugins`. Everything should pass,
  including the old tests.
- [ ] **Step 5: Commit** with the message
  `Plugins: windows can declare settings in the manifest`.

### Task 2: Form model and values (pure JS)

**Files:**
- Modify: `apps/axidps-overlay/page/plugin-host.js`: add the pure functions and export them in
  `pure`.
- Test: `apps/axidps-overlay/page/plugin-host.test.js`.

**Interfaces:**
- Consumes: the page JSON window's `settings` from Task 1. Treat it as untrusted, because the
  browser demo serves it too.
- Produces:
  - `settingsModel(win) -> Array<{key, type, label, default, options?, min?, max?, step?}>`.
    It checks every entry again against the Global Constraints and drops a bad one rather than
    failing. It stops at 16. Labels and options are cut at 60 and 40 chars.
  - `settingValues(model, cfg) -> {[key]: value}`. A stored value that fits its setting wins.
    Otherwise the setting's default is used.
    - A toggle value fits if it is a boolean.
    - A choice value fits if it is one of the options.
    - A number value fits if it is finite and in `min`..`max`.
  - `windowOpts(saved, fullId) -> {bar: boolean, clear: boolean}`, read from `saved.pwin`. The
    defaults are `{bar: true, clear: false}`.
  - `panelModel(p, saved) -> Array<{fullId, title, opts, fields}>`, one entry per valid window
    of `p` (use `validManifestWindows([p])`).
    - `fields` is `settingsModel(win)` with `value` added from `settingValues`.
    - `title` is the window's title cut to `MAX_TEXT`.
  - `fitSetting(field, value) -> {ok: boolean, value}`. Task 3 uses it.
    - A number is snapped to `step` from `min`, then clamped.

- [ ] **Step 1: Write the failing tests.**
  - `settingsModel drops bad entries, fills defaults, caps at 16`. Use the inputs from Task 1's
    reject list and expect each one to be dropped. A good toggle with no default gets
    `default: false`.
  - `settingValues uses stored values that fit, defaults otherwise`:
    - `{big: "yes", rows: 50, unit: "s"}` against the toggle, number (1..20) and choice
      (`ms`/`s`) gives `{big: false, rows: 1, unit: "s"}`.
    - An undeclared key `old: 1` is not in the result.
  - `windowOpts defaults and reads pwin`:
    - `windowOpts({}, "x.y/a")` gives `{bar: true, clear: false}`.
    - With `pwin: {"x.y/a": {bar: false}}` it gives `{bar: false, clear: false}`.
    - A non-boolean stored value gives the default.
  - `panelModel: one section per window, values filled`: a plugin with two windows, one of them
    declaring a toggle.
  - `fitSetting snaps and clamps numbers, refuses the wrong type`: number 1..20, step 2, given
    `4.2`, gives `{ok: true, value: 5}`. A toggle given `"x"` gives `{ok: false}`.
- [ ] **Step 2: Run the tests and check they fail.**
  Run `node --test apps/axidps-overlay/page/plugin-host.test.js`. It should fail with
  `ph.settingsModel is not a function`.
- [ ] **Step 3: Implement the five functions** and add them to `pure`.
- [ ] **Step 4: Run the tests and check they pass.** Run the full JS test command. All tests
  should pass.
- [ ] **Step 5: Commit** with the message `Plugins: the hub's settings form model`.

### Task 3: Saving and applying changes

**Files:**
- Modify: `apps/axidps-overlay/page/plugin-host.js`:
  - `host.add`;
  - `hello` in `host.handle`;
  - the new `host.setSetting`, `host.setWindowOpt` and `host.applyOpts`.
- Modify: `apps/axidps-overlay/page/ui.js`:
  - the `saved` defaults at :183, adding `pwin: {}`;
  - the load at :2112, adding `pwin` with the same object check as `plugins`;
  - `report()` at :433-434, so `r` and `blur` are 0 when `w.def.bare && !w.def.modal` or when
    `w.clear`.
- Modify: `apps/axidps-overlay/page/index.html`. Next to `.win.bare` at :125, add:
  - `.win.nobar .bar { display: none; }`
  - `.win.clear { background: transparent; border-color: transparent; }`
- Test: `apps/axidps-overlay/page/plugin-host.test.js`.

**Interfaces:**
- Consumes: Task 2's `settingsModel`, `settingValues`, `windowOpts` and `fitSetting`.
- Produces (ui.js uses these in Task 4):
  - `host.setSetting(fullId, key, value) -> boolean`.
    - It finds the entry by `fullId` and the field by `key` in `settingsModel(ent.win)`, then
      runs `fitSetting`.
    - It does `saved.cfg[fullId] = Object.assign({}, own(saved.cfg, fullId) && <object> ? that : {}, {[key]: value})`,
      calls `env.save()`, and posts `settings` with
      `Object.assign({}, defaults, saved.cfg[fullId])` if `ent.hello`.
    - Unknown entries, unknown keys and values that don't fit return `false` and change nothing.
  - `host.setWindowOpt(fullId, name, on) -> boolean`. `name` is `"bar"` or `"clear"`. It writes
    `saved.pwin[fullId]` (creating `saved.pwin` if it's missing), calls `env.save()`,
    `applyOpts(ent)` and `env.scheduleReport()`.
  - `host.applyOpts(ent)`:
    - toggles `nobar` and `clear` on `ent.w.el.classList`;
    - sets `ent.w.clear`.
    - `add()` calls it after `defineWindow`. Guard `classList` with an `if`: the fake el in the
      tests has none.
  - On hello, the `settings` message carries `Object.assign({}, defaults from settingValues(settingsModel(win), {}), cfg)`.

- [ ] **Step 1: Write the failing tests.** Use `fakeHost`: give `env.scheduleReport` a stub and
  `defineWindow` an el with a fake `classList` that records `toggle` calls. Add the entry with
  `h.add(w)`, setting `hello = true` and `cw` to a recorder for `postMessage`.
  - `setSetting merges, saves, and sends the whole store`:
    - Start from `cfg: {"x.y/a": {mine: 1}}` and run `setSetting("x.y/a", "big", true)`.
    - The store is now `{mine: 1, big: true}`.
    - The last post is `{type: "settings", value: {mine: 1, big: true}}`.
  - `setSetting refuses unknown keys, wrong types, unknown windows and __proto__`: each returns
    `false`, and `saved` is unchanged (compare with `JSON.stringify`).
  - `after a plugin's own settings.set drops hub keys, the next change merges into it`:
    - Send a frame `settings` message `{value: {mine: 2}}` through `h.handle`.
    - Then run `setSetting("x.y/a", "rows", 3)`.
    - The store is now `{mine: 2, rows: 3}`.
  - `hello sends declared defaults under the stored values`:
    - Declare `big` (default false) and store `{mine: 1}`.
    - The hello reply's settings are `{big: false, mine: 1}`.
  - `setWindowOpt saves pwin and applies the classes; the frame never hears of it`:
    - Run `setWindowOpt("x.y/a", "bar", false)`.
    - `saved.pwin["x.y/a"]` is now `{bar: false, clear: false}`.
    - The classList saw `nobar` on and `clear` off.
    - No post contains `pwin`.
  - `add applies saved window options`: with `pwin: {"x.y/a": {clear: true}}` saved, `add`
    toggles `clear` on.
- [ ] **Step 2: Run the tests and check they fail.** Run
  `node --test apps/axidps-overlay/page/plugin-host.test.js`. The failures should be missing
  methods.
- [ ] **Step 3: Implement the methods,** then make the ui.js and index.html edits listed under
  Files.
- [ ] **Step 4: Run the tests and check they pass.** Run the full JS test command. All tests
  should pass.
- [ ] **Step 5: Commit** with the message
  `Plugins: save and apply per-window settings and window options`.

### Task 4: The cog and the panel in the hub

**Files:**
- Modify: `apps/axidps-overlay/page/ui.js`:
  - the plugin cards at :2040-2057;
  - the `.pcards` click handler at :1991;
  - `GLYPHS` at about :130, if you add a cog glyph.
- Modify: `apps/axidps-overlay/page/index.html`: the card CSS at about :207-216.

**Interfaces:**
- Consumes:
  - `pluginHost.panelModel(p, saved)` from Task 2;
  - `pluginHost.setSetting` and `pluginHost.setWindowOpt` from Task 3.

- [ ] **Step 1: Add the cog button.**
  - The button is `<button data-pact="settings" aria-label="Settings" data-tip="settings">`,
    with the `sliders` glyph (`<canvas class="ico" data-g="sliders">`, the same way the nav
    draws icons). Don't add a new glyph.
  - Show it only when `m.action === "disable"`, before Disable.
  - Which panels are open is kept in `ui.plugOpen`, a `Set` of plugin ids for the session only.
    `data-pact="settings"` toggles the id and calls `renderAll()`.
- [ ] **Step 2: Draw the panel inside the card,** below the `.pl` lines, as `<div class="pset">`.
  - **One section per window,** each with `<h4>` showing the window's title, but only when the
    plugin has more than one window.
  - **Then the two window options** as `.opt` rows with a `.tog`, the same way Settings does it:
    `data-pwin="bar"` / `data-pwin="clear"` and `data-full`.
    - *Show title bar* (on when `opts.bar` is true): "Hides the window's title. Move it in
      Arrange."
    - *Transparent background*: "No background or border behind the plugin."
  - **Then the declared fields,** each carrying `data-full` and `data-k`:
    - a toggle is an `.opt` with a `.tog`;
    - a choice is a `.seg` with one `<button data-v>` per option;
    - a number is a `.step` with −`step` / `<b>` value / +`step` buttons, using `data-d`.
  - **Escaping:** every label, option and title goes through `esc()`.
  - **Redrawing:** the panel's HTML depends on `ui.plugOpen` and on `panelModel`, so add both to
    `plugKey`. For example, append `JSON.stringify(open ids)` and each open plugin's
    `panelModel`. A change then redraws the cards.
- [ ] **Step 3: Handle clicks** in the `.pcards` handler, before the `[data-pact]` branch.
  - `[data-pwin]` calls `pluginHost.setWindowOpt(full, name, !current)`.
  - A field toggle calls `setSetting(full, k, !value)`.
  - A `.seg` button calls `setSetting(full, k, v)`.
  - A step button calls `setSetting(full, k, value + d)`. `fitSetting` clamps it.
  - Then `renderAll()`.
- [ ] **Step 4: Add the CSS.**
  - `.pc .pset`: a top border of `var(--line)`, `margin-top: 6px`, `padding-top: 4px`.
  - Reuse `.opt`, `.seg` and `.step` unchanged where you can. Tighten their padding inside
    `.pset` only.
- [ ] **Step 5: Run the JS tests** with the full JS test command, then load the page headless
  to check it by hand. Use the browser demo
  (`cargo run -p axidps-app -- plugin preview apps/axidps-overlay/plugins/axidps.perf` at
  nice 19), or open `page/index.html` from a static server with `plugins.json`.
  - Use headless chrome only; the owner may be gaming.
  - Check these:
    1. The cog shows on enabled cards only.
    2. The panel opens and closes.
    3. *Show title bar* off hides Performance's bar.
    4. Transparent removes the background.
    5. The choices survive a reload.
    6. Arrange still drags the window.
  - Take a screenshot of the panel.
- [ ] **Step 6: Commit** with the message `Hub: a settings cog on each enabled plugin`.

### Task 5: Performance's example setting, and the docs

**Files:**
- Modify: `apps/axidps-overlay/plugins/axidps.perf/axidps-plugin.json`: add `"settings": [{"key": "big", "type": "toggle", "label": "Large numbers", "default": false}]`.
- Modify: `apps/axidps-overlay/plugins/axidps.perf/perf.js`: in
  `axidps.on("settings", s => document.body.classList.toggle("big", s.big === true))`.
- Modify: `apps/axidps-overlay/plugins/axidps.perf/perf.html`: add
  `body.big .line { font-size: 16px; }` and `body.big .line b { font-size: 12px; }`.
- Modify: `docs/plugins/README.md`.
  - Add a "Settings in the hub" section after `axidps.settings` (around :228):
    - the manifest `settings` field, with the types, limits and defaults from Global
      Constraints;
    - the cog, and the two window options every plugin gets;
    - that the hub merges keys into the window's store and sends the whole store, so
      `settings.set` must keep the keys it didn't write (show
      `axidps.settings.set({...axidps.settings.get(), mine: 1})`);
    - why there's no text type.
  - Add `settings` to the manifest field table. It is near the top; grep `"place"`.
- Check whether anything bundles or hashes the built-in plugins, for example a test that
  compares their manifests: `grep -rn "axidps.perf" crates apps --include=*.rs`. Update any
  fixture it finds.

- [ ] **Step 1: Make the edits.**
- [ ] **Step 2: Run both test suites.** Run the full JS test command and
  `nice -n 19 env CARGO_BUILD_JOBS=6 cargo test -p axidps-plugins -p axidps-app`. Both should
  pass. The bundled Performance manifest has to pass the new checks.
- [ ] **Step 3: Check headless** that *Large numbers* appears in Performance's panel and makes
  the line bigger.
- [ ] **Step 4: Commit** with the message
  `Performance: a Large numbers setting; docs for hub settings`.
