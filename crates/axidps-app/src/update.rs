//! Self-update from this repo's releases (public `darkharasho/axidps`, GitHub's unauthenticated
//! API; no token is ever used). The check and download run on a background thread; only the
//! apply step runs on the run loop.

use std::collections::HashMap;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use regex::Regex;
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::game::gw2_running;
use crate::supervise::Supervisor;

pub const RELEASES_URL: &str =
    "https://api.github.com/repos/darkharasho/axidps/releases?per_page=20";
/// SHA-256 of the display name (CN) every Windows installer's signer must carry. Kept as a
/// hash so the name is not in the source; the signed installer itself carries it in the clear.
#[cfg_attr(any(not(windows), test), allow(dead_code))]
const PUBLISHER_SHA256: [u8; 32] =
    hex32("377dbaa704dddf7ba546359940e6e2680bd2a27a3589de531bf1b68cfbcc4524");
const CHECK_EVERY: Duration = Duration::from_secs(6 * 3600);
const WAIT_RECHECK: Duration = Duration::from_secs(60);
const STATUS_FILE: &str = "update-status.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Windows,
    Linux,
}

impl Os {
    pub fn current() -> Os {
        if cfg!(windows) {
            Os::Windows
        } else {
            Os::Linux
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Asset {
    pub name: String,
    #[serde(rename = "browser_download_url")]
    pub url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Release {
    #[serde(rename = "tag_name")]
    pub tag: String,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    #[allow(dead_code)]
    pub prerelease: bool,
    #[serde(default)]
    pub assets: Vec<Asset>,
}

#[derive(Debug)]
pub enum UpdateError {
    Http(u16),
    Network(String),
    Parse(String),
    VerifyFailed,
    Io(io::Error),
}

impl std::fmt::Display for UpdateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UpdateError::Http(code) => write!(f, "HTTP {code}"),
            UpdateError::Network(e) => write!(f, "network: {e}"),
            UpdateError::Parse(e) => write!(f, "bad response: {e}"),
            UpdateError::VerifyFailed => write!(f, "checksum or signature check failed"),
            UpdateError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl From<io::Error> for UpdateError {
    fn from(e: io::Error) -> Self {
        UpdateError::Io(e)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    UpToDate,
    Available,
    WaitingForGame,
    ManagedByAxiom,
    CheckFailed,
    VerifyFailed,
    Applying,
}

/// What `update-status.json` holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Status {
    pub current: String,
    pub latest: Option<String>,
    pub state: State,
}

/// How a request's body is fetched: `fetch` returns it whole (API JSON, SHA256SUMS), `download`
/// streams it into a writer (the installer).
pub type Fetch<'a> = &'a dyn Fn(&str) -> Result<Vec<u8>, UpdateError>;
pub type Download<'a> = &'a dyn Fn(&str, &mut dyn Write) -> Result<(), UpdateError>;
/// [`apply`], or a stand-in for it in tests.
type Apply<'a> = &'a dyn Fn(Os, &Path, Option<&Path>) -> io::Result<()>;

pub fn asset_for(r: &Release, os: Os) -> Option<&Asset> {
    static PATTERNS: OnceLock<[Regex; 2]> = OnceLock::new();
    let [win, linux] = PATTERNS.get_or_init(|| {
        [
            Regex::new(r"(?i)axidps.*Setup.*\.exe$").unwrap(),
            Regex::new(r"(?i)axidps.*\.AppImage$").unwrap(),
        ]
    });
    let re = if os == Os::Windows { win } else { linux };
    r.assets.iter().find(|a| re.is_match(&a.name))
}

fn parse_tag(tag: &str) -> Option<Version> {
    Version::parse(tag.strip_prefix('v').unwrap_or(tag)).ok()
}

/// The newest non-draft release with an asset for `os` and a version above `current`.
/// Tags that are not semver are skipped.
pub fn pick_latest<'a>(rs: &'a [Release], current: &str, os: Os) -> Option<&'a Release> {
    let current = Version::parse(current).ok()?;
    rs.iter()
        .filter(|r| !r.draft && asset_for(r, os).is_some())
        .filter_map(|r| Some((parse_tag(&r.tag)?, r)))
        .filter(|(v, _)| *v > current)
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, r)| r)
}

/// `<hex>  <name>` or `<hex> *<name>` lines, hex lowercased.
pub fn parse_sha256sums(text: &str) -> HashMap<String, String> {
    text.lines()
        .filter_map(|line| {
            let (hash, name) = line.trim().split_once(char::is_whitespace)?;
            let name = name.trim_start().trim_start_matches('*');
            (!name.is_empty()).then(|| (name.to_owned(), hash.to_ascii_lowercase()))
        })
        .collect()
}

pub fn verify_sha256(file: &Path, expected_hex: &str) -> io::Result<bool> {
    let mut f = fs::File::open(file)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let hex: String = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    Ok(hex.eq_ignore_ascii_case(expected_hex.trim()))
}

/// `app.autoUpdate` in `ui.json`: on unless it is `false`.
pub fn auto_update_enabled(ui: &serde_json::Value) -> bool {
    ui.pointer("/app/autoUpdate") != Some(&serde_json::Value::Bool(false))
}

/// Whether Gear Lever (which keeps AppImages in `~/AppImages`) manages `appimage`.
pub fn gear_lever_managed(appimage: &Path, home: &Path) -> bool {
    appimage.starts_with(home.join("AppImages"))
}

/// One check: the status, and the release to install when there is one.
pub fn check(current: &str, os: Os, fetch: Fetch) -> (Status, Option<Release>) {
    let status = |latest, state| Status {
        current: current.to_owned(),
        latest,
        state,
    };
    let releases = fetch(RELEASES_URL).and_then(|body| {
        serde_json::from_slice::<Vec<Release>>(&body).map_err(|e| UpdateError::Parse(e.to_string()))
    });
    match releases {
        Err(e) => {
            eprintln!("axidps: update check failed: {e}");
            (status(None, State::CheckFailed), None)
        }
        Ok(rs) => match pick_latest(&rs, current, os) {
            None => (status(None, State::UpToDate), None),
            Some(r) => {
                let latest = r.tag.strip_prefix('v').unwrap_or(&r.tag).to_owned();
                (status(Some(latest), State::Available), Some(r.clone()))
            }
        },
    }
}

/// Whether a signer's display name, trimmed, hashes to `sha256`.
#[cfg_attr(any(not(windows), test), allow(dead_code))]
pub(crate) fn publisher_matches(name: &str, sha256: &[u8; 32]) -> bool {
    <[u8; 32]>::from(Sha256::digest(name.trim().as_bytes())) == *sha256
}

/// Parses 64 hex digits at compile time.
const fn hex32(s: &str) -> [u8; 32] {
    const fn nib(c: u8) -> u8 {
        match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'f' => c - b'a' + 10,
            _ => panic!("bad hex"),
        }
    }
    let b = s.as_bytes();
    assert!(b.len() == 64);
    let mut out = [0u8; 32];
    let mut i = 0;
    while i < 32 {
        out[i] = nib(b[2 * i]) << 4 | nib(b[2 * i + 1]);
        i += 1;
    }
    out
}

/// Windows: `file` carries a valid Authenticode signature from the signer in
/// [`PUBLISHER_SHA256`]. Linux has no signature
/// to check. Tests decide the answer with `tests::SIGNED`, since their files are not signed.
fn signature_ok(file: &Path) -> bool {
    #[cfg(test)]
    {
        let _ = file;
        tests::SIGNED.with(|s| s.get())
    }
    #[cfg(all(windows, not(test)))]
    {
        crate::sigcheck::authenticode_ok(file, &PUBLISHER_SHA256)
    }
    #[cfg(all(not(windows), not(test)))]
    {
        let _ = file;
        true
    }
}

/// Downloads `asset` to `<dir>/<name>.axidps-part`, checks it against `sums` (and, on Windows, its
/// Authenticode signature) and returns the verified file. Anything unverified is deleted.
pub fn download_verified(
    asset: &Asset,
    sums: &HashMap<String, String>,
    dir: &Path,
    download: Download,
) -> Result<PathBuf, UpdateError> {
    let safe_name = Path::new(&asset.name).file_name().and_then(|n| n.to_str());
    let (Some(name), Some(expected)) = (safe_name, sums.get(&asset.name)) else {
        return Err(UpdateError::VerifyFailed);
    };
    if name != asset.name {
        return Err(UpdateError::VerifyFailed);
    }
    fs::create_dir_all(dir)?;
    let part = dir.join(format!("{name}.{PART}"));
    // A verified Linux file gets its own name so it never lands on the AppImage it replaces.
    let done = dir.join(if cfg!(windows) {
        name.to_owned()
    } else {
        format!("{name}.new")
    });
    let result = (|| {
        let mut f = fs::File::create(&part)?;
        download(&asset.url, &mut f)?;
        f.sync_all()?;
        drop(f);
        if !verify_sha256(&part, expected)? {
            return Err(UpdateError::VerifyFailed);
        }
        fs::rename(&part, &done)?;
        if !signature_ok(&done) {
            return Err(UpdateError::VerifyFailed);
        }
        Ok(())
    })();
    match result {
        Ok(()) => Ok(done),
        Err(e) => {
            let _ = fs::remove_file(&part);
            let _ = fs::remove_file(&done);
            Err(e)
        }
    }
}

/// The flag a Linux update starts the new launcher with: it waits for this one to let go of the
/// single-instance lock instead of exiting at once.
#[cfg_attr(windows, allow(dead_code))]
pub const AFTER_UPDATE_FLAG: &str = "--after-update";

/// Where the new AppImage goes. Release AppImages are named by version
/// (`axidps-<version>.AppImage`); one still under such a name gets the new asset's name, so the
/// file name says what it runs. Any other name (the user renamed it) is kept.
#[cfg_attr(windows, allow(dead_code))]
pub fn appimage_target(current: &Path, asset_name: &str) -> PathBuf {
    let versioned = Regex::new(r"(?i)^axidps-\d+\.\d+\.\d+[0-9A-Za-z.+-]*\.AppImage$").unwrap();
    let is_versioned = |n: &str| versioned.is_match(n);
    let current_name = current.file_name().and_then(|n| n.to_str());
    match (current.parent(), current_name) {
        (Some(dir), Some(name))
            if is_versioned(name)
                && is_versioned(asset_name)
                && Path::new(asset_name).file_name() == Some(asset_name.as_ref()) =>
        {
            dir.join(asset_name)
        }
        _ => current.to_owned(),
    }
}

/// Starts the new version and returns; the caller exits. Windows: runs the installer silently,
/// detached. Linux: puts the new AppImage in place of `appimage` (under the new version's name
/// when the old one had a versioned name; the old file is then removed) and starts it as a new
/// process. The old AppImage's runtime keeps its image mounted until every holder of its
/// keepalive pipe has closed it, and this launcher inherited that pipe (it is not close-on-exec).
/// So the new AppImage is not `exec`'d in this process, and every inherited descriptor is marked
/// close-on-exec before it starts: once this process exits, the old image unmounts.
#[cfg_attr(windows, allow(unused_variables))]
pub fn apply(os: Os, file: &Path, appimage: Option<&Path>) -> io::Result<()> {
    match os {
        #[cfg(windows)]
        Os::Windows => {
            use std::os::windows::process::CommandExt;
            const DETACHED_PROCESS: u32 = 0x0000_0008; // public-check:ok
            const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000; // public-check:ok
            let spawn = |flags| {
                std::process::Command::new(file)
                    .arg("/S")
                    .creation_flags(flags)
                    .spawn()
            };
            // Breakaway is refused when the job we run in forbids it; then detach only.
            spawn(DETACHED_PROCESS | CREATE_BREAKAWAY_FROM_JOB)
                .or_else(|_| spawn(DETACHED_PROCESS))
                .map(drop)
        }
        #[cfg(unix)]
        Os::Linux => {
            let current = appimage
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no AppImage path"))?;
            let args: Vec<_> = std::env::args_os()
                .skip(1)
                .filter(|a| a != AFTER_UPDATE_FLAG)
                .collect();
            replace_appimage(file, current, &args)
        }
        #[allow(unreachable_patterns)]
        _ => Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "not this platform's update",
        )),
    }
}

/// Marks every open descriptor from `first` up close-on-exec: `close_range` (Linux 5.11+),
/// else each entry of `/proc/self/fd`. This process keeps them; only children lose them.
#[cfg(unix)]
fn cloexec_from(first: i32) {
    #[cfg(target_os = "linux")]
    // SAFETY: close_range with CLOSE_RANGE_CLOEXEC only sets a flag; nothing is closed.
    if unsafe {
        libc::close_range(
            first as libc::c_uint,
            libc::c_uint::MAX,
            libc::CLOSE_RANGE_CLOEXEC as libc::c_int,
        )
    } == 0
    {
        return;
    }
    cloexec_from_proc(first);
}

/// [`cloexec_from`]'s fallback for kernels without `close_range`.
#[cfg(unix)]
fn cloexec_from_proc(first: i32) {
    let Ok(entries) = fs::read_dir("/proc/self/fd") else {
        return;
    };
    let fds: Vec<i32> = entries
        .flatten()
        .filter_map(|e| e.file_name().to_str()?.parse().ok())
        .filter(|&fd| fd >= first)
        .collect();
    for fd in fds {
        // SAFETY: fcntl on a descriptor number; one that closed meanwhile just fails (EBADF).
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFD);
            if flags >= 0 {
                libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC);
            }
        }
    }
}

/// The Linux side of [`apply`]: moves `file` into place, starts it with `args` and
/// [`AFTER_UPDATE_FLAG`], and removes the old AppImage if the new one has another name. A
/// failed start removes the new file under a new name, leaving the old AppImage as it was.
#[cfg(unix)]
fn replace_appimage(file: &Path, current: &Path, args: &[std::ffi::OsString]) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::process::CommandExt;
    // `<asset name>.new`, as download_verified names it.
    let asset = file
        .file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.strip_suffix(".new").unwrap_or(n))
        .unwrap_or_default();
    let target = appimage_target(current, asset);
    fs::set_permissions(file, fs::Permissions::from_mode(0o755))?;
    if fs::rename(file, &target).is_err() {
        // Another filesystem: copy beside the target, then rename.
        let part = PathBuf::from(format!("{}.{PART}", target.display()));
        let copied = fs::copy(file, &part)
            .and_then(|_| fs::set_permissions(&part, fs::Permissions::from_mode(0o755)))
            .and_then(|_| fs::rename(&part, &target));
        if let Err(e) = copied {
            let _ = fs::remove_file(&part);
            return Err(e);
        }
        let _ = fs::remove_file(file);
    }
    // The old runtime's keepalive pipe (and anything else inherited) must not reach the new
    // launcher, or the old image stays mounted for as long as it runs. Not in `pre_exec`: std's
    // own exec-error pipe lives there.
    cloexec_from(3);
    let started = std::process::Command::new(&target)
        .args(args)
        .arg(AFTER_UPDATE_FLAG)
        // Its own process group: not stopped with this one's terminal or session.
        .process_group(0)
        .spawn();
    match started {
        Ok(_) => {
            if target != current
                && let Err(e) = fs::remove_file(current)
            {
                eprintln!("axidps: cannot remove the old {}: {e}", current.display());
            }
            eprintln!("axidps: started {}", target.display());
            Ok(())
        }
        Err(e) => {
            if target != current {
                let _ = fs::remove_file(&target);
            }
            Err(e)
        }
    }
}

/// Writes `update-status.json` through a temp file and a rename.
pub fn write_status(dir: &Path, status: &Status) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let tmp = dir.join(format!("{STATUS_FILE}.tmp"));
    fs::write(
        &tmp,
        serde_json::to_string(status).map_err(io::Error::other)?,
    )?;
    fs::rename(&tmp, dir.join(STATUS_FILE))
}

fn agent(total: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(total))
        .timeout_connect(Some(Duration::from_secs(15)))
        .user_agent(format!(
            "axidps/{}",
            axidps_protocol::ident::build_version()
        ))
        .build()
        .into()
}

fn get(url: &str, total: Duration) -> Result<ureq::http::Response<ureq::Body>, UpdateError> {
    agent(total)
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| match e {
            ureq::Error::StatusCode(code) => UpdateError::Http(code),
            other => UpdateError::Network(other.to_string()),
        })
}

fn http_fetch(url: &str) -> Result<Vec<u8>, UpdateError> {
    let mut resp = get(url, Duration::from_secs(30))?;
    resp.body_mut()
        .with_config()
        .limit(32 * 1024 * 1024)
        .read_to_vec()
        .map_err(|e| UpdateError::Network(e.to_string()))
}

fn http_download(url: &str, out: &mut dyn Write) -> Result<(), UpdateError> {
    let mut resp = get(url, Duration::from_secs(1800))?;
    io::copy(&mut resp.body_mut().as_reader(), out)?;
    Ok(())
}

fn read_ui(dir: &Path) -> serde_json::Value {
    fs::read_to_string(dir.join("ui.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or(serde_json::Value::Null)
}

/// `--check-update`: one check, printed and written; nothing is downloaded or applied.
pub fn check_once() -> std::process::ExitCode {
    let current = axidps_protocol::ident::build_version();
    let (status, _) = check(current, Os::current(), &http_fetch);
    if let Ok(text) = serde_json::to_string_pretty(&status) {
        println!("{text}");
    }
    if let Some(dir) = axidps_protocol::paths::config_dir()
        && let Err(e) = write_status(&dir, &status)
    {
        eprintln!("axidps: cannot write {STATUS_FILE}: {e}");
    }
    std::process::ExitCode::SUCCESS
}

/// What the background thread reports.
enum Msg {
    Status(Status),
    /// A verified file for the release in the status.
    Ready(Status, Ready),
    /// Nothing new to say (the pending release is already downloaded).
    Idle,
}

/// A downloaded, verified installer waiting to be applied.
struct Ready {
    file: PathBuf,
    tag: String,
    /// Its SHA-256, checked again right before the apply.
    sha: String,
}

struct Plan {
    os: Os,
    /// Whether an apply could happen (false on Linux outside an AppImage).
    can_apply: bool,
    current: String,
    auto: bool,
    managed: bool,
    /// The tag already downloaded and waiting.
    have: Option<String>,
    dir: PathBuf,
}

/// One background cycle: check, then (when allowed) download and verify.
fn cycle(plan: &Plan, fetch: Fetch, download: Download) -> Msg {
    let (mut status, release) = check(&plan.current, plan.os, fetch);
    let Some(release) = release else {
        return Msg::Status(status);
    };
    if !plan.auto {
        return Msg::Status(status);
    }
    if !plan.can_apply {
        return Msg::Status(status);
    }
    if plan.os == Os::Linux && plan.managed {
        status.state = State::ManagedByAxiom;
        return Msg::Status(status);
    }
    if plan.have.as_deref() == Some(release.tag.as_str()) {
        return Msg::Idle;
    }
    let verified: Result<(PathBuf, String), UpdateError> = (|| {
        let asset = asset_for(&release, plan.os).ok_or(UpdateError::VerifyFailed)?;
        // Never apply an unverified file: no SHA256SUMS means no update.
        let sums = release
            .assets
            .iter()
            .find(|a| a.name == "SHA256SUMS")
            .ok_or(UpdateError::VerifyFailed)?;
        let sums = parse_sha256sums(&String::from_utf8_lossy(&fetch(&sums.url)?));
        let file = download_verified(asset, &sums, &plan.dir, download)?;
        let sha = sums[&asset.name].clone();
        Ok((file, sha))
    })();
    match verified {
        Ok((file, sha)) => Msg::Ready(
            status,
            Ready {
                file,
                tag: release.tag,
                sha,
            },
        ),
        Err(e) => {
            eprintln!("axidps: update {} not installed: {e}", release.tag);
            status.state = match e {
                UpdateError::VerifyFailed => State::VerifyFailed,
                _ => State::CheckFailed,
            };
            Msg::Status(status)
        }
    }
}

/// Where to download: beside the AppImage (same filesystem, so the swap is a rename) when that
/// folder is writable, else the config folder's `updates`.
fn download_dir(os: Os, appimage: Option<&Path>, config: &Path) -> PathBuf {
    if os == Os::Linux
        && let Some(parent) = appimage.and_then(Path::parent)
    {
        let probe = parent.join(format!(".axidps-write-test-{}", std::process::id()));
        if fs::File::create(&probe).is_ok() {
            let _ = fs::remove_file(&probe);
            eprintln!(
                "axidps: updates download beside the AppImage in {}",
                parent.display()
            );
            return parent.to_owned();
        }
        eprintln!(
            "axidps: {} is not writable; updates use the config folder",
            parent.display()
        );
    }
    config.join("updates")
}

/// The run loop's side of the updater: starts checks, receives results, applies.
pub struct Updater {
    config: Option<PathBuf>,
    appimage: Option<PathBuf>,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    next_check: Instant,
    in_flight: bool,
    current: String,
    /// The last status reported; `None` until the first result, which is always written.
    status: Option<Status>,
    ready: Option<Ready>,
    next_wait: Instant,
    apply_blocked: bool,
    /// A check the user asked for; the next `tick` starts it.
    check_requested: bool,
}

impl Updater {
    pub fn new(now: Instant) -> Updater {
        let config = axidps_protocol::paths::config_dir();
        let appimage = std::env::var_os("APPIMAGE").map(PathBuf::from);
        cleanup_stale(config.as_deref(), appimage.as_deref());
        Updater::with(config, appimage, now)
    }

    fn with(config: Option<PathBuf>, appimage: Option<PathBuf>, now: Instant) -> Updater {
        let (tx, rx) = channel();
        let current = axidps_protocol::ident::build_version().to_owned();
        Updater {
            config,
            appimage,
            tx,
            rx,
            next_check: now,
            in_flight: false,
            current,
            status: None,
            ready: None,
            next_wait: now,
            apply_blocked: false,
            check_requested: false,
        }
    }

    /// Makes the next `tick` start a check now, as the timer would. Ignored while a check runs.
    #[cfg_attr(not(windows), allow(dead_code))] // only the Windows tray asks
    pub fn request_check(&mut self) {
        if !self.in_flight {
            self.check_requested = true;
        }
    }

    fn set_status(&mut self, status: Status) {
        if self.status.as_ref() == Some(&status) {
            return;
        }
        if let Some(dir) = &self.config
            && let Err(e) = write_status(dir, &status)
        {
            eprintln!("axidps: cannot write {STATUS_FILE}: {e}");
        }
        self.status = Some(status);
    }

    /// Forgets the waiting file and deletes it.
    fn drop_ready(&mut self) {
        if let Some(r) = self.ready.take() {
            let _ = fs::remove_file(r.file);
        }
    }

    fn start_check(&mut self) {
        let Some(config) = self.config.clone() else {
            return;
        };
        self.in_flight = true;
        let os = Os::current();
        let plan = Plan {
            os,
            can_apply: os != Os::Linux || self.appimage.is_some(),
            current: self.current.clone(),
            auto: auto_update_enabled(&read_ui(&config)),
            managed: self
                .appimage
                .as_deref()
                .is_some_and(|a| gear_lever_managed(a, &crate::run::home())),
            have: self.ready.as_ref().map(|r| r.tag.clone()),
            dir: download_dir(os, self.appimage.as_deref(), &config),
        };
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(cycle(&plan, &http_fetch, &http_download));
        });
    }

    /// Called once per run-loop tick. Never blocks on the network.
    pub fn tick(&mut self, now: Instant, supervisor: &mut Supervisor) {
        self.receive(now);
        if !self.in_flight && (self.check_requested || now >= self.next_check) {
            self.check_requested = false;
            self.next_check = now + CHECK_EVERY;
            self.start_check();
        }
        if self.ready.is_some() && !self.apply_blocked && now >= self.next_wait {
            self.try_apply(now, supervisor);
        }
    }

    /// Applies an update that is waiting, now: the caller has seen the game close and is about
    /// to quit. On success the process exits; otherwise this returns.
    pub fn apply_pending(&mut self, now: Instant, supervisor: &mut Supervisor) {
        self.apply_pending_with(now, supervisor, Os::current(), &apply);
    }

    /// `apply_pending` with the platform and the apply step passed in.
    fn apply_pending_with(
        &mut self,
        now: Instant,
        supervisor: &mut Supervisor,
        os: Os,
        apply: Apply,
    ) {
        self.receive(now);
        if self.ready.is_some() && !self.apply_blocked {
            self.try_apply_with(now, supervisor, os, false, apply);
        }
    }

    /// Takes in the results of finished checks.
    fn receive(&mut self, now: Instant) {
        while let Ok(msg) = self.rx.try_recv() {
            self.in_flight = false;
            match msg {
                Msg::Idle => {}
                // A failed check must not hide a file that is already waiting.
                Msg::Status(s) if self.ready.is_some() && s.state == State::CheckFailed => {}
                Msg::Status(s) => {
                    self.drop_ready();
                    self.set_status(s);
                }
                Msg::Ready(s, ready) => {
                    if let Some(old) = self.ready.replace(ready)
                        && Some(&old.file) != self.ready.as_ref().map(|r| &r.file)
                    {
                        let _ = fs::remove_file(old.file);
                    }
                    self.apply_blocked = false;
                    self.next_wait = now;
                    self.set_status(s);
                }
            }
        }
    }

    /// The status to report: the last one, or "up to date" before any result.
    fn status_now(&self) -> Status {
        self.status.clone().unwrap_or_else(|| Status {
            current: self.current.clone(),
            latest: None,
            state: State::UpToDate,
        })
    }

    fn try_apply(&mut self, now: Instant, supervisor: &mut Supervisor) {
        self.try_apply_with(now, supervisor, Os::current(), gw2_running(), &apply);
    }

    /// `try_apply` with the platform, the game check and the apply step passed in.
    fn try_apply_with(
        &mut self,
        now: Instant,
        supervisor: &mut Supervisor,
        os: Os,
        game_running: bool,
        apply: Apply,
    ) {
        let Some(ready) = &self.ready else {
            return;
        };
        let (file, sha) = (ready.file.clone(), ready.sha.clone());
        let with = |s: Status, state| Status { state, ..s };
        if os == Os::Linux && self.appimage.is_none() {
            eprintln!("axidps: not running from an AppImage; update downloaded but not applied");
            self.apply_blocked = true;
            self.set_status(with(self.status_now(), State::Available));
            return;
        }
        if game_running {
            self.next_wait = now + WAIT_RECHECK;
            self.set_status(with(self.status_now(), State::WaitingForGame));
            return;
        }
        // The file sat in a user-writable folder: check it is still what was verified.
        let intact = verify_sha256(&file, &sha).unwrap_or(false) && signature_ok(&file);
        if !intact {
            eprintln!(
                "axidps: {} changed after it was verified; deleted",
                file.display()
            );
            self.drop_ready();
            self.set_status(with(self.status_now(), State::VerifyFailed));
            return;
        }
        self.set_status(with(self.status_now(), State::Applying));
        supervisor.shutdown();
        match apply(os, &file, self.appimage.as_deref()) {
            Ok(()) => std::process::exit(0),
            Err(e) => {
                // The children come back on the next tick.
                eprintln!("axidps: cannot apply the update: {e}");
                self.apply_blocked = true;
                self.set_status(with(self.status_now(), State::Available));
            }
        }
    }
}

/// The suffix of a file being written: our own, not `.part`, which browsers use for their
/// downloads (startup cleanup must not delete a download of the next AppImage).
const PART: &str = "axidps-part";

/// Deletes what an earlier run left behind: the config folder's `updates`, and half-finished
/// or unapplied downloads beside the AppImage.
fn cleanup_stale(config: Option<&Path>, appimage: Option<&Path>) {
    if let Some(config) = config {
        let _ = fs::remove_dir_all(config.join("updates"));
    }
    let Some(dir) = appimage.and_then(Path::parent) else {
        return;
    };
    let stale = Regex::new(r"(?i)^axidps.*\.AppImage\.(new|axidps-part)$").unwrap();
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    // Versions before `PART` wrote `.part`, which a browser downloading the next AppImage may
    // be writing right now: only one untouched for a day is ours.
    let legacy = Regex::new(r"(?i)^axidps.*\.AppImage\.part$").unwrap();
    let day = std::time::Duration::from_secs(24 * 3600);
    let old = |e: &fs::DirEntry| {
        e.metadata()
            .and_then(|m| m.modified())
            .is_ok_and(|t| t.elapsed().is_ok_and(|age| age > day))
    };
    for e in entries.flatten() {
        let Some(n) = e.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if stale.is_match(&n) || (legacy.is_match(&n) && old(&e)) {
            let _ = fs::remove_file(e.path());
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn publisher_matches_trimmed_name_by_hash() {
        let h: [u8; 32] = sha2::Sha256::digest(b"darkharasho").into();
        assert!(super::publisher_matches(" darkharasho ", &h));
        assert!(!super::publisher_matches("someone", &h));
        assert_eq!(super::hex32(&"ab".repeat(32)), [0xab; 32]);
    }

    use super::*;
    use std::fs;
    use std::path::Path;

    thread_local! {
        /// What [`signature_ok`] answers on this test's thread.
        pub(super) static SIGNED: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
    }

    fn rel(tag: &str, draft: bool, assets: &[&str]) -> String {
        let assets: Vec<String> = assets
            .iter()
            .map(|n| format!(r#"{{"name":"{n}","browser_download_url":"https://x/{tag}/{n}"}}"#))
            .collect();
        format!(
            r#"{{"tag_name":"{tag}","draft":{draft},"prerelease":false,"assets":[{}]}}"#,
            assets.join(",")
        )
    }

    fn releases() -> Vec<Release> {
        let both = ["axidps-Setup.exe", "axidps-x86_64.AppImage", "SHA256SUMS"];
        let json = format!(
            "[{},{},{},{}]",
            rel("v0.1.9", false, &both),
            rel("v0.2.0-rc.1", false, &both[..1]),
            rel("v0.2.0", false, &both[1..]),
            rel("v9.9.9", true, &both),
        );
        serde_json::from_str(&json).unwrap()
    }

    #[test]
    fn pick_latest_orders_semver_and_skips_missing_asset() {
        let rs = releases();
        assert_eq!(
            pick_latest(&rs, "0.1.9", Os::Windows).unwrap().tag,
            "v0.2.0-rc.1"
        );
        assert_eq!(pick_latest(&rs, "0.1.9", Os::Linux).unwrap().tag, "v0.2.0");
    }

    #[test]
    fn nothing_newer_is_none() {
        let rs = releases();
        assert!(pick_latest(&rs, "0.2.0", Os::Linux).is_none());
        assert!(pick_latest(&[], "0.1.0", Os::Linux).is_none());
        let junk: Vec<Release> = serde_json::from_str(&format!(
            "[{}]",
            rel("nightly", false, &["axidps.AppImage"])
        ))
        .unwrap();
        assert!(pick_latest(&junk, "0.1.0", Os::Linux).is_none());
    }

    #[test]
    fn sha256sums_parse() {
        let m = parse_sha256sums("AB12  axidps-Setup.exe\ncd34 *axidps.AppImage\n\n");
        assert_eq!(m["axidps-Setup.exe"], "ab12");
        assert_eq!(m["axidps.AppImage"], "cd34");
        assert_eq!(m.len(), 2);
    }

    fn sha_hex(bytes: &[u8]) -> String {
        use sha2::{Digest, Sha256};
        Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    #[test]
    fn apply_rejects_bad_checksum_and_leaves_target() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("axidps.AppImage");
        fs::write(&target, b"old").unwrap();
        let asset = Asset {
            name: "axidps-new.AppImage".into(),
            url: "u".into(),
        };
        let write =
            |_: &str, w: &mut dyn std::io::Write| w.write_all(b"tampered").map_err(UpdateError::Io);
        let bad: HashMap<_, _> = [(asset.name.clone(), sha_hex(b"genuine"))].into();
        let r = download_verified(&asset, &bad, tmp.path(), &write);
        assert!(matches!(r, Err(UpdateError::VerifyFailed)));
        let missing = HashMap::new();
        let r = download_verified(&asset, &missing, tmp.path(), &write);
        assert!(matches!(r, Err(UpdateError::VerifyFailed)));
        assert_eq!(fs::read(&target).unwrap(), b"old");
        let left: Vec<_> = fs::read_dir(tmp.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(left, vec![std::ffi::OsString::from("axidps.AppImage")]);
    }

    #[test]
    fn good_checksum_downloads() {
        let tmp = tempfile::tempdir().unwrap();
        let asset = Asset {
            name: "axidps-new.AppImage".into(),
            url: "u".into(),
        };
        let sums: HashMap<_, _> = [(asset.name.clone(), sha_hex(b"genuine"))].into();
        let write =
            |_: &str, w: &mut dyn std::io::Write| w.write_all(b"genuine").map_err(UpdateError::Io);
        let path = download_verified(&asset, &sums, tmp.path(), &write).unwrap();
        assert_eq!(fs::read(path).unwrap(), b"genuine");
    }

    #[test]
    fn bad_signature_is_rejected_and_deleted() {
        SIGNED.with(|s| s.set(false));
        let tmp = tempfile::tempdir().unwrap();
        let asset = Asset {
            name: "axidps-Setup.exe".into(),
            url: "u".into(),
        };
        let sums: HashMap<_, _> = [(asset.name.clone(), sha_hex(b"genuine"))].into();
        let write =
            |_: &str, w: &mut dyn std::io::Write| w.write_all(b"genuine").map_err(UpdateError::Io);
        let r = download_verified(&asset, &sums, tmp.path(), &write);
        assert!(matches!(r, Err(UpdateError::VerifyFailed)));
        assert_eq!(fs::read_dir(tmp.path()).unwrap().count(), 0);
    }

    #[test]
    fn check_failure_is_status_not_panic() {
        let fetch = |_: &str| Err(UpdateError::Http(403));
        let (status, release) = check("0.1.9", Os::Linux, &fetch);
        assert_eq!(status.state, State::CheckFailed);
        assert_eq!(status.current, "0.1.9");
        assert!(release.is_none());
        let garbage = |_: &str| Ok(b"not json".to_vec());
        assert_eq!(
            check("0.1.9", Os::Linux, &garbage).0.state,
            State::CheckFailed
        );
    }

    #[test]
    fn check_reports_available_and_up_to_date() {
        let body = format!("[{}]", rel("v0.2.0", false, &["axidps.AppImage"])).into_bytes();
        let fetch = |_: &str| Ok(body.clone());
        let (s, r) = check("0.1.9", Os::Linux, &fetch);
        assert_eq!(
            (s.state, s.latest.as_deref()),
            (State::Available, Some("0.2.0"))
        );
        assert_eq!(r.unwrap().tag, "v0.2.0");
        let (s, r) = check("0.2.0", Os::Linux, &fetch);
        assert_eq!(s.state, State::UpToDate);
        assert!(r.is_none());
    }

    #[test]
    fn auto_update_setting() {
        assert!(auto_update_enabled(&serde_json::json!({})));
        assert!(auto_update_enabled(&serde_json::Value::Null));
        assert!(!auto_update_enabled(
            &serde_json::json!({"app":{"autoUpdate":false}})
        ));
        assert!(auto_update_enabled(
            &serde_json::json!({"app":{"autoUpdate":true}})
        ));
    }

    #[test]
    fn gear_lever_path_rule() {
        let home = Path::new("/home/u");
        assert!(gear_lever_managed(
            Path::new("/home/u/AppImages/axidps-0.2.0.AppImage"),
            home
        ));
        assert!(!gear_lever_managed(
            Path::new("/home/u/Downloads/x.AppImage"),
            home
        ));
    }

    #[test]
    fn status_json_shape() {
        let s = Status {
            current: "0.1.9".into(),
            latest: Some("0.2.0".into()),
            state: State::WaitingForGame,
        };
        assert_eq!(
            serde_json::to_string(&s).unwrap(),
            r#"{"current":"0.1.9","latest":"0.2.0","state":"waiting_for_game"}"#
        );
    }

    #[test]
    fn status_file_is_written_atomically() {
        let tmp = tempfile::tempdir().unwrap();
        let s = Status {
            current: "1.0.0".into(),
            latest: None,
            state: State::UpToDate,
        };
        write_status(tmp.path(), &s).unwrap();
        let text = fs::read_to_string(tmp.path().join("update-status.json")).unwrap();
        assert_eq!(
            text,
            r#"{"current":"1.0.0","latest":null,"state":"up_to_date"}"#
        );
        assert_eq!(fs::read_dir(tmp.path()).unwrap().count(), 1);
    }

    fn plan(dir: &Path, auto: bool, managed: bool) -> Plan {
        Plan {
            os: Os::Linux,
            can_apply: true,
            current: "0.1.9".into(),
            auto,
            managed,
            have: None,
            dir: dir.into(),
        }
    }

    fn never(_: &str, _: &mut dyn Write) -> Result<(), UpdateError> {
        panic!("must not download");
    }

    #[test]
    fn cycle_never_downloads_when_off_or_managed_or_unsummed() {
        let tmp = tempfile::tempdir().unwrap();
        let with_sums = format!(
            "[{}]",
            rel("v0.2.0", false, &["axidps.AppImage", "SHA256SUMS"])
        );
        let no_sums = format!("[{}]", rel("v0.2.0", false, &["axidps.AppImage"]));
        let f1 = |_: &str| Ok(with_sums.clone().into_bytes());
        let f2 = |_: &str| Ok(no_sums.clone().into_bytes());
        let state = |m: Msg| match m {
            Msg::Status(s) => s.state,
            _ => panic!("expected a status"),
        };
        assert_eq!(
            state(cycle(&plan(tmp.path(), false, false), &f1, &never)),
            State::Available
        );
        assert_eq!(
            state(cycle(&plan(tmp.path(), true, true), &f1, &never)),
            State::ManagedByAxiom
        );
        assert_eq!(
            state(cycle(&plan(tmp.path(), true, false), &f2, &never)),
            State::VerifyFailed
        );
    }

    #[test]
    fn cycle_downloads_verified_file() {
        let tmp = tempfile::tempdir().unwrap();
        let sums = format!("{}  axidps.AppImage\n", sha_hex(b"bits"));
        let list = format!(
            "[{}]",
            rel("v0.2.0", false, &["axidps.AppImage", "SHA256SUMS"])
        );
        let fetch = |url: &str| {
            Ok(if url.ends_with("SHA256SUMS") {
                sums.clone().into_bytes()
            } else {
                list.clone().into_bytes()
            })
        };
        let dl = |_: &str, w: &mut dyn Write| w.write_all(b"bits").map_err(UpdateError::Io);
        let mut p = plan(tmp.path(), true, false);
        let Msg::Ready(s, ready) = cycle(&p, &fetch, &dl) else {
            panic!("expected Ready")
        };
        assert_eq!((s.state, ready.tag.as_str()), (State::Available, "v0.2.0"));
        assert_eq!(fs::read(&ready.file).unwrap(), b"bits");
        assert_eq!(ready.sha, sha_hex(b"bits"));
        p.have = Some("v0.2.0".into());
        assert!(matches!(cycle(&p, &fetch, &never), Msg::Idle));
    }

    #[test]
    fn cycle_skips_download_without_appimage() {
        let tmp = tempfile::tempdir().unwrap();
        let list = format!(
            "[{}]",
            rel("v0.2.0", false, &["axidps.AppImage", "SHA256SUMS"])
        );
        let fetch = |_: &str| Ok(list.clone().into_bytes());
        let mut p = plan(tmp.path(), true, false);
        p.can_apply = false;
        let Msg::Status(s) = cycle(&p, &fetch, &never) else {
            panic!("expected a status")
        };
        assert_eq!(s.state, State::Available);
    }

    fn status(state: State) -> Status {
        Status {
            current: "0.1.9".into(),
            latest: Some("0.2.0".into()),
            state,
        }
    }

    #[test]
    fn first_result_is_written_even_if_up_to_date() {
        let tmp = tempfile::tempdir().unwrap();
        let mut u = Updater::with(Some(tmp.path().into()), None, Instant::now());
        let s = Status {
            current: "0.2.0".into(),
            latest: None,
            state: State::UpToDate,
        };
        u.set_status(s.clone());
        let text = fs::read_to_string(tmp.path().join(STATUS_FILE)).unwrap();
        assert!(text.contains(r#""state":"up_to_date""#));
        // A stale file from before the restart is replaced too.
        fs::write(tmp.path().join(STATUS_FILE), "old").unwrap();
        let mut u = Updater::with(Some(tmp.path().into()), None, Instant::now());
        u.set_status(s);
        assert!(
            fs::read_to_string(tmp.path().join(STATUS_FILE))
                .unwrap()
                .contains("up_to_date")
        );
    }

    #[test]
    fn request_check_starts_a_check_now_unless_one_is_running() {
        let t0 = Instant::now();
        let mut sup = Supervisor::new(Vec::new());
        // No config folder: a "check" starts nothing, but still restarts the 6 h timer.
        let mut u = Updater::with(None, None, t0);
        u.tick(t0, &mut sup);
        assert_eq!(u.next_check, t0 + CHECK_EVERY);
        let t1 = t0 + Duration::from_secs(1);
        u.tick(t1, &mut sup);
        assert_eq!(u.next_check, t0 + CHECK_EVERY);
        u.request_check();
        u.tick(t1, &mut sup);
        assert_eq!(u.next_check, t1 + CHECK_EVERY);
        // A request while a check runs is dropped, not queued.
        u.in_flight = true;
        u.request_check();
        u.in_flight = false;
        let t2 = t1 + Duration::from_secs(1);
        u.tick(t2, &mut sup);
        assert_eq!(u.next_check, t1 + CHECK_EVERY);
    }

    fn ready_updater(tmp: &Path, content: &[u8]) -> Updater {
        let file = tmp.join("axidps.AppImage.new");
        fs::write(&file, content).unwrap();
        let mut u = Updater::with(
            Some(tmp.into()),
            Some(tmp.join("axidps.AppImage")),
            Instant::now(),
        );
        u.ready = Some(Ready {
            file,
            tag: "v0.2.0".into(),
            sha: sha_hex(b"genuine"),
        });
        u.set_status(status(State::Available));
        u
    }

    #[test]
    fn tampered_ready_file_is_not_applied() {
        let tmp = tempfile::tempdir().unwrap();
        let mut u = ready_updater(tmp.path(), b"tampered");
        let mut sup = Supervisor::new(vec![]);
        let apply = |_: Os, _: &Path, _: Option<&Path>| -> io::Result<()> {
            panic!("a tampered file must not be applied")
        };
        u.try_apply_with(Instant::now(), &mut sup, Os::Linux, false, &apply);
        assert!(u.ready.is_none());
        assert!(!tmp.path().join("axidps.AppImage.new").exists());
        assert_eq!(u.status.unwrap().state, State::VerifyFailed);
    }

    #[test]
    fn intact_ready_file_is_applied_and_waits_for_the_game() {
        let tmp = tempfile::tempdir().unwrap();
        let mut u = ready_updater(tmp.path(), b"genuine");
        let mut sup = Supervisor::new(vec![]);
        let never_apply =
            |_: Os, _: &Path, _: Option<&Path>| -> io::Result<()> { panic!("game runs") };
        u.try_apply_with(Instant::now(), &mut sup, Os::Linux, true, &never_apply);
        assert_eq!(u.status.as_ref().unwrap().state, State::WaitingForGame);
        let seen = std::cell::Cell::new(false);
        let fail = |_: Os, f: &Path, _: Option<&Path>| -> io::Result<()> {
            seen.set(f.exists());
            Err(io::Error::other("no"))
        };
        u.try_apply_with(Instant::now(), &mut sup, Os::Linux, false, &fail);
        assert!(seen.get());
        assert_eq!(u.status.unwrap().state, State::Available);
    }

    #[test]
    fn pending_update_is_applied_at_once_when_the_game_has_closed() {
        let tmp = tempfile::tempdir().unwrap();
        let mut u = ready_updater(tmp.path(), b"genuine");
        let mut sup = Supervisor::new(vec![]);
        let now = Instant::now();
        // Waiting for the game: the next try would only be after WAIT_RECHECK.
        u.try_apply_with(
            now,
            &mut sup,
            Os::Linux,
            true,
            &|_: Os, _: &Path, _: Option<&Path>| panic!("game runs"),
        );
        let applied = std::cell::Cell::new(false);
        let apply = |_: Os, _: &Path, _: Option<&Path>| -> io::Result<()> {
            applied.set(true);
            Err(io::Error::other("test: no exit"))
        };
        u.apply_pending_with(now, &mut sup, Os::Linux, &apply);
        assert!(applied.get());
        // Nothing waiting: nothing applied.
        let mut idle = Updater::with(Some(tmp.path().into()), None, now);
        idle.apply_pending_with(
            now,
            &mut sup,
            Os::Linux,
            &|_: Os, _: &Path, _: Option<&Path>| panic!("nothing to apply"),
        );
    }

    #[test]
    fn appimage_target_follows_the_version_name() {
        let dir = Path::new("/apps");
        assert_eq!(
            appimage_target(
                &dir.join("axidps-0.2.0.AppImage"),
                "axidps-0.3.0-rc.1.AppImage"
            ),
            dir.join("axidps-0.3.0-rc.1.AppImage")
        );
        // A name the user chose is kept.
        assert_eq!(
            appimage_target(&dir.join("axidps.AppImage"), "axidps-0.3.0.AppImage"),
            dir.join("axidps.AppImage")
        );
        // An asset name that is not a plain versioned file name is never used as a path.
        assert_eq!(
            appimage_target(
                &dir.join("axidps-0.2.0.AppImage"),
                "../axidps-0.3.0.AppImage"
            ),
            dir.join("axidps-0.2.0.AppImage")
        );
    }

    #[cfg(unix)]
    #[test]
    fn linux_update_starts_the_renamed_appimage_and_removes_the_old() {
        let tmp = tempfile::tempdir().unwrap();
        let seen = tmp.path().join("args");
        let old = tmp.path().join("axidps-0.2.0.AppImage");
        fs::write(&old, b"old").unwrap();
        // The "new AppImage" records how it was started.
        let new = tmp.path().join("axidps-0.3.0.AppImage.new");
        fs::write(
            &new,
            format!("#!/bin/sh\necho \"$0 $*\" > {}\n", seen.display()),
        )
        .unwrap();
        replace_appimage(&new, &old, &["--x".into()]).unwrap();
        let target = tmp.path().join("axidps-0.3.0.AppImage");
        assert!(target.exists());
        assert!(!old.exists());
        assert!(!new.exists());
        let deadline = Instant::now() + Duration::from_secs(5);
        while !fs::read_to_string(&seen).is_ok_and(|t| t.ends_with('\n')) {
            assert!(Instant::now() < deadline, "the new AppImage never ran");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(
            fs::read_to_string(&seen).unwrap().trim(),
            format!("{} --x --after-update", target.display())
        );
    }

    /// A descriptor inherited without close-on-exec, as the AppImage runtime's keepalive pipe.
    #[cfg(unix)]
    fn inheritable_fd() -> i32 {
        let mut fds = [0; 2];
        // SAFETY: pipe(2) fills two descriptors; plain pipe() sets no close-on-exec.
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        unsafe { libc::close(fds[1]) };
        fds[0]
    }

    #[cfg(unix)]
    #[test]
    fn inherited_descriptors_are_marked_close_on_exec() {
        for mark in [cloexec_from as fn(i32), cloexec_from_proc] {
            let fd = inheritable_fd();
            mark(fd);
            // SAFETY: fcntl/close on the descriptor this test owns.
            unsafe {
                assert_ne!(libc::fcntl(fd, libc::F_GETFD) & libc::FD_CLOEXEC, 0);
                libc::close(fd);
            }
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_new_appimage_does_not_inherit_the_old_runtime_pipe() {
        let tmp = tempfile::tempdir().unwrap();
        let seen = tmp.path().join("fds");
        let old = tmp.path().join("axidps-0.2.0.AppImage");
        fs::write(&old, b"old").unwrap();
        let fd = inheritable_fd();
        // Compare what the number points at, not whether it exists: dash keeps the script it
        // reads on fd 10 or above, so the same number can be open in the child for its own use.
        let pipe = fs::read_link(format!("/proc/self/fd/{fd}")).unwrap();
        let new = tmp.path().join("axidps-0.3.0.AppImage.new");
        fs::write(
            &new,
            format!(
                "#!/bin/sh\n[ \"$(readlink /proc/$$/fd/{fd})\" = '{1}' ] && echo has > {0} || echo none > {0}\n",
                seen.display(),
                pipe.display()
            ),
        )
        .unwrap();
        replace_appimage(&new, &old, &[]).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !fs::read_to_string(&seen).is_ok_and(|t| t.ends_with('\n')) {
            assert!(Instant::now() < deadline, "the new AppImage never ran");
            std::thread::sleep(Duration::from_millis(20));
        }
        // SAFETY: closing the descriptor this test owns.
        unsafe { libc::close(fd) };
        assert_eq!(fs::read_to_string(&seen).unwrap().trim(), "none");
    }

    #[cfg(unix)]
    #[test]
    fn linux_update_that_cannot_start_keeps_the_old_appimage() {
        let tmp = tempfile::tempdir().unwrap();
        let old = tmp.path().join("axidps-0.2.0.AppImage");
        fs::write(&old, b"old").unwrap();
        // Not a runnable file: the start fails.
        let new = tmp.path().join("axidps-0.3.0.AppImage.new");
        fs::write(&new, b"\x7fELF broken").unwrap();
        assert!(replace_appimage(&new, &old, &[]).is_err());
        assert_eq!(fs::read(&old).unwrap(), b"old");
        assert!(!tmp.path().join("axidps-0.3.0.AppImage").exists());
    }

    #[test]
    fn startup_cleanup_removes_leftovers() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = tmp.path().join("cfg");
        fs::create_dir_all(cfg.join("updates")).unwrap();
        fs::write(cfg.join("updates/axidps-Setup.exe"), b"x").unwrap();
        let app = tmp.path().join("axidps.AppImage");
        for n in [
            "axidps.AppImage",
            "axidps-0.2.0.AppImage.new",
            "axidps.AppImage.axidps-part",
            // A browser's download in progress, not ours.
            "axidps-0.4.0.AppImage.part",
            "other.new",
        ] {
            fs::write(tmp.path().join(n), b"x").unwrap();
        }
        // An older version's own partial download: long untouched, so not a browser's.
        let legacy = tmp.path().join("axidps-0.3.0.AppImage.part");
        let two_days = std::time::Duration::from_secs(2 * 24 * 3600);
        fs::File::create(&legacy)
            .unwrap()
            .set_modified(std::time::SystemTime::now() - two_days)
            .unwrap();
        cleanup_stale(Some(&cfg), Some(&app));
        assert!(!cfg.join("updates").exists());
        let mut left: Vec<_> = fs::read_dir(tmp.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        assert_eq!(
            left,
            [
                "axidps-0.4.0.AppImage.part",
                "axidps.AppImage",
                "cfg",
                "other.new"
            ]
        );
    }
}
