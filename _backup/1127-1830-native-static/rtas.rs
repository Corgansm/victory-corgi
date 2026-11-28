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
    detailed: u32,
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
        let (dests, detailed) = (std::mem::take(&mut self.dests), self.detailed);
        *self = Calls { since: Some(now), frame, dests, detailed, ..Calls::default() };
    }

    /// One build: counted, and the first ones written out.
    fn build(&mut self, b: &Build, what: &str) {
        self.dests.insert(b.dest);
        if b.flags & UPDATE != 0 {
            self.updates += 1;
        }
        let detail = self.detailed < DETAIL;
        let mut text = String::new();
        if b.kind == TOP {
            self.tops += 1;
            self.instances += u64::from(b.count);
            if detail {
                // (The instances are in title memory, by address.)
                let first = read::<INSTANCE>(b.descs).map(|i| Instance::parse(&i));
                let last = (b.count > 1).then(|| read::<INSTANCE>(b.descs + INSTANCE as u64 * u64::from(b.count - 1)).map(|i| Instance::parse(&i))).flatten();
                text = format!("; first instance {first:?}; last {last:?}");
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
                }
            }
        }
        if detail {
            self.detailed += 1;
            tracing::info!("{what}: {b:x?}{text}");
        }
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

/// `BuildRaytracingAccelerationStructureBatched2X`: `count` descriptions of
/// the console's own layout (not known yet: their first bytes are written
/// out).
pub fn batched(count: u32, descs: *const u8) {
    with(|c| {
        c.batched += 1;
        c.batched_descs += u64::from(count);
        if c.detailed < DETAIL {
            c.detailed += 1;
            let first = read::<128>(descs as u64);
            tracing::info!(count, "batched build: the first description's bytes {:x?}", first.map(|b| b.to_vec()));
        }
    });
}

/// The `Rehydrate...X` calls: `count` build descriptions (the static world's
/// packed structures, unpacked in place).
pub fn rehydrate(count: u32, descs: *const u8, what: &str) {
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
        if c.detailed < DETAIL {
            c.detailed += 1;
            tracing::info!(dest = format_args!("{dest:#x}"), source = format_args!("{source:#x}"), mode, "copy");
        }
    });
}

/// `GetRaytracingAccelerationStructurePrebuildInfo`.
pub fn prebuild() {
    with(|c| c.prebuilds += 1);
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
