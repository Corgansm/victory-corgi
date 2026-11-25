//! The host APIs and subsystems the diagnostics answer for, counted where
//! the layer calls them.
//!
//! One row each: `D3D12CreateDevice` (the graphics device's start), root
//! signatures (what a shader may reach), pipeline state creation (shaders
//! and pipelines the host accepts), the DXIL the translator emits (its
//! instructions and formats), descriptor heaps (texture and buffer
//! bindings), residency (the card's memory), command queues and fences
//! (the CPU waiting for the card), `Present` (frames shown),
//! `GetDeviceRemovedReason` (a device lost), and the title's
//! `GetProcAddress` and `LoadLibrary` calls (what it resolves and loads
//! while running).
//!
//! The counters are atomics, always built. They are read by the live page
//! of the debugger (`debugger::live`, diagnostics builds) and, with
//! `VICTORY_HYDRATE_LOG` or `VICTORY_API_BOARD=1`, written to the log a
//! row a line every minute ([`tick`]), where the Hydrate records are made
//! of them (`victory_core::hydrate`).

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
#[cfg(windows)]
use std::time::Duration;

/// An API or subsystem with a row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Api {
    CreateDevice,
    RootSignatures,
    PipelineStates,
    Dxil,
    DescriptorHeaps,
    Residency,
    Queues,
    Present,
    RemovedReason,
    GetProcAddress,
    LoadLibrary,
}

/// How many rows there are.
pub const COUNT: usize = 11;

impl Api {
    /// Every row, in the order they are shown.
    pub const ALL: [Api; COUNT] = [
        Api::CreateDevice,
        Api::RootSignatures,
        Api::PipelineStates,
        Api::Dxil,
        Api::DescriptorHeaps,
        Api::Residency,
        Api::Queues,
        Api::Present,
        Api::RemovedReason,
        Api::GetProcAddress,
        Api::LoadLibrary,
    ];

    /// The API or subsystem's name.
    pub fn name(self) -> &'static str {
        match self {
            Api::CreateDevice => "D3D12CreateDevice",
            Api::RootSignatures => "Root signatures",
            Api::PipelineStates => "Pipeline state creation",
            Api::Dxil => "DXIL validation",
            Api::DescriptorHeaps => "Descriptor heaps",
            Api::Residency => "Resource residency",
            Api::Queues => "Command queues and fences",
            Api::Present => "Present",
            Api::RemovedReason => "GetDeviceRemovedReason",
            Api::GetProcAddress => "GetProcAddress",
            Api::LoadLibrary => "LoadLibrary",
        }
    }

    /// What the row tells of the run.
    pub fn tells(self) -> &'static str {
        match self {
            Api::CreateDevice => "Graphics device initialization",
            Api::RootSignatures => "Shader resource compatibility",
            Api::PipelineStates => "Shader and pipeline compatibility",
            Api::Dxil => "Shader instruction and format validity",
            Api::DescriptorHeaps => "Texture and buffer bindings",
            Api::Residency => "GPU memory availability",
            Api::Queues => "CPU/GPU synchronization and stalls",
            Api::Present => "Frame presentation",
            Api::RemovedReason => "GPU device failures",
            Api::GetProcAddress => "Dynamically resolved API dependencies",
            Api::LoadLibrary => "Runtime library dependencies",
        }
    }
}

static CALLS: [AtomicU64; COUNT] = [const { AtomicU64::new(0) }; COUNT];
static FAILED: [AtomicU64; COUNT] = [const { AtomicU64::new(0) }; COUNT];
static TOOK_US: [AtomicU64; COUNT] = [const { AtomicU64::new(0) }; COUNT];
static LONGEST_US: [AtomicU64; COUNT] = [const { AtomicU64::new(0) }; COUNT];
/// What was said of a row's calls, each once: the failures first told,
/// the libraries loaded.
static NOTES: Mutex<[Vec<String>; COUNT]> = Mutex::new([const { Vec::new() }; COUNT]);

/// Translated shaders `dxil.dll`'s validator took and rejected
/// (`VICTORY_VALIDATE=1`, see `pipeline::validate`).
static VALIDATED: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];

/// A translated shader was checked by `dxil.dll`'s validator: it is valid
/// (`ok`), or `what` says which shader and the validator's first error.
pub fn validated(ok: bool, what: impl FnOnce() -> String) {
    VALIDATED[usize::from(!ok)].fetch_add(1, Relaxed);
    if !ok {
        note(Api::Dxil, what);
    }
}

/// Notes kept for a row; later ones are counted and not kept.
const NOTES_KEPT: usize = 24;

/// A row's counters since the start.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub calls: u64,
    pub failed: u64,
    /// Microseconds spent in the calls that were timed.
    pub us: u64,
    /// The longest of them.
    pub longest_us: u64,
}

/// A call of `api` that worked (`ok`) or did not.
pub fn seen(api: Api, ok: bool) {
    CALLS[api as usize].fetch_add(1, Relaxed);
    if !ok {
        FAILED[api as usize].fetch_add(1, Relaxed);
    }
}

/// A call of `api` that failed, with what failed (kept once).
pub fn failed(api: Api, what: impl FnOnce() -> String) {
    seen(api, false);
    note(api, what);
}

/// Something to say of `api`, kept once.
pub fn note(api: Api, what: impl FnOnce() -> String) {
    let mut notes = NOTES.lock().unwrap_or_else(|e| e.into_inner());
    let notes = &mut notes[api as usize];
    if notes.len() >= NOTES_KEPT {
        return;
    }
    let what = what();
    if !notes.contains(&what) {
        notes.push(what);
    }
}

/// A call of `api` took `us` microseconds.
pub fn took(api: Api, us: u64) {
    TOOK_US[api as usize].fetch_add(us, Relaxed);
    LONGEST_US[api as usize].fetch_max(us, Relaxed);
}

/// The host's answer to a call of `api`, counted and passed on.
#[cfg(windows)]
pub(crate) fn result<T>(api: Api, what: &str, result: windows::core::Result<T>) -> windows::core::Result<T> {
    match &result {
        Ok(_) => seen(api, true),
        Err(e) => failed(api, || format!("{what}: {e}")),
    }
    result
}

/// `api`'s counters.
pub fn counts(api: Api) -> Counts {
    let i = api as usize;
    Counts {
        calls: CALLS[i].load(Relaxed),
        failed: FAILED[i].load(Relaxed),
        us: TOOK_US[i].load(Relaxed),
        longest_us: LONGEST_US[i].load(Relaxed),
    }
}

/// What was said of `api` ([`note`]).
pub fn notes(api: Api) -> Vec<String> {
    NOTES.lock().unwrap_or_else(|e| e.into_inner())[api as usize].clone()
}

/// A row as shown: the API, its state now, whether that state is good,
/// and what was said of its calls.
#[derive(Clone, Debug)]
pub struct Row {
    pub api: Api,
    pub now: String,
    pub good: bool,
    pub notes: Vec<String>,
}

/// The counters at a reader's last look, so that a row can say what
/// happened since (each reader keeps its own).
#[derive(Clone, Copy, Debug, Default)]
pub struct Last([Counts; COUNT]);

/// The rows.
///
/// `bound`: the host root signatures and pipeline states the frames bound
/// since the reader's last look, where the reader counts them (the live
/// page). `frames` in `window`: the presents and the time since that look.
///
/// Asked of the host where it can be asked (the device's removed reason,
/// the card's memory), counted where the layer makes the calls.
#[cfg(windows)]
pub(crate) fn board(last: &mut Last, bound: Option<(usize, usize)>, frames: u64, window: Duration) -> Vec<Row> {
    use std::fmt::Write;
    let host = crate::host::host();
    let now = Api::ALL.map(counts);
    let since = |api: Api| {
        let (a, b) = (now[api as usize], last.0[api as usize]);
        Counts {
            calls: a.calls.saturating_sub(b.calls),
            failed: a.failed.saturating_sub(b.failed),
            us: a.us.saturating_sub(b.us),
            longest_us: a.longest_us,
        }
    };
    let seconds = window.as_secs_f64().max(1e-3);
    let per_frame = |us: u64| us as f64 / 1e3 / frames.max(1) as f64;
    let mb = |b: u64| b >> 20;
    let room = crate::resource::room();
    let row = |api: Api| {
        let c = now[api as usize];
        let (now, good) = match api {
            Api::CreateDevice => (
                format!(
                    "{} made at feature level 12_0, {} refused; {}",
                    c.calls - c.failed,
                    c.failed,
                    if host.is_some() { "the host device exists" } else { "there is NO host device" }
                ),
                host.is_some() && c.failed == 0,
            ),
            Api::RootSignatures => {
                let mut s = format!("{} made by the host, {} refused", c.calls - c.failed, c.failed);
                if let Some((roots, _)) = bound {
                    let _ = write!(s, "; {roots} bound in the last {seconds:.0} s");
                }
                (s, c.failed == 0)
            }
            Api::PipelineStates => {
                let loaded = crate::pipeline::loaded();
                let mut s = format!(
                    "{} made or loaded ({loaded} of them from the pipeline library), {} refused by the host, {} queued",
                    c.calls - c.failed,
                    c.failed,
                    crate::pipeline::making()
                );
                if let Some((_, states)) = bound {
                    let _ = write!(s, "; {states} bound in the last {seconds:.0} s");
                }
                (s, c.failed == 0)
            }
            Api::Dxil => {
                let (valid, rejected) = (VALIDATED[0].load(Relaxed), VALIDATED[1].load(Relaxed));
                let checked = if valid + rejected == 0 {
                    "dxil.dll's validator has checked none (VICTORY_VALIDATE=1 checks each as it is translated; `victory translate <folder> --validate` checks a folder)".to_owned()
                } else {
                    format!("dxil.dll's validator took {valid} and REJECTED {rejected}")
                };
                (
                    format!(
                        "{} shaders translated to DXIL, {} did not translate; {checked}; the host refused {} pipelines",
                        c.calls - c.failed,
                        c.failed,
                        now[Api::PipelineStates as usize].failed
                    ),
                    c.failed == 0 && rejected == 0,
                )
            }
            Api::DescriptorHeaps => {
                let refused = crate::descriptor::host::REFUSED_VIEWS.load(Relaxed);
                let fallback = crate::descriptor::host::FALLBACK_SLOTS.load(Relaxed);
                (
                    format!(
                        "{} host heaps made, {} failed; {refused} buffer views refused and left null, {fallback} slots shown a stand-in",
                        c.calls - c.failed,
                        c.failed
                    ),
                    // A capped heap is told in the notes: views past the cap read nothing.
                    c.failed == 0 && refused == 0 && notes(api).is_empty(),
                )
            }
            Api::Residency => match host.and_then(|h| h.video_memory()) {
                Some([budget, used, _, shared]) => (
                    format!(
                        "the card: {} MB in use of a budget of {} MB ({:.0}%); {} MB of the PC's memory shared with it; {}",
                        mb(used),
                        mb(budget),
                        100.0 * used as f64 / budget.max(1) as f64,
                        mb(shared),
                        if room { "there is room for new textures" } else { "there is NO room for new textures (they wait, or are made in the PC's memory)" }
                    ),
                    used <= budget && room,
                ),
                None => ("the system gave no memory figures".to_owned(), false),
            },
            Api::Queues => {
                let w = since(api);
                (
                    format!(
                        "{} waits on the fence in the last {seconds:.0} s: {:.1} ms a frame our threads stood waiting for the card (the submitting thread, and those that land what the card wrote); the longest wait since the start {:.0} ms",
                        w.calls,
                        per_frame(w.us),
                        c.longest_us as f64 / 1e3
                    ),
                    c.failed == 0,
                )
            }
            Api::Present => {
                let w = since(api);
                (
                    format!(
                        "{frames} presents in the last {seconds:.1} s ({:.1} a second), {:.2} ms inside Present a frame (the longest {:.0} ms); {} failed since the start",
                        frames as f64 / seconds,
                        per_frame(w.us),
                        c.longest_us as f64 / 1e3,
                        c.failed
                    ),
                    frames > 0 && c.failed == 0,
                )
            }
            // SAFETY: a live device; the call only reads its state.
            Api::RemovedReason => match host.map(|h| unsafe { h.device.GetDeviceRemovedReason() }) {
                Some(Ok(())) => ("S_OK: the device is alive".to_owned(), true),
                Some(Err(e)) => (format!("the device is REMOVED: {e}"), false),
                None => ("no host device to ask".to_owned(), false),
            },
            Api::GetProcAddress => {
                let w = since(api);
                (
                    format!(
                        "{} lookups by the title since the start ({} in the last {seconds:.0} s), {} found nothing",
                        c.calls, w.calls, c.failed
                    ),
                    c.failed == 0,
                )
            }
            Api::LoadLibrary => (
                format!("{} loads by the title since the start, {} failed", c.calls, c.failed),
                c.failed == 0,
            ),
        };
        Row { api, now, good, notes: notes(api) }
    };
    let rows = Api::ALL.map(row).to_vec();
    last.0 = now;
    rows
}

/// Once a present: with `VICTORY_HYDRATE_LOG` (or `VICTORY_API_BOARD=1`)
/// the rows go to the log, ten seconds in and then every minute: a good
/// row as information, another as a warning, a removed device as an error.
#[cfg(windows)]
pub(crate) fn tick() {
    use std::sync::OnceLock;
    use std::time::Instant;
    static ON: OnceLock<bool> = OnceLock::new();
    if !*ON.get_or_init(|| {
        std::env::var_os("VICTORY_HYDRATE_LOG").is_some_and(|v| !v.is_empty())
            || std::env::var_os("VICTORY_API_BOARD").is_some_and(|v| v == "1")
    }) {
        return;
    }
    struct State {
        since: Instant,
        frames: u64,
        told: bool,
        last: Last,
    }
    static STATE: Mutex<Option<State>> = Mutex::new(None);
    let (mut last, frames, window) = {
        let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
        let state = state.get_or_insert_with(|| State { since: Instant::now(), frames: 0, told: false, last: Last::default() });
        state.frames += 1;
        let window = state.since.elapsed();
        if window < if state.told { Duration::from_secs(61) } else { Duration::from_secs(10) } {
            return;
        }
        let frames = std::mem::take(&mut state.frames);
        state.since = Instant::now();
        state.told = true;
        (state.last, frames, window)
    };
    // The frame the rows were taken at: Hydrate's records of them carry it.
    let frame = crate::frame::current();
    for row in board(&mut last, None, frames, window) {
        let line = line(&row);
        if row.good {
            tracing::info!(frame, "{line}");
        } else if row.api == Api::RemovedReason {
            tracing::error!(frame, "{line}");
        } else {
            tracing::warn!(frame, "{line}");
        }
    }
    if let Some(state) = STATE.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        state.last = last;
    }
}

/// A row as a line of the log: `api <name> [<what it tells>]: <state>`,
/// then what was said of its calls.
pub fn line(row: &Row) -> String {
    let mut line = format!("api {} [{}]: {}", row.api.name(), row.api.tells(), row.now);
    if !row.notes.is_empty() {
        line.push_str(" | ");
        line.push_str(&row.notes.join("; "));
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A call counts, a failure counts twice (as a call and as a failure)
    /// and is told once, and the longest time is kept.
    #[test]
    fn calls_failures_and_times_are_counted() {
        let before = counts(Api::LoadLibrary);
        seen(Api::LoadLibrary, true);
        failed(Api::LoadLibrary, || "FAILED nothing.dll".to_owned());
        failed(Api::LoadLibrary, || "FAILED nothing.dll".to_owned());
        took(Api::LoadLibrary, 30);
        took(Api::LoadLibrary, 70);
        took(Api::LoadLibrary, 50);
        let after = counts(Api::LoadLibrary);
        assert_eq!(after.calls - before.calls, 3);
        assert_eq!(after.failed - before.failed, 2);
        assert_eq!(after.us - before.us, 150);
        assert!(after.longest_us >= 70);
        assert_eq!(notes(Api::LoadLibrary).iter().filter(|n| *n == "FAILED nothing.dll").count(), 1);
    }

    /// Every row has its own name and what it tells, and a line of the log
    /// begins with them.
    #[test]
    fn every_row_has_a_name_and_a_line() {
        let mut names: Vec<&str> = Api::ALL.iter().map(|a| a.name()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), COUNT);
        for (i, api) in Api::ALL.into_iter().enumerate() {
            assert_eq!(api as usize, i);
        }
        let row = Row { api: Api::Present, now: "10 presents".to_owned(), good: true, notes: vec!["a".to_owned(), "b".to_owned()] };
        assert_eq!(line(&row), "api Present [Frame presentation]: 10 presents | a; b");
    }
}
