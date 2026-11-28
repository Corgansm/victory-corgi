//! The title's own calls into the console's ray-tracing structure builder
//! (`XboxBvhBuilder.dll`), seen from the layer: the native builder's front
//! end (`C:\Users\Corgan\victory-mcp\DXR-PLAN.md`, plan #2).
//!
//! The builder is a DLL the title ships. Its exports are D3D12's own entry
//! points as members of an `XboxBvhBuilder` object: the title hands it the
//! very `D3D12_BUILD_RAYTRACING_ACCELERATION_STRUCTURE_DESC` a PC title hands
//! a command list (destination, inputs: type, flags, count, geometry
//! descriptions or the address of the instances; source; scratch), and the
//! DLL records its 34 compute programs on the list. So the layer can know
//! every build as the title meant it, instead of decoding the programs'
//! arguments.
//!
//! `VICTORY_BVH_HOOK=1` puts `victory-hle`'s wrappers in front of the
//! exports (`victory_hle::modules::bvh`). For now they report here and call
//! the DLL as before ([`Calls`]): what the title asks, how often, and of what
//! size, for the native builder that will answer the builds itself.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

/// Whether the wrappers are in front of the builder's exports
/// (`VICTORY_BVH_HOOK=1`).
pub fn on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_BVH_HOOK").is_some_and(|v| v == "1"))
}

/// D3D12_RAYTRACING_ACCELERATION_STRUCTURE_TYPE_TOP_LEVEL.
pub const TOP: u32 = 0;
/// D3D12_RAYTRACING_ACCELERATION_STRUCTURE_BUILD_FLAG_PERFORM_UPDATE.
pub const UPDATE: u32 = 0x20;
/// D3D12_ELEMENTS_LAYOUT_ARRAY_OF_POINTERS.
pub const POINTERS: u32 = 1;
/// Bytes of a `D3D12_BUILD_RAYTRACING_ACCELERATION_STRUCTURE_DESC`.
pub const DESC: usize = 48;
/// Bytes of a `D3D12_RAYTRACING_GEOMETRY_DESC`.
pub const GEOMETRY: usize = 56;
/// Bytes of a `D3D12_RAYTRACING_INSTANCE_DESC`.
pub const INSTANCE: usize = 64;

/// A `D3D12_BUILD_RAYTRACING_ACCELERATION_STRUCTURE_DESC` as the title wrote it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Build {
    pub dest: u64,
    pub kind: u32,
    pub flags: u32,
    pub count: u32,
    pub layout: u32,
    /// The instances' address (a top level), or the geometry descriptions'
    /// (an array, or an array of pointers to them).
    pub descs: u64,
    pub source: u64,
    pub scratch: u64,
}

impl Build {
    pub fn parse(b: &[u8; DESC]) -> Build {
        let q = |at: usize| u64::from_le_bytes(b[at..at + 8].try_into().unwrap());
        let d = |at: usize| u32::from_le_bytes(b[at..at + 4].try_into().unwrap());
        Build {
            dest: q(0),
            kind: d(8),
            flags: d(12),
            count: d(16),
            layout: d(20),
            descs: q(24),
            source: q(32),
            scratch: q(40),
        }
    }
}

/// One `D3D12_RAYTRACING_GEOMETRY_DESC`: its type (0 triangles, 1 boxes),
/// flags, and for triangles the description `crate::records` reads (the
/// same 48 bytes the builder's programs are given).
pub fn geometry(b: &[u8; GEOMETRY]) -> (u32, u32, Option<crate::records::Geometry>) {
    let kind = u32::from_le_bytes(b[0..4].try_into().unwrap());
    let flags = u32::from_le_bytes(b[4..8].try_into().unwrap());
    let triangles = (kind == 0).then(|| crate::records::Geometry::parse(b[8..56].try_into().unwrap())).flatten();
    (kind, flags, triangles)
}

/// One `D3D12_RAYTRACING_INSTANCE_DESC`: transform, id, mask, flags, the
/// bottom level's address.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Instance {
    pub transform: [f32; 12],
    pub id: u32,
    pub mask: u8,
    pub hit_group: u32,
    pub flags: u8,
    pub bottom: u64,
}

impl Instance {
    pub fn parse(b: &[u8; INSTANCE]) -> Instance {
        let d = |at: usize| u32::from_le_bytes(b[at..at + 4].try_into().unwrap());
        Instance {
            transform: std::array::from_fn(|k| f32::from_bits(d(4 * k))),
            id: d(48) & 0xff_ffff,
            mask: (d(48) >> 24) as u8,
            hit_group: d(52) & 0xff_ffff,
            flags: (d(52) >> 24) as u8,
            bottom: u64::from_le_bytes(b[56..64].try_into().unwrap()),
        }
    }
}

/// Reads title memory (the CPU's view; what is not there reads as nothing).
#[cfg(windows)]
fn read<const N: usize>(address: u64) -> Option<[u8; N]> {
    let mut b = [0u8; N];
    (address != 0 && crate::host::read_there(address as *const u8, &mut b) == N).then_some(b)
}

#[cfg(not(windows))]
fn read<const N: usize>(_: u64) -> Option<[u8; N]> {
    None
}

/// What the title asked since the last report.
#[derive(Default)]
struct Calls {
    since: Option<std::time::Instant>,
    frame: u64,
    /// Builds of bottom levels, of top levels; of them updates (refits).
    bottoms: u64,
    tops: u64,
    updates: u64,
    geometries: u64,
    triangles: u64,
    boxes: u64,
    instances: u64,
    /// Of the geometries: packed indices, packed vertices (the console's
    /// formats, `crate::records`), with a transform.
    packed_indices: u64,
    packed_vertices: u64,
    transformed: u64,
    postbuilds: u64,
    batched: u64,
    batched_descs: u64,
    rehydrations: u64,
    rehydrated: u64,
    copies: [u64; 4],
    prebuilds: u64,
    /// Destinations seen (all of the run).
    dests: std::collections::HashSet<u64>,
    /// Lines of detail written, by what they are of.
    detailed: std::collections::HashMap<&'static str, u32>,
}

static CALLS: Mutex<Option<Calls>> = Mutex::new(None);

/// Lines of detail written, of each call; the rest are counted.
const DETAIL: u32 = 120;

fn with(f: impl FnOnce(&mut Calls)) {
    let mut guard = CALLS.lock().unwrap_or_else(|e| e.into_inner());
    let c = guard.get_or_insert_with(Calls::default);
    f(c);
    c.report();
}

impl Calls {
    fn report(&mut self) {
        let now = std::time::Instant::now();
        let frame = crate::frame::current();
        let Some(since) = self.since else {
            (self.since, self.frame) = (Some(now), frame);
            return;
        };
        if now.duration_since(since).as_secs_f64() < 10.0 {
            return;
        }
        let frames = frame.wrapping_sub(self.frame).max(1) as f64;
        tracing::info!(
            "the title's ray-tracing builds (VICTORY_BVH_HOOK), per frame over {frames:.0} frames: {:.1} bottom levels ({:.1} geometries, {:.0} triangles, {:.1} box geometries; {:.1} packed indices, {:.1} packed vertices, {:.1} with a transform), \
             {:.2} top levels ({:.0} instances), {:.1} updates, {:.1} post-build infos; {:.2} batched calls ({:.1} descriptions), {:.2} rehydrations ({:.1} structures); \
             copies {:.2} clone, {:.2} compact, {:.2} other; {:.1} prebuild infos; {} destinations in all",
            self.bottoms as f64 / frames,
            self.geometries as f64 / frames,
            self.triangles as f64 / frames,
            self.boxes as f64 / frames,
            self.packed_indices as f64 / frames,
            self.packed_vertices as f64 / frames,
            self.transformed as f64 / frames,
            self.tops as f64 / frames,
            self.instances as f64 / frames,
            self.updates as f64 / frames,
            self.postbuilds as f64 / frames,
            self.batched as f64 / frames,
            self.batched_descs as f64 / frames,
            self.rehydrations as f64 / frames,
            self.rehydrated as f64 / frames,
            self.copies[0] as f64 / frames,
            self.copies[1] as f64 / frames,
            (self.copies[2] + self.copies[3]) as f64 / frames,
            self.prebuilds as f64 / frames,
            self.dests.len(),
        );
        #[cfg(feature = "diagnostics")]
        last::dump_when_due(frame);
        let (dests, detailed) = (std::mem::take(&mut self.dests), std::mem::take(&mut self.detailed));
        *self = Calls { since: Some(now), frame, dests, detailed, ..Calls::default() };
    }

    /// One build: counted, and the first ones written out.
    fn build(&mut self, b: &Build, what: &'static str) {
        self.dests.insert(b.dest);
        #[cfg(feature = "diagnostics")]
        last::note(b);
        if b.flags & UPDATE != 0 {
            self.updates += 1;
        }
        let detail = self.detail(if b.kind == TOP { "top level" } else { what });
        let mut text = if detail { format!("; destination {}", held(b.dest)) } else { String::new() };
        if b.kind == TOP {
            self.tops += 1;
            self.instances += u64::from(b.count);
            if detail {
                // (The instances are in title memory, by address.)
                let first = read::<INSTANCE>(b.descs).map(|i| Instance::parse(&i));
                let last = (b.count > 1).then(|| read::<INSTANCE>(b.descs + INSTANCE as u64 * u64::from(b.count - 1)).map(|i| Instance::parse(&i))).flatten();
                text += &format!("; instances {}; first instance {first:?}; last {last:?}", held(b.descs));
                if let Some(i) = first {
                    text += &format!("; its bottom level {}", held(i.bottom));
                }
            }
        } else {
            self.bottoms += 1;
            self.geometries += u64::from(b.count);
            for n in 0..u64::from(b.count.min(4096)) {
                let at = if b.layout == POINTERS {
                    match read::<8>(b.descs + 8 * n) {
                        Some(p) => u64::from_le_bytes(p),
                        None => break,
                    }
                } else {
                    b.descs + GEOMETRY as u64 * n
                };
                let Some(g) = read::<GEOMETRY>(at) else { break };
                let (kind, flags, triangles) = geometry(&g);
                match triangles {
                    Some(t) => {
                        self.triangles += u64::from(t.triangles());
                        self.packed_indices += u64::from(t.index_format == crate::records::PACKED_INDICES);
                        self.packed_vertices += u64::from(t.vertex_format == crate::records::PACKED_VERTICES);
                        self.transformed += u64::from(t.transform != 0);
                    }
                    None => self.boxes += u64::from(kind == 1),
                }
                if detail && n < 3 {
                    text += &format!("; geometry {n}: kind {kind} flags {flags:#x} {triangles:?}");
                    if let Some(t) = triangles {
                        // (Written by the CPU, or by the card: a vertex buffer
                        // the title's programs make is not in CPU memory.)
                        text += &format!(" vertices {}", held(t.vertex_buffer));
                        if t.vertex_format == crate::records::VERTICES_FLOAT3 {
                            let v = read::<12>(t.vertex_buffer).map(|b| std::array::from_fn::<f32, 3, _>(|k| f32::from_le_bytes(b[4 * k..4 * k + 4].try_into().unwrap())));
                            text += &format!(" first vertex {v:?}");
                        }
                        if t.index_buffer != 0 {
                            text += &format!(" indices {}", held(t.index_buffer));
                        }
                    }
                }
            }
        }
        if detail {
            tracing::info!("{what}: {b:x?}{text}");
        }
    }

    /// Whether another line of detail of `what` is written (and counted).
    fn detail(&mut self, what: &'static str) -> bool {
        let n = self.detailed.entry(what).or_default();
        *n += 1;
        *n <= DETAIL
    }
}

/// `BuildRaytracingAccelerationStructure`: `desc` the title's description,
/// `posts` post-build info descriptions after it.
pub fn build(desc: *const u8, posts: u32) {
    let Some(b) = read::<DESC>(desc as u64) else { return };
    with(|c| {
        c.postbuilds += u64::from(posts);
        c.build(&Build::parse(&b), "build");
    });
}

/// Bytes of a `D3D12XBOX_BUILD_RAYTRACING_ACCELERATION_STRUCTURE_BATCHED_DESC2`
/// (run 20261127-180216's bytes): the build description, then the number
/// of post-build info descriptions (dword 12) and their address (13-14),
/// then 16 bytes of zeros.
pub const BATCHED: usize = 80;

/// `BuildRaytracingAccelerationStructureBatched2X`: `count` builds.
pub fn batched(count: u32, descs: *const u8) {
    with(|c| {
        c.batched += 1;
        c.batched_descs += u64::from(count);
        for n in 0..u64::from(count.min(4096)) {
            let at = descs as u64 + BATCHED as u64 * n;
            let Some(d) = read::<BATCHED>(at) else { break };
            let b = Build::parse(d[..DESC].try_into().unwrap());
            let posts = u32::from_le_bytes(d[48..52].try_into().unwrap());
            c.postbuilds += u64::from(posts);
            if posts > 0 && c.detailed.get("post-build").copied().unwrap_or(0) < DETAIL {
                *c.detailed.entry("post-build").or_default() += 1;
                let post = u64::from_le_bytes(d[56..64].try_into().unwrap());
                let info = read::<16>(post).map(|p| (u64::from_le_bytes(p[0..8].try_into().unwrap()), u32::from_le_bytes(p[8..12].try_into().unwrap())));
                let at = info.map_or(String::new(), |(a, _)| held(a));
                tracing::info!(posts, "batched build {n}: post-build info (destination, type) {info:x?} {at}; tail {:x?}", &d[64..80]);
            }
            c.build(&b, "batched build");
        }
    });
}

/// Where a title address is held: in a title buffer (its start and length),
/// and whether the CPU's copy of its first bytes is all zeros.
#[cfg(windows)]
fn held(address: u64) -> String {
    let buffer = crate::shadow::in_buffer(address, 1);
    let zeros = read::<64>(address).map(|b| b.iter().all(|&x| x == 0));
    format!("in buffer {buffer:x?}, CPU's first 64 bytes zero {zeros:?}")
}

#[cfg(not(windows))]
fn held(_: u64) -> String {
    String::new()
}

/// The `Rehydrate...X` calls: `count` build descriptions (the static world's
/// packed structures, unpacked in place).
pub fn rehydrate(count: u32, descs: *const u8, what: &'static str) {
    with(|c| {
        c.rehydrations += 1;
        c.rehydrated += u64::from(count);
        for n in 0..u64::from(count.min(65536)) {
            let Some(b) = read::<DESC>(descs as u64 + DESC as u64 * n) else { break };
            let b = Build::parse(&b);
            // (Counted as builds too: their geometry is what a native builder makes.)
            c.build(&b, what);
        }
    });
}

/// `CopyRaytracingAccelerationStructure`.
pub fn copy(dest: u64, source: u64, mode: u32) {
    with(|c| {
        c.copies[(mode as usize).min(3)] += 1;
        c.dests.insert(dest);
        if c.detail("copy") {
            tracing::info!(dest = format_args!("{dest:#x}"), source = format_args!("{source:#x}"), mode, "copy");
        }
    });
}

/// `GetRaytracingAccelerationStructurePrebuildInfo`.
pub fn prebuild() {
    with(|c| c.prebuilds += 1);
}

/// How much of the builder the layer does itself (`VICTORY_BVH_NATIVE`):
/// `static` the static world's unpacking ([`rehydrate_native`]; the rest is
/// still the DLL's), `all` every call (not yet).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Native {
    Off,
    Static,
}

/// [`Native`] of this run.
pub fn native() -> Native {
    static N: std::sync::OnceLock<Native> = std::sync::OnceLock::new();
    *N.get_or_init(|| match std::env::var("VICTORY_BVH_NATIVE").as_deref() {
        Ok("static") => Native::Static,
        _ => Native::Off,
    })
}

/// Title memory read once a span ([`crate::records::Geometry::spans`]), the
/// decoding's small reads served from it: one kernel read a buffer, not one
/// a vertex.
#[cfg(windows)]
#[derive(Default)]
struct Spans {
    pieces: Vec<(u64, Vec<u8>)>,
}

#[cfg(windows)]
impl Spans {
    fn take(&mut self, start: u64, len: u64) {
        if len == 0 || len > 64 << 20 || self.pieces.iter().any(|(s, b)| *s <= start && start + len <= s + b.len() as u64) {
            return;
        }
        let mut bytes = vec![0u8; len as usize];
        if crate::host::read_there(start as *const u8, &mut bytes) == bytes.len() {
            self.pieces.push((start, bytes));
        }
    }
}

#[cfg(windows)]
impl crate::records::Read for Spans {
    fn read(&mut self, address: u64, bytes: &mut [u8]) -> bool {
        let n = bytes.len() as u64;
        for (start, piece) in &self.pieces {
            if *start <= address && address + n <= start + piece.len() as u64 {
                let at = (address - start) as usize;
                bytes.copy_from_slice(&piece[at..at + bytes.len()]);
                return true;
            }
        }
        crate::host::read_there(address as *const u8, bytes) == bytes.len()
    }
}

/// A static structure's header (64 bytes, `fork-ray-tracing.md`): version 6,
/// marks in the high half (0x8000 bottom level, 0x100 still packed).
pub const PACKED: u32 = 0x0100_0000;

/// How the static world's element nodes place a geometry's points
/// (LeafRehydration's constants, as the DLL works them out from the header;
/// 300 of 300 dispatches of capture run 20261127-173215): scaled by a power
/// of two per axis (header dword 15, bytes 0-2, as a float's exponent);
/// with byte 3's bit 0x40, less the bounds' centre on each axis whose
/// extreme, scaled, passes 65,536. No rotation in any.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Space {
    pub scale: [f32; 3],
    pub origin: [f32; 3],
}

impl Space {
    pub fn of(header: &[u32; 16]) -> Space {
        let scale: [f32; 3] = std::array::from_fn(|k| f32::from_bits((header[15] >> (8 * k) & 0xff) << 23));
        let lo: [f32; 3] = std::array::from_fn(|k| f32::from_bits(header[4 + k]));
        let hi: [f32; 3] = std::array::from_fn(|k| f32::from_bits(header[7 + k]));
        let centred = header[15] >> 24 & 0x40 != 0;
        let origin = std::array::from_fn(|k| {
            if centred && lo[k].abs().max(hi[k].abs()) * scale[k] > 65_536.0 { (lo[k] + hi[k]) * 0.5 } else { 0.0 }
        });
        Space { scale, origin }
    }

    pub fn place(&self, p: [f32; 3]) -> [f32; 3] {
        std::array::from_fn(|k| (p[k] - self.origin[k]) * self.scale[k])
    }
}

/// One element node as LeafRehydration writes it (dumps of runs 171218 and
/// 173215, every flags value): triangle 0's corners at the node's vertices
/// the low nibble of the flags names (`victory_shader::bvh::triangle_vertices`:
/// the one neither barycentric weighs, then I's, then J's, of vertices 0 1 2),
/// the pair's second triangle by the high nibble (of vertices 1 3 2); vertex
/// 3 of a single triangle NaN; dword 12 geometry << 8 | kind, 13 the
/// primitive, 14 the second (all ones for none), 15 the two id bytes.
pub fn element(e: &crate::records::Entry, first: [[f32; 3]; 3], second: Option<[[f32; 3]; 3]>) -> [u32; 16] {
    let mut w = [0u32; 16];
    let nan = f32::NAN.to_bits();
    for d in &mut w[9..12] {
        *d = nan;
    }
    let mut put = |map: [usize; 3], id: u32, t: [[f32; 3]; 3]| {
        let (i, j) = ((id & 3) as usize, (id >> 2 & 3) as usize);
        let f = (3usize.wrapping_sub(i).wrapping_sub(j)) & 3;
        for (n, slot) in [f, i, j].into_iter().enumerate() {
            let v = map[slot.min(2)];
            for c in 0..3 {
                w[3 * v + c] = t[n][c].to_bits();
            }
        }
    };
    let (low, high) = (u32::from(e.flags & 0xf), u32::from(e.flags >> 4));
    put([0, 1, 2], low, first);
    if let Some(t) = second {
        put([1, 3, 2], high, t);
    }
    w[12] = e.geometry << 8 | u32::from(e.kind);
    w[13] = e.primitive;
    w[14] = e.second.unwrap_or(0xffff_ffff);
    w[15] = low | if second.is_some() { high << 8 } else { 0 };
    w
}

/// What the native unpacking did since the last line.
#[derive(Default)]
struct Unpacked {
    since: Option<std::time::Instant>,
    structures: u64,
    nodes: u64,
    triangles: u64,
    missing: u64,
    skipped: u64,
    micros: u64,
}

static UNPACKED: Mutex<Option<Unpacked>> = Mutex::new(None);

/// The `Rehydrate...X` calls done by the layer (`VICTORY_BVH_NATIVE=static`):
/// each packed structure unpacked in place on the CPU, as the DLL's four
/// programs would on the card: the header's packed mark cleared, the element
/// nodes written from the packed entries and the title's own geometry
/// (`crate::records`), the root an empty box (the title's own walk of a
/// structure the host has not built finds nothing, where the packed boxes
/// were). The box tree is not made: the host's structures answer the walks.
#[cfg(windows)]
pub fn rehydrate_native(count: u32, descs: *const u8) -> i32 {
    let started = std::time::Instant::now();
    let mut done = Unpacked::default();
    for n in 0..u64::from(count.min(65536)) {
        let Some(b) = read::<DESC>(descs as u64 + DESC as u64 * n) else { break };
        unpack(&Build::parse(&b), &mut done);
    }
    let mut guard = UNPACKED.lock().unwrap_or_else(|e| e.into_inner());
    let u = guard.get_or_insert_with(Unpacked::default);
    u.structures += done.structures;
    u.nodes += done.nodes;
    u.triangles += done.triangles;
    u.missing += done.missing;
    u.skipped += done.skipped;
    u.micros += started.elapsed().as_micros() as u64;
    let now = std::time::Instant::now();
    if u.since.is_none_or(|s| now.duration_since(s).as_secs_f64() >= 10.0) {
        tracing::info!(
            structures = u.structures,
            nodes = u.nodes,
            triangles = u.triangles,
            unreadable = u.missing,
            skipped = u.skipped,
            ms = u.micros / 1000,
            "VICTORY_BVH_NATIVE: static structures unpacked by the layer since the last line"
        );
        *u = Unpacked { since: Some(now), ..Unpacked::default() };
    }
    0
}

#[cfg(windows)]
fn unpack(b: &Build, done: &mut Unpacked) {
    let Some(h) = read::<64>(b.dest) else {
        done.skipped += 1;
        return;
    };
    let header: [u32; 16] = std::array::from_fn(|k| u32::from_le_bytes(h[4 * k..4 * k + 4].try_into().unwrap()));
    if header[0] & 0xffff != 6 || header[0] & PACKED == 0 || b.kind == TOP {
        done.skipped += 1;
        return;
    }
    let (count, first) = (header[3] as usize, u64::from(header[13]));
    let size = (u64::from(header[1]) + 1) * 64;
    if count == 0 || first + 64 * count as u64 > size || size > 256 << 20 {
        done.skipped += 1;
        return;
    }
    // The geometry (the title's descriptions, on the CPU) and what it names.
    let mut spans = Spans::default();
    let mut geometries = Vec::with_capacity(b.count as usize);
    for n in 0..u64::from(b.count.min(4096)) {
        let at = if b.layout == POINTERS {
            read::<8>(b.descs + 8 * n).map_or(0, u64::from_le_bytes)
        } else {
            b.descs + GEOMETRY as u64 * n
        };
        let g = read::<GEOMETRY>(at).and_then(|g| geometry(&g).2);
        if let Some(g) = g {
            for (start, len) in g.spans(&mut spans) {
                spans.take(start, len);
            }
        }
        geometries.push(g);
    }
    // The entries of every group first: the nodes are written over them.
    let mut entries = Vec::with_capacity(count);
    for g in 0..count.div_ceil(crate::records::GROUP as usize) {
        let n = (count - g * crate::records::GROUP as usize).min(crate::records::GROUP as usize);
        let mut block = vec![0u8; 11 * n];
        let at = b.dest + first + ((g as u64) << 17);
        if crate::host::read_there(at as *const u8, &mut block) != block.len() {
            done.skipped += 1;
            return;
        }
        entries.extend(crate::records::entries(&block, n).unwrap_or_default());
    }
    let space = Space::of(&header);
    let mut nodes = vec![0u8; 64 * count];
    for (i, e) in entries.iter().enumerate() {
        let triangle = |p: u32, spans: &mut Spans| -> Option<[[f32; 3]; 3]> {
            let g = geometries.get(e.geometry as usize).copied().flatten()?;
            g.triangle(p, spans).ok().map(|t| t.map(|v| space.place(v)))
        };
        let (a, second) = (triangle(e.primitive, &mut spans), e.second.map(|p| triangle(p, &mut spans)));
        let nan = [[f32::NAN; 3]; 3];
        if a.is_none() || second.is_some_and(|s| s.is_none()) {
            done.missing += 1;
        }
        let w = element(e, a.unwrap_or(nan), second.map(|s| s.unwrap_or(nan)));
        for (k, d) in w.iter().enumerate() {
            nodes[64 * i + 4 * k..64 * i + 4 * k + 4].copy_from_slice(&d.to_le_bytes());
        }
        done.triangles += 1 + u64::from(e.second.is_some());
    }
    // The root: an empty box (children none), where the packed boxes were.
    let root = b.dest + ((u64::from(header[12]) >> 3) << 6);
    let mut empty = [0u8; 128];
    for k in 0..4 {
        empty[4 * k..4 * k + 4].copy_from_slice(&victory_shader::bvh::NONE.to_le_bytes());
    }
    let header0 = (header[0] & !PACKED).to_le_bytes();
    crate::host::copy_mapped((b.dest + first) as *mut u8, nodes.as_ptr(), nodes.len(), true);
    if root + 128 <= b.dest + size && !(b.dest + first..b.dest + first + nodes.len() as u64).contains(&root) {
        crate::host::copy_mapped(root as *mut u8, empty.as_ptr(), empty.len(), true);
    }
    crate::host::copy_mapped(b.dest as *mut u8, header0.as_ptr(), 4, true);
    crate::memory::title_wrote(b.dest, size);
    done.structures += 1;
    done.nodes += count as u64;
}

/// `VICTORY_BVH_HOOK_DUMP=<frames>` with `VICTORY_MEMORY_DUMP=<folder>`
/// (diagnostics): that many frames after the first build, the last build of
/// each destination is written to the dump folder (`builds.txt`: each top
/// level's instance descriptions, each bottom level's geometry) and the
/// arenas dumped (`go`), so the console's own structures can be compared
/// with what they were built from.
#[cfg(feature = "diagnostics")]
mod last {
    use super::*;
    use std::collections::HashMap;
    use std::fmt::Write as _;

    struct Last {
        first: Option<u64>,
        done: bool,
        builds: HashMap<u64, (u64, Build, String)>,
    }

    static LAST: Mutex<Option<Last>> = Mutex::new(None);

    fn due() -> Option<u64> {
        static DUE: std::sync::OnceLock<Option<u64>> = std::sync::OnceLock::new();
        *DUE.get_or_init(|| std::env::var("VICTORY_BVH_HOOK_DUMP").ok().and_then(|v| v.parse().ok()))
    }

    pub(super) fn note(b: &Build) {
        if due().is_none() {
            return;
        }
        let frame = crate::frame::current();
        let mut guard = LAST.lock().unwrap_or_else(|e| e.into_inner());
        let l = guard.get_or_insert_with(|| Last { first: None, done: false, builds: HashMap::new() });
        if l.done {
            return;
        }
        l.first.get_or_insert(frame);
        let mut text = String::new();
        if b.kind == TOP {
            for k in 0..u64::from(b.count.min(8192)) {
                let Some(i) = read::<INSTANCE>(b.descs + INSTANCE as u64 * k) else { break };
                let words: Vec<String> = i.chunks_exact(4).map(|c| format!("{:08x}", u32::from_le_bytes(c.try_into().unwrap()))).collect();
                let _ = writeln!(text, "  instance {k} {}", words.join(" "));
            }
        } else {
            for n in 0..u64::from(b.count.min(4096)) {
                let at = if b.layout == POINTERS { read::<8>(b.descs + 8 * n).map_or(0, u64::from_le_bytes) } else { b.descs + GEOMETRY as u64 * n };
                let Some(g) = read::<GEOMETRY>(at) else { break };
                let _ = writeln!(text, "  geometry {n} {:?}", geometry(&g));
            }
        }
        l.builds.insert(b.dest, (frame, *b, text));
    }

    pub(super) fn dump_when_due(frame: u64) {
        let (Some(due), Some(folder)) = (due(), std::env::var_os("VICTORY_MEMORY_DUMP")) else { return };
        let mut guard = LAST.lock().unwrap_or_else(|e| e.into_inner());
        let Some(l) = guard.as_mut() else { return };
        if l.done || l.first.is_none_or(|f| frame < f + due) {
            return;
        }
        l.done = true;
        let folder = std::path::PathBuf::from(folder);
        let mut out = format!("frame {frame}\n");
        let mut all: Vec<_> = l.builds.iter().collect();
        all.sort_by_key(|(d, _)| **d);
        for (dest, (at, b, text)) in all {
            let _ = writeln!(out, "build {dest:#x} frame {at} {b:x?}");
            out += text;
        }
        let _ = std::fs::create_dir_all(&folder);
        let _ = std::fs::write(folder.join("builds.txt"), out);
        let _ = std::fs::write(folder.join("go"), "go");
        tracing::info!(builds = l.builds.len(), "VICTORY_BVH_HOOK_DUMP: the last builds written out, the arenas asked for");
    }
}

/// Said once: the wrappers could not find the DLL's own export.
pub fn missing(name: &str) {
    static SAID: AtomicBool = AtomicBool::new(false);
    if !SAID.swap(true, Ordering::Relaxed) {
        tracing::error!(name, "VICTORY_BVH_HOOK: the structure builder's own export is not known; the call is dropped");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_build_description_reads_in_d3d12s_layout() {
        let mut b = [0u8; DESC];
        b[0..8].copy_from_slice(&0x9_6362_4000u64.to_le_bytes());
        b[8..12].copy_from_slice(&1u32.to_le_bytes());
        b[12..16].copy_from_slice(&0x24u32.to_le_bytes());
        b[16..20].copy_from_slice(&3u32.to_le_bytes());
        b[24..32].copy_from_slice(&0x7ff0_1234_5678u64.to_le_bytes());
        b[40..48].copy_from_slice(&0xa_e6c0_e9e0u64.to_le_bytes());
        let p = Build::parse(&b);
        assert_eq!((p.dest, p.kind, p.flags, p.count, p.layout), (0x9_6362_4000, 1, 0x24, 3, 0));
        assert_eq!((p.descs, p.source, p.scratch), (0x7ff0_1234_5678, 0, 0xa_e6c0_e9e0));
    }

    #[test]
    fn an_instance_reads_in_d3d12s_layout() {
        let mut b = [0u8; INSTANCE];
        b[0..4].copy_from_slice(&1.0f32.to_bits().to_le_bytes());
        b[48..52].copy_from_slice(&(0xff00_0007u32).to_le_bytes());
        b[52..56].copy_from_slice(&(0x0400_0002u32).to_le_bytes());
        b[56..64].copy_from_slice(&0x9_6295_fc00u64.to_le_bytes());
        let i = Instance::parse(&b);
        assert_eq!((i.transform[0], i.id, i.mask, i.hit_group, i.flags, i.bottom), (1.0, 7, 0xff, 2, 4, 0x9_6295_fc00));
    }

    #[test]
    fn a_header_gives_the_element_nodes_space() {
        let f = f32::to_bits;
        let mut h = [0u32; 16];
        // Bounds (-0.2656, 0.0005, -0.6265) to (0.7832, 0.6626, 0.5981),
        // exponents 2^16 2^17 2^16, centred: y passes 65,536 scaled.
        [h[4], h[5], h[6], h[7], h[8], h[9]] = [f(-0.2656), f(0.0005), f(-0.6265), f(0.7832), f(0.6626), f(0.5981)];
        h[15] = 0xc08f_908f;
        let s = Space::of(&h);
        assert_eq!(s.scale, [65536.0, 131072.0, 65536.0]);
        assert_eq!(s.origin[0], 0.0);
        assert!((s.origin[1] - 0.33155).abs() < 1e-4);
        assert_eq!(s.origin[2], 0.0);
        h[15] = 0x808f_908f;
        assert_eq!(Space::of(&h).origin, [0.0; 3]);
        assert_eq!(Space::of(&h).place([1.0, 0.5, -1.0]), [65536.0, 65536.0, -65536.0]);
    }

    #[test]
    fn an_element_node_holds_the_pair_as_the_program_writes_it() {
        use crate::records::Entry;
        let (a0, a1, a2, b1) = ([1.0, 0.0, 0.0], [2.0, 0.0, 0.0], [3.0, 0.0, 0.0], [4.0, 0.0, 0.0]);
        // Flags 148 (dump: node a1, a2/b0, a0/b2, b1; id bytes 0x904): the
        // pair shares a2 = b0 and a0 = b2.
        let e = Entry { geometry: 3, kind: 0x81, primitive: 7, second: Some(8), flags: 148 };
        let w = element(&e, [a0, a1, a2], Some([a2, b1, a0]));
        let v = |k: usize| [f32::from_bits(w[3 * k]), f32::from_bits(w[3 * k + 1]), f32::from_bits(w[3 * k + 2])];
        assert_eq!([v(0), v(1), v(2), v(3)], [a1, a2, a0, b1]);
        assert_eq!(&w[12..], &[3 << 8 | 0x81, 7, 8, 0x904]);
        // And the walk's own reading of it gives the triangles back.
        let mut words = [0u32; 28];
        words[..16].copy_from_slice(&w);
        let t0 = victory_shader::bvh::triangle_vertices(&words, 0);
        let t1 = victory_shader::bvh::triangle_vertices(&words, 1);
        assert_eq!(t0, [a0, a1, a2]);
        assert_eq!(t1, [a2, b1, a0]);
        // A single triangle (flags 9): vertices 0 1 2, vertex 3 NaN, no second.
        let e = Entry { geometry: 0, kind: 1, primitive: 5, second: None, flags: 9 };
        let w = element(&e, [a0, a1, a2], None);
        assert_eq!(&w[..9], &[a0, a1, a2].concat().iter().map(|x: &f32| x.to_bits()).collect::<Vec<_>>()[..]);
        assert!(f32::from_bits(w[9]).is_nan());
        assert_eq!(&w[12..], &[1, 5, 0xffff_ffff, 0x9]);
    }

    #[test]
    fn a_geometry_description_carries_the_records_layout() {
        let mut b = [0u8; GEOMETRY];
        b[4..8].copy_from_slice(&1u32.to_le_bytes());
        b[16..20].copy_from_slice(&crate::records::PACKED_INDICES.to_le_bytes());
        b[20..24].copy_from_slice(&crate::records::PACKED_VERTICES.to_le_bytes());
        b[24..28].copy_from_slice(&300u32.to_le_bytes());
        b[40..48].copy_from_slice(&0x6_0000_0100u64.to_le_bytes());
        b[48..52].copy_from_slice(&8u32.to_le_bytes());
        let (kind, flags, t) = geometry(&b);
        let t = t.unwrap();
        assert_eq!((kind, flags, t.index_count, t.vertex_buffer, t.stride), (0, 1, 300, 0x6_0000_0100, 8));
    }
}
