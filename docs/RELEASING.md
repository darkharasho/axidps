# Releasing

A `v*` tag builds the signed Windows installer and the Linux AppImage, then publishes them to
`darkharasho/axidps-releases` as a prerelease. The release notes say "Not yet checked in game on
Windows." until you edit them.

## One-time setup

1. Create the public repo `darkharasho/axidps-releases`. It only holds release assets.
2. The DLL's game-facing crates live in the private repo `darkharasho/axidps-internal`, checked
   out at `private/` (gitignored here). Create a read-only deploy key on that repo and store its
   private half as the secret `PRIVATE_DEPLOY_KEY` in this repo's `release` environment
   (Settings, Environments). Limit the environment's deployment branches to `main` and tags
   `v*`, so only those runs can fetch the private crates.
3. Create a fine-grained personal access token limited to `axidps-releases` with Contents: read
   and write, and set it as the secret `RELEASES_TOKEN` on this repo.
4. Set the Windows signing secrets (Azure Artifact Signing; the script reads the client secret from
   1Password and never prints it):
   `~/.claude/templates/windows-signing/set-azure-signing-secrets.sh darkharasho/axidps`
5. Set `PUBLIC_CHECK_PATTERNS` (one regex per line: names and addresses that must never appear
   in this repo) as a secret, for `tools/public-check`.

## The private crates

The DLL is built from `private/` (its own Cargo workspace, depending on this repo's crates by
path). Public workflows fetch it at the commit pinned in `.github/private-ref`, only on pushes to
`main`, tags and manual runs, never for pull requests. Those jobs (`dll-check` in ci.yml, `dll` in
release.yml) never cache a target dir and send compiler output to a file that is never printed,
because it would quote private source. When a DLL build fails there, reproduce it locally.

After changing the private crates: push `private/`, then run `tools/bump-private` here and commit
`.github/private-ref`.

## Releasing a version

1. Bump the version. If the private crates changed, run `tools/bump-private`. Commit and push.
2. `git tag v<x.y.z> && git push origin v<x.y.z>`. The version is the tag without `v` and must be
   valid semver.
3. The `Release` workflow first creates a draft prerelease, then builds the DLL and signs it
   (`sign-dll`), then builds the signed Windows installer (every `.exe` and the DLL must verify
   as `Valid`, including `uninstall.exe`, which NSIS signs as it builds the installer and the job
   checks by installing silently into a temp folder) and the AppImage side by side. `appimage-check`
   then runs `ldd` on the AppImage's programs in a bare `ubuntu:24.04` container (on a
   GitHub-hosted runner, whatever built it). Any library or glibc version a plain system lacks
   fails the release. That is also the glibc floor: the `linux` job builds on Ubuntu 24.04, so
   the AppImage needs glibc 2.39 (SteamOS 3.7, Ubuntu 24.04, Debian 13 and newer). Don't move it
   to `ubuntu-latest` (26.04, glibc 2.43) or a newer system. The jobs hand files to each other through that draft
   rather than workflow artifacts, so the account's shared artifact quota can't block a release.
   If any job fails, the draft stays unpublished (rerunning the failed jobs reuses it).
4. When all are green, `finalize` adds `SHA256SUMS` and removes the hand-off DLLs, leaving the
   installer, the AppImage and `SHA256SUMS`, and `publish` takes it out of draft. `publish` also
   waits for the `tests` job, which runs `ci.yml`'s tests (Linux and Windows) on the tagged
   commit alongside the builds; if they fail, the draft stays unpublished. Fix the tests, bump
   the version and tag again, or rerun `tests` (then `publish`) if the failure was flaky. Edit the notes
   once you've checked the build in game.

Expect a tag build to take about an hour. GitHub only lets a tag's run restore caches saved on
`main` or on the tag itself, so a tag build compiles the overlay host (Servo) from scratch; the
Windows job is the long one (about 60 minutes cold, 41 with a warm cache). Caches from a test
run on another branch don't carry over. The `dll` job builds without a cache every time (see
above). `cache-warm.yml` builds the Windows and Linux jobs' dependencies on every push to `main` (it stops in seconds when the cache for the current Cargo.lock files exists),
so a tag restores them instead.

The `linux` job runs on GitHub's Ubuntu 24.04. A runner with a glibc newer than 2.39 fails
`appimage-check`.

After `publish`, check the release by hand:

    gh release download v<x.y.z> -R darkharasho/axidps-releases -D /tmp/axidps-v<x.y.z>
    cd /tmp/axidps-v<x.y.z> && sha256sum -c SHA256SUMS

The windows job's "Verify everything is signed" step prints each file's signature status
(`Valid`), never the signer's name: run logs are public.

To test the build without publishing, run the workflow manually (Actions, Release, Run workflow)
with a version such as `0.0.0-ci.1`. It builds and verifies everything into a draft named
`test-<run id>` on `axidps-releases` and never publishes it. Delete that draft after checking it:
`gh release delete test-<run id> -R darkharasho/axidps-releases -y` (a draft has no tag, so
don't pass `--cleanup-tag`).

## Linux

The AppImage ships the overlay host and the DLL. Under Steam, set this launch option (as for
arcdps) so the game loads the `d3d11.dll` in its folder:

    WINEDLLOVERRIDES="d3d11=n,b" %command%

The overlay host draws through a GL window that Wayland won't let it hide. On KDE the launcher
adds the KWin rule `axidps-overlay-hidden` to `kwinrulesrc` at each start when `kwriteconfig6` is
installed (the same rule `apps/axidps-overlay/install.sh` writes; it only writes when something is
missing or changed, then asks KWin to reload), which keeps that window off the taskbar, the pager
and Alt+Tab. Known gap: GNOME and other desktops have no such rule, so a window titled "axidps
overlay host" shows there.

The launcher logs to `~/.config/axidps/axidps.log` (`%APPDATA%\axidps\axidps.log` on Windows),
rotated to `axidps.log.1` at 4 MB. Started from a terminal on Linux, it prints there instead.
