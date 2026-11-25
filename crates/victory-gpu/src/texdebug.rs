//! `VICTORY_TEXDEBUG=<dir>`: a debugger for the texture system, everything
//! between a title texture's memory and its host texture (`crate::texture`
//! for textures read through tables, `crate::draw`'s queue for textures
//! picked by index).
//!
//! - `frames.csv`, one row per presented frame: textures detiled and why
//!   (a write-watched write, a hash of guarded or unwatched memory, a part
//!   of a streamed texture, a read through the kernel), refreshes found
//!   unchanged and how, uploads recorded, the refreshes' time; the queue of
//!   textures picked by index (slots, due, frames the oldest has waited,
//!   uploaded so far, passes the card had no room in); staging memory; the
//!   card's memory in use and its budget; the verifier's checks.
//! - `textures.txt`, rewritten every 10 s: textures whose changes were
//!   missed, those detiled most, the biggest staging buffers.
//! - `misses.csv`: each missed change as it is found.
//!
//! The verifier (`VICTORY_TEXDEBUG_VERIFY_MB`, default 64 MB a frame, 0
//! off) hashes the memory of textures in turn and compares it with what was
//! last detiled. A texture whose memory differs is marked; if its next
//! refresh still finds it unchanged by the write watch or the guard, that
//! is a missed change (the boot video that cycled through three stale
//! frames): logged with the texture and the way the refresh decided.
//! A refresh that hashes the memory cannot miss, and a texture still
//! streaming in (partly mapped) is not verified.
#![cfg(windows)]

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

/// Why a texture was detiled.
pub(crate) const WATCH: u8 = 0;
pub(crate) const GUARD_HASH: u8 = 1;
pub(crate) const PARTIAL: u8 = 2;
pub(crate) const THROUGH: u8 = 3;
pub(crate) const HASH: u8 = 4;
pub(crate) const UNLOCKED: u8 = 5;
const REASONS: [&str; 6] = ["watch", "guard_hash", "partial", "through", "hash", "unlocked"];

/// How a refresh found a texture unchanged.
pub(crate) const SKIP_WATCH: u8 = 0;
pub(crate) const SKIP_GUARD: u8 = 1;
pub(crate) const SKIP_SAME_HASH: u8 = 2;
pub(crate) const SKIP_PARTIAL: u8 = 3;
pub(crate) const SKIP_UNREADABLE: u8 = 4;
const SKIPS: [&str; 5] = ["watch", "guard", "same_hash", "partial_throttle", "unreadable"];

fn dir() -> Option<&'static PathBuf> {
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| {
        let d = PathBuf::from(std::env::var_os("VICTORY_TEXDEBUG")?);
        std::fs::create_dir_all(&d).ok()?;
        Some(d)
    })
    .as_ref()
}

/// Whether the debugger is on (cheap: the hot paths ask it first).
pub(crate) fn on() -> bool {
    dir().is_some()
}

fn verify_budget() -> u64 {
    static MB: OnceLock<u64> = OnceLock::new();
    *MB.get_or_init(|| {
        std::env::var("VICTORY_TEXDEBUG_VERIFY_MB").ok().and_then(|v| v.parse().ok()).unwrap_or(64)
    }) << 20
}

#[derive(Default)]
struct Tex {
    bytes: u64,
    desc: String,
    created: u64,
    detiles: u32,
    by_reason: [u32; 6],
    skips: [u32; 5],
    last_detile: u64,
    uploads: u32,
    last_upload: u64,
    /// The memory as last detiled (with the verifier on).
    detiled: Option<blake3::Hash>,
    /// The frame the verifier first found the memory differ from it.
    differs_since: Option<u64>,
    /// Still streaming in (partly mapped): not verified.
    partial: bool,
    misses: u32,
    last_miss: u64,
    /// Other addresses its pages show at: pieces and bytes (as last found),
    /// whether more were left out, and changes found through them.
    aliases: u32,
    alias_bytes: u64,
    aliases_more: bool,
    alias_writes: u32,
}

static TEX: Mutex<Option<HashMap<usize, Tex>>> = Mutex::new(None);

fn texs() -> std::sync::MutexGuard<'static, Option<HashMap<usize, Tex>>> {
    TEX.lock().unwrap_or_else(|e| e.into_inner())
}

/// This frame's counts: detiles by reason, their bytes, skips by kind,
/// uploads, refresh microseconds, verified textures and bytes, found
/// differing, misses.
static DETILED: [AtomicU64; 6] = [const { AtomicU64::new(0) }; 6];
static DETILED_BYTES: AtomicU64 = AtomicU64::new(0);
static SKIPPED: [AtomicU64; 5] = [const { AtomicU64::new(0) }; 5];
static UPLOADS: AtomicU64 = AtomicU64::new(0);
static REFRESH_US: AtomicU64 = AtomicU64::new(0);
static VERIFIED: [AtomicU64; 3] = [const { AtomicU64::new(0) }; 3];
static MISSES: AtomicU64 = AtomicU64::new(0);
/// This frame's changes found through another address of a texture's pages.
static ALIAS_CAUGHT: AtomicU64 = AtomicU64::new(0);

fn frame_now() -> u64 {
    crate::frame::current()
}

/// A texture's staging buffer was made: memory, staging bytes, what it is.
pub(crate) fn created(memory: usize, bytes: u64, desc: impl FnOnce() -> String) {
    if !on() {
        return;
    }
    let mut t = texs();
    let e = t.get_or_insert_with(HashMap::new).entry(memory).or_default();
    e.bytes = bytes;
    e.desc = desc();
    e.created = frame_now();
}

/// A texture's memory (`tiled`, as read) was detiled, for `why`.
pub(crate) fn detiled(memory: usize, tiled: &[u8], why: u8) {
    if !on() {
        return;
    }
    DETILED[usize::from(why).min(5)].fetch_add(1, Relaxed);
    DETILED_BYTES.fetch_add(tiled.len() as u64, Relaxed);
    let hash = (verify_budget() > 0).then(|| blake3::hash(tiled));
    let mut t = texs();
    let e = t.get_or_insert_with(HashMap::new).entry(memory).or_default();
    e.detiles += 1;
    e.by_reason[usize::from(why).min(5)] += 1;
    e.last_detile = frame_now();
    e.detiled = hash;
    e.differs_since = None;
    e.partial = why == PARTIAL;
}

/// A refresh found a texture unchanged, by `how`. If the verifier saw its
/// memory differ from what was last detiled before this, the change was
/// missed.
pub(crate) fn skipped(memory: usize, how: u8) {
    if !on() {
        return;
    }
    SKIPPED[usize::from(how).min(4)].fetch_add(1, Relaxed);
    let now = frame_now();
    let mut t = texs();
    let Some(e) = t.get_or_insert_with(HashMap::new).get_mut(&memory) else {
        return;
    };
    e.skips[usize::from(how).min(4)] += 1;
    if how == SKIP_PARTIAL {
        e.partial = true;
    }
    let missable = how == SKIP_WATCH || how == SKIP_GUARD;
    if let Some(since) = e.differs_since
        && missable
        && since < now
    {
        e.misses += 1;
        e.last_miss = now;
        MISSES.fetch_add(1, Relaxed);
        let line = format!(
            "{now},{memory:#x},{},{},{},{since},{}\n",
            SKIPS[usize::from(how)],
            e.bytes,
            e.last_detile,
            e.desc.replace(',', ";")
        );
        if e.misses <= 3 {
            tracing::warn!(
                memory = format_args!("{memory:#x}"),
                desc = %e.desc,
                how = SKIPS[usize::from(how)],
                differs_since = since,
                last_detile = e.last_detile,
                "texture debugger: a texture's memory changed but its refresh found it unchanged"
            );
        }
        if let Some(d) = dir() {
            let path = d.join("misses.csv");
            let new = !path.exists();
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
                if new {
                    let _ = f.write_all(b"frame,memory,decided_by,staging_bytes,last_detile,differs_since,texture\n");
                }
                let _ = f.write_all(line.as_bytes());
            }
        }
        // Counted once per difference.
        e.differs_since = None;
    }
}

/// The texture at `memory`'s pages show at `pieces` other places too,
/// `bytes` in all (`more`: more than were kept).
pub(crate) fn aliased(memory: usize, pieces: usize, bytes: usize, more: bool) {
    if !on() {
        return;
    }
    let mut t = texs();
    if let Some(e) = t.get_or_insert_with(HashMap::new).get_mut(&memory) {
        e.aliases = pieces as u32;
        e.alias_bytes = bytes as u64;
        e.aliases_more = more;
    }
}

/// A refresh the texture's own guard called unchanged found a write at
/// another address of its pages (or could not tell): the hash decides.
pub(crate) fn alias_written(memory: usize) {
    if !on() {
        return;
    }
    ALIAS_CAUGHT.fetch_add(1, Relaxed);
    let mut t = texs();
    if let Some(e) = t.get_or_insert_with(HashMap::new).get_mut(&memory) {
        e.alias_writes += 1;
    }
}

/// A list recorded an upload of the texture's staging buffer.
pub(crate) fn uploaded(memory: usize) {
    if !on() {
        return;
    }
    UPLOADS.fetch_add(1, Relaxed);
    let now = frame_now();
    let mut t = texs();
    if let Some(e) = t.get_or_insert_with(HashMap::new).get_mut(&memory) {
        e.uploads += 1;
        e.last_upload = now;
    }
}

/// A texture refresh took `us` microseconds.
pub(crate) fn refreshed(us: u64) {
    if on() {
        REFRESH_US.fetch_add(us, Relaxed);
    }
}

/// The staging buffer of the texture at `memory` was let go.
pub(crate) fn forgotten(memory: usize) {
    if !on() {
        return;
    }
    let mut t = texs();
    if let Some(map) = t.as_mut() {
        map.remove(&memory);
    }
}

struct Frames {
    file: BufWriter<File>,
    start: Instant,
    cursor: usize,
    dumped: Instant,
}

static FRAMES: Mutex<Option<Frames>> = Mutex::new(None);

/// Hashes the memory of textures in turn (up to the budget) and marks the
/// ones that differ from what was last detiled.
fn verify(cursor: &mut usize) {
    let budget = verify_budget();
    if budget == 0 {
        return;
    }
    // The candidates, taken under the lock; read without it.
    let mut list: Vec<(usize, u64, blake3::Hash)> = {
        let t = texs();
        let Some(map) = t.as_ref() else { return };
        map.iter()
            .filter(|(_, e)| !e.partial && e.differs_since.is_none())
            .filter_map(|(&m, e)| Some((m, e.bytes, e.detiled?)))
            .collect()
    };
    if list.is_empty() {
        return;
    }
    list.sort_unstable_by_key(|x| x.0);
    let n = list.len();
    let mut spent = 0u64;
    let mut buf = Vec::new();
    let mut found = Vec::new();
    for k in 0..n {
        if spent >= budget {
            break;
        }
        let (memory, _, detiled) = list[(*cursor + k) % n];
        // The title memory's size is what was detiled, not the staging
        // buffer's: asked of the texture module.
        let Some(size) = crate::texture::memory_size(memory) else {
            continue;
        };
        buf.resize(size, 0);
        // Through the kernel: a page not there makes it fail, and the
        // texture is left for later.
        if !crate::host::read_into(memory as *const u8, &mut buf) {
            continue;
        }
        spent += size as u64;
        VERIFIED[0].fetch_add(1, Relaxed);
        VERIFIED[1].fetch_add(size as u64, Relaxed);
        if blake3::hash(&buf) != detiled {
            found.push(memory);
        }
    }
    *cursor = (*cursor + n.min(64)) % n.max(1);
    if found.is_empty() {
        return;
    }
    VERIFIED[2].fetch_add(found.len() as u64, Relaxed);
    let now = frame_now();
    let mut t = texs();
    if let Some(map) = t.as_mut() {
        for m in found {
            if let Some(e) = map.get_mut(&m)
                && e.differs_since.is_none()
            {
                e.differs_since = Some(now);
            }
        }
    }
}

/// At a present: the verifier's turn, the frame's row, and every 10 s the
/// texture tables.
pub(crate) fn frame() {
    let Some(dir) = dir() else { return };
    let mut guard = FRAMES.lock().unwrap_or_else(|e| e.into_inner());
    if guard.is_none() {
        let Ok(f) = File::create(dir.join("frames.csv")) else { return };
        let mut file = BufWriter::new(f);
        let _ = writeln!(
            file,
            "frame,t_s,staged_textures,staged_mb,detiles,detile_mb,{},{},uploads,refresh_ms,\
             index_slots,index_due,index_oldest_frames,index_uploaded_total,index_uploaded_mb_total,index_waiting,index_away_total,index_no_room_total,\
             card_used_mb,card_budget_mb,verified,verified_mb,verify_differs,misses_total,alias_caught,system_textures",
            REASONS.map(|r| format!("detile_{r}")).join(","),
            SKIPS.map(|s| format!("unchanged_{s}")).join(",")
        );
        *guard = Some(Frames { file, start: Instant::now(), cursor: 0, dumped: Instant::now() });
    }
    let Some(fr) = guard.as_mut() else { return };
    verify(&mut fr.cursor);
    let take = |a: &AtomicU64| a.swap(0, Relaxed);
    let detiled: Vec<u64> = DETILED.iter().map(take).collect();
    let skipped: Vec<u64> = SKIPPED.iter().map(take).collect();
    let [staged_count, staged_bytes, _] = crate::texture::memory();
    let q = |i: usize| crate::draw::HEAP_TEXTURES_QUEUE[i].load(Relaxed);
    let card = crate::host::host().and_then(|h| h.video_memory()).unwrap_or([0; 4]);
    let verified: Vec<u64> = VERIFIED.iter().map(take).collect();
    let row = format!(
        "{},{:.3},{},{:.1},{},{:.1},{},{},{},{:.2},{},{},{},{},{},{},{},{},{:.0},{:.0},{},{:.1},{},{},{},{}",
        frame_now(),
        fr.start.elapsed().as_secs_f64(),
        staged_count,
        staged_bytes as f64 / 1e6,
        detiled.iter().sum::<u64>(),
        take(&DETILED_BYTES) as f64 / 1e6,
        detiled.iter().map(u64::to_string).collect::<Vec<_>>().join(","),
        skipped.iter().map(u64::to_string).collect::<Vec<_>>().join(","),
        take(&UPLOADS),
        take(&REFRESH_US) as f64 / 1e3,
        q(0),
        q(1),
        q(4),
        crate::draw::HEAP_TEXTURES.load(Relaxed),
        crate::draw::HEAP_TEXTURE_BYTES.load(Relaxed) >> 20,
        crate::draw::HEAP_TEXTURES_WAITING.load(Relaxed),
        crate::draw::HEAP_TEXTURES_AWAY.load(Relaxed),
        crate::draw::HEAP_TEXTURES_FULL.load(Relaxed),
        card[1] as f64 / 1e6,
        card[0] as f64 / 1e6,
        verified[0],
        verified[1] as f64 / 1e6,
        verified[2],
        MISSES.load(Relaxed),
        take(&ALIAS_CAUGHT),
        crate::resource::SYSTEM_TEXTURES.load(Relaxed),
    );
    let _ = writeln!(fr.file, "{row}");
    if fr.dumped.elapsed().as_secs() >= 10 {
        fr.dumped = Instant::now();
        let _ = fr.file.flush();
        dump(dir);
    }
}

/// `textures.txt`: the tables.
fn dump(dir: &std::path::Path) {
    let now = frame_now();
    let t = texs();
    let Some(map) = t.as_ref() else { return };
    let mut out = String::new();
    let total: u64 = map.values().map(|e| e.bytes).sum();
    let misses = MISSES.load(Relaxed);
    let aliased = map.values().filter(|e| e.aliases > 0 || e.aliases_more).count();
    out.push_str(&format!(
        "frame {now}: {} textures with staging buffers, {:.1} MB of staging; {misses} missed changes so far (textures since let go included); {aliased} textures whose pages show at another address too\n\n",
        map.len(),
        total as f64 / 1e6
    ));
    let row = |m: &usize, e: &Tex| {
        format!(
            "{m:#x}  {:>8.2} MB  detiles {:>5} (watch {} guard_hash {} partial {} through {} hash {} unlocked {})  unchanged {:?}  uploads {:>5}  last detile {:>6} upload {:>6}  misses {}  aliases {}{} ({:.1} MB, {} writes there)  {}\n",
            e.bytes as f64 / 1e6,
            e.detiles,
            e.by_reason[0],
            e.by_reason[1],
            e.by_reason[2],
            e.by_reason[3],
            e.by_reason[4],
            e.by_reason[5],
            e.skips,
            e.uploads,
            e.last_detile,
            e.last_upload,
            e.misses,
            e.aliases,
            if e.aliases_more { "+" } else { "" },
            e.alias_bytes as f64 / 1e6,
            e.alias_writes,
            e.desc
        )
    };
    out.push_str("== missed changes (memory changed, refresh said unchanged) ==\n");
    let mut v: Vec<_> = map.iter().filter(|(_, e)| e.misses > 0).collect();
    v.sort_by_key(|(_, e)| std::cmp::Reverse(e.misses));
    for (m, e) in v.iter().take(60) {
        out.push_str(&row(m, e));
    }
    out.push_str("\n== shown at another address too (writes there are asked of that address's guards) ==\n");
    let mut v: Vec<_> = map.iter().filter(|(_, e)| e.aliases > 0 || e.aliases_more).collect();
    v.sort_by_key(|(_, e)| std::cmp::Reverse(e.alias_writes));
    for (m, e) in v.iter().take(40) {
        out.push_str(&row(m, e));
    }
    out.push_str("\n== detiled most (cost every time: read, detile, upload) ==\n");
    let mut v: Vec<_> = map.iter().collect();
    v.sort_by_key(|(_, e)| std::cmp::Reverse(e.detiles));
    for (m, e) in v.iter().take(40) {
        out.push_str(&row(m, e));
    }
    out.push_str("\n== biggest staging buffers (memory held per texture) ==\n");
    let mut v: Vec<_> = map.iter().collect();
    v.sort_by_key(|(_, e)| std::cmp::Reverse(e.bytes));
    for (m, e) in v.iter().take(40) {
        out.push_str(&row(m, e));
    }
    out.push_str("\n== never uploaded since made (staged for nothing, or sampled only by index) ==\n");
    let mut v: Vec<_> = map.iter().filter(|(_, e)| e.uploads == 0).collect();
    v.sort_by_key(|(_, e)| std::cmp::Reverse(e.bytes));
    for (m, e) in v.iter().take(30) {
        out.push_str(&row(m, e));
    }
    let _ = std::fs::write(dir.join("textures.txt"), out);
}
