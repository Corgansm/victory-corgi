//! Settings changed while the title runs (`VICTORY_LIVE_FILE=<file>`; off by
//! default).
//!
//! The file holds `NAME=value` lines, one for each of [`SPECS`], and is read
//! again whenever it changes (a thread looks every [`POLL`]). At the start of
//! a run it is written with the run's own values, and `<file>.menu` lists the
//! settings (`name|label|least|most|step|switch`, one a line) for what
//! changes them: the window of `scripts\live-menu.ps1`, and the overlay here,
//! over the game's top right corner, shown and hidden with F8 (drag a bar,
//! the wheel steps it, a right click puts back the run's value).
//!
//! A setting keeps the run's own value (its variable, read once as before)
//! until a menu changes it: [`get`] answers `None` till then.
//!
//! The render scale is the one setting for the next start: the title works
//! its render size out once, as it starts, and makes its tile grids and
//! render targets for it (a size changed under it while it runs breaks its
//! pictures). victory-loader reads it from this file as the title starts
//! (`victory_loader::hooks`), and writes `<file>.render` with the title's
//! render, output and largest sizes, which the menus show.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering::Relaxed};
use std::sync::{Mutex, Once, OnceLock};
use std::time::{Duration, SystemTime};

/// How often the file is looked at.
const POLL: Duration = Duration::from_millis(200);

/// A setting the menus change.
pub(crate) struct Spec {
    pub(crate) name: &'static str,
    pub(crate) label: &'static str,
    pub(crate) least: f32,
    pub(crate) most: f32,
    pub(crate) step: f32,
    /// On (1) or off (0) rather than a number.
    pub(crate) switch: bool,
    /// The run's own value.
    start: fn() -> f32,
}

pub(crate) const RENDER_SCALE: usize = 0;
pub(crate) const AMBIENT: usize = 1;
pub(crate) const SHADE_STRENGTH: usize = 2;
pub(crate) const SHADE_RADIUS: usize = 3;
pub(crate) const AIR: usize = 4;
pub(crate) const CARD_BUFFERS_MB: usize = 5;
pub(crate) const WINDOWS_BAR_MB: usize = 6;
pub(crate) const PANEL: usize = 7;
pub(crate) const DXR: usize = 8;
pub(crate) const RAY_TRACING: usize = 9;
pub(crate) const FRAME_CAP: usize = 10;
const COUNT: usize = 11;

fn env_number(name: &str) -> Option<f32> {
    std::env::var(name).ok().and_then(|v| v.trim().parse::<f32>().ok()).filter(|v| v.is_finite())
}

/// `name` in the file as it is before this run writes it.
fn file_number(name: &str) -> Option<f32> {
    let text = std::fs::read_to_string(file()?).ok()?;
    text.lines()
        .filter_map(|l| l.split_once('='))
        .find(|(n, _)| n.trim() == name)
        .and_then(|(_, v)| v.trim().parse::<f32>().ok())
        .filter(|v| v.is_finite())
}

fn env_shade(n: usize) -> f32 {
    let set = std::env::var("VICTORY_AMBIENT_SHADE").unwrap_or_default();
    set.split(',').nth(n).and_then(|v| v.trim().parse().ok()).unwrap_or([1.0, 0.6][n])
}

pub(crate) const SPECS: [Spec; COUNT] = [
    Spec {
        name: "VICTORY_RENDER_SCALE",
        label: "Render scale (next start)",
        // (victory-loader raises a smaller one to this: at 0.2 the title
        // crashed past its loading screen.)
        least: 0.39,
        most: 1.0,
        step: 0.01,
        switch: false,
        // What victory-loader started the title with: the file's, as the
        // last run left it (made the title's own when victory-loader says
        // the sizes).
        start: || file_number("VICTORY_RENDER_SCALE").or_else(|| env_number("VICTORY_RENDER_SCALE")).unwrap_or(2.0 / 3.0),
    },
    Spec {
        name: "VICTORY_AMBIENT",
        label: "Ambient light",
        least: 0.0,
        most: 2.0,
        step: 0.05,
        switch: false,
        start: || {
            let set = std::env::var("VICTORY_AMBIENT").unwrap_or_default();
            let set = set.trim();
            set.strip_prefix("constant:").unwrap_or(set).trim().parse::<f32>().ok().filter(|v| v.is_finite()).unwrap_or(0.0)
        },
    },
    Spec {
        name: "VICTORY_AMBIENT_SHADE_STRENGTH",
        label: "Crease shade",
        least: 0.0,
        most: 2.0,
        step: 0.05,
        switch: false,
        start: || env_shade(0),
    },
    Spec {
        name: "VICTORY_AMBIENT_SHADE_RADIUS",
        label: "Crease radius (m)",
        least: 0.1,
        most: 3.0,
        step: 0.05,
        switch: false,
        start: || env_shade(1),
    },
    Spec {
        name: "VICTORY_AMBIENT_AIR",
        label: "Air light limit (0: title's)",
        least: 0.0,
        most: 8.0,
        step: 0.1,
        switch: false,
        start: || env_number("VICTORY_AMBIENT_AIR").unwrap_or(0.0),
    },
    Spec {
        name: "VICTORY_CARD_BUFFERS_MB",
        label: "Card buffers cap (MB)",
        least: 256.0,
        most: 8192.0,
        step: 128.0,
        switch: false,
        start: || env_number("VICTORY_CARD_BUFFERS_MB").unwrap_or(2048.0),
    },
    Spec {
        name: "VICTORY_WINDOWS_BAR_MB",
        label: "BAR windows cap (MB)",
        least: 0.0,
        most: 8192.0,
        step: 128.0,
        switch: false,
        start: || env_number("VICTORY_WINDOWS_BAR_MB").unwrap_or(0.0),
    },
    Spec {
        name: "VICTORY_PANEL",
        label: "Stats panel",
        least: 0.0,
        most: 1.0,
        step: 1.0,
        switch: true,
        start: || f32::from(u8::from(crate::switch::on("VICTORY_PANEL"))),
    },
    Spec {
        name: "VICTORY_DXR",
        label: "DXR reader (log only)",
        least: 0.0,
        most: 1.0,
        step: 1.0,
        switch: true,
        start: || f32::from(u8::from(std::env::var_os("VICTORY_DXR").is_some_and(|v| v == "1"))),
    },
    Spec {
        name: "VICTORY_RAY_TRACING",
        label: "Ray tracing: 2 RTX, 1 title, 0 off",
        least: 0.0,
        most: 2.0,
        step: 1.0,
        switch: false,
        // What the run was started with (`run-highram.ps1 -Guided`, `-RT`).
        start: || {
            let on = |name: &str| std::env::var_os(name).is_some_and(|v| v == "1");
            if on("VICTORY_DXR_GUIDED") {
                2.0
            } else if on("VICTORY_BVH_BUILDER") {
                1.0
            } else {
                0.0
            }
        },
    },
    Spec {
        name: "VICTORY_FRAME_CAP",
        label: "Frame rate cap (30: title's)",
        least: 30.0,
        most: 120.0,
        step: 5.0,
        switch: false,
        // (`crate::frame`: the title's own interval is 30 a second.)
        start: || env_number("VICTORY_FRAME_CAP").filter(|v| (30.0..=240.0).contains(v)).unwrap_or(30.0),
    },
];

/// The live switch of ray tracing, when a menu has moved it: 2 the host's
/// structures guide the title's walks (as `VICTORY_DXR_GUIDED` translates
/// them), 1 the title's own walk only, 0 off (every node missed, and the
/// stand-in for the indirect light on, `crate::ambient`). Works in a run
/// whose trace programs were translated with `VICTORY_DXR_GUIDED=1`.
pub(crate) fn ray_tracing() -> Option<u32> {
    get(RAY_TRACING).map(|v| v.round().clamp(0.0, 2.0) as u32)
}

/// Each setting's value now and the run's own, as bits.
static NOW: [AtomicU32; COUNT] = [const { AtomicU32::new(0) }; COUNT];
static START: [AtomicU32; COUNT] = [const { AtomicU32::new(0) }; COUNT];
/// Bumped whenever a setting's value changes.
static GENERATION: [AtomicU64; COUNT] = [const { AtomicU64::new(0) }; COUNT];
/// The title's sizes as victory-loader last wrote them.
static RENDER: Mutex<String> = Mutex::new(String::new());
/// Writes of the file, one at a time.
static WRITING: Mutex<()> = Mutex::new(());

fn file() -> Option<&'static PathBuf> {
    static FILE: OnceLock<Option<PathBuf>> = OnceLock::new();
    FILE.get_or_init(|| std::env::var_os("VICTORY_LIVE_FILE").filter(|v| !v.is_empty()).map(PathBuf::from)).as_ref()
}

fn beside(suffix: &str) -> Option<PathBuf> {
    let mut path = file()?.clone().into_os_string();
    path.push(suffix);
    Some(path.into())
}

/// Starts the settings' thread and the overlay once, if a file is set.
pub(crate) fn start() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let Some(path) = file() else { return };
        for (n, spec) in SPECS.iter().enumerate() {
            let v = round(spec, (spec.start)());
            NOW[n].store(v.to_bits(), Relaxed);
            START[n].store(v.to_bits(), Relaxed);
        }
        let mut menu = String::new();
        for s in &SPECS {
            menu += &format!("{}|{}|{}|{}|{}|{}\n", s.name, s.label, s.least, s.most, s.step, u8::from(s.switch));
        }
        if let Some(at) = beside(".menu")
            && let Err(e) = std::fs::write(&at, menu)
        {
            tracing::warn!(error = %e, path = %at.display(), "live settings: the menu list not written");
        }
        write_file();
        tracing::info!(path = %path.display(), "live settings: the menus change them (F8 shows the overlay)");
        let _ = std::thread::Builder::new().name("victory-live-settings".into()).spawn(watch);
        overlay::start();
    });
}

/// A setting's value when a menu has changed it from the run's own.
pub(crate) fn get(n: usize) -> Option<f32> {
    start();
    let now = NOW[n].load(Relaxed);
    (file().is_some() && GENERATION[n].load(Relaxed) > 0 && now != START[n].load(Relaxed)).then(|| f32::from_bits(now))
}

/// As [`get`], once for each change, with `seen` the caller's own count.
pub(crate) fn fresh(n: usize, seen: &AtomicU64) -> Option<f32> {
    start();
    let generation = GENERATION[n].load(Relaxed);
    if seen.swap(generation, Relaxed) == generation {
        return None;
    }
    Some(f32::from_bits(NOW[n].load(Relaxed)))
}

fn round(spec: &Spec, v: f32) -> f32 {
    let v = v.clamp(spec.least, spec.most);
    let steps = ((v - spec.least) / spec.step).round();
    // (Two places past the step's, so 0.05 steps do not print 0.150000006.)
    let v = (spec.least + steps * spec.step).clamp(spec.least, spec.most);
    (v * 1000.0).round() / 1000.0
}

/// Sets setting `n` (a menu of ours) and writes the file.
fn set(n: usize, v: f32) {
    if store(n, v) {
        write_file();
    }
}

/// True if the value changed.
fn store(n: usize, v: f32) -> bool {
    let v = round(&SPECS[n], v);
    if NOW[n].swap(v.to_bits(), Relaxed) == v.to_bits() {
        return false;
    }
    GENERATION[n].fetch_add(1, Relaxed);
    tracing::info!(setting = SPECS[n].name, value = v, "live settings: changed");
    true
}

fn write_file() {
    let Some(path) = file() else { return };
    let _one = WRITING.lock().unwrap_or_else(|e| e.into_inner());
    let mut text = String::from("# VIctory live settings: NAME=value (VICTORY_LIVE_FILE)\n");
    for (n, spec) in SPECS.iter().enumerate() {
        text += &format!("{}={}\n", spec.name, f32::from_bits(NOW[n].load(Relaxed)));
    }
    // Whole or not at all: a reader never sees half of it.
    let mut part = path.clone().into_os_string();
    part.push(".part");
    let part = PathBuf::from(part);
    let written = std::fs::write(&part, &text).and_then(|()| std::fs::rename(&part, path));
    if written.is_err() {
        // (Another reader held it open: in place.)
        let _ = std::fs::remove_file(&part);
        if let Err(e) = std::fs::write(path, &text) {
            tracing::warn!(error = %e, "live settings: the file not written");
        }
    }
}

fn modified(path: &std::path::Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// The thread: the file and the title's sizes, read again when they change.
fn watch() {
    let Some(path) = file() else { return };
    let render = beside(".render");
    let (mut seen, mut seen_render) = (modified(path), None);
    let mut adopted = false;
    loop {
        std::thread::sleep(POLL);
        let now = modified(path);
        if now != seen {
            seen = now;
            if let Ok(text) = std::fs::read_to_string(path) {
                for line in text.lines() {
                    let Some((name, value)) = line.split_once('=') else { continue };
                    let Some(n) = SPECS.iter().position(|s| s.name == name.trim()) else { continue };
                    if let Ok(v) = value.trim().parse::<f32>()
                        && v.is_finite()
                    {
                        store(n, v);
                    }
                }
            }
        }
        let Some(render) = render.as_ref() else { continue };
        let now = modified(render);
        if now == seen_render {
            continue;
        }
        seen_render = now;
        let Ok(text) = std::fs::read_to_string(render) else { continue };
        let sizes: Vec<u32> = text.split_whitespace().filter_map(|w| w.parse().ok()).collect();
        if let [w, h, ow, oh, mw, mh] = sizes[..] {
            *RENDER.lock().unwrap_or_else(|e| e.into_inner()) =
                format!("render {w}x{h} of {ow}x{oh} (largest {mw}x{mh})");
            // The title's own scale is the slider's start, until it is moved.
            if !adopted && ow > 0 && GENERATION[RENDER_SCALE].load(Relaxed) == 0 {
                adopted = true;
                let scale = round(&SPECS[RENDER_SCALE], w as f32 / ow as f32);
                NOW[RENDER_SCALE].store(scale.to_bits(), Relaxed);
                START[RENDER_SCALE].store(scale.to_bits(), Relaxed);
                write_file();
            }
        }
    }
}

/// The overlay: a window of its own over the game's top right corner, on its
/// own thread, shown and hidden with F8. It takes clicks but never the
/// keyboard (the game keeps it).
mod overlay {
    use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering::Relaxed};

    use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
    use windows::Win32::Graphics::Gdi::*;
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, ReleaseCapture, SetCapture, VK_F8};
    use windows::Win32::UI::WindowsAndMessaging::*;
    use windows::core::w;

    use super::{SPECS, set};

    static VISIBLE: AtomicBool = AtomicBool::new(false);
    static WINDOW: AtomicIsize = AtomicIsize::new(0);
    /// The row a bar is dragged on, plus one (0: none).
    static DRAGGING: AtomicIsize = AtomicIsize::new(0);
    const WIDTH: i32 = 470;
    const TOP: i32 = 52;
    const ROW: i32 = 30;
    const BAR: (i32, i32) = (200, 390);

    fn height() -> i32 {
        TOP + ROW * SPECS.len() as i32 + 50
    }

    pub(super) fn start() {
        let _ = std::thread::Builder::new().name("victory-live-overlay".into()).spawn(run);
    }

    fn run() {
        // SAFETY: our class and window, used on this thread only.
        unsafe {
            let Ok(instance) = GetModuleHandleW(None) else { return };
            let class = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(proc),
                hInstance: instance.into(),
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                lpszClassName: w!("VIctoryLiveSettings"),
                ..Default::default()
            };
            RegisterClassExW(&class);
            let Ok(hwnd) = CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                w!("VIctoryLiveSettings"),
                w!("VIctory live settings"),
                WS_POPUP,
                0,
                0,
                WIDTH,
                height(),
                None,
                None,
                Some(instance.into()),
                None,
            ) else {
                tracing::warn!("live settings: no overlay window");
                return;
            };
            WINDOW.store(hwnd.0 as isize, Relaxed);
            let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 230, LWA_ALPHA);
            SetTimer(Some(hwnd), 1, 100, None);
            let mut message = MSG::default();
            while GetMessageW(&mut message, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
    }

    /// The row under `y` and the value under `x` on its bar.
    fn at(lparam: LPARAM) -> (Option<usize>, i32) {
        let x = i32::from((lparam.0 & 0xffff) as u16 as i16);
        let y = i32::from(((lparam.0 >> 16) & 0xffff) as u16 as i16);
        let row = (y >= TOP).then(|| ((y - TOP) / ROW) as usize).filter(|&r| r < SPECS.len());
        (row, x)
    }

    fn value_at(n: usize, x: i32) -> f32 {
        let s = &SPECS[n];
        let t = ((x - BAR.0) as f32 / (BAR.1 - BAR.0) as f32).clamp(0.0, 1.0);
        s.least + t * (s.most - s.least)
    }

    fn now(n: usize) -> f32 {
        f32::from_bits(super::NOW[n].load(Relaxed))
    }

    extern "system" fn proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        // SAFETY: messages of our window, on its thread.
        unsafe {
            match message {
                WM_TIMER => {
                    keys();
                    follow(hwnd);
                    let _ = InvalidateRect(Some(hwnd), None, false);
                    LRESULT(0)
                }
                WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
                WM_LBUTTONDOWN => {
                    if let (Some(n), x) = at(lparam) {
                        if SPECS[n].switch {
                            set(n, if now(n) >= 0.5 { 0.0 } else { 1.0 });
                        } else if x >= BAR.0 - 8 {
                            DRAGGING.store(n as isize + 1, Relaxed);
                            SetCapture(hwnd);
                            set(n, value_at(n, x));
                        }
                        let _ = InvalidateRect(Some(hwnd), None, false);
                    }
                    LRESULT(0)
                }
                WM_MOUSEMOVE => {
                    let dragged = DRAGGING.load(Relaxed);
                    if dragged > 0 {
                        let (_, x) = at(lparam);
                        set(dragged as usize - 1, value_at(dragged as usize - 1, x));
                        let _ = InvalidateRect(Some(hwnd), None, false);
                    }
                    LRESULT(0)
                }
                WM_LBUTTONUP => {
                    if DRAGGING.swap(0, Relaxed) > 0 {
                        let _ = ReleaseCapture();
                    }
                    LRESULT(0)
                }
                WM_RBUTTONDOWN => {
                    if let (Some(n), _) = at(lparam) {
                        set(n, f32::from_bits(super::START[n].load(Relaxed)));
                        let _ = InvalidateRect(Some(hwnd), None, false);
                    }
                    LRESULT(0)
                }
                WM_MOUSEWHEEL => {
                    // (The wheel's message has the point on the screen.)
                    let mut point = POINT {
                        x: i32::from((lparam.0 & 0xffff) as u16 as i16),
                        y: i32::from(((lparam.0 >> 16) & 0xffff) as u16 as i16),
                    };
                    let _ = ScreenToClient(hwnd, &mut point);
                    let row = (point.y >= TOP).then(|| ((point.y - TOP) / ROW) as usize).filter(|&r| r < SPECS.len());
                    if let Some(n) = row {
                        let turns = ((wparam.0 >> 16) & 0xffff) as u16 as i16;
                        let by = if turns > 0 { SPECS[n].step } else { -SPECS[n].step };
                        set(n, now(n) + by);
                        let _ = InvalidateRect(Some(hwnd), None, false);
                    }
                    LRESULT(0)
                }
                WM_PAINT => {
                    paint(hwnd);
                    LRESULT(0)
                }
                WM_ERASEBKGND => LRESULT(1),
                _ => DefWindowProcW(hwnd, message, wparam, lparam),
            }
        }
    }

    /// F8, with one of our windows in front.
    fn keys() {
        static DOWN: AtomicBool = AtomicBool::new(false);
        // SAFETY: a plain query.
        let down = unsafe { GetAsyncKeyState(i32::from(VK_F8.0)) } < 0;
        if DOWN.swap(down, Relaxed) || !down {
            return;
        }
        let mut process = 0;
        // SAFETY: plain queries.
        unsafe { GetWindowThreadProcessId(GetForegroundWindow(), Some(&mut process)) };
        if process == std::process::id() {
            VISIBLE.fetch_xor(true, Relaxed);
        }
    }

    /// Over the game window's top right corner, shown or hidden.
    unsafe fn follow(hwnd: HWND) {
        let game = if VISIBLE.load(Relaxed) { game_window(hwnd) } else { None };
        // SAFETY: our window; the game's is only asked where it is.
        unsafe {
            match game {
                Some(game) => {
                    let mut r = RECT::default();
                    let _ = GetClientRect(game, &mut r);
                    let mut at = POINT { x: r.right - WIDTH - 8, y: 8 };
                    let _ = ClientToScreen(game, &mut at);
                    let _ = SetWindowPos(
                        hwnd,
                        Some(HWND_TOPMOST),
                        at.x,
                        at.y,
                        WIDTH,
                        height(),
                        SWP_NOACTIVATE | SWP_SHOWWINDOW,
                    );
                }
                None => {
                    let _ = ShowWindow(hwnd, SW_HIDE);
                }
            }
        }
    }

    /// The game's window: the largest visible top-level window of this
    /// process that is not `ours`.
    fn game_window(ours: HWND) -> Option<HWND> {
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
                if process != std::process::id() || !IsWindowVisible(hwnd).as_bool() || hwnd == found.skip {
                    return true.into();
                }
                let mut r = RECT::default();
                if GetClientRect(hwnd, &mut r).is_ok() {
                    let area = i64::from(r.right - r.left) * i64::from(r.bottom - r.top);
                    if found.best.is_none_or(|(_, a)| area > a) {
                        found.best = Some((hwnd, area));
                    }
                }
                true.into()
            }
        }
        let mut found = Found { best: None, skip: ours };
        // SAFETY: `found` outlives the enumeration.
        unsafe {
            let _ = EnumWindows(Some(visit), LPARAM(&mut found as *mut Found as isize));
        }
        found.best.map(|(hwnd, _)| hwnd)
    }

    fn text(dc: HDC, s: &str, r: RECT, flags: DRAW_TEXT_FORMAT) {
        let mut wide: Vec<u16> = s.encode_utf16().collect();
        let mut r = r;
        // SAFETY: drawing on a device context of our window.
        unsafe { DrawTextW(dc, &mut wide, &mut r, flags | DT_NOPREFIX | DT_SINGLELINE | DT_VCENTER) };
    }

    fn fill(dc: HDC, r: RECT, color: u32) {
        // SAFETY: a brush of ours, deleted after use.
        unsafe {
            let brush = CreateSolidBrush(COLORREF(color));
            FillRect(dc, &r, brush);
            let _ = DeleteObject(brush.into());
        }
    }

    unsafe fn paint(hwnd: HWND) {
        let mut ps = PAINTSTRUCT::default();
        let (w, h) = (WIDTH, height());
        // SAFETY: painting our window between BeginPaint and EndPaint, into
        // a bitmap of ours first (no flicker).
        unsafe {
            let screen = BeginPaint(hwnd, &mut ps);
            let dc = CreateCompatibleDC(Some(screen));
            let bitmap = CreateCompatibleBitmap(screen, w, h);
            let old_bitmap = SelectObject(dc, bitmap.into());
            fill(dc, RECT { left: 0, top: 0, right: w, bottom: h }, 0x0018_1410);
            let font = CreateFontW(
                16,
                0,
                0,
                0,
                FW_NORMAL.0 as i32,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                CLEARTYPE_QUALITY,
                (FIXED_PITCH.0 | FF_MODERN.0) as u32,
                w!("Consolas"),
            );
            let old_font = SelectObject(dc, font.into());
            SetBkMode(dc, TRANSPARENT);
            SetTextColor(dc, COLORREF(0x0078_FF64));
            text(dc, "VIctory live settings   F8 hides", RECT { left: 10, top: 4, right: w, bottom: 24 }, DT_LEFT);
            let render = super::RENDER.lock().unwrap_or_else(|e| e.into_inner()).clone();
            SetTextColor(dc, COLORREF(0x00B4_B4B4));
            let render = if render.is_empty() { "render size: not known yet".to_owned() } else { render };
            text(dc, &render, RECT { left: 10, top: 24, right: w, bottom: 44 }, DT_LEFT);
            for (n, spec) in SPECS.iter().enumerate() {
                let y = TOP + ROW * n as i32;
                let v = now(n);
                let changed = super::NOW[n].load(Relaxed) != super::START[n].load(Relaxed);
                SetTextColor(dc, COLORREF(if changed { 0x0040_D0FF } else { 0x00E6_E6E6 }));
                text(dc, spec.label, RECT { left: 10, top: y, right: BAR.0 - 6, bottom: y + ROW }, DT_LEFT);
                let mid = y + ROW / 2;
                if spec.switch {
                    let on = v >= 0.5;
                    fill(dc, RECT { left: BAR.0, top: mid - 8, right: BAR.0 + 40, bottom: mid + 8 }, if on { 0x0050_B040 } else { 0x0050_5050 });
                    text(dc, if on { "on" } else { "off" }, RECT { left: BAR.0 + 48, top: y, right: w, bottom: y + ROW }, DT_LEFT);
                    continue;
                }
                let t = ((v - spec.least) / (spec.most - spec.least)).clamp(0.0, 1.0);
                let x = BAR.0 + (t * (BAR.1 - BAR.0) as f32) as i32;
                fill(dc, RECT { left: BAR.0, top: mid - 3, right: BAR.1, bottom: mid + 3 }, 0x0050_5050);
                fill(dc, RECT { left: BAR.0, top: mid - 3, right: x, bottom: mid + 3 }, 0x00D0_9040);
                fill(dc, RECT { left: x - 4, top: mid - 9, right: x + 4, bottom: mid + 9 }, 0x00F0_F0F0);
                let shown = if spec.step >= 1.0 {
                    format!("{v:.0}")
                } else if spec.step >= 0.1 {
                    format!("{v:.1}")
                } else {
                    format!("{v:.2}")
                };
                text(dc, &shown, RECT { left: BAR.1 + 10, top: y, right: w, bottom: y + ROW }, DT_LEFT);
                if n == super::RENDER_SCALE && changed {
                    SetTextColor(dc, COLORREF(0x0040_D0FF));
                    text(dc, "restart the game to apply", RECT { left: 10, top: h - 48, right: w, bottom: h - 28 }, DT_LEFT);
                }
            }
            SetTextColor(dc, COLORREF(0x0090_9090));
            text(
                dc,
                "drag or wheel: change   right click: the run's value",
                RECT { left: 10, top: h - 28, right: w, bottom: h - 4 },
                DT_LEFT,
            );
            let _ = BitBlt(screen, 0, 0, w, h, Some(dc), 0, 0, SRCCOPY);
            SelectObject(dc, old_font);
            let _ = DeleteObject(font.into());
            SelectObject(dc, old_bitmap);
            let _ = DeleteObject(bitmap.into());
            let _ = DeleteDC(dc);
            let _ = EndPaint(hwnd, &ps);
        }
    }

    #[allow(dead_code)]
    pub(super) fn window() -> HWND {
        HWND(WINDOW.load(Relaxed) as *mut _)
    }
}
