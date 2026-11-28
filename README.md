# axidps

A combat meter and log recorder for Guild Wars 2. A small in-game DLL captures combat data, and a
separate program writes it as standard EVTC logs, which Elite Insights and other log tools read.
It has built-in healing and barrier stats, an in-game overlay drawn with Servo, plugins, and
self-updates. Windows and Linux (Proton, including the Steam Deck) are both supported.

Downloads: [Releases](https://github.com/darkharasho/axidps/releases).

## What's in this repo

Everything except the DLL's game-facing code. That part finds and reads the game client's memory,
and it stays in a private repo so this one isn't a ready-made map for cheat makers. The shipped DLL
is signed, and the launcher only installs signed builds.

```
crates/
  axidps-protocol/       Event format between the DLL and the host (v2)
  axidps-ringbuf/        Shared-memory ring the DLL writes and the host reads
  axidps-overlay-shm/    Shared memory between the DLL and the overlay host
  axidps-evtc/           EVTC model, encoder and .zevtc container
  axidps-core/           Fight processor: turns ring events into EVTC logs
  axidps-app/            The launcher: installs the DLL, runs core and the overlay, updates
  axidps-plugins/        Plugin manifests, permissions, registry and signatures
  axidps-mock-producer/  Fake WvW events on the ring, for working without the game
apps/
  axidps-overlay/        Overlay host (Servo) and the plugin SDK
packaging/               Windows installer (NSIS) and Linux AppImage
docs/plugins/            Writing plugins
docs/RELEASING.md        How releases are built and published
```

## How it fits together

```
GW2 process (Windows or Proton)               Host
┌──────────────────────────────┐              ┌──────────────────────────────┐
│ axidps DLL (d3d11.dll proxy) │   v2 event   │ axidps-core                  │
│  captures combat events and  │──ring file──→│  ├─ agent registry           │
│  turns itself off for any    │   (mmap)     │  ├─ fight start/end rules    │
│  feature it can't verify on  │              │  ├─ EVTC assembly            │
│  the running game build      │              │  └─ .zevtc → log folder      │
└──────────────────────────────┘              └──────────────────────────────┘
```

`axidps-core` writes logs to `Documents/Guild Wars 2/addons/arcdps/arcdps.cbtlogs`, the folder log
uploaders already watch, so they pick them up with no setup. The ring is multi-producer and
lock-free. On Proton the DLL writes `Z:\tmp\axidps_ring_v2` (`/tmp/axidps_ring_v2` on the host).
On Windows both sides use `%TEMP%\axidps_ring_v2`; pass `--ring <path>` to `axidps-core` to point it elsewhere.

## Development

Rust stable. The launcher on Linux needs `libwayland-dev libdbus-1-dev pkg-config` (or your
distro's equivalents).

```bash
cargo test --workspace

# Capture logs while the game runs
cargo run -p axidps-core -- --trigger any --min-squad 0

# Without the game: fake fights on the ring (start axidps-core first, or within 5 s)
cargo run -p axidps-mock-producer -- --fights 2
```

The overlay host is its own Cargo workspace in `apps/axidps-overlay` (Servo; see its
`rust-toolchain.toml`). For plugins, start with [docs/plugins](docs/plugins/README.md).

Pull requests are welcome for everything here. CI runs the public crates' tests on them; the DLL
is built only on `main` and tags.

## License

MIT
