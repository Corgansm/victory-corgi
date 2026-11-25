//! The overlay: a window of its own over the game's top left corner, on its
//! own thread, that clicks go through. It shows [`set`]'s text (the frame
//! line's figures, every second) and what the recorder is doing.

use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering::Relaxed};
use std::sync::{Mutex, Once};

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::w;

static TEXT: Mutex<String> = Mutex::new(String::new());
static VISIBLE: AtomicBool = AtomicBool::new(true);
static WINDOW: AtomicIsize = AtomicIsize::new(0);
const WIDTH: i32 = 620;
const HEIGHT: i32 = 330;

/// The overlay's window (null before it exists).
pub(super) fn window() -> HWND {
    HWND(WINDOW.load(Relaxed) as *mut _)
}

/// Replaces the overlay's text.
pub(super) fn set(text: String) {
    *TEXT.lock().unwrap_or_else(|e| e.into_inner()) = text;
}

pub(super) fn toggle() {
    show(!VISIBLE.load(Relaxed));
}

pub(super) fn show(on: bool) {
    VISIBLE.store(on, Relaxed);
}

/// Starts the overlay's thread once.
pub(super) fn start_once() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = std::thread::Builder::new()
            .name("victory-debugger-overlay".into())
            .spawn(run);
    });
}

fn run() {
    // SAFETY: our class and window, used on this thread only.
    unsafe {
        let Ok(instance) = GetModuleHandleW(None) else { return };
        let class = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(proc),
            hInstance: instance.into(),
            lpszClassName: w!("VIctoryDebuggerOverlay"),
            ..Default::default()
        };
        RegisterClassExW(&class);
        let Ok(hwnd) = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            w!("VIctoryDebuggerOverlay"),
            w!("VIctory debugger"),
            WS_POPUP,
            0,
            0,
            WIDTH,
            HEIGHT,
            None,
            None,
            Some(instance.into()),
            None,
        ) else {
            tracing::warn!("debugger: no overlay window");
            return;
        };
        WINDOW.store(hwnd.0 as isize, Relaxed);
        let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 210, LWA_ALPHA);
        SetTimer(Some(hwnd), 1, 250, None);
        let mut message = MSG::default();
        while GetMessageW(&mut message, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}

extern "system" fn proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // SAFETY: messages of our window, on its thread.
    unsafe {
        match message {
            WM_TIMER => {
                follow(hwnd);
                let _ = InvalidateRect(Some(hwnd), None, false);
                LRESULT(0)
            }
            WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
            WM_PAINT => {
                paint(hwnd);
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            _ => DefWindowProcW(hwnd, message, wparam, lparam),
        }
    }
}

/// Over the game window's top left corner, shown or hidden.
unsafe fn follow(hwnd: HWND) {
    let game = super::game_window();
    let show = VISIBLE.load(Relaxed) && game.is_some();
    // SAFETY: our window; the game's is only asked where it is.
    unsafe {
        if let Some(game) = game.filter(|_| show) {
            let mut at = POINT { x: 8, y: 8 };
            let _ = ClientToScreen(game, &mut at);
            let _ = SetWindowPos(
                hwnd,
                Some(HWND_TOPMOST),
                at.x,
                at.y,
                WIDTH,
                HEIGHT,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
        } else {
            let _ = ShowWindow(hwnd, SW_HIDE);
        }
    }
}

unsafe fn paint(hwnd: HWND) {
    let mut ps = PAINTSTRUCT::default();
    // SAFETY: painting our window between BeginPaint and EndPaint.
    unsafe {
        let dc = BeginPaint(hwnd, &mut ps);
        let mut r = RECT { left: 0, top: 0, right: WIDTH, bottom: HEIGHT };
        let back = CreateSolidBrush(COLORREF(0x0018_1410));
        FillRect(dc, &r, back);
        let _ = DeleteObject(back.into());
        let font = CreateFontW(
            15,
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
        let old = SelectObject(dc, font.into());
        SetBkMode(dc, TRANSPARENT);
        SetTextColor(dc, COLORREF(0x0078_FF64));
        r.left += 8;
        r.top += 6;
        let mut text = header();
        text.push_str(&TEXT.lock().unwrap_or_else(|e| e.into_inner()));
        let mut wide: Vec<u16> = text.encode_utf16().collect();
        DrawTextW(dc, &mut wide, &mut r, DT_LEFT | DT_TOP | DT_NOPREFIX | DT_WORDBREAK);
        SelectObject(dc, old);
        let _ = DeleteObject(font.into());
        let _ = EndPaint(hwnd, &ps);
    }
}

/// The recorder's state, above the figures.
fn header() -> String {
    let frame = crate::queue::presented();
    let recording = super::RECORDING.load(Relaxed);
    let state = if recording == u64::MAX {
        "F9 record frame + pixel under mouse | F11 + texture pictures | F10 hide".to_owned()
    } else if frame < recording {
        format!("recording frame {recording} ...")
    } else {
        format!("frame {recording} recorded, writing the report ...")
    };
    format!("VIctory debugger  frame {frame}\n{state}\n\n")
}
