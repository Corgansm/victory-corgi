//! RenderDoc frame captures (diagnostics builds).
//!
//! When VIctory runs inside RenderDoc (`renderdoccmd capture ...
//! victory.exe run ...`, or RenderDoc's "Launch Application"), RenderDoc
//! has injected `renderdoc.dll` and hooks our D3D12 device and swap chain.
//! `VICTORY_RENDERDOC_FRAMES` (comma-separated presented-frame numbers)
//! then captures each listed frame through RenderDoc's in-application API
//! (`RENDERDOC_API_1_1_2`): at that present, `TriggerCapture` records the
//! next frame, from that present to the following one. Each capture file
//! written is logged with its path. Without RenderDoc in the process the
//! list is ignored, with one warning. The capture trigger file
//! (`VICTORY_CAPTURE_TRIGGER`, see `capture::poll_trigger`) captures the
//! next frame as well.

use std::ffi::c_void;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};

use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};

/// `eRENDERDOC_API_Version_1_1_2`.
const API_1_1_2: i32 = 10102;
/// Entries of `RENDERDOC_API_1_1_2`, a table of function pointers, by
/// position.
const GET_NUM_CAPTURES: usize = 13;
const GET_CAPTURE: usize = 14;
const TRIGGER_CAPTURE: usize = 15;

type GetApi = unsafe extern "C" fn(i32, *mut *mut c_void) -> i32;
type GetNumCaptures = unsafe extern "C" fn() -> u32;
type GetCapture = unsafe extern "C" fn(u32, *mut u8, *mut u32, *mut u64) -> u32;
type TriggerCapture = unsafe extern "C" fn();

/// The API table RenderDoc hands out; it lives as long as the process.
struct Api(*const *const c_void);

// SAFETY: RenderDoc's in-application API may be called from any thread.
unsafe impl Send for Api {}
// SAFETY: as above; the table itself is never written by us.
unsafe impl Sync for Api {}

impl Api {
    /// Entry `index` as a function of type `F`.
    ///
    /// # Safety
    /// `F` must be the type of that entry.
    unsafe fn entry<F: Copy>(&self, index: usize) -> F {
        // SAFETY: the table has at least TRIGGER_CAPTURE + 1 entries
        // (version 1.1.2), and the caller names the entry's type.
        unsafe { std::mem::transmute_copy(&*self.0.add(index)) }
    }
}

/// RenderDoc's API, if RenderDoc is in the process.
fn api() -> Option<&'static Api> {
    static API: OnceLock<Option<Api>> = OnceLock::new();
    API.get_or_init(|| {
        let name: Vec<u16> = "renderdoc.dll".encode_utf16().chain([0]).collect();
        // SAFETY: plain Win32 lookups; the DLL stays loaded (RenderDoc
        // injected it and never unloads it).
        unsafe {
            let module = GetModuleHandleW(name.as_ptr());
            if module.is_null() {
                return None;
            }
            let get = GetProcAddress(module, c"RENDERDOC_GetAPI".as_ptr().cast())?;
            let get: GetApi = std::mem::transmute(get);
            let mut table: *mut c_void = std::ptr::null_mut();
            if get(API_1_1_2, &mut table) != 1 || table.is_null() {
                tracing::warn!("RenderDoc is loaded but its API 1.1.2 is not available");
                return None;
            }
            tracing::info!("running inside RenderDoc: frame captures available");
            Some(Api(table.cast()))
        }
    })
    .as_ref()
}

/// The frames `VICTORY_RENDERDOC_FRAMES` lists.
fn frames() -> &'static [u64] {
    static FRAMES: OnceLock<Vec<u64>> = OnceLock::new();
    FRAMES.get_or_init(|| {
        std::env::var("VICTORY_RENDERDOC_FRAMES")
            .unwrap_or_default()
            .split(',')
            .filter_map(|f| f.trim().parse().ok())
            .collect()
    })
}

/// Captures written so far that were already logged.
static LOGGED: AtomicU32 = AtomicU32::new(0);

/// Called at each present with its frame number (from 1); `triggered`:
/// the capture trigger file asked for the next frame.
pub(crate) fn on_present(frame: u64, triggered: bool) {
    let listed = frames();
    if listed.is_empty() && !(triggered && api().is_some()) {
        return;
    }
    let Some(api) = api() else {
        if frame == 1 {
            tracing::warn!(
                "VICTORY_RENDERDOC_FRAMES is set but VIctory is not running inside RenderDoc; no captures"
            );
        }
        return;
    };
    log_new_captures(api);
    if triggered || listed.contains(&frame) {
        // SAFETY: entry 15 of the 1.1.2 table is TriggerCapture().
        unsafe { api.entry::<TriggerCapture>(TRIGGER_CAPTURE)() };
        tracing::info!(frame, "RenderDoc capture triggered (the next frame is recorded)");
    }
}

/// Logs each capture file RenderDoc wrote since the last call.
fn log_new_captures(api: &Api) {
    // SAFETY: entries 13 and 14 of the 1.1.2 table, with valid pointers;
    // the first GetCapture call asks for the path's length only.
    unsafe {
        let count = api.entry::<GetNumCaptures>(GET_NUM_CAPTURES)();
        let from = LOGGED.swap(count, Ordering::Relaxed);
        let get = api.entry::<GetCapture>(GET_CAPTURE);
        for index in from..count {
            let (mut length, mut stamp) = (0u32, 0u64);
            if get(index, std::ptr::null_mut(), &mut length, &mut stamp) == 0 || length == 0 {
                continue;
            }
            let mut path = vec![0u8; length as usize];
            if get(index, path.as_mut_ptr(), &mut length, &mut stamp) == 0 {
                continue;
            }
            let end = path.iter().position(|&b| b == 0).unwrap_or(path.len());
            tracing::info!(
                index,
                path = %String::from_utf8_lossy(&path[..end]),
                "RenderDoc capture written"
            );
        }
    }
}
