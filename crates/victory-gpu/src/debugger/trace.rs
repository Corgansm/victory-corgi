//! Shader traces. A shader named in `VICTORY_SHADER_TRACE` (hashes as the log
//! names them, comma separated) is translated traced
//! (`victory_shader::emit::with_trace`): after each console instruction the
//! registers it wrote go, for one lane, to a ring of records in a buffer of
//! ours bound as its root UAV `SLOT_TRACE`. User data tells it which lane: in
//! a recorded frame with a picked pixel, the pixel (pixel shaders, in the
//! draw's first target) or the thread at that point of the window (compute:
//! its group id times the group size plus its id in the group); elsewhere
//! none, and the shader records nothing. Each bind of a traced shader in the
//! recorded frame gets a tag, which the frame's listing names (`[tag n]`).

use std::cell::Cell;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex, OnceLock};

use victory_shader::emit::{TRACE_FIRST, TRACE_PIXEL, TRACE_THREAD, TRACE_WORDS};
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;

use crate::pipeline::Shader;

/// The ring's bytes.
const BYTES: u64 = 32 << 20;
const RING: u32 = ((BYTES - TRACE_FIRST as u64) / (TRACE_WORDS as u64 * 4)) as u32;

/// Whether shader `hash` is traced.
pub(crate) fn traced(hash: &str) -> bool {
    static LIST: OnceLock<Vec<String>> = OnceLock::new();
    LIST.get_or_init(|| {
        std::env::var("VICTORY_SHADER_TRACE")
            .unwrap_or_default()
            .split(',')
            .map(|h| h.trim().to_ascii_lowercase())
            .filter(|h| !h.is_empty())
            .collect()
    })
    .iter()
    .any(|h| hash.to_ascii_lowercase().starts_with(h.as_str()))
}

/// The ring, in the card's memory, always in the unordered access state.
fn ring() -> Option<&'static ID3D12Resource> {
    static RING_BUFFER: OnceLock<Option<ID3D12Resource>> = OnceLock::new();
    RING_BUFFER
        .get_or_init(|| {
            let host = crate::host::host()?;
            let heap = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_DEFAULT, ..Default::default() };
            let desc = D3D12_RESOURCE_DESC {
                Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
                Width: BYTES,
                Height: 1,
                DepthOrArraySize: 1,
                MipLevels: 1,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
                Flags: D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
                ..Default::default()
            };
            let mut buffer: Option<ID3D12Resource> = None;
            // SAFETY: plain creation.
            let made = unsafe {
                host.device.CreateCommittedResource(
                    &heap,
                    D3D12_HEAP_FLAG_NONE,
                    &desc,
                    D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                    None,
                    &mut buffer,
                )
            };
            if made.is_err() {
                tracing::warn!("debugger: no buffer for shader traces");
            }
            made.ok().and(buffer)
        })
        .as_ref()
}

/// The GPU address of the ring (0 without one), for the root UAV.
pub(crate) fn ring_address() -> u64 {
    // SAFETY: a live buffer.
    ring().map_or(0, |r| unsafe { r.GetGPUVirtualAddress() })
}

/// What a recorded frame traces: its number and the picked point.
static ARMED: Mutex<Option<(u64, (f32, f32), [u32; 3])>> = Mutex::new(None);
static TAGS: Mutex<Vec<Bound>> = Mutex::new(Vec::new());
static NEXT_TAG: AtomicU32 = AtomicU32::new(1);
static FRAME: AtomicU64 = AtomicU64::new(u64::MAX);

/// A bind of a traced shader in the recorded frame.
struct Bound {
    tag: u32,
    shader: Arc<Shader>,
    compute: bool,
    lane: (u32, u32),
}

thread_local! {
    /// The bind being made on this thread: its tag, mode and lane.
    static CURRENT: Cell<(u32, u32, u32, u32)> = const { Cell::new((0, 0, 0, 0)) };
    /// The last tag given on this thread, for the frame's listing.
    static LAST: Cell<u32> = const { Cell::new(0) };
    /// The traced compute thread's z.
    static Z: Cell<u32> = const { Cell::new(0) };
}

pub(super) fn arm(frame: u64, pick: Option<(f32, f32)>, thread: [u32; 3]) {
    TAGS.lock().unwrap_or_else(|e| e.into_inner()).clear();
    // (without a picked pixel, pixel shaders trace the window's middle)
    let pick = pick.unwrap_or((0.5, 0.5));
    *ARMED.lock().unwrap_or_else(|e| e.into_inner()) = Some((frame, pick, thread));
    FRAME.store(frame, Relaxed);
}

/// Before the user data of a stage whose shader is traced is set: a tag for
/// this bind when the frame is traced. `target`: the size of the draw's first
/// render target (pixel shaders).
pub(crate) fn stage(shader: &Arc<Shader>, compute: bool, target: Option<(u64, u32)>) {
    CURRENT.with(|c| c.set((0, 0, 0, 0)));
    if FRAME.load(Relaxed) == u64::MAX || !super::recording() {
        return;
    }
    let Some((_, pick, thread)) = *ARMED.lock().unwrap_or_else(|e| e.into_inner()) else { return };
    // pixel shaders: the picked pixel of the draw's first target; compute: the thread asked
    // for (a compute pass's threads need not be pixels: tiled passes walk a list of tiles)
    let (lane, z) = if compute {
        ((thread[0], thread[1]), thread[2])
    } else {
        let (w, h) = match target {
            Some((w, h)) => (w as f32, h as f32),
            None => super::window_size().unwrap_or((1920.0, 1080.0)),
        };
        (((pick.0 * w) as u32, (pick.1 * h) as u32), 0)
    };
    let tag = NEXT_TAG.fetch_add(1, Relaxed);
    let mode = if compute { TRACE_THREAD } else { TRACE_PIXEL };
    CURRENT.with(|c| c.set((tag, mode, lane.0, lane.1)));
    Z.with(|c| c.set(z));
    LAST.with(|l| l.set(tag));
    TAGS.lock().unwrap_or_else(|e| e.into_inner()).push(Bound { tag, shader: shader.clone(), compute, lane });
}

/// The tag [`stage`] last gave on this thread (0: none), for the listing.
pub(crate) fn current_tag() -> u32 {
    LAST.with(|l| l.replace(0))
}

/// User-data dword `n` of a traced shader (`victory_shader::emit::USER_TRACE` + n).
pub(crate) fn word(n: u32) -> u32 {
    let (tag, mode, x, y) = CURRENT.with(Cell::get);
    match n {
        0 if tag != 0 => mode,
        1 => x,
        2 => y,
        3 => Z.with(Cell::get),
        4 => FRAME.load(Relaxed) as u32,
        5 => tag,
        6 => RING,
        _ => 0,
    }
}

/// One step of a traced lane.
#[derive(Clone, Debug)]
pub(crate) struct Step {
    pub(crate) order: u32,
    pub(crate) offset: u32,
    pub(crate) text: String,
    pub(crate) register: String,
    pub(crate) value: u32,
}

/// A traced lane's run in one bind.
#[derive(Clone, Debug)]
pub(crate) struct Trace {
    pub(crate) tag: u32,
    pub(crate) hash: String,
    pub(crate) stage: &'static str,
    pub(crate) lane: (u32, u32),
    pub(crate) steps: Vec<Step>,
    /// The console program, one line per instruction (offset, text).
    pub(crate) code: Vec<(u32, String)>,
}

/// The recorded frame's traces (the frame has run).
pub(super) fn finish(frame: u64) -> Vec<Trace> {
    FRAME.store(u64::MAX, Relaxed);
    let bound = std::mem::take(&mut *TAGS.lock().unwrap_or_else(|e| e.into_inner()));
    if bound.is_empty() {
        return Vec::new();
    }
    let Some(records) = read_ring() else { return Vec::new() };
    let stamp = frame as u32;
    let mut by_tag: std::collections::BTreeMap<u32, Vec<[u32; 8]>> = Default::default();
    for r in records.chunks_exact(TRACE_WORDS as usize) {
        if r[0] == stamp && r[1] != 0 {
            let mut w = [0u32; 8];
            w.copy_from_slice(&r[..8]);
            by_tag.entry(r[1]).or_default().push(w);
        }
    }
    let mut out = Vec::new();
    let mut code_of: std::collections::HashMap<String, Vec<(u32, String)>> = Default::default();
    for b in bound {
        let Some(mut rs) = by_tag.remove(&b.tag) else { continue };
        rs.sort_by_key(|r| r[5]);
        let code = code_of.entry(b.shader.hash.clone()).or_insert_with(|| disassemble(b.shader.container())).clone();
        let text_at = |offset: u32| {
            code.binary_search_by_key(&offset, |c| c.0).map(|i| code[i].1.clone()).unwrap_or_default()
        };
        let steps = rs
            .iter()
            .map(|r| Step {
                order: r[5],
                offset: r[2],
                text: text_at(r[2]),
                register: if r[3] & 0x1_0000 != 0 { format!("v{}", r[3] & 0xffff) } else { format!("s{}", r[3]) },
                value: r[4],
            })
            .collect();
        out.push(Trace {
            tag: b.tag,
            hash: b.shader.hash.clone(),
            stage: if b.compute { "compute" } else { b.shader.stage },
            lane: b.lane,
            steps,
            code,
        });
    }
    out
}

/// The ring's records (from [`TRACE_FIRST`] on) as dwords.
fn read_ring() -> Option<Vec<u32>> {
    let ring = ring()?;
    let host = crate::host::host()?;
    let heap = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_READBACK, ..Default::default() };
    let desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
        Width: BYTES,
        Height: 1,
        DepthOrArraySize: 1,
        MipLevels: 1,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        ..Default::default()
    };
    let mut back: Option<ID3D12Resource> = None;
    // SAFETY: plain creation; an open list of ours; live buffers.
    unsafe {
        host.device
            .CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc, D3D12_RESOURCE_STATE_COPY_DEST, None, &mut back)
            .ok()?;
        let back = back?;
        let list = host.new_list().ok()?;
        list.list.ResourceBarrier(&[crate::host::transition(
            ring,
            D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
            D3D12_RESOURCE_STATE_COPY_SOURCE,
        )]);
        list.list.CopyBufferRegion(&back, 0, ring, 0, BYTES);
        list.list.ResourceBarrier(&[crate::host::transition(
            ring,
            D3D12_RESOURCE_STATE_COPY_SOURCE,
            D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
        )]);
        list.close();
        host.execute(&[&list]);
        let mut data = std::ptr::null_mut();
        back.Map(0, None, Some(&mut data)).ok()?;
        let words = std::slice::from_raw_parts(data.cast::<u32>(), (BYTES / 4) as usize);
        let out = words[(TRACE_FIRST / 4) as usize..].to_vec();
        back.Unmap(0, None);
        Some(out)
    }
}

/// The console program of a shader container: (offset, text) per instruction.
pub(crate) fn disassemble(container: &[u8]) -> Vec<(u32, String)> {
    use victory_shader::{Container, FourCC, HwStage};
    let Ok(c) = Container::parse(container) else { return Vec::new() };
    let Some(part) = c.part(FourCC::DXIL) else { return Vec::new() };
    let Ok(programs) = victory_shader::native_programs(part.data) else { return Vec::new() };
    let Some(p) = programs
        .iter()
        .find(|p| matches!(p.stage, HwStage::Vs | HwStage::Ps | HwStage::Ms | HwStage::Cs))
    else {
        return Vec::new();
    };
    let Ok(insts) = victory_shader::gfx10::decode(&p.code) else { return Vec::new() };
    insts
        .iter()
        .map(|i| {
            let t = match i.name() {
                Some(_) => i.text(p.wave64),
                None => format!("<{:?} {:#x}>", i.encoding, i.opcode),
            };
            (i.offset as u32, t)
        })
        .collect()
}
