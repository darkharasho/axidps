#!/usr/bin/env python3
"""Drive the overlay host (apps/axidps-overlay) without the game: play the DLL's side of the
shared memory (layout v3, crates/axidps-overlay-shm), check the host's window tables, and write
what the game would show to a PNG.

    python3 tools/overlay-fake-dll.py [--host PATH] [--out DIR]
    python3 tools/overlay-fake-dll.py --devtools   # also check Servo's developer-tools server
    python3 tools/overlay-fake-dll.py --dev        # developer mode only (reload, storm, devtools,
                                                   # error count, restart)
    python3 tools/overlay-fake-dll.py --plugins    # the test plugins in tools/plugin-tests: escapes,
                                                   # hang, flood, bad manifests, input, navigation
                                                   # (behind a local leak proxy), sandbox and CSP
                                                   # checks the backstop can't mask, file: and
                                                   # never-enabled plugins refused, the window cap
    python3 tools/overlay-fake-dll.py --parity-save DIR   # crop Fight, Timeline and Healing in
    python3 tools/overlay-fake-dll.py --parity-check DIR  # every theme x compact; compare to DIR's

Checks that need to see inside the page load it as `index.html?test` (the host passes a `?query`
after the page path on to the page): the page then logs `debug {"windows": [{window, alias, status,
errors}], "plugins": [{id, error}], "notice"}` lines with each plugin window's status text and error
count, every listed plugin's error, and the hub's window note.

Runs the host with --software on a private shm file, ui.json and live feed, so an installed
host service is not touched. Exits non-zero if a check fails.
"""
import argparse, atexit, json, mmap, os, re, shutil, socket, struct, subprocess, sys, tempfile, threading, time, zlib

MAGIC, VERSION, MAX_W, MAX_H = 0x56454F41, 3, 2048, 2048  # public-check:ok
SEQ, IN_WRITE, IN_READ, RING_OFF, RING, INPUT_LEN = 16, 48, 56, 64, 256, 48
DLL_HB, SCREEN, EDIT = 12352, 12360, 12368
TABLES, WIN_LEN, FRAME_OFF = (12416, 13504), 64, 16384
FRAME_LEN = MAX_W * MAX_H * 4
SCREEN_W, SCREEN_H = 1600, 900

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
FAILED = []


def check(ok, what):
    print(("ok   " if ok else "FAIL ") + what)
    if not ok:
        FAILED.append(what)


class Shm:
    def __init__(self, path):
        self.f = open(path, "r+b")
        self.m = mmap.mmap(self.f.fileno(), 0)

    def u32(self, o):
        return struct.unpack_from("<I", self.m, o)[0]

    def u64(self, o):
        return struct.unpack_from("<Q", self.m, o)[0]

    def put(self, fmt_, o, *v):
        struct.pack_into(fmt_, self.m, o, *v)

    def table(self):
        seq = self.u64(SEQ)
        b = seq & 1
        t = TABLES[b]
        n, edit_seq = struct.unpack_from("<II", self.m, t)
        wins = []
        for i in range(min(n, 16)):
            e = t + 16 + i * WIN_LEN
            name = bytes(self.m[e:e + 16]).split(b"\0")[0].decode()
            v = struct.unpack_from("<10iIBB", self.m, e + 16)
            wins.append(dict(name=name, atlas=v[0:4], sx=v[4], sy=v[5], drag=v[6:10], flags=v[10],
                             radius=v[11], blur=v[12]))
        return seq, b, edit_seq, wins

    def push(self, kind, button=0, x=0.0, y=0.0, wheel=0.0, name=""):
        n = self.u64(IN_WRITE)
        o = RING_OFF + (n % RING) * INPUT_LEN
        struct.pack_into("<IIfffI24s", self.m, o, kind, button, x, y, wheel, 0, name.encode())
        self.put("<Q", IN_WRITE, n + 1)


def png(path, w, h, rgba):
    raw = b"".join(b"\0" + rgba[y * w * 4:(y + 1) * w * 4] for y in range(h))
    chunk = lambda t, d: struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d))
    with open(path, "wb") as f:
        f.write(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0))
                + chunk(b"IDAT", zlib.compress(raw, 6)) + chunk(b"IEND", b""))


def composite(shm, path, dim=False):
    """What the DLL would draw: each window's atlas rect at its screen position."""
    seq, b, _, wins = shm.table()
    scr = bytearray()
    for y in range(SCREEN_H):  # a stand-in for the game: a dull gradient
        c = 40 + y * 60 // SCREEN_H
        scr += bytes((c, c + 10, c + 20, 255)) * SCREEN_W
    if dim:
        scr = bytearray(v // 2 if i % 4 != 3 else 255 for i, v in enumerate(scr))
    base = FRAME_OFF + b * FRAME_LEN
    dimmed = False
    for w in sorted(wins, key=lambda w: bool(w["flags"] & 2)):  # the DLL draws modal windows on top
        if w["flags"] & 2 and not dimmed:  # ... over the dimmed game and other windows
            dimmed = True
            scr = bytearray(v // 2 if i % 4 != 3 else 255 for i, v in enumerate(scr))
        ax, ay, ww, hh = w["atlas"]
        for row in range(hh):
            y = w["sy"] + row
            if not 0 <= y < SCREEN_H:
                continue
            x0, x1 = max(0, w["sx"]), min(SCREEN_W, w["sx"] + ww)
            if x0 >= x1:
                continue
            src = base + (ay + row) * MAX_W * 4 + (ax + x0 - w["sx"]) * 4
            px = shm.m[src:src + (x1 - x0) * 4]
            o = (y * SCREEN_W + x0) * 4
            if all(px[i] == 255 for i in range(3, len(px), 4)):
                scr[o:o + len(px)] = px
                continue
            for i in range(0, len(px), 4):  # premultiplied "over"
                k = 255 - px[i + 3]
                for c in range(3):
                    scr[o + i + c] = min(255, px[i + c] + scr[o + i + c] * k // 255)
    png(path, SCREEN_W, SCREEN_H, bytes(scr))
    return wins


# The squad boons, as the feed sends them: {"<id>": [up, self, group, squad]} in ms (stack-ms for
# Might and Stability), nonzero ones only. Players 0 and 1 are the supports.
DEMO_BOONS = [740, 725, 1187, 30328, 717, 718, 726, 743, 1122, 719, 26980, 873, 5974]
DEMO_INTENSITY = {740: 18, 1122: 2}


def demo_boons(i, ms):
    out = {}
    for j, b in enumerate(DEMO_BOONS):
        up = (0.95 - 0.06 * ((i + j) % 9)) * ms
        if (i * 5 + j) % 11 == 0:
            up = 0
        give = 0.6 if i < 2 else 0.05 * (j % 3)
        row = [up, up * give * 0.4, ms * give * 0.8, ms * give * 0.45]
        k = DEMO_INTENSITY.get(b, 1)
        row = [int(x * k) for x in row]
        if any(row):
            out[str(b)] = row
    return out


def demo_live(path, t):
    names = ["Sentinel Vael", "Darkpaw", "Ironwake", "Morrow Kestrel", "Brannoc", "Ysolde Tam", "Oren"]
    ps = [dict(name=n, account=n.split()[0] + ".1234", prof=1 + i, elite=0, group=1 + i % 2, me=i == 3,
               damage=(900000 - i * 90000) * t // 60, power=0, condi=0, dps=(900000 - i * 90000) / 60.0,
               downs=i % 3, kills=i % 2, deaths=0, downed=i % 2, taken=300000 - i * 10000,
               strips=10 - i, cleanses=3 + i) for i, n in enumerate(names)]
    for i, p in enumerate(ps):
        p_boons = demo_boons(i, t * 1000)
        if p_boons:
            p["boons"] = p_boons
    tmp = path + ".tmp"
    with open(tmp, "w") as f:
        json.dump(dict(v=1, t=int(time.time() * 1000), state="fight",
                       fight=dict(duration_ms=t * 1000, map=38), players=ps), f)
    os.replace(tmp, path)


def devtools_first_packet(port=6080, timeout=5.0):
    """Connect to Servo's developer-tools server and read the first packet (`<length>:<json>`)."""
    with socket.create_connection(("127.0.0.1", port), timeout=timeout) as s:
        s.settimeout(timeout)
        buf = b""
        while b":" not in buf:
            c = s.recv(1)
            if not c:
                raise OSError("closed before a packet")
            buf += c
            if len(buf) > 12:
                raise OSError(f"not length-prefixed framing: {buf!r}")
        n = int(buf[:-1])
        body = b""
        while len(body) < n:
            c = s.recv(n - len(body))
            if not c:
                raise OSError("closed mid-packet")
            body += c
        return buf + body, json.loads(body)


def devtools_check(run, log):
    """Servo's developer-tools server (`--devtools`) answers on 127.0.0.1:6080."""
    run(5, lambda: "devtools: server started" in open(log, errors="replace").read())
    started = [l.strip() for l in open(log, errors="replace") if l.startswith("devtools:")]
    check(any("server started" in l for l in started), f"the host log has the devtools server-started line ({started})")
    try:
        raw, pkt = devtools_first_packet()
        check(pkt.get("from") == "root", f"127.0.0.1:6080 sends a root packet ({raw[:120]!r})")
    except (OSError, ValueError) as e:
        check(False, f"devtools first packet ({e})")


_SDK = []


def sdk_dir():
    """The SDK folder as install.sh lays it out: the source tree's sdk/ with the page's fonts in
    sdk/fonts/ (plugin frames load /_sdk/fonts/*.ttf). A private copy, made once per run."""
    if not _SDK:
        d = tempfile.mkdtemp(prefix="axidps-overlay-sdk-")
        atexit.register(shutil.rmtree, d, True)
        src = os.path.join(ROOT, "apps/axidps-overlay/sdk")
        for n in os.listdir(src):
            if n.endswith(".js") and not n.endswith(".test.js"):
                shutil.copy(os.path.join(src, n), d)
        shutil.copytree(os.path.join(ROOT, "apps/axidps-overlay/page/fonts"), os.path.join(d, "fonts"))
        _SDK.append(d)
    return _SDK[0]


def plugin_args(user_plugins):
    return ["--sdk", sdk_dir(),
            "--plugins", os.path.join(ROOT, "apps/axidps-overlay/plugins"), "--user-plugins", user_plugins]


def start_host(a, d, shm_path, ui, live, user_plugins, extra=(), forge=False, log="host.log", env_extra=None):
    """The host on private files, with the source tree's SDK and bundled plugins and the given
    player plugin folder (never ~/.config/axidps/plugins). --headless runs it in a private
    headless Wayland compositor (cage), for when the desktop's display server is busy."""
    cmd = [a.host, "--software", "--shm", shm_path, "--ui", ui, "--live", live, *plugin_args(user_plugins),
           *extra, a.page]
    env = dict(os.environ)
    if forge:
        # The host writes forged untokened `axidps:` console messages (a plugin frame could);
        # checked at the end that it dropped them.
        env["AXIDPS_TEST_FORGE"] = "1"
    env.update(env_extra or {})
    if a.headless:
        cmd = ["cage", "--", *cmd]
        env.update(WLR_BACKENDS="headless", WLR_RENDERER="pixman", WLR_LIBINPUT_NO_DEVICES="1")
        env.pop("DISPLAY", None)
    return subprocess.Popen(cmd, stdout=open(os.path.join(d, log), "w"), stderr=subprocess.STDOUT, env=env)


def stop_host(host, shm_path):
    while DLLS:
        DLLS.pop().stop()
    host.terminate()
    try:
        host.wait(5)
    except subprocess.TimeoutExpired:
        host.kill()
        host.wait(5)
    if os.path.exists(shm_path):
        os.unlink(shm_path)


def wait_shm(shm_path, sec=15):
    deadline = time.time() + sec
    while not (os.path.exists(shm_path) and os.path.getsize(shm_path) > 0):
        if time.time() > deadline:
            raise SystemExit("host made no shm file (with the desktop busy, try --headless)")
        time.sleep(0.1)
    return Shm(shm_path)


class Dll:
    """The DLL's side: screen size, edit flag and a moving heartbeat. The heartbeat has its own
    thread (every 30 ms), so slow work in the main thread (composite() takes seconds) can't stall
    it: the host takes 5 s without one for the game going away and exits."""

    def __init__(self, shm):
        self.shm = shm
        shm.put("<II", SCREEN, SCREEN_W, SCREEN_H)
        shm.put("<I", EDIT, 0)
        self.stopped = threading.Event()
        self.thread = threading.Thread(target=self.beat, daemon=True)
        self.thread.start()
        DLLS.append(self)

    def beat(self):
        hb, last = 1, time.monotonic()
        self.longest = 0.0  # the longest gap between two beats (s), for when the host says "game gone"
        while not self.stopped.is_set():
            hb += 1
            try:
                self.shm.put("<Q", DLL_HB, hb)
            except (ValueError, OSError):  # the mapping is gone
                return
            now = time.monotonic()
            self.longest, last = max(self.longest, now - last), now
            self.stopped.wait(0.03)

    def stop(self):
        self.stopped.set()
        self.thread.join(1)

    def run(self, sec, until=None):
        """Wait `sec` seconds, or until `until()` is true (then True)."""
        end = time.time() + sec
        while time.time() < end:
            if until and until():
                return True
            time.sleep(0.03)
        return until is None


DLLS = []


# A player plugin for the frame check: magenta 150x50, and it reports what reached it with
# console.warn (the SDK forwards that to the page, the page to the host log as `plugin <id>: ...`).
TEST_PLUGIN = {
    "axidps-plugin.json": json.dumps({
        "id": "test.hello", "name": "Hello test", "version": "1.0.0", "author": "harness", "api": 1,
        "permissions": ["fight"],
        "windows": [{"id": "card", "title": "Hello", "entry": "card.html", "description": "harness frame",
                     "icon": "plug", "open": True}]}),
    "card.html": """<!doctype html>
<html><head><meta charset="utf-8"><script src="/_sdk/axidps.js"></script>
<style>#box { width: 150px; height: 50px; background: #ff00ff; }</style></head>
<body><div id="box" data-tip="a tip"></div>
<script>
axidps.ready(info => {
  console.warn("ready " + info.plugin + "/" + info.window + " api " + info.api + " perms " + info.permissions.join(","));
  axidps.window.setTitle("Hello frame");
  axidps.window.setStatus("hi", {live: true});
  axidps.settings.set({n: 1});
  // A body naming another plugin: the page logs it under this plugin all the same.
  parent.postMessage({axidps: 1, type: "log", level: "warn", msg: "spoof attempt", plugin: "other.plugin"}, "*");
  // The page's console channel: a frame has no token, so the host must drop this.
  console.log('axidps:{"save":{"v":1,"forgedByFrame":true}}');
});
let fights = 0, looks = 0;
axidps.on("fight", f => { if (fights++ === 0) console.warn("fight players " + (f && f.players ? f.players.length : -1)); });
axidps.on("look", l => {
  if (looks++ === 0) console.warn("look " + l.theme + " " + getComputedStyle(document.documentElement).getPropertyValue("--accent").trim());
});
</script></body></html>
""",
}


# Developer mode's test plugin: a 150x50 box in COLOUR, and two console.error lines once ready.
def dev_plugin_html(colour):
    return f"""<!doctype html>
<html><head><meta charset="utf-8"><script src="/_sdk/axidps.js"></script>
<style>#box {{ width: 150px; height: 50px; background: {colour}; }}</style></head>
<body><div id="box"></div>
<script>
axidps.ready(() => {{ console.error("dev boom 1"); console.error("dev boom 2"); axidps.window.setStatus("dev"); }});
</script></body></html>
"""


def dev_check(a, d):
    """Developer mode (`dev: true` in ui.json): a saved plugin file reloads its frame within 1 s,
    a burst of writes reloads it once, the devtools server runs, frame errors are counted, and
    switching developer mode off in the hub makes the host exit (code 0) to be restarted."""
    d = os.path.join(d, "dev")
    real = os.path.join(d, "src", "test.dev")
    user_plugins = os.path.join(d, "user-plugins")
    os.makedirs(os.path.join(real, "many"), exist_ok=True)
    os.makedirs(user_plugins, exist_ok=True)
    with open(os.path.join(real, "axidps-plugin.json"), "w") as f:
        json.dump({"id": "test.dev", "name": "Dev test", "version": "1.0.0", "author": "harness", "api": 1,
                   "permissions": [], "windows": [{"id": "card", "title": "Dev", "entry": "card.html",
                                                   "open": True}]}, f)
    card = os.path.join(real, "card.html")
    with open(card, "w") as f:
        f.write(dev_plugin_html("#ff0000"))
    many = [os.path.join(real, "many", f"{i}.txt") for i in range(300)]
    for m in many:
        with open(m, "w") as f:
            f.write("0")
    # The plugin reached through a symlink, as a developer links their working folder.
    os.symlink(real, os.path.join(user_plugins, "test.dev"))
    shm_path = f"/dev/shm/axidps-overlay-test-{os.getpid()}-dev"
    ui, live = os.path.join(d, "ui.json"), os.path.join(d, "live.json")
    with open(ui, "w") as f:
        json.dump({"v": 1, "dev": True, "plugins": {"test.dev": {"enabled": True, "granted": []}},
                   "pos": {"test.dev/card": [700, 100]}}, f)
    demo_live(live, 30)
    page = a.page
    a.page = page + "?test"
    host = start_host(a, d, shm_path, ui, live, user_plugins)
    a.page = page
    logf = os.path.join(d, "host.log")
    log = lambda: open(logf, errors="replace").read()

    def debug():
        lines = [l for l in log().splitlines() if l.startswith("debug ")]
        try:
            return json.loads(lines[-1][len("debug "):]) if lines else {}
        except ValueError:
            return {}

    def errors_of(alias):
        return next((w.get("errors") for w in debug().get("windows", []) if w.get("alias") == alias), None)

    try:
        shm = wait_shm(shm_path)
        dll = Dll(shm)
        win = lambda name: next((w for w in shm.table()[3] if w["name"] == name), None)

        def box():
            p1 = win("p1")
            if not p1 or p1["atlas"][2:] != (152, 84):
                return None
            _, b, _, _ = shm.table()
            ax, ay = p1["atlas"][:2]
            o = FRAME_OFF + b * FRAME_LEN + ((ay + 1 + 32 + 25) * MAX_W + ax + 1 + 75) * 4
            return tuple(shm.m[o:o + 4])

        check("developer mode on" in log(), "the host read dev: true from ui.json")
        ok = dll.run(30, lambda: box() == (255, 0, 0, 255))
        check(ok, f"the plugin, linked into the player folder, paints its red box ({box()})")
        ok = dll.run(10, lambda: errors_of("p1") == 2)
        check(ok, f"badge: two console.error lines show an error count of 2 ({debug()})")

        # Reload on save.
        time.sleep(0.5)
        t0 = time.time()
        with open(card + ".tmp", "w") as f:
            f.write(dev_plugin_html("#0000ff"))
        os.replace(card + ".tmp", card)
        ok = dll.run(5, lambda: box() == (0, 0, 255, 255))
        took = time.time() - t0
        check(ok and took <= 1.0, f"reload: the saved colour is in the atlas within 1 s ({took:.2f} s, {box()})")
        dll.run(1.5)
        check(errors_of("p1") == 2, f"the reload restarted the error count (2 again, not 4: {errors_of('p1')})")

        # A storm of writes: one reload.
        dll.run(1)
        before = log().count("dev: pluginChanged test.dev")
        t0 = time.time()
        for i, m in enumerate(many):
            with open(m, "w") as f:
                f.write(str(i))
            time.sleep(max(0.0, t0 + 0.2 * (i + 1) / len(many) - time.time()))
        took = time.time() - t0
        dll.run(2)
        n = log().count("dev: pluginChanged test.dev") - before
        check(n == 1, f"storm: 300 files rewritten in {took:.2f} s give exactly one pluginChanged line ({n})")

        # The devtools server, as with --devtools.
        devtools_check(dll.run, logf)

        # Settings: Developer mode off -> the page saves dev: false, the host exits 0.
        dll.run(1)
        shm.push(1, 0, 0, 0)
        launcher = win("launcher")
        click = lambda w, x, y: [shm.push(k, 0, w["atlas"][0] + x, w["atlas"][1] + y) for k in (1, 2, 3)]
        click(launcher, 16, 16)
        dll.run(5, lambda: win("hub") is not None)
        hub = win("hub")
        check(hub is not None, "the hub opens")
        if hub:
            click(hub, 95, 218)  # nav: Plugins (the cards, with developer mode's Reload)
            dll.run(1)
            composite(shm, os.path.join(d, "plugins.png"))
            click(win("hub"), 95, 184)  # nav: Settings
            dll.run(1)
            hub = win("hub")
            shm.push(1, 0, hub["atlas"][0] + 450, hub["atlas"][1] + 300)
            # Scroll to the end (the host's wheel: positive is the page's deltaY > 0).
            shm.push(4, 0, hub["atlas"][0] + 450, hub["atlas"][1] + 300, wheel=5)
            dll.run(1)
            composite(shm, os.path.join(d, "settings.png"))
            click(win("hub"), DEV_TOGGLE[0], DEV_TOGGLE[1])
            end = time.time() + 5
            while host.poll() is None and time.time() < end:
                dll.run(0.1)
            code = host.poll()
            try:
                dev_saved = json.load(open(ui)).get("dev")
            except (OSError, ValueError):
                dev_saved = None
            check(code == 0 and "developer mode off; restarting" in log() and dev_saved is False,
                  f"restart: switching developer mode off saved dev: false and the host exited 0 "
                  f"(code {code}, dev {dev_saved})")
        page_lines = [l for l in log().splitlines() if l.startswith("page: ") and "dev boom" not in l]
        check(not page_lines, f"no page errors ({page_lines[:3]})")
    finally:
        stop_host(host, shm_path)


# Where the Developer mode switch is in the hub's Settings tab, scrolled to the end.
DEV_TOGGLE = (725, 424)


def plugin_frame_check(a, d):
    """A player plugin, enabled in ui.json, gets an axidps-plugin:// frame that loads the SDK,
    says hello, gets ready/fight/look, and can only speak as itself."""
    d = os.path.join(d, "plugin-frame")
    user_plugins = os.path.join(d, "user-plugins")
    pdir = os.path.join(user_plugins, "test.hello")
    os.makedirs(pdir, exist_ok=True)
    for name, body in TEST_PLUGIN.items():
        with open(os.path.join(pdir, name), "w") as f:
            f.write(body)
    shm_path = f"/dev/shm/axidps-overlay-test-{os.getpid()}-plugin"
    ui, live = os.path.join(d, "ui.json"), os.path.join(d, "live.json")
    with open(ui, "w") as f:
        json.dump({"v": 1, "plugins": {"test.hello": {"enabled": True, "granted": ["fight"]}},
                   "pos": {"test.hello/card": [700, 100]}}, f)
    demo_live(live, 30)
    host = start_host(a, d, shm_path, ui, live, user_plugins)
    logf = os.path.join(d, "host.log")
    log = lambda: open(logf, errors="replace").read()

    def saved_ui():
        try:
            return json.load(open(ui))
        except (OSError, ValueError):
            return {}
    try:
        shm = wait_shm(shm_path)
        dll = Dll(shm)
        win = lambda name: next((w for w in shm.table()[3] if w["name"] == name), None)
        ok = dll.run(30, lambda: win("p1") is not None)
        check(ok, f"the enabled player plugin's window is in the table as p1 ({[w['name'] for w in shm.table()[3]]})")
        ok = dll.run(15, lambda: "plugin test.hello: ready" in log())
        check("plugin test.hello: ready test.hello/card api 1 perms fight" in log(),
              "the frame loaded the SDK from /_sdk/, said hello and got ready (its log reached the host)")
        dll.run(5, lambda: "plugin test.hello: fight players" in log())
        check("plugin test.hello: fight players 7" in log(), "the frame got the live feed (7 players)")
        dll.run(5, lambda: "plugin test.hello: look" in log())
        check("plugin test.hello: look midnight #7aa2f7" in log(),
              "the frame got the look and the SDK set the theme's CSS variables")
        dll.run(5, lambda: "spoof attempt" in log())
        check("plugin test.hello: spoof attempt" in log() and "plugin other.plugin" not in log(),
              "a log message naming another plugin is logged as the frame's own plugin")
        dll.run(5, lambda: saved_ui().get("cfg", {}).get("test.hello/card") == {"n": 1})
        s = saved_ui()
        check(s.get("cfg", {}).get("test.hello/card") == {"n": 1}, f"settings.set saved cfg[test.hello/card] ({s.get('cfg')})")
        check(s.get("plugins", {}).get("test.hello", {}).get("enabled") is True, "restore keeps saved.plugins")
        check("forgedByFrame" not in json.dumps(s), "the frame's untokened console message did not reach ui.json")
        check(log().count("dropped untokened host message") == 1 and "forgedByFrame" not in log(),
              "the host dropped the frame's untokened console message without echoing it")
        ok = dll.run(5, lambda: (win("p1") or {}).get("atlas", (0, 0, 0, 0))[2:] == (152, 84))
        p1 = win("p1")
        check(ok, f"the frame sized itself to its 150x50 body (window {p1 and p1['atlas'][2:]})")
        check(p1 is not None and (p1["sx"], p1["sy"]) == (700, 100), f"the window sits at its saved spot ({p1 and (p1['sx'], p1['sy'])})")
        if p1:
            seq, b, _, _ = shm.table()
            ax, ay = p1["atlas"][:2]
            o = FRAME_OFF + b * FRAME_LEN + ((ay + 1 + 32 + 25) * MAX_W + ax + 1 + 75) * 4
            px = tuple(shm.m[o:o + 4])
            check(px == (255, 0, 255, 255), f"the frame's magenta box is painted in the atlas ({px})")
        composite(shm, os.path.join(d, "plugin.png"))
        shm.push(5, x=800, y=150, name="p1")
        ok = dll.run(5, lambda: saved_ui().get("pos", {}).get("test.hello/card") == [800, 150])
        check(ok, f"a drop of p1 saves the position under the full id ({saved_ui().get('pos')})")
        # The frame's own console.warn lines also show as `page: ...` (frames share the page's
        # console); anything else there is a page error.
        mine = {l[len("plugin test.hello: "):] for l in log().splitlines() if l.startswith("plugin test.hello: ")}
        page_lines = [l for l in log().splitlines() if l.startswith("page: ") and l[len("page: "):] not in mine]
        check(not page_lines, f"no page errors ({page_lines[:3]})")
    finally:
        stop_host(host, shm_path)


def migration_check(a, d):
    """A ui.json from before Fight, Timeline and Healing were plugins: the page copies their saved
    position and open state to the full ids, the windows keep them, and ui.json is rewritten with
    both (an older build still reads the old keys)."""
    d = os.path.join(d, "migration")
    user_plugins = os.path.join(d, "user-plugins")
    os.makedirs(user_plugins, exist_ok=True)
    shm_path = f"/dev/shm/axidps-overlay-test-{os.getpid()}-migration"
    ui, live = os.path.join(d, "ui.json"), os.path.join(d, "live.json")
    with open(ui, "w") as f:
        json.dump({"v": 1, "pos": {"fight": [700, 300]}, "open": {"fight": True, "healing": True}}, f)
    demo_live(live, 30)
    host = start_host(a, d, shm_path, ui, live, user_plugins)

    def saved_ui():
        try:
            return json.load(open(ui))
        except (OSError, ValueError):
            return {}
    try:
        shm = wait_shm(shm_path)
        dll = Dll(shm)
        win = lambda name: next((w for w in shm.table()[3] if w["name"] == name), None)
        ok = dll.run(30, lambda: win("fight") is not None and win("healing") is not None)
        names = [w["name"] for w in shm.table()[3]]
        check(ok, f"migration: the saved open Fight and Healing (old keys) are open ({names})")
        f = win("fight")
        check(f is not None and (f["sx"], f["sy"]) == (700, 300),
              f"migration: fight sits at its old saved spot 700, 300 ({f and (f['sx'], f['sy'])})")
        dll.run(5, lambda: "axidps.fight/fight" in saved_ui().get("pos", {}))
        s = saved_ui()
        check(s.get("pos", {}).get("axidps.fight/fight") == [700, 300] and s.get("pos", {}).get("fight") == [700, 300]
              and s.get("open", {}).get("axidps.healing/healing") is True and s.get("open", {}).get("healing") is True,
              f"migration: ui.json has the full ids, and keeps the old keys for an older build "
              f"(pos {s.get('pos')}, open {s.get('open')})")
        composite(shm, os.path.join(d, "migration.png"))
    finally:
        stop_host(host, shm_path)


def example_check(a, d):
    """docs/plugins/example, copied into a player plugin folder and enabled: its window opens."""
    d = os.path.join(d, "example")
    user_plugins = os.path.join(d, "user-plugins")
    shutil.copytree(os.path.join(ROOT, "docs/plugins/example"), os.path.join(user_plugins, "example.your-dps"))
    shm_path = f"/dev/shm/axidps-overlay-test-{os.getpid()}-example"
    ui, live = os.path.join(d, "ui.json"), os.path.join(d, "live.json")
    with open(ui, "w") as f:
        json.dump({"v": 1, "plugins": {"example.your-dps": {"enabled": True, "granted": ["fight"]}}}, f)
    demo_live(live, 30)
    host = start_host(a, d, shm_path, ui, live, user_plugins)
    try:
        shm = wait_shm(shm_path)
        dll = Dll(shm)
        size = lambda: (next((w for w in shm.table()[3] if w["name"] == "p1"), None) or {}).get("atlas", (0, 0, 0, 0))[2:]
        # A window around an unreported frame (240x120) is 242x154: the card's own size shows it loaded.
        ok = dll.run(30, lambda: size()[0] > 0 and size()[1] > 0 and size() != (242, 154))
        check(ok, f"the example plugin's window is in the table as p1, sized to its card ({size()})")
        dll.run(1)  # resizes are held to 4 a second: let the card's last one land before the shot
        composite(shm, os.path.join(d, "example.png"))
    finally:
        stop_host(host, shm_path)


# ---- plugin isolation and robustness (--plugins) ---------------------------------------------

PLUGIN_TESTS = os.path.join(ROOT, "tools/plugin-tests")
ESCAPES = ["parent", "top", "file", "net", "storage", "open"]


class LeakProxy:
    """A local HTTP(S) proxy that only records: the host runs with http_proxy/https_proxy set to
    it, so any request that gets past the host's backstop arrives here (as `CONNECT host:443`)
    instead of leaving the machine, with no DNS lookup. It answers 502 and hangs up."""

    def __init__(self):
        self.sock = socket.socket()
        self.sock.bind(("127.0.0.1", 0))
        self.sock.listen(16)
        self.url = "http://127.0.0.1:%d" % self.sock.getsockname()[1]
        self.seen = []
        threading.Thread(target=self.serve, daemon=True).start()

    def serve(self):
        while True:
            try:
                c, _ = self.sock.accept()
            except OSError:
                return
            try:
                c.settimeout(2)
                buf = b""
                while b"\r\n" not in buf and len(buf) < 4096:
                    got = c.recv(4096)
                    if not got:
                        break
                    buf += got
                self.seen.append(buf.split(b"\r\n")[0].decode(errors="replace"))
                c.sendall(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            except OSError:
                pass
            finally:
                c.close()

    def hosts(self):
        """The host of each request seen (`CONNECT host:port` or `GET http://host/...`)."""
        out = []
        for l in self.seen:
            parts = l.split()
            if len(parts) > 1:
                target = parts[1].split("//", 1)[-1]
                out.append(target.split("/")[0].rsplit(":", 1)[0])
        return out

    def close(self):
        self.sock.close()


def crop(shm, name):
    """A window's atlas pixels, or None."""
    _, b, _, wins = shm.table()
    w = next((w for w in wins if w["name"] == name), None)
    if not w:
        return None
    ax, ay, ww, hh = w["atlas"]
    base = FRAME_OFF + b * FRAME_LEN
    return b"".join(bytes(shm.m[base + ((ay + r) * MAX_W + ax) * 4:base + ((ay + r) * MAX_W + ax + ww) * 4])
                    for r in range(hh))


def plugin_ui(path, enable, dev=False, open_=None):
    """ui.json enabling `enable` (ids) with everything they ask for granted, and every test
    window placed on a grid right of the meter (so no two overlap in the page)."""
    grants = {}
    for pid in enable:
        m = json.load(open(os.path.join(PLUGIN_TESTS, pid, "axidps-plugin.json")))
        grants[pid] = {"enabled": True, "granted": list(m.get("permissions", []))
                       + ["net:" + h for h in m.get("network", [])]}
    pos, i = {}, 0
    for pid in sorted(enable):
        m = json.load(open(os.path.join(PLUGIN_TESTS, pid, "axidps-plugin.json")))
        for w in m.get("windows", []):
            pos[f"{m['id']}/{w['id']}"] = [620 + (i // 5) * 260, 10 + (i % 5) * 180]
            i += 1
    with open(path, "w") as f:
        json.dump({"v": 1, "dev": dev, "plugins": grants, "pos": pos, "open": open_ or {}}, f)


class PluginRun:
    """One host on tools/plugin-tests (as the player plugin folder), `index.html?test`, the local
    leak proxy and a feed that moves every second."""

    def __init__(self, a, d, name, enable, dev=False, open_=None):
        self.d = os.path.join(d, name)
        os.makedirs(self.d, exist_ok=True)
        self.shm_path = f"/dev/shm/axidps-overlay-test-{os.getpid()}-{name}"
        self.ui, self.live = os.path.join(self.d, "ui.json"), os.path.join(self.d, "live.json")
        plugin_ui(self.ui, enable, dev, open_)
        self.t = 30
        demo_live(self.live, self.t)
        self.proxy = LeakProxy()
        env = {"http_proxy": self.proxy.url, "https_proxy": self.proxy.url, "HTTP_PROXY": self.proxy.url,
               "HTTPS_PROXY": self.proxy.url, "no_proxy": "", "NO_PROXY": ""}
        page = a.page
        a.page = page + "?test"
        try:
            self.host = start_host(a, self.d, self.shm_path, self.ui, self.live, PLUGIN_TESTS, env_extra=env)
        finally:
            a.page = page
        self.logf = os.path.join(self.d, "host.log")
        self.shm = wait_shm(self.shm_path)
        self.dll = Dll(self.shm)
        self.fed = time.time()

    def log(self):
        return open(self.logf, errors="replace").read()

    def lines(self, prefix):
        return [l[len(prefix):] for l in self.log().splitlines() if l.startswith(prefix)]

    def debug(self):
        for l in reversed(self.lines("debug ")):
            try:
                return json.loads(l)
            except ValueError:
                continue
        return {}

    def window(self, full):
        return next((w for w in self.debug().get("windows", []) if w.get("window") == full), None)

    def listed(self, pid):
        return next((p for p in self.debug().get("plugins", []) if p.get("id") == pid), None)

    def win(self, name):
        return next((w for w in self.shm.table()[3] if w["name"] == name), None)

    def run(self, sec, until=None):
        """Like Dll.run, and the feed moves on a second every second (the meter's numbers change)."""
        def step():
            if time.time() - self.fed >= 1:
                self.fed = time.time()
                self.t += 1
                demo_live(self.live, self.t)
            return bool(until and until())
        ok = self.dll.run(sec, step)
        return ok or until is None

    def stop(self):
        gone = "game gone" in self.log()
        longest = self.dll.longest
        stop_host(self.host, self.shm_path)
        self.proxy.close()
        if gone:
            print(f"     the host exited with 'game gone'; the harness heartbeat's longest gap was {longest * 1000:.0f} ms")


def escape_checks(r, label):
    """t.escape's attempts are all blocked, the overlay is still there, and t.deeper's files are
    not served (t.escape's `deeper` window navigates to them)."""
    r.run(30, lambda: "plugin t.escape: escape done" in r.log())
    got = r.lines("plugin t.escape: ")
    for what in ESCAPES:
        line = next((l for l in got if l.startswith(what + " ")), None)
        check(line is not None and line.startswith(what + " blocked"),
              f"{label}: plugin t.escape: {what} blocked ({line})")
    before = crop(r.shm, "meter")
    r.run(3)
    check(r.win("meter") is not None and crop(r.shm, "meter") != before,
          f"{label}: the overlay is still there and the meter follows the feed after the escape attempts")
    r.run(10, lambda: "not found: axidps-plugin://t.deeper/" in r.log())
    check("plugin scheme: not found: axidps-plugin://t.deeper/index.html" in r.log()
          and "t.deeper was served" not in r.log(),
          f"{label}: axidps-plugin://t.deeper/index.html is 404 (the file exists: "
          f"{os.path.exists(os.path.join(PLUGIN_TESTS, 't.deeper/index.html'))})")
    p = r.listed("t.deeper")
    check(p is not None and p.get("error") == "needs verification",
          f"{label}: t.deeper (valid, asks for world) is listed with needs verification ({p})")
    framed = [w for w in r.debug().get("windows", []) if w.get("window", "").startswith("t.deeper/")]
    check(not framed and "plugin t.deeper:" not in r.log(),
          f"{label}: no frame is created for t.deeper, though ui.json enables it with world granted ({framed})")
    # t.off is valid and installed, never enabled: t.escape's `off` window navigates itself to it.
    r.run(10, lambda: "not found: axidps-plugin://t.off/" in r.log())
    check("plugin scheme: not found: axidps-plugin://t.off/index.html" in r.log() and "t.off was served" not in r.log(),
          f"{label}: axidps-plugin://t.off/index.html (valid, never enabled) is 404 to a frame navigating to it")
    p = r.listed("t.off")
    check(p is not None and not p.get("error"), f"{label}: t.off is listed without an error ({p})")


def navigate_checks(r, label):
    """Nothing t.navigate (or t.escape's svg) tries reaches undeclared.invalid: the host's
    backstop logs the block, and the leak proxy sees the declared host (the positive control)
    but never undeclared.invalid."""
    r.run(15, lambda: "allowed.invalid" in r.proxy.hosts() and "blocked undeclared.invalid" in r.log())
    r.run(2)
    hosts = r.proxy.hosts()
    check("allowed.invalid" in hosts,
          f"{label}: leak detector live: t.navigate's declared https://allowed.invalid/ reached the local proxy ({r.proxy.seen})")
    check("blocked undeclared.invalid" in r.log(), f"{label}: the host log has blocked undeclared.invalid")
    check("undeclared.invalid" not in hosts and not any("undeclared.invalid" in l for l in r.proxy.seen),
          f"{label}: no request to undeclared.invalid left the host (proxy saw {r.proxy.seen})")
    check(not any("off.invalid" in l for l in r.proxy.seen),
          f"{label}: no request to off.invalid (declared only by t.off, never enabled) left the host")
    probes = (r.lines("plugin t.navigate: ") + [l for l in r.lines("plugin t.escape: ") if l.startswith("svg ")]
              + [l for l in r.lines("page: ") if l.startswith("srcdoc ")])
    opened = [l for l in probes if " OPEN" in l]
    check(not opened, f"{label}: no probe reports a successful load ({opened})")
    print(f"     probe reports: {sorted(set(probes))}")
    before = crop(r.shm, "meter")
    r.run(3)
    check(crop(r.shm, "meter") != before, f"{label}: the overlay keeps updating after the navigations")


def plugins_check(a, d):
    """--plugins: the test plugins in tools/plugin-tests against the real host."""
    enable = ["t.escape", "t.hang", "t.flood", "t.input", "t.navigate", "t.deeper", "t.bad"]
    r = PluginRun(a, d, "plugins", enable)
    try:
        ok = r.run(30, lambda: r.win("meter") is not None and r.window("t.input/main") is not None)
        check(ok, f"the overlay came up with the test plugins ({[w['name'] for w in r.shm.table()[3]]})")
        bad = r.listed("t.bad")
        bad_log = [l for l in r.log().splitlines() if l.startswith("plugin folder t.bad:")]
        check(bad is not None and bad.get("error") and bad_log,
              f"t.bad is listed with an error ({bad}; {bad_log}) and the overlay still came up")
        check(r.window("t.bad/main") is None and "plugin t.bad:" not in r.log(), "t.bad gets no frame")

        escape_checks(r, "escape")

        # t.hang: the meter keeps updating, and t.hang shows "not responding".
        ok = r.run(15, lambda: "plugin t.hang: hanging" in r.log())
        check(ok, "t.hang said it is about to hang")
        t0 = time.time()
        seq0, before = r.shm.table()[0], crop(r.shm, "meter")
        ok = r.run(10, lambda: (r.window("t.hang/main") or {}).get("status") == "not responding")
        took = time.time() - t0
        check(ok, f"t.hang's status shows not responding ({took:.1f} s after it hung; {r.window('t.hang/main')})")
        r.run(2)
        seq1, after = r.shm.table()[0], crop(r.shm, "meter")
        check(seq1 > seq0 and after is not None and after != before,
              f"while t.hang hangs the table seq advances ({seq0} -> {seq1}) and the meter's pixels follow the feed")
        others = {w: (r.window(w) or {}).get("status") for w in ("t.input/main", "t.escape/main")}
        check(all(v not in (None, "not responding") for v in others.values()),
              f"the other frames still answer ({others})")

        # t.flood: at most 20 of its lines per second, then one "dropped" line. Its lines are
        # numbered, so each second's passed lines are a run of consecutive numbers, followed by a
        # gap (the dropped ones; the "… N lines dropped" line may land either side of the next
        # run's first line, so the runs, not the log order, delimit the seconds).
        r.run(1)
        n0 = len(r.lines("plugin t.flood: "))
        t0 = time.time()
        r.run(5)
        took = time.time() - t0
        got = r.lines("plugin t.flood: ")
        nums = [int(l.split()[1]) for l in got if re.fullmatch(r"flood \d+", l)]
        drops = [int(m[1]) for l in got if (m := re.fullmatch(r"… (\d+) lines dropped", l))]
        runs = []
        for n in nums:
            if runs and n == runs[-1][1] + 1:
                runs[-1][1] = n
            else:
                runs.append([n, n])
        sizes = [b - a + 1 for a, b in runs]
        gaps = [runs[i + 1][0] - runs[i][1] - 1 for i in range(len(runs) - 1)]
        check(len(runs) >= 5 and max(sizes) <= 20 and len(drops) >= len(gaps) and drops[:len(gaps)] == gaps,
              f"t.flood: each second at most 20 of its lines, then one '… N lines dropped' line with the right N "
              f"({len(runs)} seconds; runs {sorted(set(sizes))}; dropped {drops[:3]}..., gaps {gaps[:3]}...)")
        span = got[n0:]
        check(len(span) <= 21 * (took + 1), f"t.flood: {len(span)} lines in {took:.1f} s (it logs 200 a second)")
        check(r.win("meter") is not None and r.win("meter")["atlas"][2] > 0, "the overlay survives the flood")

        # t.input: hover shows its tooltip inside the frame, a click at its centre arrives.
        alias = (r.window("t.input/main") or {}).get("alias")
        w = r.win(alias) if alias else None
        check(w is not None and w["atlas"][2:] == (162, 94), f"t.input's window is in the table ({alias}: {w and w['atlas']})")
        if w:
            cx, cy = w["atlas"][0] + 1 + 80, w["atlas"][1] + 33 + 30
            r.shm.push(1, 0, cx - 10, cy - 5)
            r.shm.push(1, 0, cx, cy)
            r.run(5, lambda: any(l.startswith("tip ") for l in r.lines("plugin t.input: ")))
            tip = next((l for l in r.lines("plugin t.input: ") if l.startswith("tip ")), "")
            m = re.match(r'tip (\S+) "(.*)" at ([-\d.]+),([-\d.]+) ([\d.]+)x([\d.]+) in ([\d.]+)x([\d.]+)$', tip)
            ok = False
            if m:
                x, y, tw, th, fw, fh = map(float, m.groups()[2:])
                ok = m[1] == "block" and m[2] == "input tip" and x >= 0 and y >= 0 and x + tw <= fw and y + th <= fh
            check(ok, f"t.input: hovering shows the data-tip tooltip inside the frame ({tip})")
            # Aimed with the table as read now, as the DLL does: t.flood (resizing non-stop, earlier
            # in the atlas) must not throw it off (the page holds frame resizes to 4 a second).
            w = r.win(alias)
            cx, cy = w["atlas"][0] + 1 + 80, w["atlas"][1] + 33 + 30
            for k in (1, 2, 3):
                r.shm.push(k, 0, cx, cy)
            r.run(5, lambda: any(l.startswith("click ") for l in r.lines("plugin t.input: ")))
            click = next((l for l in r.lines("plugin t.input: ") if l.startswith("click ")), "")
            m = re.match(r"click ([-\d.]+),([-\d.]+) of ([\d.]+)x([\d.]+)$", click)
            ok = bool(m) and abs(float(m[1]) - float(m[3]) / 2) <= 2 and abs(float(m[2]) - float(m[4]) / 2) <= 2
            check(ok, f"t.input: a click at its centre is received there ({click})")

        navigate_checks(r, "navigate")
        svg = [l for l in r.lines("plugin t.escape: ") if l.startswith("svg ") and l != "svg navigating"]
        print(f"     t.escape's x.svg: {svg or 'no report (its script did not run)'}")
        composite(r.shm, os.path.join(r.d, "plugins.png"))
    finally:
        r.stop()

    # The same escape and navigation attempts in developer mode: it grants nothing.
    r = PluginRun(a, d, "plugins-dev", ["t.escape", "t.navigate", "t.deeper"], dev=True)
    try:
        ok = r.run(30, lambda: r.win("meter") is not None and "developer mode on" in r.log())
        check(ok, "dev: true: the overlay came up in developer mode")
        escape_checks(r, "dev")
        navigate_checks(r, "dev")
    finally:
        r.stop()

    isolation_check(a, d)
    cap_check(a, d)
    atlas_check(a, d)


def isolation_check(a, d):
    """Checks that fail if the frame sandbox or the injected CSP meta is missing (the escape
    checks above hold through opaque origins and the backstop alone), and the host's file: rule.
    t.navigate declares *.csp.invalid, so the backstop lets those hosts through: only t.isolate's
    CSP keeps its fetches (escape.csp.invalid, and comment.csp.invalid from a file that opens with
    an early-ended comment) off the network, and only the sandbox its form submission
    (form.csp.invalid); the leak proxy must see none of them."""
    r = PluginRun(a, d, "isolation", ["t.isolate", "t.navigate"])
    try:
        done = lambda: ("plugin t.isolate: csp done" in r.log() and "plugin t.isolate: form blocked" in r.log()
                        and "ok.csp.invalid" in r.proxy.hosts() and "blocked file://unc.invalid" in r.log()
                        and any(l.startswith("comment ") for l in r.lines("page: ")))
        ok = r.run(30, done)
        r.run(2)
        got = r.lines("plugin t.isolate: ")
        print(f"     t.isolate reports: {sorted(set(got))}")
        check(ok, "isolation: every t.isolate probe reported and the csp control reached the proxy")
        check("ok.csp.invalid" in r.proxy.hosts(),
              f"isolation: control: t.navigate's https://ok.csp.invalid/ (declared *.csp.invalid) reached the proxy ({r.proxy.seen})")
        csp = next((l for l in got if l.startswith("csp ")), None)
        check(csp is not None and csp.startswith("csp blocked") and "escape.csp.invalid" not in r.proxy.hosts(),
              f"isolation: CSP: t.isolate's fetch of https://escape.csp.invalid/ (allowed by the backstop) never "
              f"left the host ({csp}; proxy saw {r.proxy.seen})")
        img = next((l for l in got if l.startswith("fileimg ")), None)
        check(img is not None and img.startswith("fileimg blocked"), f"isolation: a file: image does not load ({img})")
        comment = next((l for l in r.lines("page: ") if l.startswith("comment ")), None)
        check("comment.csp.invalid" not in r.proxy.hosts() and comment is not None and " OPEN" not in comment,
              f"isolation: CSP: a file opening with <!--> gets the meta before its script ({comment}; proxy saw {r.proxy.seen})")
        check("form.csp.invalid" not in r.proxy.hosts() and "form blocked (still here)" in got,
              f"isolation: sandbox: a form submit() to https://form.csp.invalid/ (allowed by the backstop) does nothing "
              f"({[l for l in got if l.startswith('form')]}; proxy saw {r.proxy.seen})")
        check("blocked file://unc.invalid" in r.log(),
              "isolation: the host refuses a frame navigating to file://unc.invalid/... (a UNC path on Windows)")
        check("blocked file: outside the overlay" in r.log(),
              "isolation: the host refuses a frame navigating to file:///etc/hostname")
        before = crop(r.shm, "meter")
        r.run(3)
        check(r.win("meter") is not None and crop(r.shm, "meter") != before,
              "isolation: the overlay keeps updating after the probes")
    finally:
        r.stop()


CAP_NOTE = "16 windows are open; close one first"


def cap_check(a, d):
    """The window cap, live: 15 windows want to open (7 built in + t.many's and t.more's 4 each);
    14 get a slot (the hub and Arrange have their own), the last one is refused and the hub says
    why; the hub still opens."""
    r = PluginRun(a, d, "cap", ["t.many", "t.more"],
                  open_={"support": True, "defense": True, "axidps.fight/fight": True,
                         "axidps.timeline/timeline": True, "axidps.healing/healing": True})
    try:
        want = lambda: len([w for w in r.debug().get("windows", []) if w["window"].startswith(("t.many/", "t.more/"))]) == 8
        ok = r.run(30, lambda: want() and len(r.shm.table()[3]) >= 14)
        r.run(2)
        wins = r.debug().get("windows", [])
        names = [w["name"] for w in r.shm.table()[3]]
        last = wins[-1]["alias"] if wins else None
        check(ok and len(names) == 14 and last not in names,
              f"cap: 15 windows want to open, 14 are in the table, and the last ({last}) is not ({len(names)}: {names})")
        check(r.debug().get("notice") == CAP_NOTE, f"cap: the hub's note reads {CAP_NOTE!r} ({r.debug().get('notice')!r})")
        launcher = r.win("launcher")
        if launcher:
            for k in (1, 2, 3):
                r.shm.push(k, 0, launcher["atlas"][0] + 16, launcher["atlas"][1] + 16)
        ok = r.run(5, lambda: r.win("hub") is not None)
        names = [w["name"] for w in r.shm.table()[3]]
        check(ok and len(names) == 15 and last not in names,
              f"cap: the hub still opens over a full table ({len(names)} windows)")
        composite(r.shm, os.path.join(r.d, "cap.png"))
    finally:
        r.stop()


ATLAS_NOTE = "No room for more windows; close one first"


def atlas_check(a, d):
    """Windows that don't fit the 2048x2048 atlas: t.big's four 1000x1000 windows can't all fit
    beside the built-in ones. Those that don't are closed and the hub says why, and t.input's
    window (after them in the atlas) is still shown."""
    r = PluginRun(a, d, "atlas", ["t.big", "t.input"])
    try:
        alias = lambda: (r.window("t.input/main") or {}).get("alias")
        big = lambda: [w for w in r.debug().get("windows", []) if w["window"].startswith("t.big/")]
        settled = lambda: (len(big()) == 4 and alias() and r.win(alias()) is not None
                           and r.debug().get("notice") == ATLAS_NOTE)
        ok = r.run(30, settled)
        r.run(3)
        names = [w["name"] for w in r.shm.table()[3]]
        shown = [w["alias"] for w in big() if w["alias"] in names]
        check(ok and alias() in names and 0 < len(shown) < 4,
              f"atlas: t.input is shown after t.big's windows, of which only those that fit are ({shown}; table {names})")
        check(r.debug().get("notice") == ATLAS_NOTE,
              f"atlas: the hub's note reads {ATLAS_NOTE!r} ({r.debug().get('notice')!r})")
        inside = [w["atlas"] for w in r.shm.table()[3]]
        check(all(x >= 0 and y >= 0 and x + w <= 2048 and y + h <= 2048 for x, y, w, h in inside),
              f"atlas: every window in the table is inside the atlas ({inside})")
        composite(r.shm, os.path.join(r.d, "atlas.png"))
    finally:
        r.stop()


# ---- pixel parity (--parity-save / --parity-check) -------------------------------------------

PARITY_THEMES = ["midnight", "glass", "terminal", "ember", "verdant", "frost", "contrast"]
PARITY_WINDOWS = ["fight", "timeline", "healing"]
# A crop passes when at most this share of its pixels differ by more than PARITY_TOL in a channel.
PARITY_SHARE, PARITY_TOL = 0.005, 16


def parity_live(path):
    """demo_live's fixed feed, plus what Timeline and Healing draw: a 30 s timeline and heal totals
    (yours and two squadmates' sharing theirs). Nothing in it changes between runs."""
    demo_live(path, 30)
    with open(path) as f:
        d = json.load(f)
    for i, p in enumerate(d["players"]):
        p.update(down_contrib=40000 - i * 5000, heal_full=i in (1, 3, 5),
                 healing=(250000 if i == 3 else 90000 - i * 9000), barrier=(60000 if i == 3 else 12000 + i * 1000))
        p["hps"] = (p["healing"] + p["barrier"]) / 30.0
    d["timeline"] = dict(bucket_ms=1000,
                         out=[200000 + (k * 37 % 11) * 30000 + k * 4000 for k in range(30)],
                         **{"in": [90000 + (k * 53 % 7) * 20000 for k in range(30)]})
    with open(path + ".tmp", "w") as f:
        json.dump(d, f)
    os.replace(path + ".tmp", path)


def parity_crops(shm):
    """The fight, timeline and healing windows' atlas pixels: {name: (w, h, rgba)}."""
    _, b, _, wins = shm.table()
    base = FRAME_OFF + b * FRAME_LEN
    out = {}
    for w in wins:
        if w["name"] not in PARITY_WINDOWS:
            continue
        ax, ay, ww, hh = w["atlas"]
        rows = [bytes(shm.m[base + ((ay + r) * MAX_W + ax) * 4:base + ((ay + r) * MAX_W + ax + ww) * 4])
                for r in range(hh)]
        out[w["name"]] = (ww, hh, b"".join(rows))
    return out


def parity_shot(a, d, theme, compact):
    """Start the host with only Fight, Timeline and Healing opened (today's saved keys: the page
    migrates them) in `theme`, wait until all three windows are painted and still, and crop them."""
    run_dir = os.path.join(d, f"run-{theme}-{int(compact)}")
    os.makedirs(run_dir, exist_ok=True)
    user_plugins = os.path.join(run_dir, "user-plugins")
    os.makedirs(user_plugins, exist_ok=True)
    shm_path = f"/dev/shm/axidps-overlay-test-{os.getpid()}-parity"
    ui, live = os.path.join(run_dir, "ui.json"), os.path.join(run_dir, "live.json")
    with open(ui, "w") as f:
        json.dump({"v": 1, "theme": theme, "compact": compact,
                   "open": {"fight": True, "timeline": True, "healing": True}}, f)
    parity_live(live)
    host = start_host(a, run_dir, shm_path, ui, live, user_plugins)
    try:
        shm = wait_shm(shm_path)
        dll = Dll(shm)
        names = lambda: {w["name"] for w in shm.table()[3]}
        if not dll.run(30, lambda: set(PARITY_WINDOWS) <= names()):
            raise SystemExit(f"{theme}/{compact}: the windows never opened ({sorted(names())}; log in {run_dir})")
        # Fonts load and the page lays out again at 0.3 s and 1.5 s; plugin frames load and size
        # themselves. Then wait for four identical reads 0.5 s apart.
        dll.run(3)
        prev, same, end = None, 0, time.time() + 30
        while time.time() < end:
            cur = parity_crops(shm)
            same = same + 1 if cur == prev and len(cur) == len(PARITY_WINDOWS) else 0
            if same == 3:
                return cur
            prev = cur
            dll.run(0.5)
        raise SystemExit(f"{theme}/{compact}: the windows never settled (log in {run_dir})")
    finally:
        stop_host(host, shm_path)


def parity(a, d, save_dir=None, check_dir=None):
    """--parity-save DIR: write each crop to DIR as <theme>-<compact>-<name>.rgba (+ .json, w/h).
    --parity-check DIR: compare against DIR's crops: equal sizes, and at most 0.5% of the pixels
    differing by more than 16 in any channel. A failing crop gets <name>.diff.png (differing pixels
    red) in the output folder."""
    target = save_dir or check_dir
    if save_dir:
        os.makedirs(save_dir, exist_ok=True)
    written = 0
    for theme in PARITY_THEMES:
        for compact in (False, True):
            crops = parity_shot(a, d, theme, compact)
            for name in PARITY_WINDOWS:
                key = f"{theme}-{str(compact).lower()}-{name}"
                w, h, px = crops[name]
                png(os.path.join(d, key + ".png"), w, h, px)
                if save_dir:
                    with open(os.path.join(target, key + ".rgba"), "wb") as f:
                        f.write(px)
                    with open(os.path.join(target, key + ".json"), "w") as f:
                        json.dump({"w": w, "h": h}, f)
                    written += 1
                    print(f"wrote {key} {w}x{h}")
                    continue
                try:
                    ref = json.load(open(os.path.join(target, key + ".json")))
                    ref_px = open(os.path.join(target, key + ".rgba"), "rb").read()
                except OSError as e:
                    check(False, f"{key}: no saved crop ({e})")
                    continue
                if (ref["w"], ref["h"]) != (w, h):
                    check(False, f"{key}: size {w}x{h}, saved {ref['w']}x{ref['h']}")
                    continue
                diff = bytearray(px)
                bad = 0
                for i in range(0, len(px), 4):
                    if any(abs(px[i + c] - ref_px[i + c]) > PARITY_TOL for c in range(4)):
                        bad += 1
                        diff[i:i + 4] = b"\xff\x00\x00\xff"
                    else:
                        diff[i:i + 3] = bytes(v // 3 for v in px[i:i + 3])
                share = bad / (w * h)
                ok = share <= PARITY_SHARE
                if not ok:
                    png(os.path.join(d, key + ".diff.png"), w, h, bytes(diff))
                check(ok, f"{key}: {w}x{h}, {bad} pixels differ ({share * 100:.3f}%)")
    if save_dir:
        print(f"{written} crops written to {save_dir}")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--host", default=os.path.join(ROOT, "apps/axidps-overlay/target/release/axidps-overlay"))
    ap.add_argument("--out", default=tempfile.mkdtemp(prefix="axidps-overlay-test-"))
    ap.add_argument("--page", default=os.path.join(ROOT, "apps/axidps-overlay/page/index.html"))
    ap.add_argument("--shot", action="store_true",
                    help="no checks: wait for the page to settle and write shot.png (e.g. with a demo page)")
    ap.add_argument("--devtools", action="store_true", help="start the host with --devtools and check the server")
    ap.add_argument("--dev", action="store_true", help="only the developer mode check")
    ap.add_argument("--plugins", action="store_true",
                    help="only the plugin isolation and robustness checks (tools/plugin-tests)")
    ap.add_argument("--headless", action="store_true",
                    help="run the host in a private headless Wayland compositor (cage)")
    ap.add_argument("--parity-save", metavar="DIR",
                    help="only pixel parity: crop Fight, Timeline and Healing in every theme and compact mode to DIR")
    ap.add_argument("--parity-check", metavar="DIR", help="only pixel parity: compare the crops with DIR's")
    a = ap.parse_args()
    d = a.out
    os.makedirs(d, exist_ok=True)
    if a.parity_save or a.parity_check:
        parity(a, d, a.parity_save, a.parity_check)
        print(f"output in {d}")
        sys.exit(1 if FAILED else 0)
    if a.plugins:
        plugins_check(a, d)
        print(f"output in {d}")
        sys.exit(1 if FAILED else 0)
    if a.dev:
        dev_check(a, d)
        print(f"output in {d}")
        sys.exit(1 if FAILED else 0)
    shm_path, ui, live = f"/dev/shm/axidps-overlay-test-{os.getpid()}", os.path.join(d, "ui.json"), os.path.join(d, "live.json")
    for f in (ui, ui + ".tmp"):
        if os.path.exists(f):
            os.unlink(f)
    demo_live(live, 30)
    extra = ["--devtools"] if a.devtools else []
    # The source tree's SDK and bundled plugins, and an empty player plugin folder: results never
    # depend on what is installed in ~/.config/axidps/plugins.
    user_plugins = os.path.join(d, "user-plugins")
    os.makedirs(user_plugins, exist_ok=True)
    host = start_host(a, d, shm_path, ui, live, user_plugins, extra, forge=True)
    try:
        shm = wait_shm(shm_path)
        check(shm.u32(0) == MAGIC and shm.u32(4) == VERSION, "header magic/version 3")
        run = Dll(shm).run
        got = run(30, lambda: shm.table()[3])
        if a.shot:
            run(3)
            composite(shm, os.path.join(d, "shot.png"))
            return
        check(got, "host publishes a window table once the DLL heartbeat moves")
        if a.devtools:
            devtools_check(run, os.path.join(d, "host.log"))
        run(1.5)
        seq, b, edit_seq, wins = shm.table()
        names = [w["name"] for w in wins]
        check(sorted(names) == ["launcher", "meter"], f"the meter and the axidps button are open by default ({names})")
        btn = next((w for w in wins if w["name"] == "launcher"), None)
        check(btn is not None and btn["atlas"][2:] == (32, 32) and btn["drag"][2] == 0 and btn["sy"] == 4
              and btn["radius"] == 0 and btn["blur"] == 0,
              f"the axidps button is a bare 32px glyph at the top, with no drag zone ({btn})")
        m = wins[0] if wins else None
        check(m is not None and (m["sx"], m["sy"]) == (60, 220), "meter at its default position")
        check(m is not None and m["drag"] == (0, 0, 0, 0),
              f"the meter is locked in place: no drag zone outside edit mode ({m and m['drag']})")
        check(m is not None and m["radius"] == 10 and m["blur"] == 1,
              f"the meter asks for Midnight's corners and soft blur ({m and (m['radius'], m['blur'])})")
        composite(shm, os.path.join(d, "1-default.png"))

        shm.push(6, x=300, y=150)  # legacy ini position
        ok = run(5, lambda: any(w["name"] == "meter" and (w["sx"], w["sy"]) == (300, 150) for w in shm.table()[3]))
        check(ok, "legacy position applied to the meter")
        shm.push(5, x=120, y=400, name="meter")
        ok = run(5, lambda: any(w["name"] == "meter" and (w["sx"], w["sy"]) == (120, 400) for w in shm.table()[3]))
        check(ok, "a drop (kind 5) moves the meter")
        shm.push(6, x=900, y=900)
        run(1)
        check(any((w["sx"], w["sy"]) == (120, 400) for w in shm.table()[3]), "legacy position ignored once placed")
        def saved_ui():
            try:
                return json.load(open(ui))
            except (OSError, ValueError):
                return {}
        run(5, lambda: saved_ui().get("pos", {}).get("meter") == [120, 400])
        saved = saved_ui()
        check(saved.get("pos", {}).get("meter") == [120, 400], f"ui.json saved the position ({saved.get('pos')})")

        def click(w, x, y):
            ax, ay = w["atlas"][0] + x, w["atlas"][1] + y
            shm.push(1, 0, ax, ay)
            shm.push(2, 0, ax, ay)
            shm.push(3, 0, ax, ay)

        def win(name):
            return next((w for w in shm.table()[3] if w["name"] == name), None)

        click(win("launcher"), 16, 16)
        ok = run(5, lambda: win("hub") is not None)
        check(ok, "clicking the axidps button opens the hub")
        hub = win("hub")
        if hub:
            check(hub["flags"] & 2 and hub["sx"] == (SCREEN_W - 760) // 2 and hub["blur"] >= 2,
                  f"the hub is a modal, centred, blurred window ({hub['sx']},{hub['sy']} flags {hub['flags']})")
            # Windows tab, second row, third card: Timeline (the bundled plugins' windows follow the
            # built-in tables in list order: Fight, Healing, Timeline).
            click(hub, 640, 240)
            ok = run(5, lambda: win("timeline") is not None)
            check(ok, "a hub card opens the Timeline window")
            composite(shm, os.path.join(d, "1b-hub.png"))
            click(win("hub"), 95, 150)  # nav: Appearance
            run(1)
            click(win("hub"), 700, 140)  # first row, fourth theme card: Ember
            run(5, lambda: saved_ui().get("theme") == "ember")
            check(saved_ui().get("theme") == "ember", f"a theme card picks a theme, saved in ui.json ({saved_ui().get('theme')})")
            composite(shm, os.path.join(d, "1c-appearance.png"))
            click(win("hub"), 95, 184)  # nav: Settings
            run(1)
            composite(shm, os.path.join(d, "1d-settings.png"))
            click(win("hub"), 725, 385)  # Share healing with my squad (third Combat row): off
            run(5, lambda: saved_ui().get("core", {}).get("relay") is False)
            check(saved_ui().get("core", {}).get("relay") is False,
                  f"the relay toggle saves core.relay=false ({saved_ui().get('core')})")
            shm.push(8)  # the DLL: a click outside the hub, or Esc
            ok = run(5, lambda: win("hub") is None)
            check(ok, "a dismiss (kind 8) closes the hub")
            click(win("launcher"), 16, 16)
            run(5, lambda: win("hub") is not None)
            hub = win("hub")
            if hub:
                click(hub, 95, 449)  # Arrange windows
                ok = run(5, lambda: shm.table()[2] % 2 == 1 and win("hub") is None)
                check(ok, f"Arrange windows asks for edit mode and closes the hub (edit_seq {shm.table()[2]})")

        shm.put("<I", EDIT, 1)
        ok = run(5, lambda: any(w["name"] == "layout" for w in shm.table()[3]))
        check(ok, "edit mode shows the Layout window")
        lay = next((w for w in shm.table()[3] if w["name"] == "layout"), None)
        check(lay is not None and lay["flags"] == 1, "Layout window is interactive in edit mode")
        if lay:
            click(lay, 40, 155)  # the second toggle row: Support
            ok = run(5, lambda: any(w["name"] == "support" for w in shm.table()[3]))
            check(ok, "clicking a Layout toggle opens the Support window")
            check(any(w["name"] == "meter" for w in shm.table()[3]), "the meter stays open")
        composite(shm, os.path.join(d, "2-edit.png"), dim=True)
        lay = next((w for w in shm.table()[3] if w["name"] == "layout"), None)  # atlas moved
        if lay:
            click(lay, lay["atlas"][2] - 36, lay["atlas"][3] - 24)  # Done
            ok = run(5, lambda: shm.table()[2] % 2 == 0 and shm.table()[2] > 0)
            check(ok, f"Done asks to leave edit mode (edit_seq {shm.table()[2]})")
        shm.put("<I", EDIT, 0)
        ok = run(5, lambda: not any(w["name"] == "layout" for w in shm.table()[3]))
        check(ok, "leaving edit mode hides the Layout window")
        composite(shm, os.path.join(d, "3-after.png"))
        run(5, lambda: saved_ui().get("open", {}).get("support") is True)
        saved = saved_ui()
        check(saved.get("open", {}).get("support") is True, "ui.json saved the open windows")
        log = open(os.path.join(d, "host.log"), errors="replace").read()
        check("forged" not in json.dumps(saved_ui()),
              "forged untokened/wrong-token console messages did not reach ui.json")
        check(log.count("dropped untokened host message") == 1,
              f"the host logged the dropped forged messages once ({log.count('dropped untokened host message')}x)")
        check("forged" not in log, "the dropped message's content is not echoed")
        page_lines = [l for l in log.splitlines() if l.startswith("page: ")]
        check(not page_lines, f"the page (bundled plugins only) ran without errors ({page_lines[:3]})")
    finally:
        stop_host(host, shm_path)
    if not a.shot:
        plugin_frame_check(a, d)
        migration_check(a, d)
        example_check(a, d)
    print(f"output in {d}")
    sys.exit(1 if FAILED else 0)


if __name__ == "__main__":
    main()
