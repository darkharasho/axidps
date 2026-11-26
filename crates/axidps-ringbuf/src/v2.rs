// crates/axidps-ringbuf/src/v2.rs
//! Multi-producer ring (protocol v2).
//!
//! Many threads push concurrently without locks; one reader polls. Each slot is a seqlock.
//! All slot fields are accessed with Relaxed atomics, ordered by the seq store/fences, so the
//! protocol is data-race free under Rust's memory model:
//!
//! ```text
//! push:  k = reserve.fetch_add(1, AcqRel); s = slot(k % N)
//!        s.seq.store(0, Relaxed); fence(Release)
//!        write time/kind/payload (Relaxed atomics)
//!        s.seq.store(k + 1, Release)
//! read:  e = next + 1; v = s.seq.load(Acquire)
//!        v == e -> copy, fence(Acquire), re-load seq and reserve; deliver only if seq is
//!                  unchanged and the slot has not been claimed again (reserve <= next + N);
//!                  otherwise the copy may be a mix of two producers and counts as dropped
//!        v >  e -> lapped: skip ahead to reserve - N, counting the gap as dropped
//!        v <  e -> not committed yet: stop this read; skip it if stuck for STUCK_TIMEOUT_MS
//! ```
//!
//! Opening a ring replays the retained window, but only for state: the claim index at open
//! (`open_reserve`) splits history from live events. While [`EventReader::catching_up`] is
//! true, a `read` delivers only claims below it; `ReadStats::history` tells the phases apart.

use axidps_protocol::v2::{
    ring_path, Event, RingHost, RingHeader, Slot, FLAG_ACTIVE, HEADER_SIZE, MAGIC, MAX_GROUPS, PAYLOAD_SIZE,
    PROTOCOL_VERSION, SHM_SIZE, SLOT_COUNT, SLOT_SIZE,
};
use memmap2::{Mmap, MmapOptions, MmapRaw};
use std::fs::{self, File, OpenOptions};
use std::io;
use std::mem::{align_of, offset_of};
use std::path::{Path, PathBuf};
use std::sync::atomic::{fence, AtomicU16, AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// How long the reader waits on an uncommitted slot before skipping it. Generous: a producer
/// thread preempted on a loaded machine must not lose its event.
pub const STUCK_TIMEOUT_MS: u64 = 1000;

const OFF_MAGIC: usize = offset_of!(RingHeader, magic);
const OFF_PROTOCOL: usize = offset_of!(RingHeader, protocol);
const OFF_RESERVE: usize = offset_of!(RingHeader, reserve);
const OFF_DLL_VERSION: usize = offset_of!(RingHeader, dll_version);
const OFF_FLAGS: usize = offset_of!(RingHeader, flags);
const OFF_GROUP_STATE: usize = offset_of!(RingHeader, group_state);
const SLOT_SEQ: usize = offset_of!(Slot, seq);
const SLOT_TIME: usize = offset_of!(Slot, time_ms);
const SLOT_KIND: usize = offset_of!(Slot, kind);
const SLOT_PAYLOAD: usize = offset_of!(Slot, payload);
const MASK: u64 = SLOT_COUNT as u64 - 1;
const WORDS: usize = PAYLOAD_SIZE / 8;

// Every atomic access below must be naturally aligned. The mapping is page-aligned and slots are
// SLOT_SIZE apart, so offsets within header/slot being multiples of the atomic's alignment is
// sufficient.
const fn aligned(off: usize, align: usize) -> bool {
    off.is_multiple_of(align)
}

const _: () = {
    assert!(aligned(HEADER_SIZE, align_of::<AtomicU64>()) && aligned(SLOT_SIZE, align_of::<AtomicU64>()));
    assert!(aligned(OFF_MAGIC, align_of::<AtomicU32>()));
    assert!(aligned(OFF_PROTOCOL, align_of::<AtomicU32>()));
    assert!(aligned(OFF_RESERVE, align_of::<AtomicU64>()));
    assert!(aligned(OFF_DLL_VERSION, align_of::<AtomicU32>()));
    assert!(aligned(OFF_FLAGS, align_of::<AtomicU32>()));
    assert!(aligned(OFF_GROUP_STATE, align_of::<AtomicU64>()) && MAX_GROUPS == 16);
    assert!(aligned(SLOT_SEQ, align_of::<AtomicU64>()));
    assert!(aligned(SLOT_TIME, align_of::<AtomicU64>()));
    assert!(aligned(SLOT_KIND, align_of::<AtomicU16>()));
    assert!(aligned(SLOT_PAYLOAD, align_of::<AtomicU64>()) && aligned(PAYLOAD_SIZE, 8));
    assert!(SLOT_COUNT.is_power_of_two());
};

/// Pointer to the slot for `claim` (masked) from the mapping base.
#[inline]
fn slot_ptr(base: *const u8, claim: u64) -> *const u8 {
    let idx = (claim & MASK) as usize;
    // SAFETY: idx < SLOT_COUNT and the mapping is SHM_SIZE bytes, so the offset stays in bounds.
    unsafe { base.add(HEADER_SIZE + idx * SLOT_SIZE) }
}

/// SAFETY (caller): `base + off` must be 8-aligned and inside the live mapping.
#[inline]
unsafe fn atomic_u64<'a>(base: *const u8, off: usize) -> &'a AtomicU64 {
    unsafe { &*(base.add(off) as *const AtomicU64) }
}

/// SAFETY (caller): `base + off` must be 4-aligned and inside the live mapping.
#[inline]
unsafe fn atomic_u32<'a>(base: *const u8, off: usize) -> &'a AtomicU32 {
    unsafe { &*(base.add(off) as *const AtomicU32) }
}

/// SAFETY (caller): `base + off` must be 2-aligned and inside the live mapping.
#[inline]
unsafe fn atomic_u16<'a>(base: *const u8, off: usize) -> &'a AtomicU16 {
    unsafe { &*(base.add(off) as *const AtomicU16) }
}

/// Identity of a file, to tell whether a path still names the same file as an open handle.
/// Unix: `(dev, ino)`. Windows: `(volume serial, file index)`. Not the creation time: NTFS
/// "tunnels" it, so a file made within 15 s of deleting one by the same name, which is what
/// `create_at` does, gets its predecessor's creation time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileId(u64, u64);

impl FileId {
    #[cfg(unix)]
    fn of(file: &File) -> io::Result<Self> {
        use std::os::unix::fs::MetadataExt;
        let m = file.metadata()?;
        Ok(FileId(m.dev(), m.ino()))
    }

    #[cfg(windows)]
    fn of(file: &File) -> io::Result<Self> {
        use std::os::windows::io::AsRawHandle;
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
        };
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        // SAFETY: the handle is the open `file`'s, valid for the call; `info` is a local.
        unsafe { GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut info) }?;
        Ok(FileId(
            u64::from(info.dwVolumeSerialNumber),
            (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
        ))
    }
}

/// True if `path` currently names the file with identity `id`.
fn path_has_id(path: &Path, id: FileId) -> bool {
    File::open(path)
        .and_then(|f| FileId::of(&f))
        .is_ok_and(|now| now == id)
}

/// Producer side. `Sync`: share a reference between threads and call [`push`](Self::push).
pub struct EventWriter {
    mmap: MmapRaw,
    _file: File,
    path: PathBuf,
    id: FileId,
}

impl EventWriter {
    /// Create the ring at the default platform path ([`ring_path`]).
    pub fn create() -> io::Result<Self> {
        Self::create_at(ring_path(RingHost::current_build()))
    }

    /// Create the ring file at `path` and mark the producer active. Any existing file is
    /// removed first, so every run gets a fresh inode (a reader still mapped to the old file
    /// is never truncated under).
    pub fn create_at(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref().to_path_buf();
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        let file = OpenOptions::new().read(true).write(true).create_new(true).open(&path)?;
        file.set_len(SHM_SIZE as u64)?; // a freshly extended file is zero-filled
        let mmap = MmapOptions::new().len(SHM_SIZE).map_raw(&file)?;
        let id = FileId::of(&file)?;
        let w = Self { mmap, _file: file, path, id };
        let base = w.base();
        // SAFETY: the mapping is SHM_SIZE bytes and page-aligned; header offsets are in bounds
        // and aligned (const-asserted above). Every header field is written before magic, which
        // is stored last with Release so a reader that sees the magic sees a complete header.
        unsafe {
            atomic_u32(base, OFF_PROTOCOL).store(PROTOCOL_VERSION, Ordering::Relaxed);
            atomic_u32(base, OFF_FLAGS).store(FLAG_ACTIVE, Ordering::Relaxed);
            atomic_u32(base, OFF_MAGIC).store(MAGIC, Ordering::Release);
        }
        Ok(w)
    }

    #[inline]
    fn base(&self) -> *mut u8 {
        self.mmap.as_mut_ptr()
    }

    #[cfg(test)]
    fn reserve(&self) -> &AtomicU64 {
        // SAFETY: OFF_RESERVE is 8-aligned and inside the mapping, which lives as long as `self`.
        unsafe { atomic_u64(self.base(), OFF_RESERVE) }
    }

    /// Push an event. Lock-free and never blocks; if the reader is too slow the oldest
    /// events are overwritten.
    pub fn push(&self, time_ms: u64, ev: &Event) {
        let (kind, payload) = ev.encode();
        self.push_raw(time_ms, kind, &payload);
    }

    fn push_raw(&self, time_ms: u64, kind: u16, payload: &[u8; PAYLOAD_SIZE]) {
        let base = self.base();
        // SAFETY: all addresses below are inside the mapping (see `slot_ptr`) and aligned
        // (const-asserted). Two producers can touch one slot only after a full lap; every
        // access is an atomic, so that is not a data race, and readers discard any copy that
        // may mix producers (see `EventReader::copy_slot`).
        unsafe {
            let k = atomic_u64(base, OFF_RESERVE).fetch_add(1, Ordering::AcqRel);
            let s = slot_ptr(base, k);
            let seq = atomic_u64(s, SLOT_SEQ);
            seq.store(0, Ordering::Relaxed);
            fence(Ordering::Release);
            atomic_u64(s, SLOT_TIME).store(time_ms, Ordering::Relaxed);
            atomic_u16(s, SLOT_KIND).store(kind, Ordering::Relaxed);
            for (i, chunk) in payload.chunks_exact(8).enumerate() {
                let mut b = [0u8; 8];
                b.copy_from_slice(chunk);
                atomic_u64(s, SLOT_PAYLOAD + i * 8).store(u64::from_le_bytes(b), Ordering::Relaxed);
            }
            seq.store(k.wrapping_add(1), Ordering::Release);
        }
    }

    /// Set the DLL version in the header (`major << 16 | minor << 8 | patch`).
    pub fn set_dll_version(&self, version: u32) {
        // SAFETY: OFF_DLL_VERSION is 4-aligned and inside the mapping.
        unsafe { atomic_u32(self.base(), OFF_DLL_VERSION) }.store(version, Ordering::Release);
    }

    /// Clear the producer-active flag. One atomic op: no unlink, no IO (safe under the loader
    /// lock). `Drop` does the unlink and flush.
    pub fn mark_inactive(&self) {
        // SAFETY: OFF_FLAGS is 4-aligned and inside the mapping.
        unsafe { atomic_u32(self.base(), OFF_FLAGS) }.fetch_and(!FLAG_ACTIVE, Ordering::Release);
    }

    /// Publish per-group state bytes for the app to display.
    pub fn set_group_state(&self, state: [u8; MAX_GROUPS]) {
        let (lo, hi) = split16(state);
        // SAFETY: OFF_GROUP_STATE is 8-aligned and 16 bytes fit in the header.
        unsafe {
            atomic_u64(self.base(), OFF_GROUP_STATE).store(lo, Ordering::Release);
            atomic_u64(self.base(), OFF_GROUP_STATE + 8).store(hi, Ordering::Release);
        }
    }
}

impl Drop for EventWriter {
    fn drop(&mut self) {
        // SAFETY: OFF_FLAGS is 4-aligned and inside the mapping.
        unsafe { atomic_u32(self.base(), OFF_FLAGS) }.fetch_and(!FLAG_ACTIVE, Ordering::Release);
        let _ = self.mmap.flush();
        // Readers keep their mapping (and the cleared flag) alive after the unlink. Only
        // unlink our own file: a newer writer may have replaced it at this path.
        if path_has_id(&self.path, self.id) {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn split16(b: [u8; 16]) -> (u64, u64) {
    let mut lo = [0u8; 8];
    let mut hi = [0u8; 8];
    lo.copy_from_slice(&b[..8]);
    hi.copy_from_slice(&b[8..]);
    (u64::from_le_bytes(lo), u64::from_le_bytes(hi))
}

/// What a [`EventReader::read`] call skipped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ReadStats {
    /// Events lost: overwritten before being read, torn during the copy, or abandoned
    /// by a stuck producer.
    pub dropped: u64,
    /// Committed slots whose kind or payload did not decode.
    pub unknown: u64,
    /// The part of `dropped` at or after the open boundary: lost events that were pushed after
    /// this reader attached, so they belong to live play even when the batch is history.
    pub dropped_live: u64,
    /// The batch delivered only history (events pushed before this reader attached). Decided
    /// inside `read`, after it detects a producer restart.
    pub history: bool,
}

/// Consumer side. Single reader.
pub struct EventReader {
    mmap: Mmap,
    _file: File,
    /// Next claim index to read.
    next: u64,
    /// `reserve` when the ring was opened: claims below it were pushed before this reader
    /// attached (history, delivered for state only).
    open_reserve: u64,
    /// Claim index we are waiting on and since when.
    stuck: Option<(u64, Instant)>,
    stuck_timeout: Duration,
    path: PathBuf,
    id: FileId,
}

type RawSlot = (u64, u16, [u8; PAYLOAD_SIZE]);

impl EventReader {
    /// Open the ring at the default platform path ([`ring_path`]).
    pub fn open() -> io::Result<Self> {
        Self::open_at(ring_path(RingHost::current_build()))
    }

    /// Open an existing ring file. Anything that is not a complete v2 ring is
    /// `io::ErrorKind::InvalidData`.
    ///
    /// The reader starts at the oldest event the ring still retains
    /// (`reserve - SLOT_COUNT`), so events pushed before the app attached (agents, map) are
    /// delivered, and the first read reports no phantom drops. Those events are history: see
    /// [`catching_up`](Self::catching_up).
    pub fn open_at(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref().to_path_buf();
        let file = File::open(&path)?;
        let bad = |m: &str| io::Error::new(io::ErrorKind::InvalidData, m.to_string());
        let meta = file.metadata()?;
        let len = meta.len();
        let id = FileId::of(&file)?;
        if len < HEADER_SIZE as u64 {
            return Err(bad("ring file shorter than its header"));
        }
        // SAFETY: the file is shared with a producer that may write concurrently; every access
        // goes through atomics, never through references to the bytes.
        let mmap = unsafe { MmapOptions::new().len(HEADER_SIZE).map(&file)? };
        let base = mmap.as_ptr();
        // SAFETY: the mapping is HEADER_SIZE bytes and page-aligned; offsets are aligned.
        let (magic, protocol) = unsafe {
            let magic = atomic_u32(base, OFF_MAGIC).load(Ordering::Acquire);
            (magic, atomic_u32(base, OFF_PROTOCOL).load(Ordering::Relaxed))
        };
        if magic != MAGIC {
            return Err(bad("not an axidps v2 ring (bad magic)"));
        }
        if protocol != PROTOCOL_VERSION {
            return Err(bad("unsupported ring protocol version"));
        }
        if len < SHM_SIZE as u64 {
            return Err(bad("ring file shorter than the v2 layout"));
        }
        drop(mmap);
        // SAFETY: as above; the file is at least SHM_SIZE bytes.
        let mmap = unsafe { MmapOptions::new().len(SHM_SIZE).map(&file)? };
        let mut r = Self {
            mmap,
            _file: file,
            next: 0,
            open_reserve: 0,
            stuck: None,
            stuck_timeout: Duration::from_millis(STUCK_TIMEOUT_MS),
            path,
            id,
        };
        r.open_reserve = r.reserve();
        r.next = r.open_reserve.saturating_sub(SLOT_COUNT as u64);
        Ok(r)
    }

    /// Override the stuck-slot timeout (tests).
    #[cfg(test)]
    fn set_stuck_timeout(&mut self, d: Duration) {
        self.stuck_timeout = d;
    }

    #[inline]
    fn base(&self) -> *const u8 {
        self.mmap.as_ptr()
    }

    /// True while the reader is still replaying events pushed before it opened the ring.
    /// While it is true, `read` delivers only those historic events (never one pushed after the
    /// open); [`ReadStats::history`] reports which phase a batch was.
    pub fn catching_up(&self) -> bool {
        self.next < self.open_reserve
    }

    /// True if claims exist past the last one read (committed or not): the ring is not
    /// drained yet.
    pub fn has_pending(&self) -> bool {
        self.next < self.reserve()
    }

    #[inline]
    fn reserve(&self) -> u64 {
        // SAFETY: OFF_RESERVE is 8-aligned and inside the mapping.
        unsafe { atomic_u64(self.base(), OFF_RESERVE) }.load(Ordering::Acquire)
    }

    /// True when the file at this reader's path is missing or is no longer the file this
    /// reader has open (a new producer run replaced it). After a game crash the old file keeps
    /// its active flag and never gets new events, so the app should call this when idle and
    /// reopen with [`EventReader::open_at`] when it returns true.
    pub fn is_replaced(&self) -> bool {
        !path_has_id(&self.path, self.id)
    }

    pub fn is_producer_active(&self) -> bool {
        // SAFETY: OFF_FLAGS is 4-aligned and inside the mapping.
        unsafe { atomic_u32(self.base(), OFF_FLAGS) }.load(Ordering::Acquire) & FLAG_ACTIVE != 0
    }

    pub fn dll_version(&self) -> u32 {
        // SAFETY: OFF_DLL_VERSION is 4-aligned and inside the mapping.
        unsafe { atomic_u32(self.base(), OFF_DLL_VERSION) }.load(Ordering::Acquire)
    }

    pub fn group_state(&self) -> [u8; MAX_GROUPS] {
        // SAFETY: OFF_GROUP_STATE is 8-aligned and 16 bytes fit in the header.
        let (lo, hi) = unsafe {
            (
                atomic_u64(self.base(), OFF_GROUP_STATE).load(Ordering::Acquire),
                atomic_u64(self.base(), OFF_GROUP_STATE + 8).load(Ordering::Acquire),
            )
        };
        let mut out = [0u8; MAX_GROUPS];
        out[..8].copy_from_slice(&lo.to_le_bytes());
        out[8..].copy_from_slice(&hi.to_le_bytes());
        out
    }

    /// Copy the slot for claim `claim` if it is committed as `claim + 1` and provably not
    /// overwritten. `None` means the copy must not be delivered: it may be torn, or a mix of
    /// two producers that raced on this slot a full lap apart.
    fn copy_slot(&self, claim: u64) -> Option<RawSlot> {
        let e = claim.wrapping_add(1);
        let s = slot_ptr(self.base(), claim);
        // SAFETY: `s` is a slot inside the mapping; seq/time/kind/payload offsets are aligned
        // (const-asserted) and all accesses are atomic.
        let (copy, seq_after) = unsafe {
            let seq = atomic_u64(s, SLOT_SEQ);
            if seq.load(Ordering::Acquire) != e {
                return None;
            }
            let time_ms = atomic_u64(s, SLOT_TIME).load(Ordering::Relaxed);
            let kind = atomic_u16(s, SLOT_KIND).load(Ordering::Relaxed);
            let mut payload = [0u8; PAYLOAD_SIZE];
            for i in 0..WORDS {
                let w = atomic_u64(s, SLOT_PAYLOAD + i * 8).load(Ordering::Relaxed);
                payload[i * 8..i * 8 + 8].copy_from_slice(&w.to_le_bytes());
            }
            fence(Ordering::Acquire);
            ((time_ms, kind, payload), seq.load(Ordering::Relaxed))
        };
        if seq_after != e {
            return None;
        }
        // A producer that claimed `claim + SLOT_COUNT` may have zeroed and rewritten this slot
        // while a stale producer for `claim` finished late, leaving a mix under seq == e.
        if self.reserve() > claim.saturating_add(SLOT_COUNT as u64) {
            return None;
        }
        Some(copy)
    }

    /// Count claims `from..to` as dropped, and the part at or after the open boundary as live.
    fn count_dropped(&self, stats: &mut ReadStats, from: u64, to: u64) {
        stats.dropped = stats.dropped.saturating_add(to.saturating_sub(from));
        stats.dropped_live = stats
            .dropped_live
            .saturating_add(to.saturating_sub(from.max(self.open_reserve)));
    }

    /// Append every event committed since the last call to `out`, in claim order.
    pub fn read(&mut self, out: &mut Vec<(u64, Event)>) -> ReadStats {
        let mut stats = ReadStats::default();
        let mut reserve = self.reserve();
        // The producer restarted on this file (reserve went backwards): start over. The new
        // run's events were never seen, so none of them is history.
        if reserve < self.next {
            self.next = reserve.saturating_sub(SLOT_COUNT as u64);
            self.open_reserve = 0;
            self.stuck = None;
        }
        // While catching up, stop at the open boundary so one batch never mixes phases.
        let catch_up_end = self.catching_up().then_some(self.open_reserve);
        stats.history = catch_up_end.is_some();
        loop {
            // Lapped: the oldest events were overwritten; resume at the oldest survivor.
            if reserve.saturating_sub(self.next) > SLOT_COUNT as u64 {
                let target = reserve - SLOT_COUNT as u64;
                self.count_dropped(&mut stats, self.next, target);
                self.next = target;
                self.stuck = None;
            }
            if self.next >= reserve || catch_up_end.is_some_and(|end| self.next >= end) {
                break;
            }
            let e = self.next.wrapping_add(1);
            // SAFETY: slot inside the mapping, 8-aligned.
            let v = unsafe { atomic_u64(slot_ptr(self.base(), self.next), SLOT_SEQ) }
                .load(Ordering::Acquire);
            if v == e {
                let copy = self.copy_slot(self.next);
                self.next = self.next.wrapping_add(1);
                self.stuck = None;
                match copy {
                    Some((time_ms, kind, payload)) => match Event::decode(kind, &payload) {
                        Some(ev) => out.push((time_ms, ev)),
                        None => stats.unknown = stats.unknown.saturating_add(1),
                    },
                    None => self.count_dropped(&mut stats, self.next - 1, self.next),
                }
            } else if v > e {
                // Lapped while we were looking; jump to the oldest survivor.
                reserve = self.reserve();
                let target = reserve
                    .saturating_sub(SLOT_COUNT as u64)
                    .max(self.next.saturating_add(1));
                self.count_dropped(&mut stats, self.next, target);
                self.next = target;
                self.stuck = None;
            } else {
                // Claimed but not committed yet (or a stale slot from the previous lap).
                let now = Instant::now();
                let since = match self.stuck {
                    Some((pos, t)) if pos == self.next => t,
                    _ => {
                        self.stuck = Some((self.next, now));
                        now
                    }
                };
                reserve = self.reserve();
                if reserve > e && now.duration_since(since) >= self.stuck_timeout {
                    self.count_dropped(&mut stats, self.next, self.next.wrapping_add(1));
                    self.next = self.next.wrapping_add(1);
                    self.stuck = None;
                } else {
                    break;
                }
            }
        }
        stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axidps_protocol::v2::{SLOT_COUNT, SLOT_SIZE};
    use std::collections::HashSet;
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    struct TmpFile(PathBuf);
    impl TmpFile {
        fn new(name: &str) -> Self {
            let p = std::env::temp_dir().join(format!("axidps_v2_{}_{}", std::process::id(), name));
            let _ = std::fs::remove_file(&p);
            Self(p)
        }
    }
    impl Drop for TmpFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    fn pov(id: u32) -> Event {
        Event::Pov { id }
    }

    #[test]
    fn mark_inactive_clears_active_flag() {
        let f = TmpFile::new("inactive");
        let w = EventWriter::create_at(&f.0).unwrap();
        let r = EventReader::open_at(&f.0).unwrap();
        assert!(r.is_producer_active());
        w.mark_inactive();
        assert!(!r.is_producer_active());
    }

    #[test]
    fn single_writer_in_order() {
        let f = TmpFile::new("single");
        let w = EventWriter::create_at(&f.0).unwrap();
        let mut r = EventReader::open_at(&f.0).unwrap();
        for i in 0..100u32 {
            w.push(u64::from(i) * 10, &pov(i));
        }
        let mut out = Vec::new();
        let stats = r.read(&mut out);
        assert_eq!(stats, ReadStats::default());
        assert_eq!(out.len(), 100);
        for (i, (t, ev)) in out.iter().enumerate() {
            assert_eq!(*t, i as u64 * 10);
            assert_eq!(*ev, pov(i as u32));
        }
        out.clear();
        assert_eq!(r.read(&mut out), ReadStats::default());
        assert!(out.is_empty());
    }

    #[test]
    fn four_writers_no_loss() {
        const THREADS: u32 = 4;
        const PER: u32 = 10_000;
        let f = TmpFile::new("four");
        let w = EventWriter::create_at(&f.0).unwrap();
        let mut r = EventReader::open_at(&f.0).unwrap();
        r.set_stuck_timeout(Duration::from_secs(10)); // a preempted writer must not look stuck
        let mut got: Vec<(u64, Event)> = Vec::new();
        let mut dropped = 0;
        std::thread::scope(|s| {
            for t in 0..THREADS {
                let w = &w;
                s.spawn(move || {
                    for i in 0..PER {
                        w.push(0, &Event::Strike { src: t, dst: 0, skill: 0, value: i as i32, overstack: 0, result: 0 });
                    }
                });
            }
            let deadline = Instant::now() + Duration::from_secs(20);
            while got.len() < (THREADS * PER) as usize && Instant::now() < deadline {
                dropped += r.read(&mut got).dropped;
                std::thread::yield_now();
            }
        });
        assert_eq!(dropped, 0);
        assert_eq!(got.len(), (THREADS * PER) as usize);
        let mut last = [-1i64; THREADS as usize];
        let mut seen = HashSet::new();
        for (_, ev) in &got {
            let Event::Strike { src, value, .. } = *ev else { panic!("unexpected {ev:?}") };
            assert!(seen.insert((src, value)), "duplicate");
            assert!(i64::from(value) > last[src as usize], "out of order");
            last[src as usize] = i64::from(value);
        }
    }

    #[test]
    fn reader_reports_lap() {
        let f = TmpFile::new("lap");
        let w = EventWriter::create_at(&f.0).unwrap();
        let mut r = EventReader::open_at(&f.0).unwrap();
        let total = SLOT_COUNT as u32 + 10;
        for i in 0..total {
            w.push(0, &pov(i));
        }
        let mut out = Vec::new();
        let stats = r.read(&mut out);
        assert!(stats.dropped >= 10);
        assert_eq!(out.len() as u64 + stats.dropped, u64::from(total));
        let first = total - out.len() as u32;
        for (i, (_, ev)) in out.iter().enumerate() {
            assert_eq!(*ev, pov(first + i as u32));
        }
        assert_eq!(out.last().unwrap().1, pov(total - 1));
    }

    #[test]
    fn reader_rejects_v1_file() {
        // A v1 ring: 64-byte header (write index first, no magic) and 4096 64-byte slots.
        let f = TmpFile::new("v1");
        let mut bytes = vec![0u8; 64 + 4096 * 64];
        bytes[..8].copy_from_slice(&5u64.to_le_bytes());
        std::fs::write(&f.0, &bytes).unwrap();
        let err = EventReader::open_at(&f.0).err().unwrap();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn reader_rejects_bad_protocol_and_short_file() {
        let f = TmpFile::new("badproto");
        let mut bytes = vec![0u8; HEADER_SIZE];
        bytes[..4].copy_from_slice(&MAGIC.to_le_bytes());
        bytes[4..8].copy_from_slice(&3u32.to_le_bytes());
        std::fs::write(&f.0, &bytes).unwrap();
        assert_eq!(EventReader::open_at(&f.0).err().unwrap().kind(), io::ErrorKind::InvalidData);
        std::fs::write(&f.0, [1u8, 2]).unwrap();
        assert_eq!(EventReader::open_at(&f.0).err().unwrap().kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn group_state_visible_to_reader() {
        let f = TmpFile::new("groups");
        let w = EventWriter::create_at(&f.0).unwrap();
        let r = EventReader::open_at(&f.0).unwrap();
        assert!(r.is_producer_active());
        assert_eq!(r.group_state(), [0; 16]);
        let gs = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
        w.set_group_state(gs);
        w.set_dll_version(0x0001_0203); // public-check:ok
        assert_eq!(r.group_state(), gs);
        assert_eq!(r.dll_version(), 0x0001_0203); // public-check:ok
        drop(w);
        assert!(!r.is_producer_active());
    }

    #[test]
    fn stuck_slot_is_skipped_after_timeout() {
        let f = TmpFile::new("stuck");
        let w = EventWriter::create_at(&f.0).unwrap();
        let mut r = EventReader::open_at(&f.0).unwrap();
        r.set_stuck_timeout(Duration::from_millis(100));
        w.push(0, &pov(0));
        // Claim a slot and never commit it (a producer that died mid-write).
        w.reserve().fetch_add(1, Ordering::AcqRel);
        w.push(0, &pov(2));
        let mut out = Vec::new();
        let stats = r.read(&mut out);
        assert_eq!(stats.dropped, 0);
        assert_eq!(out.len(), 1);
        assert!(r.has_pending());
        std::thread::sleep(Duration::from_millis(130));
        let stats = r.read(&mut out);
        assert_eq!(stats.dropped, 1);
        assert_eq!(out.len(), 2);
        assert_eq!(out[1].1, pov(2));
        assert!(!r.has_pending());
    }

    #[test]
    fn undecodable_slot_counts_unknown() {
        let f = TmpFile::new("unknown");
        let w = EventWriter::create_at(&f.0).unwrap();
        let mut r = EventReader::open_at(&f.0).unwrap();
        w.push_raw(0, 999, &[0; PAYLOAD_SIZE]);
        w.push(0, &pov(1));
        let mut out = Vec::new();
        let stats = r.read(&mut out);
        assert_eq!(stats, ReadStats { unknown: 1, ..ReadStats::default() });
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn file_size_matches_layout() {
        let f = TmpFile::new("size");
        let _w = EventWriter::create_at(&f.0).unwrap();
        assert_eq!(std::fs::metadata(&f.0).unwrap().len(), (HEADER_SIZE + SLOT_SIZE * SLOT_COUNT) as u64);
    }

    #[test]
    fn mixed_slot_with_stale_reserve_is_dropped() {
        let f = TmpFile::new("mixed");
        let w = EventWriter::create_at(&f.0).unwrap();
        let r = EventReader::open_at(&f.0).unwrap();
        // Slot 0 holds a committed (seq == 1) event; a late producer for claim 0 could have
        // mixed its fields with those of claim SLOT_COUNT.
        w.push(5, &pov(7));
        assert_eq!(r.copy_slot(0).map(|c| c.0), Some(5));
        // Claim SLOT_COUNT has now been made: the copy for claim 0 must not be delivered.
        w.reserve().store(SLOT_COUNT as u64 + 1, Ordering::Release);
        assert_eq!(r.copy_slot(0), None);
        // Just inside the window it is still fine.
        w.reserve().store(SLOT_COUNT as u64, Ordering::Release);
        assert_eq!(r.copy_slot(0).map(|c| c.0), Some(5));
    }

    #[test]
    fn producer_restart_on_same_file_resets_reader() {
        let f = TmpFile::new("restart");
        let w = EventWriter::create_at(&f.0).unwrap();
        let mut r = EventReader::open_at(&f.0).unwrap();
        for i in 0..10 {
            w.push(0, &pov(i));
        }
        let mut out = Vec::new();
        r.read(&mut out);
        assert_eq!(out.len(), 10);
        // Simulate the producer restarting in place: reserve goes back to 0.
        w.reserve().store(0, Ordering::Release);
        for i in 100..103 {
            w.push(0, &pov(i));
        }
        out.clear();
        let stats = r.read(&mut out);
        assert_eq!(stats.dropped, 0);
        assert_eq!(out.iter().map(|x| x.1).collect::<Vec<_>>(), vec![pov(100), pov(101), pov(102)]);
    }

    #[cfg(unix)]
    #[test]
    fn create_at_makes_a_new_file_and_drop_removes_it() {
        use std::os::unix::fs::MetadataExt;
        let f = TmpFile::new("inode");
        let w1 = EventWriter::create_at(&f.0).unwrap();
        let ino1 = std::fs::metadata(&f.0).unwrap().ino();
        let w2 = EventWriter::create_at(&f.0).unwrap();
        assert_ne!(std::fs::metadata(&f.0).unwrap().ino(), ino1);
        drop(w2);
        assert!(!f.0.exists());
        drop(w1);
    }

    #[test]
    fn open_after_pushes_starts_at_retained_window() {
        let f = TmpFile::new("late_open");
        let w = EventWriter::create_at(&f.0).unwrap();
        let total = SLOT_COUNT as u32 + 10;
        for i in 0..total {
            w.push(0, &pov(i));
        }
        let mut r = EventReader::open_at(&f.0).unwrap();
        assert!(r.catching_up());
        let mut out = Vec::new();
        let stats = r.read(&mut out);
        assert_eq!(stats, ReadStats { history: true, ..ReadStats::default() });
        assert_eq!(out.len(), SLOT_COUNT);
        assert_eq!(out[0].1, pov(10));
        assert_eq!(out.last().unwrap().1, pov(total - 1));
        assert!(!r.catching_up());
    }

    #[test]
    fn catch_up_stops_at_open_boundary() {
        let f = TmpFile::new("catch_up");
        let w = EventWriter::create_at(&f.0).unwrap();
        // An empty ring has no history.
        assert!(!EventReader::open_at(&f.0).unwrap().catching_up());
        for i in 0..5 {
            w.push(0, &pov(i));
        }
        let mut r = EventReader::open_at(&f.0).unwrap();
        // Pushed after the open, before the first read: live, not history.
        for i in 5..8 {
            w.push(0, &pov(i));
        }
        assert!(r.catching_up());
        let mut out = Vec::new();
        assert_eq!(
            r.read(&mut out),
            ReadStats { history: true, ..ReadStats::default() }
        );
        assert_eq!(out.iter().map(|x| x.1).collect::<Vec<_>>(), (0..5).map(pov).collect::<Vec<_>>());
        assert!(!r.catching_up());
        out.clear();
        r.read(&mut out);
        assert_eq!(out.iter().map(|x| x.1).collect::<Vec<_>>(), (5..8).map(pov).collect::<Vec<_>>());
        assert!(!r.catching_up());
    }

    #[test]
    fn catch_up_lapped_during_replay_skips_to_live() {
        let f = TmpFile::new("catch_up_lap");
        let w = EventWriter::create_at(&f.0).unwrap();
        for i in 0..5 {
            w.push(0, &pov(i));
        }
        let mut r = EventReader::open_at(&f.0).unwrap();
        // A full lap after the open overwrites the whole history.
        for i in 0..SLOT_COUNT as u32 + 5 {
            w.push(0, &pov(100 + i));
        }
        assert!(r.catching_up());
        let mut out = Vec::new();
        let stats = r.read(&mut out);
        // Nothing from after the open is delivered as history.
        assert!(out.is_empty());
        assert!(stats.history);
        // Claims 0..10 were overwritten; 0..5 were history, 5..10 were live and are charged.
        assert_eq!(stats.dropped, 10);
        assert_eq!(stats.dropped_live, 5);
        assert!(!r.catching_up());
    }

    #[test]
    fn producer_restart_ends_catch_up() {
        let f = TmpFile::new("catch_up_restart");
        let w = EventWriter::create_at(&f.0).unwrap();
        // More than a lap, so the reader starts past claim 0 and sees reserve go backwards.
        for i in 0..SLOT_COUNT as u32 + 10 {
            w.push(0, &pov(i));
        }
        let mut r = EventReader::open_at(&f.0).unwrap();
        assert!(r.catching_up());
        w.reserve().store(0, Ordering::Release);
        w.push(0, &pov(100));
        let mut out = Vec::new();
        let stats = r.read(&mut out);
        assert!(!r.catching_up());
        // The restarted run's events are live, not history.
        assert!(!stats.history);
        assert_eq!(out.iter().map(|x| x.1).collect::<Vec<_>>(), vec![pov(100)]);
    }

    #[test]
    fn hostile_reserve_near_u64_max_does_not_panic() {
        let f = TmpFile::new("hostile");
        let w = EventWriter::create_at(&f.0).unwrap();
        let mut r = EventReader::open_at(&f.0).unwrap();
        let mut out = Vec::new();
        w.reserve().store(u64::MAX - 1, Ordering::Release);
        let _ = r.read(&mut out);
        w.reserve().store(u64::MAX, Ordering::Release);
        let _ = r.read(&mut out);
        w.reserve().store(u64::MAX - 1, Ordering::Release);
        w.push(0, &pov(1)); // claim u64::MAX - 1, seq = u64::MAX
        w.push(0, &pov(2)); // claim u64::MAX, seq wraps to 0
        let _ = r.read(&mut out);
        let _ = r.read(&mut out);
    }

    #[test]
    fn is_replaced_detects_new_producer_run() {
        let f = TmpFile::new("replaced");
        let w1 = EventWriter::create_at(&f.0).unwrap();
        let r = EventReader::open_at(&f.0).unwrap();
        assert!(!r.is_replaced());
        // A crashed producer never runs Drop; the next run recreates the file.
        std::mem::forget(w1);
        let w2 = EventWriter::create_at(&f.0).unwrap();
        assert!(r.is_replaced());
        assert!(!EventReader::open_at(&f.0).unwrap().is_replaced());
        drop(w2);
        assert!(r.is_replaced()); // file gone
    }

    #[test]
    fn late_drop_of_old_writer_keeps_newer_ring() {
        let f = TmpFile::new("late_drop");
        let w1 = EventWriter::create_at(&f.0).unwrap();
        let w2 = EventWriter::create_at(&f.0).unwrap();
        drop(w1);
        assert!(f.0.exists());
        let r = EventReader::open_at(&f.0).unwrap();
        assert!(r.is_producer_active());
        drop(w2);
        assert!(!f.0.exists());
    }
}
