# Plugin settings in the hub

## Why

A plugin's options can only be changed inside the plugin's own window, if the plugin
draws controls at all. Players want to change how a plugin window looks without the
author's help: for example, Performance as a bare line of numbers with no title bar and
no background. Authors also want somewhere to put a few options without building their
own settings screen.

## What the player sees

Each plugin card in the hub's Plugins tab gets a cog button next to Enable/Disable,
shown only while the plugin is enabled. Clicking the cog opens a settings panel under
the card, and clicking it again closes the panel. The panel has one section for each of
the plugin's windows:

- **Window** (every plugin window, from the host):
  - *Show title bar*: on by default.
  - *Transparent background*: off by default. It removes the window's background and
    border.
- **The plugin's own settings**, if the manifest declares any for that window, in the
  order they are declared.

Changes apply at once and are saved. With the title bar hidden, the window is still
moved in Arrange mode, the same way the built-in bare windows are.

## Manifest

Each window can list `settings`:

```json
"settings": [
  {"key": "big", "type": "toggle", "label": "Large numbers", "default": false},
  {"key": "unit", "type": "choice", "label": "Units", "options": ["ms", "s"], "default": "ms"},
  {"key": "rows", "type": "number", "label": "Rows", "min": 1, "max": 20, "default": 5},
  {"key": "name", "type": "text", "label": "Label", "max": 40, "default": ""}
]
```

- The types are `toggle`, `choice`, `number` and `text`.
- `key` follows the window id rules, and keys must be unique within a window.
- `label` is at most 60 characters.
- `choice` needs 1–12 options, each at most 40 characters.
- `number` needs `min` ≤ `max`.
- `text` has a `max` of at most 200 characters.
- A window has at most 16 settings.
- `default` must fit its type. If left out, it is false, the first option, `min`, or an
  empty string.
- The Rust manifest check (`crates/axidps-plugins/src/manifest.rs`) reports a bad
  setting as a manifest problem, the same as other fields. The page checks the list
  again before drawing anything. Labels are shown as text, never as HTML.

## Where values live

- **Plugin settings** go in the window's existing settings store, `saved.cfg[fullId]`.
  - The plugin already reads this store through `axidps.settings` and the `settings`
    event, so the SDK and the plugin API need no changes.
  - A change from the hub merges the key into the store, saves it, and sends the whole
    store to the window as a `settings` message.
  - A window whose store has no value for a declared key gets its `default`.
  - A plugin that calls `settings.set` must keep the keys it didn't write itself. The
    docs say so.
- **Window options** go in a new host-only entry, `saved.pwin[fullId] = {bar, clear}`.
  - The plugin never sees or writes this entry, so `settings.set` can't wipe a player's
    choice. (My call: this changes the earlier proposal, which put them in `cfg`.)
  - The host applies them to the window's element.
  - *Show title bar* off adds the existing `bare` styling: no bar. The arrange overlay
    still lets you drag the window.
  - *Transparent background* on adds a new `clear` class:
    `background: transparent; border-color: transparent`.

## Approaches considered

1. **Hub-drawn form from a declared list (picked).** Every plugin gets the same
   consistent controls, the plugin needs no new code, and the host can apply the window
   options without trusting the plugin.
2. **A plugin-drawn settings page** (a second entry HTML shown in the hub). This is more
   flexible, but every plugin has to build its own UI, and it means a second sandboxed
   frame inside the hub.
3. **Window options only, with no plugin settings.** This covers the Performance example
   but not the general ask.

## Built-in Performance plugin

- Performance declares one setting, *Large numbers* (`big`), and uses it in `perf.js`.
  It mostly serves as a working example for the docs.
- The title bar and background options come from the host, so Performance needs nothing
  extra for those.

## Out of scope

- Settings shared across a plugin's windows (each window has its own store today).
- Plugin-wide settings that live outside any window.
- Validating the values a plugin writes back itself (that's limited only by the existing
  size cap).

## Docs and tests

- `docs/plugins/README.md`: the `settings` manifest field, how the store is merged, and
  the cog.
- Rust tests: manifest checks for each type and each limit.
- `plugin-host.test.js`:
  - the form model built from a manifest;
  - merging and the `settings` message;
  - defaults;
  - `pwin` applied and saved.
- The SDK is unchanged.
