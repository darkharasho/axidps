# Releasing

A `v*` tag builds the signed Windows installer and the Linux AppImage, then publishes them as a
prerelease on this repo (`darkharasho/axidps`), with the workflow's own token. The release notes say "Not yet checked in game on
Windows." until you edit them.

## One-time setup

1. The DLL's game-facing crates live in the private repo `darkharasho/axidps-internal`, checked
   out at `private/` (gitignored here). Create a read-only deploy key on that repo and store its
   private half as the secret `PRIVATE_DEPLOY_KEY` in this repo's `release` environment
   (Settings, Environments). Limit the environment's deployment branches to `main` and tags
   `v*`, so only those runs can fetch the private crates.
2. For the `mirror` job while it lasts (see below): a fine-grained personal access token limited
   to `axidps-releases` with Contents: read and write, as the secret `RELEASES_TOKEN`.
3. Set the Windows signing secrets (Azure Artifact Signing; the script reads the client secret from
   1Password and never prints it):
   `~/.claude/templates/windows-signing/set-azure-signing-secrets.sh darkharasho/axidps`
4. Set `PUBLIC_CHECK_PATTERNS` (one regex per line: names and addresses that must never appear
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
   installer, the AppImage and `SHA256SUMS`, and `publish` takes it out of draft. `mirror` then
   copies it to the old `darkharasho/axidps-releases` (see below). `publish` also
   waits for the `tests` job, which runs `ci.yml`'s tests (Linux and Windows) on the tagged
   commit alongside the builds; if they fail, the draft stays unpublished. Fix the tests, bump
   the version and tag again, or rerun `tests` (then `publish`) if the failure was flaky. Edit the notes
   once you've checked the build in game, on both repos while `mirror` runs (it copies the notes
   as they are at publish time).

A tag build takes about 7 to 10 minutes when the overlay host (Servo) hasn't changed. GitHub only
lets a tag's run restore caches saved on `main` or on the tag itself, so `cache-warm.yml` builds
on every push to `main` what a release needs: the app's build cache, the overlay host's build
cache, and the overlay host binary itself, keyed by its sources (`tools/overlay-host-key`). A
release whose overlay host matches restores that binary and skips Servo entirely; one that
changed it builds it from the restored cache. Tag the commit after cache-warm's run for it
finishes, or the release builds the overlay host itself. Release, test and pull request runs only
restore caches; only `main` saves them, so they stay under the repo's 10 GB cache limit. Bump
`SALT` in `tools/overlay-host-key` when the overlay host's build setup changes in a way its files
don't show (LLVM, MozTools, the runner image). The `dll` job builds without a cache every time
(see above).

The `linux` job runs on GitHub's Ubuntu 24.04. A runner with a glibc newer than 2.39 fails
`appimage-check`.

After `publish`, check the release by hand:

    gh release download v<x.y.z> -R darkharasho/axidps -D /tmp/axidps-v<x.y.z>
    cd /tmp/axidps-v<x.y.z> && sha256sum -c SHA256SUMS

The windows job's "Verify everything is signed" step prints each file's signature status
(`Valid`), never the signer's name: run logs are public.

To test the build without publishing, run the workflow manually (Actions, Release, Run workflow)
with a version such as `0.0.0-ci.1`. It builds and verifies everything into a draft named
`test-<run id>` on this repo and never publishes it. Delete that draft after checking it:
`gh release delete test-<run id> -R darkharasho/axidps -y` (a draft has no tag, so
don't pass `--cleanup-tag`).

## The old releases repo

Releases up to rc.22 were published to `darkharasho/axidps-releases`, and builds up to rc.22 and
AxiOM up to v0.7.8 look for updates only there. The `mirror` job copies each release there until
everyone has a build and an AxiOM that read this repo. Then delete the `mirror` job and
`MIRROR_REPO` from release.yml, delete the `RELEASES_TOKEN` secret and its token, and archive
`axidps-releases`.

## Linux

The AppImage ships the overlay host and the DLL. Under Steam, set this launch option so the
game loads the `d3d11.dll` in its folder:

    WINEDLLOVERRIDES="d3d11=n,b" %command%

The overlay host draws through a GL window that Wayland won't let it hide. On KDE the launcher
adds the KWin rule `axidps-overlay-hidden` to `kwinrulesrc` at each start when `kwriteconfig6` is
installed (the same rule `apps/axidps-overlay/install.sh` writes; it only writes when something is
missing or changed, then asks KWin to reload), which keeps that window off the taskbar, the pager
and Alt+Tab. Known gap: GNOME and other desktops have no such rule, so a window titled "axidps
overlay host" shows there.

The launcher logs to `~/.config/axidps/axidps.log` (`%APPDATA%\axidps\axidps.log` on Windows),
rotated to `axidps.log.1` at 4 MB. Started from a terminal on Linux, it prints there instead.
