# Native DXR for VIctory-NVIDIA-HighRAM: the plan

## Progress (2026-11-26 16:30 Pacific)
- Step 1 done (offline, `bvh_flat`).
- Steps 2-3 done in the runtime (`crates/victory-gpu/src/memory.rs`, `mod dxr` and `Memory::dxr_*`, behind
  `VICTORY_DXR=1`, also the live menus' switch): top levels read back from the trace dispatches' views, bottom-level
  headers read back, host BLAS built from each plain bottom level's elements (copied from the arenas, the element pass
  `dxr::ELEMENTS_CS` on the card, NaN for absent triangles), host TLAS per top level (instance transform = inverse of
  dwords 0-11, id/mask from dword 12). The work goes on the trace dispatch's own list; `Ready::clobbered` makes
  `draw.rs` set the dispatch's root signature and pipeline again.
  - Run 20261126-160937 (low-spec, -RT, render 0.39): stable 5+ min, 199 BLAS (1.5M triangles, 103 MB), top level
    with 333 of 635 instances placed, 16-22 fps.
  - Run 20261126-160359 lost the device 2 min in: VRAM full (8.1/9.6 GB) and stale instance records naming
    "bottom levels" with billions of elements. Guards now: only marks 0x8002/0x8004, at most 65,536 elements, 512 MB
    kept, no building over 85% of the card's budget (most world frames are over it: VRAM is the limit).
- Step 4 next.

## Step 4, as it will be built (stateless guided traversal)
At an `image_bvh*_intersect_ray` whose descriptor base is a known structure and whose node is its root:
- top level root: one RayQuery (`RAY_FLAG_FORCE_OPAQUE`, mask 0xff) against our TLAS of it, the ray as given
  (world space); the result is a box result with one child: the committed instance's node pointer
  `((list offset + 128 * record) >> 3) | 6` (InstanceID of our desc = the record's index in the title's list, not the
  title's id), or all NONE for a miss.
- bottom level root: one RayQuery against a one-instance TLAS (identity) of our BLAS of it, the ray as given (the
  title's code has put it in object space); one child: the element's pointer `((element offset + 64 * (p / 4)) >> 3)
  | (p % 4)`, p the committed primitive index.
- anything else (triangle nodes, unknown structures, inner boxes): the emulated test as today. So the hit and its
  barycentrics are the title's own, bit for bit; no state is kept between node tests.
- The table on the card (a root SRV): open addressing by structure base address -> (heap slot of its RTAS SRV,
  root pointer, list or element offset), filled by `memory.rs` when it builds; AS SRVs placed in the bound host heap
  next to the memory views (`ready`'s run).
- Translation switch `VICTORY_DXR_GUIDED=1`, added to the cache key list in `shader_cache.rs`.

DXIL (from `dxc -T cs_6_6` of `D:\victory-data\snap\rq.hlsl`):
- `i32 @dx.op.allocateRayQuery(i32 178, i32 constRayFlags)`;
- `void @dx.op.rayQuery_TraceRayInline(i32 179, i32 rq, %dx.types.Handle tlas, i32 flags, i32 mask, float ox, oy, oz,
  float tmin, float dx, dy, dz, float tmax)`;
- `i1 @dx.op.rayQuery_Proceed.i1(i32 180, i32 rq)`;
- `i32 @dx.op.rayQuery_StateScalar.i32(i32 op, i32 rq)` (readonly): 184 CommittedStatus (1 = triangle hit),
  207 CommittedInstanceIndex, 208 CommittedInstanceID, 210 CommittedPrimitiveIndex; `.f32` 200 CommittedRayT;
- the TLAS handle: `createHandleFromHeap` then `annotateHandle` props `{16, 0}`;
- shader flags: + bit 25 (0x2000000, ray query / raytracing tier 1.1) with bit 30 (heap indexing).

Status 2026-11-26 16:55 (Chicago). Proven offline (`crates/victory-gpu/src/bvh_flat.rs`, tests `bvh_flat::dxr`,
run on the RTX 3080 Ti against dump `D:\victory-data\bvh-dump`, run 20261126-140839 on the boat):

- one bottom level (37,402 triangles): 8192/8192 rays agree with the instruction's triangle test, 6425/6425 hits at
  the same distance;
- the biggest top level (529 instances, 48 bottom levels, 144,827 triangles): 2048/2048 agree, 2047/2047 hits at the
  same distance (62 ties between duplicate triangles).

## What the title gives us

| what | where |
|---|---|
| top level header | dword 0 version 6 (bit 31 clear), 3 instance count, 13 instance list offset, 12 root pointer |
| instance record (128 B, in the list) | dwords 0-11 world-to-object 3x4 (row-major), 12 id/mask, 13 flags, 14 root pointer (bit 30 static), 15 bottom level address >> 8; 16-27 a scaled inverse (not needed) |
| bottom level header | dword 0 version 6 with marks (0x8002 rehydrated, 0x8004 compacted, 0x8102 still packed), 3 element count, 13 element offset, 12 root |
| element (64 B triangle node) | up to 4 vertices at dwords 0-11, dword 15 an id byte per triangle (0 = none), the unused vertex slot holds the title's primitive numbers |

The title's own box trees of dynamic bottom levels are broken in this tree (children 0, 64 m cubes): built over or
not built (`fork-ray-tracing.md`, "Dynamic structures overlap"). The element nodes are whole, so the host
structures are built from the elements and do not depend on the trees: rays should hit what the emulated walk
misses today.

## Runtime design

### 1. Knowing the structures (victory-gpu, `memory.rs`)
- Top levels: the addresses the trace programs are given (`walked` in the dump code, `ready(.., Use::Reads)`), the
  first page is a header with bit 31 clear.
- Bottom levels: read on the GPU from each top level's instance list (dword 15 << 8).
- When complete: after the builder's dispatches of the frame and before the first trace dispatch that reads the top
  level. The host already orders lists; the DXR build goes into the list just before the first trace dispatch that
  names a top level this frame.

### 2. Host structures (victory-gpu, new `dxr.rs`)
- `VICTORY_DXR=1` (off by default). Needs ray tracing tier 1.1 (`ID3D12Device5`, `ID3D12GraphicsCommandList4`).
- Per bottom level address: a compute pass reads its elements through the by-address directory (the same one the
  translated shaders use), writes three vertices per triangle into a vertex buffer of ours and a side table entry
  (element pointer | triangle number) per primitive; then `BuildRaytracingAccelerationStructure`.
  - static (root pointer bit 30, marks 0x8002 after rehydration): built once per address and generation, cached;
  - dynamic: rebuilt every frame the builder touched it (refit when the element count is unchanged).
- Per top level: a compute pass turns the instance list into `D3D12_RAYTRACING_INSTANCE_DESC`s (world-to-object
  inverted on the GPU, InstanceID = instance index, mask from dword 12, the bottom level's host address from a
  table address -> GPU VA), then the TLAS build.
- A table on the card: title top-level address (and its root node address) -> slot of its TLAS SRV, and per
  TLAS the instance index -> instance record address and per BLAS primitive -> element pointer.

### 3. The trace programs (victory-shader, `emit/intersect.rs`), guided traversal
The title's loop stays; only what the node test returns changes, when `VICTORY_DXR` is on:
1. at a node whose address is a known top level's root (one lookup in the table above): one `RayQuery`
   (`RAY_FLAG_FORCE_OPAQUE` at first) against that TLAS with the ray as given; keep the committed instance and
   primitive in two registers of the lane; return a box result holding only the hit instance node's pointer (or all
   `NONE` for a miss);
2. at the hit bottom level's root (the next node test of the lane, the ray now in object space as the title's
   code transformed it): return a box result holding only the hit element's pointer `| k`;
3. at the triangle node: the instruction's own exact test, as today. So hit distance and barycentrics are the
   title's bit for bit, and its code finds the primitive data through the pointer as it always has.
Traversal per ray: about 3 node tests instead of hundreds. The emitted shader needs the TLAS SRVs (a root
descriptor table of acceleration structure SRVs) and the side tables (a root SRV), shader model 6.5.

### 4. Known gaps, in order
- Alpha-tested geometry (foliage) is opaque at first; the title tests alpha after a triangle hit and continues:
  later, `RayQuery` without FORCE_OPAQUE and the candidate loop.
- Instance masks and culling flags: carry dword 12's mask into the instance desc; the ray mask is the title's own
  code, unknown at the root call: 0xff at first.
- Procedural nodes (7 seen): leave those structures on the emulated walk.
- Still-packed bottom levels (0x8102): skipped until rehydrated.

## Order of work, each step tested before the next
1. GPU element extraction pass, tested against `bvh_flat::elements` on the dump.
2. `dxr.rs` BLAS/TLAS build from title memory on the card, tested with the scene test's rays.
3. `VICTORY_DXR=1` in a run: build only, log sizes and times per frame, no shader change.
4. Guided traversal in `intersect.rs` behind the switch; translator tests with a fake TLAS; then the game, compared
   with `-RT` screenshots and timings.

## 2026-11-27: where the native path stands, and the native builder (plan #2)

### Fixed today (VIctory-NVIDIA-HighRAM-main, `crates/victory-gpu/src/memory.rs`, `crates/victory-shader/src/emit/`)
- Settle: host top levels name only bottom levels whose builds have run (`dxr.building`).
- Marks: the card writes a mark (`WriteBufferImmediate`, MARKER_OUT) after each list's reads and builds; nothing
  is entered in the walk's table, and no read is looked at, before its mark (frame counts were not enough: one
  submission took 4.7 s to prepare; runs 151722, 152214, 161146 lost the card in a trace dispatch).
- Grace: bottom levels stay alive while a top level built before they went is published, gone (`top_gone`, 120
  frames) or still waiting to be entered.
- Table: 16,384 entries, 4,095 structure slots (was 1,024 / 1,023: most structures never got in).
- Eviction (`dxr_let_go`): bottom levels no top level named for 600 frames *of top-level building* go when the 512 MB
  fills or the card is over 93%.
- Kind `DXR_BOTTOM_WHOLE` (static bottom levels: a miss is a miss); `VICTORY_DXR_WHOLE=0` turns it off.
- Switch 3 "RTX fast" (host structures alone; lossy: what they lack casts nothing). Optional, not a fix.
- Pool (`dxr::Pool`): transient buffers of builds and reads reused (was 7 buffers made per build, 1-6 ms each on
  the recording thread under the memory lock). Built? see the run logs.
- Status line: why bottom levels are not usable, table state, marks waiting, per-build cost.

### Measured
- Guided at 0 costs ~10 ms a frame over plain: the layer's tracking of the builder's memory (490 kernel askings over
  6.7 GB a frame, 9.8 ms; 16 GB checked; 9,125 card compares against 534).
- Guided at 2: 80-210 ms a frame; 6-15% of native queries hit; a third to a half of a scene's instances in the host
  top levels; misses walk the title's tree after the query.
- Memory: card at 94% of its budget, 6 GB of PC memory shared with it, 783 MB of PC memory free (26.5 GB private).

### Plan #2: a native builder instead of the console's (user's choice, 2026-11-27 16:42)
Goal: ray tracing without running `XboxBvhBuilder.dll`'s 52 programs or tracking their memory.

The four unpacking programs (disassembly in `D:\victory-data\snap\rehydrate\`):
| program | hash | threads | what it touches |
|---|---|---|---|
| DecompressHeader | 4ddd6f9702fab2a7 | 32 | one dwordx4 + dwordx3 in, a header out (52 instructions) |
| Box32Rehydration | 28918b7150a7aad5 | 64 | 24 ushort loads, box nodes with 32-bit bounds out |
| Box16Rehydration | caa78b8ca62bc3d6 | 64 | 25 ushort loads, 24 short stores: 16-bit boxes |
| LeafRehydration | 5e68767498c57eb6 | 1024 | **402 global loads through 64-bit pointers** + an SRV of 48-byte records |

Finding: the packed leaves hold no triangles. LeafRehydration rebuilds the triangle nodes from the meshes' own vertex
and index data, through pointers (the 48-byte records look like per-geometry descriptors: buffers, stride, format,
counts). So a native builder can take each structure's geometry straight from those buffers into a DXR bottom level,
without decoding the box tree at all.

The hard part is the other end: the title's trace programs, after a hit, read the triangle node (its element, with
the title's primitive numbers in the unused vertex slot) to shade. With no rehydration those nodes do not exist. So
either (a) keep LeafRehydration alone and skip the rest of the builder, or (b) answer the hit fully in the guided walk
(distance, barycentrics, and the primitive numbers the title would have read), which means knowing where the title
reads them.

Next steps, each tested before the next:
1. Read the 48-byte records: dump them (and the vertex/index buffers they point at) from a guided run, decode the
   fields against LeafRehydration's listing.
2. Offline: from a record and its buffers, produce the triangles; compare with the rehydrated element nodes in the
   same dump (`bvh_flat::elements`), every triangle.
3. Decide (a) or (b) from what the trace programs read after a hit (their listings).
4. In the runtime: native bottom levels from the records; the builder's programs not run.

### 2026-11-27 17:00: step 1 done, the records decoded (capture run 20261127-164943)

The capture (`VICTORY_REHYDRATE_CAPTURE=D:\victory-data\rehydrate-capture`) logged 300 dispatches of each of the four
programs and 359 MB of the memory their views point at. LeafRehydration's views on the title's root table: u0 = u1 =
the arena (same address), t0 = the 48-byte records.

The records are the title's triangle geometry descriptions, in D3D12's own layout
(`D3D12_RAYTRACING_GEOMETRY_TRIANGLES_DESC`): +0 transform (3x4 floats, address or 0), +8 index format, +12 vertex
format, +16 index count, +20 vertex count, +24 index buffer, +32 vertex buffer, +40 vertex stride (bit 15: winding
flipped). Every record seen uses two console formats:
- index format 53, a packed index stream. Header 3 dwords h0,h1,h2 at the index buffer. Triangle p's dword D is at
  +(h0&0x1ffff)*16+12+4p. Group g=(D>>21)&0x7ff has 8 bytes Q0,Q1 at +g*16+12; base=((Q0<<2)&0x10000)|(Q1&0xffff).
  The three indices are 16-bit words at +(h0&0x1ffff)*16+12 + h1*4 + 2*(base + f), f = D's 7-bit fields at bits 0,
  7, 14 (reversed when the record's bit 15 is set).
- vertex format 16, 8 bytes a vertex: x = d0>>8 (24 bits), y = (d0&0xff)|((d1&0xffff)<<8) (24 bits), z = d1>>16
  (16 bits), each times a scale plus an offset kept just before the vertices: offset xyz + an axis-order word at
  vertex buffer -32, scale xyz at -16 (axis word 2 = x,y,z as stored).
- Format 6 (three floats) and index formats 42/57 (32/16-bit) have plain paths in the program too.

Offline check on six geometries: every decoded index lands below the vertex count, the largest equals count-1
exactly, the meshes are coherent (2-4 cm edges on 20-60 cm pieces) and the transforms are rotations.

What the leaf writes per thread: one 64-byte element node, four vertices (+0, +12, +24, +52) and the ids at +36
(geometry number <<8 | flags, the primitive numbers, the second primitive or none). Each packed leaf entry is a
24-bit geometry number, an 8-bit flags byte and two 24-bit primitive numbers (a pair sharing an edge).

So for step 3: after a hit the guided walk hands the title the element node (entry[3] + (primitive>>2)*64), and the
title's own code reads that node. The nodes must exist; the box tree (Box16/Box32) does not need to. The host now
knows enough to write the element nodes itself, from the records, without any console program.

Open: the crash at 00:51:08Z in the capture run (card lost 11 s after the capture finished, in a title compute
program 36f04a4ed0ad1d9b, after a 4.5 s submission stall; no host structures had been built, so not the native path).
