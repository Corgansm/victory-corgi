//! The in-game debugger, `VICTORY_DEBUGGER=<folder>` (diagnostics builds).
//!
//! - **Overlay**: a window over the game's (it follows it and lets clicks
//!   through) with the frame rate and where the frame's time goes, the draws
//!   dropped and why, and the texture queue. F10 shows or hides it.
//! - **Frame recorder**: F9 records the frame after next. Every draw, dispatch
//!   and copy goes to `frame-<n>-ops.txt` with its pipeline, shaders, user data
//!   and views (the capture listing, `crate::capture`), every texture it renders
//!   to or writes is saved at the end of the frame as a picture, and
//!   `frame-<n>.html` (and `latest.html`) opens it all: the operations by pass,
//!   filterable, with the pictures, the pixel history, the textures and the
//!   shader traces.
//! - **Pixel history**: the pixel under the mouse when F9 is pressed (in the
//!   game's window) is read from each texture a draw or dispatch writes,
//!   before and after it ([`pixel`]): the report lists the operations that
//!   changed it, and with which values.
//! - **Texture inspector**: every title texture alive, its address, size,
//!   format and levels, whether the host has its copy, whether that copy was
//!   ever filled ([`textures`]). F11 records like F9 and also saves a small
//!   picture of each texture's first level.
//! - **Shader trace**: shaders named in `VICTORY_SHADER_TRACE` (hashes, comma
//!   separated) are translated traced (`victory_shader::emit::with_trace`):
//!   in a recorded frame, the picked pixel's (pixel shaders) or thread's
//!   (compute) run is recorded instruction by instruction, every register
//!   each console instruction wrote with its value, and the report shows it
//!   next to the console disassembly ([`trace`]).
//!
//! - **Live view**: `live.html` in the folder, written every two seconds
//!   while the title runs and reloading itself in a browser: where the
//!   frame's time goes, and every pipeline's calls a frame with what
//!   binding it costs the CPU (and, in a timing run, the card) ([`live`]).
//!
//! Without the keys: `command.txt` in the folder, read each frame and
//! removed, one command a line: `capture`, `capture pick <x> <y>` (a
//! fraction of the window, or pixels of it), `capture previews`,
//! `capture thread <x> <y> <z>` (the compute thread traced shaders record:
//! its console dispatch thread id; thread 0, 0, 0 without), `overlay on`,
//! `overlay off`; the words combine.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Mutex, OnceLock};

pub(crate) mod live;
mod overlay;
pub(crate) mod pixel;
mod report;
mod textures;
pub(crate) mod trace;

/// The debugger's folder, if it is on.
pub(crate) fn dir() -> Option<&'static PathBuf> {
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| {
        let d = PathBuf::from(std::env::var_os("VICTORY_DEBUGGER")?);
        std::fs::create_dir_all(&d).ok()?;
        tracing::info!(folder = %d.display(), "debugger on: F9 records a frame (and the pixel under the mouse), F10 shows or hides the overlay, F11 records with texture pictures");
        Some(d)
    })
    .as_ref()
}

/// Whether the debugger is on.
pub(crate) fn on() -> bool {
    dir().is_some()
}

/// What a recording is asked for.
#[derive(Clone, Debug, Default)]
pub(crate) struct Request {
    /// The pixel to follow, as fractions of the window (0..1).
    pub(crate) pick: Option<(f32, f32)>,
    /// Pictures of the textures' first levels too.
    pub(crate) previews: bool,
    /// The compute thread traced shaders record (the console's dispatch thread id: group id
    /// times the group size plus the id in it); the first one without.
    pub(crate) thread: Option<[u32; 3]>,
    /// Who asked (a key, the command file).
    pub(crate) why: String,
}

static PENDING: Mutex<Option<Request>> = Mutex::new(None);
static ACTIVE: Mutex<Option<Request>> = Mutex::new(None);
/// The frame being recorded (`u64::MAX`: none).
static RECORDING: AtomicU64 = AtomicU64::new(u64::MAX);
/// Presents after the recorded ones before the report is made: what the card
/// still runs of them has run by then.
const SETTLE: u64 = 2;
/// Frames recorded together: the title records a frame's lists while the one
/// before is presented (by our count, its first passes fall in one frame and
/// its lighting in the next), so a recording spans two.
pub(crate) const SPAN: u64 = 2;

/// Whether frame `n` is being recorded.
pub(crate) fn records(n: u64) -> bool {
    let r = RECORDING.load(Relaxed);
    r != u64::MAX && n >= r && n < r + SPAN
}

/// Whether the frame being built now is recorded.
pub(crate) fn recording() -> bool {
    records(crate::queue::presented() + 1)
}

/// The pixel followed in the frame being built, as fractions of the window.
pub(crate) fn pick() -> Option<(f32, f32)> {
    if !recording() {
        return None;
    }
    ACTIVE.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|r| r.pick)
}

/// Asks for a recording (the next frame that can start one).
pub(crate) fn request(r: Request) {
    tracing::info!(?r, "debugger: recording asked for");
    *PENDING.lock().unwrap_or_else(|e| e.into_inner()) = Some(r);
}

/// The overlay's text, made with the frame line (`crate::host`).
pub(crate) fn stats(text: String) {
    if on() {
        live::status(&text);
        overlay::set(text);
    }
}

/// At the present of `frame`: the keys and the command file, the end of a
/// recording, the start of the next.
pub(crate) fn on_present(frame: u64) {
    if !on() {
        return;
    }
    overlay::start_once();
    live::tick();
    poll_keys();
    poll_commands();
    let recorded = RECORDING.load(Relaxed);
    if recorded != u64::MAX && frame >= recorded + SPAN - 1 + SETTLE {
        finish(recorded);
        RECORDING.store(u64::MAX, Relaxed);
    }
    if RECORDING.load(Relaxed) == u64::MAX
        && let Some(r) = PENDING.lock().unwrap_or_else(|e| e.into_inner()).take()
    {
        // The frame after next: lists of the next one may already be recording.
        let n = frame + 2;
        pixel::arm(n);
        trace::arm(n, r.pick, r.thread.unwrap_or([0; 3]));
        tracing::info!(frame = n, pick = ?r.pick, why = r.why, "debugger: recording frame");
        *ACTIVE.lock().unwrap_or_else(|e| e.into_inner()) = Some(r);
        RECORDING.store(n, Relaxed);
    }
}

fn finish(frame: u64) {
    let started = std::time::Instant::now();
    let request = ACTIVE.lock().unwrap_or_else(|e| e.into_inner()).take().unwrap_or_default();
    // What the card still has of the frame runs first.
    crate::host::settle_for_present();
    let pixels = pixel::finish();
    let traces = trace::finish(frame);
    let textures = textures::inventory(frame, request.previews);
    match report::write(frame, &request, &pixels, &traces, &textures) {
        Ok(path) => tracing::info!(
            path = %path.display(),
            ms = started.elapsed().as_millis() as u64,
            pixel_samples = pixels.len(),
            traced_records = traces.iter().map(|t| t.steps.len()).sum::<usize>(),
            textures = textures.len(),
            "debugger: frame report written"
        ),
        Err(e) => tracing::warn!(error = %e, "debugger: frame report not written"),
    }
}

/// The game's window: the largest visible top-level window of this process
/// that is not the overlay.
pub(crate) fn game_window() -> Option<windows::Win32::Foundation::HWND> {
    use windows::Win32::Foundation::{HWND, LPARAM, RECT};
    use windows::Win32::UI::WindowsAndMessaging::*;
    struct Found {
        best: Option<(HWND, i64)>,
        skip: HWND,
    }
    extern "system" fn visit(hwnd: HWND, found: LPARAM) -> windows::core::BOOL {
        // SAFETY: `found` is ours; a window handle from the enumeration.
        unsafe {
            let found = &mut *(found.0 as *mut Found);
            let mut process = 0;
            GetWindowThreadProcessId(hwnd, Some(&mut process));
            if process != windows::Win32::System::Threading::GetCurrentProcessId()
                || !IsWindowVisible(hwnd).as_bool()
                || hwnd == found.skip
            {
                return true.into();
            }
            let mut r = RECT::default();
            if GetClientRect(hwnd, &mut r).is_ok() {
                let area = i64::from(r.right - r.left) * i64::from(r.bottom - r.top);
                if found.best.is_none_or(|(_, a)| area > a) {
                    found.best = Some((hwnd, area));
                }
            }
        }
        true.into()
    }
    let mut found = Found { best: None, skip: overlay::window() };
    // SAFETY: the callback only touches `found` during the call.
    unsafe {
        let _ = EnumWindows(Some(visit), LPARAM(std::ptr::from_mut(&mut found) as isize));
    }
    found.best.map(|b| b.0)
}

/// The mouse in the game's window, as fractions of its client area; None
/// outside it.
fn cursor() -> Option<(f32, f32)> {
    use windows::Win32::Foundation::{POINT, RECT};
    use windows::Win32::UI::WindowsAndMessaging::{GetClientRect, GetCursorPos};
    use windows::Win32::Graphics::Gdi::ScreenToClient;
    let hwnd = game_window()?;
    let mut p = POINT::default();
    let mut r = RECT::default();
    // SAFETY: plain queries of our window.
    unsafe {
        GetCursorPos(&mut p).ok()?;
        if !ScreenToClient(hwnd, &mut p).as_bool() {
            return None;
        }
        GetClientRect(hwnd, &mut r).ok()?;
    }
    let (w, h) = ((r.right - r.left) as f32, (r.bottom - r.top) as f32);
    if w <= 0.0 || h <= 0.0 || p.x < 0 || p.y < 0 || p.x as f32 >= w || p.y as f32 >= h {
        return None;
    }
    Some(((p.x as f32 + 0.5) / w, (p.y as f32 + 0.5) / h))
}

/// The client size of the game's window.
fn window_size() -> Option<(f32, f32)> {
    use windows::Win32::Foundation::RECT;
    use windows::Win32::UI::WindowsAndMessaging::GetClientRect;
    let hwnd = game_window()?;
    let mut r = RECT::default();
    // SAFETY: a plain query of our window.
    unsafe { GetClientRect(hwnd, &mut r).ok()? };
    Some(((r.right - r.left) as f32, (r.bottom - r.top) as f32))
}

/// F9, F10 and F11, while one of our windows has the keyboard.
fn poll_keys() {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_F9, VK_F10, VK_F11};
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
    static DOWN: [std::sync::atomic::AtomicBool; 3] = [const { std::sync::atomic::AtomicBool::new(false) }; 3];
    // SAFETY: plain queries.
    let ours = unsafe {
        let mut process = 0;
        GetWindowThreadProcessId(GetForegroundWindow(), Some(&mut process));
        process == windows::Win32::System::Threading::GetCurrentProcessId()
    };
    for (n, key) in [VK_F9, VK_F10, VK_F11].into_iter().enumerate() {
        // SAFETY: a plain query.
        let down = ours && unsafe { GetAsyncKeyState(i32::from(key.0)) } < 0;
        let was = DOWN[n].swap(down, Relaxed);
        if !down || was {
            continue;
        }
        match n {
            0 => request(Request { pick: cursor(), previews: false, why: "F9".into(), ..Default::default() }),
            1 => overlay::toggle(),
            _ => request(Request { pick: cursor(), previews: true, why: "F11".into(), ..Default::default() }),
        }
    }
}

/// `command.txt` in the folder (see the module notes).
fn poll_commands() {
    let Some(dir) = dir() else { return };
    let path = dir.join("command.txt");
    let Ok(text) = std::fs::read_to_string(&path) else { return };
    let _ = std::fs::remove_file(&path);
    for line in text.lines() {
        let words: Vec<&str> = line.split_whitespace().collect();
        match words.as_slice() {
            ["overlay", "on"] => overlay::show(true),
            ["overlay", "off"] => overlay::show(false),
            ["capture", rest @ ..] => {
                let mut r = Request { why: format!("command: {line}"), ..Default::default() };
                let mut i = 0;
                while i < rest.len() {
                    match rest[i] {
                        "previews" => r.previews = true,
                        "thread" => {
                            let at = |k: usize| rest.get(i + k).and_then(|v| v.parse::<u32>().ok());
                            if let (Some(x), Some(y), Some(z)) = (at(1), at(2), at(3)) {
                                r.thread = Some([x, y, z]);
                            }
                            i += 3;
                        }
                        "pick" => {
                            let x: Option<f32> = rest.get(i + 1).and_then(|v| v.parse().ok());
                            let y: Option<f32> = rest.get(i + 2).and_then(|v| v.parse().ok());
                            if let (Some(x), Some(y)) = (x, y) {
                                r.pick = if x > 1.0 || y > 1.0 {
                                    window_size().map(|(w, h)| ((x + 0.5) / w, (y + 0.5) / h))
                                } else {
                                    Some((x, y))
                                };
                            }
                            i += 2;
                        }
                        _ => {}
                    }
                    i += 1;
                }
                request(r);
            }
            [] => {}
            _ => tracing::warn!(line, "debugger: unknown command"),
        }
    }
}

/// A JSON string literal.
pub(crate) fn json(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            // (`</script>` must not end the page's script)
            '<' => out.push_str("\\u003c"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
