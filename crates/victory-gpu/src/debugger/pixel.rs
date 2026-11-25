//! Pixel history: in a recorded frame with a picked pixel, the pixel is read
//! from every texture a draw renders to or a dispatch writes through
//! unordered access, before the operation and after it (one texel copied
//! into a readback buffer each time, on the operation's own list). The
//! report lists the operations that changed it.
//!
//! The pixel is the same fraction of every texture (a half-size target's
//! texel under the same point of the screen). Depth-stencil and multisampled
//! textures are left out: the host copies those only whole.

use std::cell::RefCell;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;

use crate::host::HostList;
use crate::object::This;

/// Texels the readback buffer holds; each at its own 512 bytes (the
/// placement alignment of a copied footprint).
const SLOTS: u32 = 131072;
const SLOT_BYTES: u64 = 512;

/// One read of the pixel.
#[derive(Clone, Debug)]
pub(crate) struct Sample {
    /// The operation (in the order the frame's lists recorded them, across
    /// threads), as the frame's listing names it (`[op n]`).
    pub(crate) op: u64,
    pub(crate) label: String,
    /// Read after the operation (else before).
    pub(crate) after: bool,
    /// The title texture.
    pub(crate) target: usize,
    pub(crate) format: u32,
    pub(crate) size: (u64, u32),
    pub(crate) texel: (u32, u32),
    slot: u32,
    /// The texel's bytes, once read back.
    pub(crate) bytes: Vec<u8>,
}

struct Probe {
    frame: u64,
    buffer: ID3D12Resource,
    samples: Vec<Sample>,
    next: u32,
}

static PROBE: Mutex<Option<Probe>> = Mutex::new(None);
/// The frame [`PROBE`] reads (`u64::MAX`: none): checked before the lock.
static FRAME: AtomicU64 = AtomicU64::new(u64::MAX);
static OPS: AtomicU64 = AtomicU64::new(0);

thread_local! {
    /// Textures the operation being bound writes through unordered access.
    static UAVS: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
    /// The operation [`before`] numbered last on this thread, for the listing.
    static CURRENT: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Readies the reads for frame `frame` (when a pixel is picked).
pub(super) fn arm(frame: u64) {
    *PROBE.lock().unwrap_or_else(|e| e.into_inner()) = None;
    FRAME.store(u64::MAX, Relaxed);
    let Some(host) = crate::host::host() else { return };
    let heap = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_READBACK, ..Default::default() };
    let desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
        Width: u64::from(SLOTS) * SLOT_BYTES,
        Height: 1,
        DepthOrArraySize: 1,
        MipLevels: 1,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        ..Default::default()
    };
    let mut buffer: Option<ID3D12Resource> = None;
    // SAFETY: plain creation.
    let made = unsafe {
        host.device.CreateCommittedResource(
            &heap,
            D3D12_HEAP_FLAG_NONE,
            &desc,
            D3D12_RESOURCE_STATE_COPY_DEST,
            None,
            &mut buffer,
        )
    };
    let Some(buffer) = made.ok().and(buffer) else {
        tracing::warn!("debugger: no readback buffer for the pixel history");
        return;
    };
    *PROBE.lock().unwrap_or_else(|e| e.into_inner()) = Some(Probe { frame, buffer, samples: Vec::new(), next: 0 });
    FRAME.store(frame, Relaxed);
    OPS.store(0, Relaxed);
}

/// Whether the frame being built reads the pixel.
fn active() -> bool {
    FRAME.load(Relaxed) != u64::MAX && super::recording() && super::pick().is_some()
}

/// A texture the operation being bound writes through unordered access.
pub(crate) fn note_uav(resource: This) {
    if active() {
        UAVS.with(|u| {
            let mut u = u.borrow_mut();
            if !u.contains(&(resource as usize)) {
                u.push(resource as usize);
            }
        });
    }
}

/// The operation [`before`] last numbered on this thread (0: none), for the
/// frame's listing.
pub(crate) fn current_op() -> u64 {
    CURRENT.with(|c| c.replace(0))
}

/// What [`before`] read, for [`after`].
pub(crate) struct Token {
    op: u64,
    label: String,
    targets: Vec<(usize, u32)>,
}

/// Before an operation: reads the pixel from `targets` (title textures and
/// the host state each is in) and the textures it writes through unordered
/// access. None when the frame reads nothing.
pub(crate) fn before(list: &HostList, targets: &[(usize, u32)], label: impl FnOnce() -> String) -> Option<Token> {
    let uavs = UAVS.with(|u| std::mem::take(&mut *u.borrow_mut()));
    if !active() {
        return None;
    }
    const UNORDERED_ACCESS: u32 = 0x8;
    let mut all: Vec<(usize, u32)> = targets.to_vec();
    for u in uavs {
        if !all.iter().any(|t| t.0 == u) {
            all.push((u, UNORDERED_ACCESS));
        }
    }
    let op = OPS.fetch_add(1, Relaxed) + 1;
    CURRENT.with(|c| c.set(op));
    let label = label();
    for &(t, state) in &all {
        read(list, t, state, op, &label, false);
    }
    Some(Token { op, label, targets: all })
}

/// After the operation [`before`] read for.
pub(crate) fn after(list: &HostList, token: Option<Token>) {
    let Some(token) = token else { return };
    for &(t, state) in &token.targets {
        read(list, t, state, token.op, &token.label, true);
    }
}

/// Copies the picked texel of title texture `t` (in host state `state`).
fn read(list: &HostList, t: usize, state: u32, op: u64, label: &str, after: bool) {
    const COPY_SOURCE: u32 = 0x800;
    let Some(pick) = super::pick() else { return };
    let Some(texture) = crate::resource::host_resource(t as This) else { return };
    // SAFETY: a live resource.
    let d = unsafe { texture.GetDesc() };
    if d.Dimension != D3D12_RESOURCE_DIMENSION_TEXTURE2D
        || d.SampleDesc.Count > 1
        || d.Flags.0 & D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL.0 != 0
        || d.Width == 0
        || d.Height == 0
    {
        return;
    }
    let x = ((pick.0 * d.Width as f32) as u32).min(d.Width as u32 - 1);
    let y = ((pick.1 * d.Height as f32) as u32).min(d.Height - 1);
    let mut guard = PROBE.lock().unwrap_or_else(|e| e.into_inner());
    let Some(probe) = guard.as_mut() else { return };
    if probe.next >= SLOTS {
        return;
    }
    let slot = probe.next;
    probe.next += 1;
    crate::list::ensure_state(list, t as This, COPY_SOURCE);
    let to = D3D12_TEXTURE_COPY_LOCATION {
        // SAFETY: a borrowed pointer the location never releases.
        pResource: unsafe { std::mem::transmute_copy(&probe.buffer) },
        Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
            PlacedFootprint: D3D12_PLACED_SUBRESOURCE_FOOTPRINT {
                Offset: u64::from(slot) * SLOT_BYTES,
                Footprint: D3D12_SUBRESOURCE_FOOTPRINT {
                    Format: d.Format,
                    Width: 1,
                    Height: 1,
                    Depth: 1,
                    RowPitch: 256,
                },
            },
        },
    };
    let from = D3D12_TEXTURE_COPY_LOCATION {
        // SAFETY: as above.
        pResource: unsafe { std::mem::transmute_copy(texture) },
        Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: 0 },
    };
    let area = D3D12_BOX { left: x, top: y, front: 0, right: x + 1, bottom: y + 1, back: 1 };
    // SAFETY: an open list; live resources; one texel inside the texture.
    unsafe { list.list.CopyTextureRegion(&to, 0, 0, 0, &from, Some(&area)) };
    crate::list::ensure_state(list, t as This, state);
    list.keep(&probe.buffer);
    probe.samples.push(Sample {
        op,
        label: label.to_owned(),
        after,
        target: t,
        format: d.Format.0 as u32,
        size: (d.Width, d.Height),
        texel: (x, y),
        slot,
        bytes: Vec::new(),
    });
}

/// The samples of the recorded frame with their bytes (the frame has run).
pub(super) fn finish() -> Vec<Sample> {
    FRAME.store(u64::MAX, Relaxed);
    let Some(probe) = PROBE.lock().unwrap_or_else(|e| e.into_inner()).take() else {
        return Vec::new();
    };
    let mut samples = probe.samples;
    let mut data = std::ptr::null_mut();
    // SAFETY: the copies have run; the mapping spans the buffer.
    unsafe {
        if probe.buffer.Map(0, None, Some(&mut data)).is_err() {
            return samples;
        }
        let all = std::slice::from_raw_parts(data.cast::<u8>(), (u64::from(SLOTS) * SLOT_BYTES) as usize);
        for s in &mut samples {
            let at = (u64::from(s.slot) * SLOT_BYTES) as usize;
            let n = texel_bytes(s.format).unwrap_or(16);
            s.bytes = all[at..at + n].to_vec();
        }
        probe.buffer.Unmap(0, None);
    }
    let _ = probe.frame;
    samples.sort_by_key(|s| (s.op, s.after, s.target));
    samples
}

/// Bytes of one texel of `format` (DXGI numbering), for the formats a target
/// or an unordered-access texture has.
pub(crate) fn texel_bytes(format: u32) -> Option<usize> {
    Some(match format {
        1..=4 => 16,
        5..=8 => 12,
        9..=22 => 8,
        23..=47 => 4,
        48..=59 => 2,
        60..=65 => 1,
        85..=93 => if format <= 86 { 2 } else { 4 },
        115 => 2,
        _ => return None,
    })
}

fn half(h: u16) -> f32 {
    let s = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
    let e = i32::from(h >> 10 & 0x1f);
    let m = f32::from(h & 0x3ff);
    s * match e {
        0 => m * 2f32.powi(-24),
        31 => if m == 0.0 { f32::INFINITY } else { f32::NAN },
        e => (1.0 + m / 1024.0) * 2f32.powi(e - 15),
    }
}

fn small(bits: u32, man: u32) -> f32 {
    let e = (bits >> man) as i32;
    let m = (bits & ((1 << man) - 1)) as f32;
    let scale = (1u32 << man) as f32;
    match e {
        0 => m / scale * 2f32.powi(-14),
        31 => f32::INFINITY,
        e => (1.0 + m / scale) * 2f32.powi(e - 15),
    }
}

/// A texel's channels as numbers (for the report), from its bytes.
pub(crate) fn decode(format: u32, b: &[u8]) -> Vec<f32> {
    let u16_at = |i: usize| b.get(2 * i..2 * i + 2).map_or(0, |s| u16::from_le_bytes([s[0], s[1]]));
    let u32_at = |i: usize| b.get(4 * i..4 * i + 4).map_or(0, |s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]));
    let unorm8 = |i: usize| f32::from(b.get(i).copied().unwrap_or(0)) / 255.0;
    match format {
        1 | 2 => (0..4).map(|i| f32::from_bits(u32_at(i))).collect(),
        3 => (0..4).map(|i| u32_at(i) as f32).collect(),
        4 => (0..4).map(|i| u32_at(i) as i32 as f32).collect(),
        5 | 6 => (0..3).map(|i| f32::from_bits(u32_at(i))).collect(),
        9 | 10 => (0..4).map(|i| half(u16_at(i))).collect(),
        11 => (0..4).map(|i| f32::from(u16_at(i)) / 65535.0).collect(),
        12 => (0..4).map(|i| f32::from(u16_at(i))).collect(),
        15 | 16 => (0..2).map(|i| f32::from_bits(u32_at(i))).collect(),
        17 => (0..2).map(|i| u32_at(i) as f32).collect(),
        23 | 24 => {
            let v = u32_at(0);
            vec![(v & 0x3ff) as f32 / 1023.0, (v >> 10 & 0x3ff) as f32 / 1023.0, (v >> 20 & 0x3ff) as f32 / 1023.0, (v >> 30) as f32 / 3.0]
        }
        25 => {
            let v = u32_at(0);
            vec![(v & 0x3ff) as f32, (v >> 10 & 0x3ff) as f32, (v >> 20 & 0x3ff) as f32, (v >> 30) as f32]
        }
        26 => {
            let v = u32_at(0);
            vec![small(v & 0x7ff, 6), small(v >> 11 & 0x7ff, 6), small(v >> 22, 5)]
        }
        27..=29 => (0..4).map(unorm8).collect(),
        30 => (0..4).map(|i| f32::from(b.get(i).copied().unwrap_or(0))).collect(),
        33 | 34 => (0..2).map(|i| half(u16_at(i))).collect(),
        35 => (0..2).map(|i| f32::from(u16_at(i)) / 65535.0).collect(),
        36 => (0..2).map(|i| f32::from(u16_at(i))).collect(),
        39 | 40 | 41 => vec![f32::from_bits(u32_at(0))],
        42 => vec![u32_at(0) as f32],
        43 => vec![u32_at(0) as i32 as f32],
        48 | 49 => (0..2).map(unorm8).collect(),
        50 => (0..2).map(|i| f32::from(b.get(i).copied().unwrap_or(0))).collect(),
        53 | 54 => vec![half(u16_at(0))],
        56 => vec![f32::from(u16_at(0)) / 65535.0],
        57 => vec![f32::from(u16_at(0))],
        61 => vec![unorm8(0)],
        62 => vec![f32::from(b.first().copied().unwrap_or(0))],
        67 => {
            // R9G9B9E5
            let v = u32_at(0);
            let e = 2f32.powi((v >> 27) as i32 - 15 - 9);
            vec![(v & 0x1ff) as f32 * e, (v >> 9 & 0x1ff) as f32 * e, (v >> 18 & 0x1ff) as f32 * e]
        }
        87 | 88 | 90 | 91 => vec![unorm8(2), unorm8(1), unorm8(0), unorm8(3)],
        _ => Vec::new(),
    }
}
