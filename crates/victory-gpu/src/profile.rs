//! `VICTORY_PROFILE=<dir>` (diagnostics): where a frame's time goes.
//!
//! - `frames.csv`: one row per presented frame. The frame's wall time, the
//!   process's processor time and page faults over it, the layer's own
//!   timers over it (the same counters the frame lines average over a
//!   second or five, taken here as differences frame by frame, so those
//!   lines are not disturbed), the draws, and the four threads that ran
//!   most in the frame.
//! - `threads.csv`: each thread once: id, name, start address (the title's
//!   own threads start in its image: `title+rva`, Ghidra `0x140000000+rva`).
//! - `samples.txt`: a sampling profiler. Every `VICTORY_PROFILE_US`
//!   microseconds (default 2000) each thread that ran since the last look
//!   (its cycle count moved) is stopped for a moment and its instruction
//!   address taken. Rewritten every 10 s: samples by module, by function
//!   (our code, from victory.exe's symbols), by title address (rva, to look
//!   up in Ghidra), and by thread.
//!
//! The stopped thread is let go before anything is allocated or locked, so
//! it cannot hold a lock the sampler then waits for.
#![cfg(all(windows, feature = "diagnostics"))]

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, HANDLE, INVALID_HANDLE_VALUE, LocalFree};
use windows_sys::Win32::System::Diagnostics::Debug::{
    CONTEXT, CONTEXT_CONTROL_AMD64, GetThreadContext, SYMBOL_INFOW, SYMOPT_DEFERRED_LOADS, SYMOPT_UNDNAME,
    SymFromAddrW, SymInitializeW, SymSetOptions,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
};
use windows_sys::Win32::System::LibraryLoader::{
    GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, GetModuleFileNameW,
    GetModuleHandleA, GetModuleHandleExW, GetProcAddress,
};
use windows_sys::Win32::System::WindowsProgramming::QueryThreadCycleTime;
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentProcessId, GetCurrentThreadId, GetProcessTimes, GetThreadDescription,
    GetThreadTimes, OpenThread, ResumeThread, SuspendThread, THREAD_GET_CONTEXT,
    THREAD_QUERY_INFORMATION, THREAD_QUERY_LIMITED_INFORMATION, THREAD_SUSPEND_RESUME,
};

/// The folder the profile goes to, if `VICTORY_PROFILE` names one.
fn dir() -> Option<&'static PathBuf> {
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| {
        let d = PathBuf::from(std::env::var_os("VICTORY_PROFILE")?);
        std::fs::create_dir_all(&d).ok()?;
        Some(d)
    })
    .as_ref()
}

fn micros(t: FILETIME) -> u64 {
    ((u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime)) / 10
}

fn zero_ft() -> FILETIME {
    FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 }
}

/// A thread's processor time so far (user + kernel), in microseconds.
fn thread_cpu(h: HANDLE) -> u64 {
    let (mut c, mut e, mut k, mut u) = (zero_ft(), zero_ft(), zero_ft(), zero_ft());
    // SAFETY: an open thread handle and four valid out pointers.
    if unsafe { GetThreadTimes(h, &mut c, &mut e, &mut k, &mut u) } == 0 {
        return 0;
    }
    micros(k) + micros(u)
}

/// The process's processor time so far (user + kernel), in microseconds.
fn process_cpu() -> u64 {
    let (mut c, mut e, mut k, mut u) = (zero_ft(), zero_ft(), zero_ft(), zero_ft());
    // SAFETY: our own process and four valid out pointers.
    unsafe { GetProcessTimes(GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u) };
    micros(k) + micros(u)
}

fn page_faults() -> u64 {
    use windows_sys::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    let mut c = PROCESS_MEMORY_COUNTERS { cb: size_of::<PROCESS_MEMORY_COUNTERS>() as u32, ..Default::default() };
    // SAFETY: our own process and a sized structure.
    unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) };
    u64::from(c.PageFaultCount)
}

/// The ids of this process's threads.
fn thread_ids() -> Vec<u32> {
    let mut out = Vec::new();
    // SAFETY: a snapshot of all threads; closed below.
    let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if snap == INVALID_HANDLE_VALUE || snap.is_null() {
        return out;
    }
    // SAFETY: plain data.
    let mut te: THREADENTRY32 = unsafe { std::mem::zeroed() };
    te.dwSize = size_of::<THREADENTRY32>() as u32;
    // SAFETY: GetCurrentProcessId has no preconditions.
    let me = unsafe { GetCurrentProcessId() };
    // SAFETY: an open snapshot and a sized entry.
    let mut ok = unsafe { Thread32First(snap, &mut te) } != 0;
    while ok {
        if te.th32OwnerProcessID == me {
            out.push(te.th32ThreadID);
        }
        // SAFETY: as above.
        ok = unsafe { Thread32Next(snap, &mut te) } != 0;
    }
    // SAFETY: the snapshot opened above.
    unsafe { CloseHandle(snap) };
    out
}

/// A loaded module's base and file name (cached), for an address in it.
fn module_of(addr: u64) -> Option<(u64, &'static str)> {
    static NAMES: Mutex<Vec<(u64, u64, &'static str)>> = Mutex::new(Vec::new());
    {
        let names = NAMES.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(&(base, _, name)) = names.iter().find(|(b, end, _)| addr >= *b && addr < *end) {
            return Some((base, name));
        }
    }
    let mut h = std::ptr::null_mut();
    // SAFETY: an address looked up, no reference taken.
    let ok = unsafe {
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            addr as *const u16,
            &mut h,
        )
    };
    if ok == 0 || h.is_null() {
        return None;
    }
    let mut buf = [0u16; 520];
    // SAFETY: a module handle and a buffer of that length.
    let n = unsafe { GetModuleFileNameW(h, buf.as_mut_ptr(), buf.len() as u32) } as usize;
    let path = String::from_utf16_lossy(&buf[..n]);
    let file = path.rsplit(['\\', '/']).next().unwrap_or(&path).to_owned();
    let name: &'static str = if file.starts_with("victory-image") { "title" } else { Box::leak(file.into_boxed_str()) };
    let base = h as u64;
    // The module's end: its image size, from the PE header.
    // SAFETY: a loaded image starts with its headers.
    let size = unsafe {
        let e_lfanew = (base as *const u8).add(0x3c).cast::<u32>().read_unaligned() as u64;
        (base as *const u8).add((e_lfanew + 0x18 + 0x38) as usize).cast::<u32>().read_unaligned() as u64
    };
    NAMES.lock().unwrap_or_else(|e| e.into_inner()).push((base, base + size.max(1), name));
    Some((base, name))
}

/// `module+0xrva`, or the address alone.
fn place(addr: u64) -> String {
    match module_of(addr) {
        Some((base, name)) => format!("{name}+{:#x}", addr - base),
        None => format!("{addr:#x}"),
    }
}

/// A thread's name and start address.
fn describe(h: HANDLE) -> (String, String) {
    let mut name = String::new();
    let mut p = std::ptr::null_mut();
    // SAFETY: an open thread handle; the text is freed below.
    if unsafe { GetThreadDescription(h, &mut p) } >= 0 && !p.is_null() {
        // SAFETY: a NUL-terminated text from the call above.
        unsafe {
            let mut n = 0;
            while *p.add(n) != 0 {
                n += 1;
            }
            name = String::from_utf16_lossy(std::slice::from_raw_parts(p, n));
            LocalFree(p.cast());
        }
    }
    // The start address the thread was created with (ThreadQuerySetWin32StartAddress).
    type Query = unsafe extern "system" fn(HANDLE, u32, *mut u64, u32, *mut u32) -> i32;
    static QUERY: OnceLock<Option<Query>> = OnceLock::new();
    let q = *QUERY.get_or_init(|| {
        // SAFETY: ntdll is always loaded; the export has this signature.
        unsafe {
            let ntdll = GetModuleHandleA(c"ntdll.dll".as_ptr().cast());
            GetProcAddress(ntdll, c"NtQueryInformationThread".as_ptr().cast()).map(|f| std::mem::transmute::<_, Query>(f))
        }
    });
    let mut start = 0u64;
    if let Some(q) = q {
        // SAFETY: an open thread handle and an 8-byte out value.
        unsafe { q(h, 9, &mut start, 8, std::ptr::null_mut()) };
    }
    (name.replace(',', ";"), if start == 0 { String::new() } else { place(start) })
}

struct Thread {
    handle: HANDLE,
    cpu: u64,
    cycles: u64,
    name: String,
}

// SAFETY: thread handles are process-wide.
unsafe impl Send for Thread {}

/// The threads looked at, by id; new ones written to `threads.csv`.
struct Threads {
    all: HashMap<u32, Thread>,
    list: Option<BufWriter<File>>,
    refreshed: Option<Instant>,
}

impl Threads {
    /// Opens the threads that are new since the last look (at most every `every`).
    fn refresh(&mut self, every: Duration) {
        if self.refreshed.is_some_and(|t| t.elapsed() < every) {
            return;
        }
        self.refreshed = Some(Instant::now());
        let ids = thread_ids();
        // Threads gone: close them.
        self.all.retain(|id, t| {
            let alive = ids.contains(id);
            if !alive {
                // SAFETY: a handle opened below.
                unsafe { CloseHandle(t.handle) };
            }
            alive
        });
        for id in ids {
            if self.all.contains_key(&id) {
                continue;
            }
            let access = THREAD_GET_CONTEXT | THREAD_SUSPEND_RESUME | THREAD_QUERY_INFORMATION | THREAD_QUERY_LIMITED_INFORMATION;
            // SAFETY: a thread of this process; closed when it is gone.
            let h = unsafe { OpenThread(access, 0, id) };
            if h.is_null() {
                continue;
            }
            let (name, start) = describe(h);
            if let Some(f) = self.list.as_mut() {
                let _ = writeln!(f, "{id},{name},{start}");
                let _ = f.flush();
            }
            let label = if name.is_empty() { start.clone() } else { name };
            let cpu = thread_cpu(h);
            self.all.insert(id, Thread { handle: h, cpu, cycles: 0, name: label });
        }
    }
}

static THREADS: Mutex<Option<Threads>> = Mutex::new(None);

fn threads() -> std::sync::MutexGuard<'static, Option<Threads>> {
    THREADS.lock().unwrap_or_else(|e| e.into_inner())
}

/// The counters a row is made of, read now.
fn counters() -> Vec<u64> {
    let mut v = crate::host::profile_counters();
    v.extend(crate::copystat::counts());
    v.push(crate::draw::drawn());
    v
}

struct Frames {
    file: BufWriter<File>,
    start: Instant,
    last: Instant,
    frame: u64,
    prev: Vec<u64>,
    cpu: u64,
    faults: u64,
}

static FRAMES: Mutex<Option<Frames>> = Mutex::new(None);

fn frames() -> std::sync::MutexGuard<'static, Option<Frames>> {
    FRAMES.lock().unwrap_or_else(|e| e.into_inner())
}

/// The columns of `frames.csv` after the first eight, as `counters()` lays
/// them out: (name, index into the counters, divisor to milliseconds or
/// megabytes; 1 for counts).
fn columns() -> Vec<(&'static str, usize, f64)> {
    use crate::copystat::C;
    let h = crate::host::PROFILE_COUNTERS;
    let c = |x: C| h + x as usize;
    vec![
        ("prepare_ms", 0, 1e3),
        ("gpu_wait_ms", 1, 1e3),
        ("back_ms", 2, 1e3),
        ("back_mb", 3, 1e6),
        ("copies_ms", 4, 1e3),
        ("copies_mb", 5, 1e6),
        ("textures_ms", 6, 1e3),
        ("textures_mb", 7, 1e6),
        ("submissions", c(C::Submissions), 1.0),
        ("record_ms", c(C::RecordNs), 1e6),
        ("record_calls", c(C::RecordCalls), 1.0),
        ("pipeline_wait_ms", c(C::PipelineWaitUs), 1e3),
        ("pace_sleep_ms", c(C::PaceUs), 1e3),
        ("until_record_ms", c(C::UntilRecordUs), 1e3),
        ("until_submit_ms", c(C::UntilSubmitUs), 1e3),
        ("tail_ms", c(C::TailUs), 1e3),
        ("watch_ms", c(C::WatchUs), 1e3),
        ("guard_ms", c(C::GuardUs), 1e3),
        ("guard_faults", c(C::GuardFaults), 1.0),
        ("query_ms", c(C::QueryUs), 1e3),
        ("move_ms", c(C::MoveUs), 1e3),
        ("run_ms", c(C::RunUs), 1e3),
        ("refresh_mb", c(C::RefreshBytes), 1e6),
        ("snapshot_ms", c(C::SnapshotUs), 1e3),
        ("back_scan_ms", c(C::BackScanUs), 1e3),
        ("back_sync_ms", c(C::BackSyncUs), 1e3),
        ("whole_ms", c(C::WholeUs), 1e3),
        ("sparse_ms", c(C::SparseUs), 1e3),
        ("host_lists_ms", c(C::HostListNs), 1e6),
        ("reset_ms", c(C::ResetNs), 1e6),
        ("wait_check_ms", c(C::WaitCheckNs), 1e6),
        ("dynamic_ms", c(C::DynamicNs), 1e6),
        ("note_ms", c(C::NoteNs), 1e6),
        ("card_over32_ms", c(C::CardOver32Us), 1e3),
        ("tex_uploads", c(C::TexUploads), 1.0),
    ]
}

/// At a present, before the frame lines take their counters: a row for the
/// frame that ends here.
pub(crate) fn frame_begins() {
    let Some(dir) = dir() else { return };
    let mut guard = frames();
    let now = Instant::now();
    if guard.is_none() {
        let Ok(f) = File::create(dir.join("frames.csv")) else { return };
        let mut file = BufWriter::new(f);
        let names: Vec<&str> = columns().iter().map(|c| c.0).collect();
        let _ = writeln!(
            file,
            "frame,t_s,frame_ms,cpu_ms,cpu_cores,faults,draws,{},t1,t1_ms,t2,t2_ms,t3,t3_ms,t4,t4_ms",
            names.join(",")
        );
        *guard = Some(Frames {
            file,
            start: now,
            last: now,
            frame: 0,
            prev: counters(),
            cpu: process_cpu(),
            faults: page_faults(),
        });
        *threads() = Some(Threads {
            all: HashMap::new(),
            list: File::create(dir.join("threads.csv")).ok().map(|f| {
                let mut w = BufWriter::new(f);
                let _ = writeln!(w, "id,name,start");
                w
            }),
            refreshed: None,
        });
        start_sampler(dir.clone());
        return;
    }
    let Some(fr) = guard.as_mut() else { return };
    fr.frame += 1;
    let ms = now.duration_since(fr.last).as_secs_f64() * 1e3;
    fr.last = now;
    let cur = counters();
    let delta: Vec<u64> = cur.iter().zip(&fr.prev).map(|(c, p)| c.saturating_sub(*p)).collect();
    let cpu = process_cpu();
    let cpu_ms = cpu.saturating_sub(fr.cpu) as f64 / 1e3;
    fr.cpu = cpu;
    let faults = page_faults();
    let df = faults.saturating_sub(fr.faults);
    fr.faults = faults;
    let draws = delta.last().copied().unwrap_or(0);
    // The threads that ran most in the frame.
    let mut top: Vec<(f64, String)> = Vec::new();
    if let Some(t) = threads().as_mut() {
        t.refresh(Duration::from_millis(500));
        for (id, th) in t.all.iter_mut() {
            let c = thread_cpu(th.handle);
            let d = c.saturating_sub(th.cpu);
            th.cpu = c;
            if d > 0 {
                top.push((d as f64 / 1e3, format!("{id} {}", th.name)));
            }
        }
    }
    top.sort_by(|a, b| b.0.total_cmp(&a.0));
    let mut row = format!(
        "{},{:.3},{:.2},{:.2},{:.2},{},{}",
        fr.frame,
        now.duration_since(fr.start).as_secs_f64(),
        ms,
        cpu_ms,
        if ms > 0.0 { cpu_ms / ms } else { 0.0 },
        df,
        draws
    );
    for (_, i, div) in columns() {
        let v = delta.get(i).copied().unwrap_or(0) as f64 / div;
        if div == 1.0 {
            row.push_str(&format!(",{v:.0}"));
        } else {
            row.push_str(&format!(",{v:.3}"));
        }
    }
    for k in 0..4 {
        match top.get(k) {
            Some((ms, name)) => row.push_str(&format!(",{},{ms:.2}", name.replace(',', ";"))),
            None => row.push_str(",,"),
        }
    }
    let _ = writeln!(fr.file, "{row}");
    if fr.frame % 30 == 0 {
        let _ = fr.file.flush();
    }
}

/// After the frame lines took (and zeroed) their counters: where the next
/// frame's differences start.
pub(crate) fn frame_ends() {
    if dir().is_none() {
        return;
    }
    if let Some(fr) = frames().as_mut() {
        fr.prev = counters();
    }
}

/// Our own code's function at an address (victory.exe's symbols), once
/// dbghelp is set up. Only the sampler thread calls dbghelp.
fn symbol(addr: u64) -> Option<String> {
    static READY: OnceLock<bool> = OnceLock::new();
    let ready = *READY.get_or_init(|| {
        // SAFETY: dbghelp for our own process, from this thread only.
        unsafe {
            SymSetOptions(SYMOPT_UNDNAME | SYMOPT_DEFERRED_LOADS);
            // victory.exe's symbols lie beside it (and in deps/ for cargo builds).
            let mut path = String::new();
            if let Ok(exe) = std::env::current_exe()
                && let Some(d) = exe.parent()
            {
                path = format!("{};{}", d.display(), d.join("deps").display());
            }
            let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
            SymInitializeW(GetCurrentProcess(), if path.is_empty() { std::ptr::null() } else { wide.as_ptr() }, 1) != 0
        }
    });
    if !ready {
        return None;
    }
    const MAX: usize = 400;
    let mut buf = vec![0u64; (size_of::<SYMBOL_INFOW>() + MAX * 2) / 8 + 1];
    let info = buf.as_mut_ptr().cast::<SYMBOL_INFOW>();
    // SAFETY: a buffer that holds the structure and MAX name characters.
    unsafe {
        (*info).SizeOfStruct = size_of::<SYMBOL_INFOW>() as u32;
        (*info).MaxNameLen = MAX as u32;
        let mut disp = 0u64;
        if SymFromAddrW(GetCurrentProcess(), addr, &mut disp, info) == 0 {
            return None;
        }
        let n = ((*info).NameLen as usize).min(MAX);
        let name = std::slice::from_raw_parts(std::ptr::addr_of!((*info).Name).cast::<u16>(), n);
        Some(String::from_utf16_lossy(name))
    }
}

fn start_sampler(dir: PathBuf) {
    let every = std::env::var("VICTORY_PROFILE_US")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(2000)
        .max(200);
    let _ = std::thread::Builder::new().name("victory-profiler".into()).spawn(move || sample(dir, Duration::from_micros(every)));
}

/// Cycles a thread must have run since the last look to be looked at
/// (`VICTORY_PROFILE_CYCLES`): about 50 microseconds of work.
static MIN_CYCLES_ENV: OnceLock<u64> = OnceLock::new();
fn min_cycles() -> u64 {
    *MIN_CYCLES_ENV.get_or_init(|| {
        std::env::var("VICTORY_PROFILE_CYCLES").ok().and_then(|v| v.parse().ok()).unwrap_or(200_000)
    })
}

/// The sampler's loop (see the module text).
fn sample(dir: PathBuf, every: Duration) {
    // SAFETY: no preconditions.
    let me = unsafe { GetCurrentThreadId() };
    let mut by_addr: HashMap<u64, u64> = HashMap::new();
    let mut by_thread: HashMap<u32, u64> = HashMap::new();
    let mut by_thread_addr: HashMap<(u32, u64), u64> = HashMap::new();
    let mut taken: Vec<(u32, u64)> = Vec::with_capacity(4096);
    let mut total = 0u64;
    let mut written = Instant::now();
    let mut dumps = 0u32;
    loop {
        std::thread::sleep(every);
        taken.clear();
        {
            let mut guard = threads();
            let Some(t) = guard.as_mut() else { continue };
            t.refresh(Duration::from_millis(500));
            for (&id, th) in t.all.iter_mut() {
                if id == me {
                    continue;
                }
                let mut cycles = 0u64;
                // SAFETY: an open thread handle.
                unsafe { QueryThreadCycleTime(th.handle, &mut cycles) };
                // Stopping a thread to look at it runs a little of it (the kernel's
                // suspend call), so a waiting thread moves a few thousand cycles per look:
                // only a thread that ran for real since the last look is looked at.
                let ran = cycles.saturating_sub(th.cycles);
                th.cycles = cycles;
                if ran < min_cycles() {
                    continue;
                }
                // SAFETY: plain data; aligned as the type asks.
                let mut ctx: CONTEXT = unsafe { std::mem::zeroed() };
                ctx.ContextFlags = CONTEXT_CONTROL_AMD64;
                // SAFETY: an open handle with suspend and context rights.
                // Nothing is allocated or locked while it is stopped.
                let rip = unsafe {
                    if SuspendThread(th.handle) == u32::MAX {
                        continue;
                    }
                    let ok = GetThreadContext(th.handle, &mut ctx) != 0;
                    ResumeThread(th.handle);
                    if !ok {
                        continue;
                    }
                    ctx.Rip
                };
                // The look's own cycles are not the thread's work.
                // SAFETY: an open thread handle.
                unsafe { QueryThreadCycleTime(th.handle, &mut th.cycles) };
                if taken.len() < taken.capacity() {
                    taken.push((id, rip));
                }
            }
        }
        for &(id, rip) in &taken {
            *by_addr.entry(rip).or_default() += 1;
            *by_thread.entry(id).or_default() += 1;
            *by_thread_addr.entry((id, rip)).or_default() += 1;
            total += 1;
        }
        if written.elapsed() >= Duration::from_secs(10) && total > 0 {
            written = Instant::now();
            dumps += 1;
            write_samples(&dir, total, &by_addr, &by_thread, &by_thread_addr, every, dumps);
        }
    }
}

fn write_samples(
    dir: &std::path::Path,
    total: u64,
    by_addr: &HashMap<u64, u64>,
    by_thread: &HashMap<u32, u64>,
    by_thread_addr: &HashMap<(u32, u64), u64>,
    every: Duration,
    dumps: u32,
) {
    let pct = |n: u64| 100.0 * n as f64 / total as f64;
    let mut modules: HashMap<&'static str, u64> = HashMap::new();
    let mut functions: HashMap<String, u64> = HashMap::new();
    let mut title: HashMap<u64, u64> = HashMap::new();
    for (&addr, &n) in by_addr {
        match module_of(addr) {
            Some((base, "title")) => {
                *modules.entry("title").or_default() += n;
                *title.entry(addr - base).or_default() += n;
            }
            Some((base, name)) => {
                *modules.entry(name).or_default() += n;
                let f = if name.eq_ignore_ascii_case("victory.exe") {
                    symbol(addr).unwrap_or_else(|| format!("victory.exe+{:#x}", addr - base))
                } else {
                    symbol(addr).map(|s| format!("{name}!{s}")).unwrap_or_else(|| format!("{name}+{:#x}", (addr - base) & !0xfff))
                };
                *functions.entry(f).or_default() += n;
            }
            None => {
                *modules.entry("(no module)").or_default() += n;
                *functions.entry(format!("(no module) {:#x}", addr & !0xffff)).or_default() += n;
            }
        }
    }
    let names: HashMap<u32, String> = threads()
        .as_ref()
        .map(|t| t.all.iter().map(|(id, th)| (*id, th.name.clone())).collect())
        .unwrap_or_default();
    let mut out = String::new();
    out.push_str(&format!(
        "samples {total} (one look every {} us; dump {dumps})\n\n== by module ==\n",
        every.as_micros()
    ));
    let mut m: Vec<_> = modules.into_iter().collect();
    m.sort_by(|a, b| b.1.cmp(&a.1));
    for (name, n) in m {
        out.push_str(&format!("{:6.2}%  {n:8}  {name}\n", pct(n)));
    }
    out.push_str("\n== by thread ==\n");
    let mut t: Vec<_> = by_thread.iter().collect();
    t.sort_by(|a, b| b.1.cmp(a.1));
    for (id, n) in t.iter().take(40) {
        let name = names.get(id).map(String::as_str).unwrap_or("");
        out.push_str(&format!("{:6.2}%  {:8}  {id} {name}\n", pct(**n), n));
    }
    out.push_str("\n== our code and libraries, by function ==\n");
    let mut f: Vec<_> = functions.into_iter().collect();
    f.sort_by(|a, b| b.1.cmp(&a.1));
    for (name, n) in f.iter().take(150) {
        out.push_str(&format!("{:6.2}%  {n:8}  {name}\n", pct(*n)));
    }
    out.push_str("\n== title code, by address (Ghidra 0x140000000 + rva) ==\n");
    let mut ti: Vec<_> = title.into_iter().collect();
    ti.sort_by(|a, b| b.1.cmp(&a.1));
    for (rva, n) in ti.iter().take(300) {
        out.push_str(&format!("{:6.2}%  {n:8}  title+{rva:#x}  ghidra {:#x}\n", pct(*n), 0x1_4000_0000u64 + rva));
    }
    out.push_str("\n== the 6 busiest threads, their top places ==\n");
    for (id, _) in t.iter().take(6) {
        let name = names.get(id).map(String::as_str).unwrap_or("");
        out.push_str(&format!("-- {id} {name}\n"));
        let mut places: HashMap<String, u64> = HashMap::new();
        for (&(tid, addr), &n) in by_thread_addr {
            if tid == **id {
                let p = match module_of(addr) {
                    Some((base, "title")) => format!("title+{:#x}", addr - base),
                    Some((base, name)) if name.eq_ignore_ascii_case("victory.exe") => {
                        symbol(addr).unwrap_or_else(|| format!("victory.exe+{:#x}", addr - base))
                    }
                    Some((base, name)) => {
                        symbol(addr).map(|s| format!("{name}!{s}")).unwrap_or_else(|| format!("{name}+{:#x}", (addr - base) & !0xfff))
                    }
                    None => format!("(no module) {:#x}", addr & !0xffff),
                };
                *places.entry(p).or_default() += n;
            }
        }
        let mut p: Vec<_> = places.into_iter().collect();
        p.sort_by(|a, b| b.1.cmp(&a.1));
        for (place, n) in p.iter().take(15) {
            out.push_str(&format!("   {:6.2}%  {n:8}  {place}\n", pct(*n)));
        }
    }
    let _ = std::fs::write(dir.join("samples.txt"), out);
    // the frame rows so far, on disk
    if let Some(fr) = frames().as_mut() {
        let _ = fr.file.flush();
    }
}
