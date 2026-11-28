# Writing axidps plugins

This guide is for people who want to add their own windows to the axidps overlay. It covers
how to build and test a plugin, the manifest, permissions, the API, theming and the limits.
The example plugin in [`example/`](example/) is walked through below. The bundled Performance
window in `apps/axidps-overlay/plugins/` is a plugin too, built on the same public API.

## What a plugin is

A plugin is a folder of plain HTML, CSS and JavaScript with a manifest, `axidps-plugin.json`.
There is no build step. Each plugin adds one to four windows to the in-game overlay. Each
window is a sandboxed frame that loads one of your HTML files. Your page talks to axidps only
through the plugin SDK, `axidps.js`. Through it you get the live fight data (if you ask for it),
the player's theme, and a small settings store, and you can set the window's title and status.

The player installs a plugin by putting its folder in their plugins folder, then turns it on
in the hub's Plugins tab. The card there shows what the plugin asks for, and nothing loads until
the player clicks Enable.

## Developing

### Check the manifest

```
axidps plugin check <folder>
```

This prints `ok: <id> <version>` and exits 0, or prints one `problem: <field>: <what is wrong>`
line for every problem and exits 1.

### Preview in a browser

```
axidps plugin preview <folder>
```

This serves the overlay page, the SDK, the bundled plugins and your plugin on
`http://127.0.0.1:8711/index.html?demo` and opens it in your browser. The page plays a sample
fight that keeps going, and your plugin's windows start open and enabled. When you save a file
in your plugin folder, its windows reload. Your plugin gets the same Content-Security-Policy as
in the overlay, so anything it blocks there is blocked here too.

Options: `--port <n>` (default 8711), `--no-open` (don't open the browser). The page takes the
demo's own options after `?demo`: `&theme=frost` (any theme id, see [Theming](#theming)) and
`&edit` (arrange mode).

### Work in the game

Put your plugin in the player plugins folder (see [Installing](#installing)). While you work on
it, it is easiest to link the folder to your checkout instead of copying it:

```
# Linux
ln -s ~/src/my-plugin ~/.config/axidps/plugins/my-plugin
# Windows (cmd)
mklink /D "%APPDATA%\axidps\plugins\my-plugin" C:\src\my-plugin
```

The plugin folder itself may be a link. Links *inside* the folder that point outside it are not
served.

### Developer mode

Turn on Developer mode in the hub, under Settings > Plugins. As the hub says, "The overlay
restarts to switch developer mode." Developer mode changes how fast you see your work, never what a plugin may do: the
sandbox, the Content-Security-Policy and the permissions stay the same. With it on:

- **Reload on save.** The overlay checks the player plugins folder every 250 ms. Once a
  plugin's files have been still for 200 ms, its windows reload (within about a second of the
  save) and keep their position, open state and settings. If you changed the manifest, the list
  is read again first. If the manifest now asks for more than the player allowed, the plugin is
  switched off until they click Enable again. A plugin folder with more than 2000 files is not
  watched.
- **Error counts.** A plugin window's title bar shows a red count of the errors its frame logged
  since it last loaded. Hover it to see the last 5.
- **Reload.** Each enabled player plugin's hub card gets a Reload button, for changes the
  watcher can't see (on a network share, for example).
- **Developer tools.** The overlay runs Servo's developer-tools server on `127.0.0.1:6080`
  (this machine only). In Firefox, open `about:debugging`, add `127.0.0.1:6080` as a network
  location and connect. You can then inspect your frames, use the console and set breakpoints.

### Developer passes

A developer pass lets your own unverified plugin use deeper permissions while you build it.
It works in developer mode only, never for store installs, and only until it expires.

To ask for one, run `axidps plugin dev-key`. It makes a key on this machine the first time and
prints `public key: ...`. Send that key and your GitHub login to the axidps owner (`darkharasho`).

`axidps plugin dev-key --new` makes a new key. A new key cancels any pass you already have.

### The log

`console.warn`, `console.error`, uncaught errors and unhandled promise rejections in your frame
go to the axidps log as `plugin <id>: <message>`. `console.log` does not. Each line is cut to
500 characters on one line. About 20 lines a second are kept per plugin; past that, lines are
dropped and one `… N lines dropped` line says how many.

Where the log is:

- **Windows:** `%APPDATA%\axidps\axidps.log` (the previous one is `axidps.log.1`).
- **Linux:** `~/.config/axidps/axidps.log`, or the terminal if you started axidps from one. If
  you run the overlay as the `axidps-overlay` systemd user unit (a source install), use
  `journalctl --user -u axidps-overlay`.

## The example: "Your DPS"

[`example/`](example/) is a small window that shows your own DPS in large type:

- `axidps-plugin.json` names one window, `card`, with the `fight` permission so it gets the live
  feed.
- `card.html` loads the SDK with `<script src="/_sdk/axidps.js"></script>`, then `card.js`. Its
  styles use the theme's CSS variables (`--title`, `--dim`), so it matches every theme.
- `card.js` listens to `fight`: it finds your row (`me: true`), shows its `dps` with
  `axidps.fmt`, and sets the title bar status from `axidps.fightStatus(feed)`.
- It listens to `look` to draw the number a little smaller in compact mode.
- Clicking the number switches between big and small. The choice is saved with
  `axidps.settings.set({big})` and comes back through the `settings` event the next time the
  window loads.
- The window sizes itself to its content; the code never sets a size.

Try it with `axidps plugin preview docs/plugins/example`.

## The folder and manifest

```
my-plugin/
  axidps-plugin.json
  card.html
  card.js
  ...any other files: CSS, images, fonts
```

The folder's name doesn't matter; the manifest's `id` does. The folder may not have a top-level
`_sdk` folder (that path is the SDK's). Paths are case-sensitive on Linux.

`axidps-plugin.json` (at most 64 KB):

```json
{
  "id": "example.your-dps",
  "name": "Your DPS",
  "version": "1.0.0",
  "author": "axidps",
  "api": 1,
  "permissions": ["fight"],
  "network": [],
  "windows": [
    {"id": "card", "title": "Your DPS", "entry": "card.html", "description": "your DPS, big",
     "icon": "chart", "open": true, "place": "beside-meter"}
  ]
}
```

| Field | Required | Rules |
| --- | --- | --- |
| `id` | yes | 3–48 characters of `a-z 0-9 . -`. Unique: if your id is the same as a bundled plugin's, yours is ignored (the log says so). |
| `name` | yes | Shown on the hub card. |
| `version` | yes | A semantic version, like `1.0.0`. |
| `author` | no | Shown on the hub card. |
| `api` | yes | Must be `1`. Any other number lists the plugin as "needs a newer axidps" and it doesn't load. |
| `permissions` | no | A list from `fight` (basic) and `world`, `position`, `squad`, `enemies`, `textures` (deeper), and `outline` (built in only). Any other name makes the plugin invalid. See [Permissions](#permissions). |
| `network` | no | Host names your plugin may reach over HTTPS on port 443, like `api.example.com`, or `*.example.com` for any subdomain (not the bare domain). No scheme, port or path. Public names only: no IP addresses, no single-label names like `localhost`, no `*.com`, and nothing under `.localhost`, `.local`, `.lan`, `.internal` or `.home.arpa`. |
| `windows` | yes | 1 to 4 windows. |

Each window:

| Field | Required | Rules |
| --- | --- | --- |
| `id` | yes | `a-z 0-9 -`, unique within the plugin. |
| `title` | yes | The title bar text and the window's name in the hub. |
| `entry` | yes | The HTML file, a path inside the plugin folder (`card.html`, `ui/main.html`). |
| `description` | no | A short line for the hub's Windows tab. |
| `icon` | no | One of `chart users shield swords activity x grid layers sliders plug download columns plus`. Anything else shows `plug`. |
| `open` | no | `true` to show the window until the player closes it. Default `false`: the player opens it from the hub's Windows tab. |
| `place` | no | Where the window first appears: `beside-meter` (default; in a row to the right of the Support window) or `below-meter`. The player can move it; its position is saved. |
| `in` | no | `overlay` (default) or `hub`. A `hub` window is drawn as a page in the hub's Options tab, listed under Plugins, with an on/off switch for the plugin at the top. `open` and `place` are ignored for it, it isn't listed in the Windows tab, and it keeps running while the hub is closed. Its frame is as wide as the page; `resize` sets only its height. axidps versions before 0.2.0-rc.18 ignore this key and show the window as an ordinary window. |
| `settings` | no | Options the player sets from the hub's cog, at most 16. See [Settings in the hub](#settings-in-the-hub). |

Text the page shows (titles, names, statuses) is cut to 80 characters.

## Permissions

| Permission | Level | The player sees |
| --- | --- | --- |
| `fight` | basic | "Reads your fight data" |
| `world` | deeper | "Draws over the game world" |
| `position` | deeper | "Reads your position and camera" |
| `squad` | deeper | "Reads your squad and party" |
| `enemies` | deeper | "Reads enemy players seen in combat" |
| `textures` | deeper | "Changes how the game's icons look (commander tags, markers, targets)" |
| `outline` | privileged | "Outlines your character" |

Each `network` host adds the line "Talks to <domain>".

The hub card lists these lines. When the player clicks Enable, they allow exactly what the
plugin asks for at that moment. If a later version of the manifest asks for more, the plugin is
off until they click Enable again.

Only `fight` works today. A plugin that names a deeper permission is listed with
"Needs verification" and is never loaded. Deeper permissions will need the axidps team's review
of the plugin, which arrives with the plugin store in a later release. `textures` has its API
([`axidps.textures`](#axidpstextures)); the others don't have one yet.

`outline` is reserved for plugins built into axidps; any other plugin that names it doesn't load.

Every plugin you install yourself shows the label "Unverified" on its hub card and in its
window's title-bar tooltip. Bundled plugins show no label. Verification also arrives with the
plugin store.

## API reference

Load the SDK first, in every window's HTML:

```html
<script src="/_sdk/axidps.js"></script>
```

It sets `window.axidps`. The overlay serves the copy that matches the running axidps, so never
ship your own. It also adds the overlay's fonts and base styles to your page (see
[Theming](#theming)).

### `axidps.ready(fn)`

Calls `fn(info)` once the page has answered your frame (or at once, if it already has).
`info` is `{plugin, window, api, permissions}`: your plugin id, this window's id, the API
version (`1`) and the permissions the manifest names.

### `axidps.on(type, fn)`

Calls `fn(value)` each time the page sends `type`. A handler added later gets the latest value
at once. Each handler gets its own copy of the value. Errors thrown in a handler are logged and
don't stop other handlers.

| `type` | `value` |
| --- | --- |
| `"fight"` | The live feed (see [The live feed](#the-live-feed)), or `null` until axidps core has sent one. Only sent if the manifest names `fight`. |
| `"look"` | `{theme, tokens, style, opacity, compact, rows}` (see [Theming](#theming)). |
| `"edit"` | `true` while the player is arranging windows. Your frame gets no mouse input then. |
| `"settings"` | This window's saved settings (`{}` if none). |
| `"perf"` | The game's performance over the last second, once a second: `{fps, ping, tick_rate}` (frames per second, ping in ms, server ticks per second, normally 25). `0` means not known yet. Needs no permission. |

### `axidps.settings`

- `axidps.settings.get()`: a copy of this window's settings (`{}` until they arrive).
- `axidps.settings.set(obj)`: replace this window's settings with `obj` and save them. `obj`
  must be a plain object, at most 16 KB as JSON; a larger one is not saved (the SDK logs an
  error and the settings stay as they were).

Settings are per window and are kept when the plugin is switched off and on again. They are the
only storage a plugin has.

### Settings in the hub

Each enabled plugin's card in the hub has a cog. It opens a panel with, for each of the plugin's
windows:

- two options every plugin window gets, kept by axidps and never sent to your page:
  - *Show title bar* (on by default). Off hides the title bar; the player still moves the window
    in Arrange. The bar comes back while the window is not responding, or, in developer mode,
    while it has errors, so those warnings still show.
  - *Transparent background* (off by default). On removes the window's background, border and
    blurred backdrop, so only what your page draws shows.
- the settings your manifest declares for that window, in order:

```json
"settings": [
  {"key": "big", "type": "toggle", "label": "Large numbers", "default": false},
  {"key": "unit", "type": "choice", "label": "Units", "options": ["ms", "s"], "default": "ms"},
  {"key": "rows", "type": "number", "label": "Rows", "min": 1, "max": 20, "step": 1, "default": 5}
]
```

| Field | Rules |
| --- | --- |
| `key` | `a-z 0-9 -`, unique within the window. The name of the value in `axidps.settings`. |
| `type` | `toggle`, `choice` or `number`. |
| `label` | 1–60 characters, shown as text. |
| `options` | `choice` only: 1–12 names of 1–40 characters each. |
| `min`, `max` | `number` only: required, `min` ≤ `max`. |
| `step` | `number` only: above 0, default 1. The panel shows − and + buttons that move by it. |
| `default` | Must fit the type. Left out, it is `false`, the first option, or `min`. |

A bad entry makes the manifest invalid, like any other field (`axidps plugin check` says which).
There is no text type: the overlay gets mouse clicks from the game but no keys, so nothing could
be typed into it.

The values live in the window's settings store, the same one `axidps.settings` reads. A
declared key that isn't stored yet comes to your page as its `default`. When the player changes
one, axidps merges it into the stored object, saves it, and sends the whole object in a
`settings` event. So when your page calls `settings.set`, keep the keys it didn't write itself:

```js
axidps.settings.set({...axidps.settings.get(), mine: 1});
```

The bundled Performance plugin is a small example: its *Large numbers* toggle makes the line
bigger.

### `axidps.window`

- `axidps.window.setTitle(text)`: the title bar text (`""` puts back the manifest's `title`).
- `axidps.window.setStatus(text, {live})`: the small status beside the title. `live: true`
  shows it in the "in combat" style.
- `axidps.window.autoSize(on)`: on by default. The window follows the size of your page's
  `<body>`.
- `axidps.window.resize(w, h)`: set the size yourself. Call `axidps.window.autoSize(false)`
  first, or the next body size change replaces it.
- `axidps.window.tip(el, text)`: show `text` as a tooltip when the mouse is over `el`. A
  `data-tip="..."` attribute on any element does the same.

### `axidps.textures`

Needs the `textures` permission. Restyles the game's own icons: recolours them, outlines them,
puts a ring behind them, scales or fades them. axidps finds the icons in the game's textures and
does the drawing; your plugin only says how each one should look.

- `axidps.textures.targets`: the icons a rule can name: `commander.red`, `commander.orange`,
  `commander.yellow`, `commander.green`, `commander.cyan`, `commander.blue`, `commander.purple`,
  `commander.magenta`, `commander.white`, `squad.commander`, `squad.lieutenant`, `mentor`,
  `marker.arrow`, `marker.circle`, `marker.heart`, `marker.square`, `marker.star`,
  `marker.spiral`, `marker.triangle`, `marker.cross`, `selection.orange`, `selection.blue`,
  `selection.green`, `selection.red`, `target`, `target.personal`, `target.combined`,
  `target.attack`, `target.attack-selected`. One target covers every version of that icon (on
  the world, the cat-shaped commander tag, the minimap).
- `axidps.textures.set(rules)`: replace your plugin's rules (at most 64). Rules stay while your
  plugin is enabled, so set them again whenever your settings load or change.
- `axidps.textures.clear()`: remove your plugin's rules.

A rule: every field but `target` is optional, and values outside their range are clamped.

| Field | Values | Default | |
| --- | --- | --- | --- |
| `target` | a name from `targets` | | The icon. |
| `mode` | `keep`, `oklab`, `blend`, `luminance`, `solid` | `keep` | How the icon's colours become `color`. `oklab` keeps its shading around the new colour's lightness; `blend` keeps its lightness and takes the new colour's hue; `luminance` multiplies its brightness by the colour; `solid` fills it flat. `keep` leaves the colours alone. |
| `color` | `"#rrggbb"`, `"#rgb"`, `"#rrggbbaa"` or `[r, g, b]` | white | The new colour. |
| `brightness` | 0 to 3 | 1 | Multiplies the result. |
| `contrast` | -0.3 to 0.3 | 0 | `oklab` only: flattens or deepens the shading. |
| `keepExtremes` | 0 to 1 | 0.8 | `oklab` only: how much of the icon's own highlights and shadows survive. |
| `normalize` | true, false | true | `oklab` only: lifts the darkest shading a little. |
| `outline` | `{style, width, color, outer, softness, minimap}` or absent | none | Drawn under the icon. `style`: `round`, `diamond` (pointed corners) or `double` (the colour inside, `outer` beyond). `width`: pixels of a 128 px icon, 0 to 16 (default 4). `outer`: a colour, or `"auto"` (black or white, whichever stands out). `softness`: 0.5 to 8, higher is a harder edge (default 3). `minimap`: the width on minimap icons, pixels of a 32 px icon, 0 to 4 (default 1). |
| `ring` | `{radius, thickness, color, outline}` or absent | none | A ring behind the icon. `radius` and `thickness` are fractions of half the icon's width (defaults 0.45 and 0.08). `outline`: a colour, `"auto"`, or absent for none. Not drawn on minimap icons. |
| `scale` | 0.5 to 2 | 1 | Size about the centre (not on minimap icons). |
| `offsetY` | -0.5 to 0.5 | 0 | Moves the icon down by this fraction of its height. |
| `opacity` | 0 to 1 | 1 | 0 hides the icon. |

```js
axidps.textures.set([
  {target: "commander.red", mode: "oklab", color: "#d55e00",
   outline: {style: "round", width: 6, color: "#ffffff"}},
  {target: "marker.star", ring: {color: "#000000", outline: "auto"}},
]);
```

When two plugins style the same icon, the one whose id sorts last wins. axidps leaves icons alone
while the Commander Tag Accessibility addon is loaded, since it restyles the same icons. The list
of icon textures matches the one that addon uses, with thanks.

### Helpers

The same helpers the built-in windows use:

- `axidps.fmt(n)`: a short number: `1,234`, `12.3k`, `456k`, `1.23M`, `12.3M`.
- `axidps.clock(ms)`: `m:ss`.
- `axidps.esc(s)`: `s` escaped for HTML.
- `axidps.profColor(prof)`: the colour for a profession id (`axidps.PROF` is the list, index 1–9:
  Guardian, Warrior, Engineer, Ranger, Thief, Elementalist, Mesmer, Necromancer, Revenant).
- `axidps.specIcon(spec)`: the URL of a specialization's icon for a player's `spec` (like `"Firebrand"`), the icons the built-in tables show: `<img src="${axidps.specIcon(p.spec)}">`. `""` for no spec.
- `axidps.fightStatus(feed)`: `{text, live}` for the title bar status, like the built-in
  windows: "in combat 1:23" (live), "last fight 1:23", "out of combat", "cleared" or
  "waiting for core".
- `axidps.emptyText(feed)`: what an empty window says for this feed, like "No fight yet.".
- `axidps.api`: `1`.

### Errors

See [The log](#the-log). Errors (`console.error`, uncaught errors, unhandled rejections) also
count toward the developer mode error count.

## The live feed

The `fight` event's value. axidps core writes it at most twice a second while something
changes, and the page passes it on.

| Field | Meaning |
| --- | --- |
| `v` | The feed's format version, `1`. |
| `t` | When it was written (Unix time, ms). |
| `state` | `"idle"` (no fight yet), `"fight"` (in combat) or `"last"` (the last fight, ended). |
| `cleared` | `true` after the player used Clear fight in the hub; `players` is then empty until the next fight. |
| `fight` | `{duration_ms, map}`: the fight's length so far and the map id (`0` when idle). |
| `players` | Your squad's rows, highest damage first (below). |
| `timeline` | `{bucket_ms, out, in}`: the squad's damage dealt (`out`) and taken (`in`) per second (`bucket_ms` is `1000`), or `null` when idle. |
| `healing` | `{out, in}`: your healing given by target and received by source, largest first. Each entry is `{name, prof, elite, me, healing, barrier}`. `null` when idle. |
| `detail` | The breakdown of the player open in the built-in Details window, or `null` when it is closed: `{account, name, prof, spec, bucket_ms, out, in, skills_out, skills_in, states, out_condi, heal_full, heal_out, heal_in, heals, heals_in, heal_targets, heal_sources, strip_series, cleanse_series, strips, cleanses, casts, skill, boon}`. `out`/`in` are that player's damage dealt and taken per second, `out_condi` the condition part of `out`; `skills_out`/`skills_in` list up to 60 skills each as `{id, name, damage, hits, condi}` (`condi`: a condition), largest first, with any of `min`, `max` (the smallest and largest hit), `crit`, `glance`, `block`, `evade`, `interrupt`, `absorb`, `miss` (strikes by result) and `barrier` (damage that hit barrier) that are above zero; `states` lists `[ms into the fight, "down" \| "dead" \| "up"]`. `heal_out`/`heal_in` are healing plus barrier given and received per second; `heals`/`heals_in` list skills as `{id, name, healing, barrier, hits}`, `heal_targets`/`heal_sources` players as `{name, account, prof, healing, barrier}`; for anyone but you and players sharing their heals (`heal_full` false) these hold only what passed between them and you. `strips`/`cleanses` list the boons stripped and conditions cleansed as `{id, name, count}` (stacks), and `strip_series`/`cleanse_series` count them per second. `casts` lists the player's cast starts as `[ms into the fight, skill id]`, oldest first; `skill` is the skill selected in the Details window as `{id, out, heal}` (its damage dealt and healing given per second), or `null`; when the selection is a squad boon, `boon` is `{id, up}`: the player's uptime of it per second in milliseconds (stack-milliseconds for Might and Stability), else `null`. |

Each player row:

| Field | Meaning |
| --- | --- |
| `name`, `account` | Character and account name. |
| `prof`, `elite`, `spec` | Profession id (1–9), elite specialization id, and its name. |
| `group` | Subgroup (0 if none). |
| `me` | `true` for you. |
| `damage`, `power`, `condi` | Damage dealt: total, strike, condition. |
| `dps` | Damage per second. |
| `downs`, `kills` | Enemies downed and killed. |
| `down_contrib` | Down contribution: damage that led to enemy downs. |
| `deaths`, `downed` | Times died and times downed. |
| `taken` | Damage taken. |
| `strips`, `cleanses` | Boon stacks stripped from enemies; condition stacks cleansed from allies. |
| `healing`, `barrier`, `hps` | Healing and barrier given, and both per second. |
| `healing_in`, `barrier_in` | Healing and barrier received. |
| `heal_full` | `true` if the healing numbers are complete (you, and players sharing their heals). Otherwise they only count what passed between that player and you. |
| `boons` | Boon uptime and generation: `{"<boon id>": [up, self, group, squad]}`, in milliseconds since the fight started (stack-milliseconds for Might and Stability). `group` and `squad` are per target: the time the player kept the boon up on subgroup mates (or squad mates), averaged over them. Only boons with a nonzero value appear; the field is left out when there are none. The values can go down a little. See [Boons](../custom-windows-stats.md#boons). |

## Theming

The SDK sets the player's theme on your page as CSS variables and keeps them current. Use them
and your window matches every theme (`midnight glass terminal ember verdant frost contrast`).

Colours (the `look` event's `tokens`, set as `--<name>`):

| Variable | Use |
| --- | --- |
| `--bg`, `--bar` | Window and title bar background. |
| `--raise`, `--raise-hi` | Raised surfaces (rows, cards), and their hover. |
| `--btn`, `--btn-hi` | Buttons, and their hover. |
| `--line`, `--line-hi` | Borders and dividers, and a stronger one. |
| `--text`, `--title`, `--dim`, `--faint` | Body text, headings, secondary and faint text. |
| `--accent`, `--accent-hi`, `--accent-line`, `--accent-soft` | The accent colour, a lighter one, an accent border, and an accent tint. |
| `--good`, `--bad` | Good and bad numbers. |
| `--tip` | Tooltip background. |

The player's opacity setting is already applied to `--bg`, `--bar`, `--raise` and `--raise-hi`.
Your page's own background is transparent: the window around it draws the background.

Shape and type (the `look` event's `style`):

| Variable | Use |
| --- | --- |
| `--font` | The font stack. IBM Plex Sans, Fira Code and Lato are included. |
| `--fs` | Font size. |
| `--r`, `--rs` | Window and control corner radius. |
| `--bw` | Border width. |
| `--rh`, `--bh` | Table row height and title bar height. |

The SDK also adds the class `compact` to `<body>` in compact mode (it makes `--rh`, `--bh` and
`--fs` smaller) and `caps` for themes with uppercase titles and headers.

The `look` event's other fields: `theme` (the theme id), `style.bar` (`"fill"`, `"flat"` or
`"line"`), `style.rows` (`"fill"` or `"line"`, how meter rows draw their bars), `style.caps`,
`opacity` (0.5–1), `compact` (bool) and `rows` (the most table rows the player wants shown,
1–50).

The SDK's base styles make the page transparent, set the font, turn off text selection, and make
`<body>` `inline-block` and as wide as its content, so the window fits it.

## Limits and the sandbox

- A window is 40–1000 px wide and 24–1000 px tall (fractions are fine). It is 240x120 until your
  page reports its first size. Size changes are applied at most 4 times a second.
- At most 4 windows per plugin. The game shows at most 16 windows at once; 2 are kept for the
  hub and Arrange, so every other window, built-in or plugin, shares 14. Past that, windows stay
  closed and the hub says so.
- Settings: at most 16 KB of JSON per window.
- A frame that hasn't answered for 5 s shows "not responding" in its title bar. The SDK answers
  for you, so this only happens when your code blocks the page (a long loop, for example).
- Each window is an `<iframe sandbox="allow-scripts">` with its own opaque origin. It can't
  reach the overlay page, other plugins, or the player's files. There is no `localStorage`,
  cookies or IndexedDB; use `axidps.settings`. Popups, dialogs (`alert`), forms and navigating
  the overlay are not available.
- The Content-Security-Policy allows scripts, styles, fonts and images from your own folder,
  inline scripts and styles, and `data:` images. `fetch` and `XMLHttpRequest` may reach only
  your `network` hosts, over HTTPS (images may load from them too). No workers, no nested
  frames, nothing else. Redirects aren't followed: a request your host answers with a redirect
  fails, so point `fetch` at the final URL. Your window can't navigate to a web page either,
  even one on your own hosts: load data with `fetch` and render it yourself.
- Hosts are checked by name, not by address. A name you declare could later be pointed at the
  player's own network, but only HTTPS on port 443 is allowed, and a device there can't present
  a valid certificate for your domain, so such a connection fails.

## Installing

Copy the plugin folder into the player plugins folder:

- **Windows:** `%APPDATA%\axidps\plugins\`
- **Linux:** `~/.config/axidps/plugins/` (`$XDG_CONFIG_HOME/axidps/plugins/` if that is set)

The overlay reads the folder when it starts (in developer mode, also whenever it changes). Then
open the hub's Plugins tab and click Enable on the plugin's card. Its windows are listed in the
hub's Windows tab.

## Wire protocol

The SDK does all of this for you; it is here for reference. Frame and page talk with
`postMessage`. Every message is an object `{axidps: 1, type, ...}`. The page knows which frame a
message came from by its sender alone, so a frame can only speak for itself.

Frame to page:

| `type` | Fields | |
| --- | --- | --- |
| `hello` | | The SDK has loaded (sent once per page load). |
| `resize` | `w`, `h` | The page's size in px. |
| `title` | `text` | Title bar text (at most 80 characters). |
| `status` | `text`, `live` | Status text (at most 80 characters) and whether it is live. |
| `settings` | `value` | Save this object as the window's settings (at most 16 KB as JSON). |
| `log` | `level`, `msg` | `"warn"` or `"error"`, and the message. |
| `pong` | `n` | The answer to `ping`. |
| `textures` | `rules` | This plugin's icon rules (at most 64; only with the `textures` permission). |

Page to frame:

| `type` | Fields | |
| --- | --- | --- |
| `ready` | `plugin`, `window`, `api`, `permissions` | The answer to `hello`. |
| `settings` | `value` | The window's settings. |
| `look` | `theme`, `tokens`, `style`, `opacity`, `compact`, `rows` | The theme and display settings. |
| `edit` | `on` | Arrange mode on or off. |
| `fight` | `feed` | The live feed (only with the `fight` permission). |
| `ping` | `n` | Sent every 2 s; answer with `pong`. |

After `hello` the page sends `ready`, `settings`, `look`, `edit`, then `fight`. The page drops
any message that isn't one of these with fields of the right types.

## Versioning

`api` is the major version of everything in this guide. Within version 1, calls, events,
message types and fields are only ever added, never removed or changed. The overlay serves
`/_sdk/axidps.js` itself, so your plugin always runs with the SDK that matches the player's
axidps. A plugin for a later `api` is listed as "needs a newer axidps" instead of loading.
