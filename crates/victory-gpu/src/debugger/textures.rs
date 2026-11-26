//! The texture inspector: every title texture alive when a frame is recorded
//! (`crate::resource::debug_textures`), with what the host has of it, and
//! with F11 a small picture of each one used lately (its smallest level of
//! at least 64 texels a side, read back from its host copy; block-compressed
//! formats BC1 to BC5 decoded here).

use windows::Win32::Graphics::Direct3D12::*;

/// A texture as the report lists it.
#[derive(Clone, Debug)]
pub(crate) struct Texture {
    pub(crate) this: usize,
    pub(crate) address: u64,
    pub(crate) kind: &'static str,
    pub(crate) size: (u64, u32, u16),
    pub(crate) mips: u16,
    pub(crate) format: u32,
    pub(crate) flags: u32,
    /// The host has a copy; the bytes it takes.
    pub(crate) host: bool,
    pub(crate) host_bytes: u64,
    /// The title fills it, and a list uploaded into the host copy since it was made.
    pub(crate) title_fills: bool,
    pub(crate) filled: bool,
    pub(crate) refused: bool,
    pub(crate) gone: bool,
    pub(crate) placed: bool,
    /// Frames since its host copy was last asked for.
    pub(crate) frames_ago: u64,
    /// The picture's file name, if one was saved.
    pub(crate) preview: Option<String>,
}

/// Textures with pictures at most.
const PREVIEWS: usize = 1500;

pub(super) fn inventory(frame: u64, previews: bool) -> Vec<Texture> {
    let now = crate::frame::current();
    let all = crate::resource::debug_textures();
    let mut out: Vec<(Texture, Option<std::sync::Arc<crate::resource::HostTexture>>)> = all
        .into_iter()
        .map(|t| {
            let used_frame = t.used >> 32;
            (
                Texture {
                    this: t.this,
                    address: t.address,
                    kind: match t.dimension {
                        2 => "1D",
                        3 => "2D",
                        4 => "3D",
                        _ => "?",
                    },
                    size: t.size,
                    mips: t.mips,
                    format: t.format,
                    flags: t.flags,
                    host: t.host.is_some(),
                    host_bytes: t.host_bytes,
                    title_fills: t.title_fills,
                    filled: crate::texture::filled(t.this as crate::object::This),
                    refused: t.refused,
                    gone: t.gone,
                    placed: t.placed,
                    frames_ago: if t.used == 0 { u64::MAX } else { now.saturating_sub(used_frame) },
                    preview: None,
                },
                t.host,
            )
        })
        .collect();
    if previews && let Some(dir) = super::dir() {
        let folder = dir.join(format!("textures-{frame}"));
        let _ = std::fs::create_dir_all(&folder);
        let mut order: Vec<usize> = (0..out.len()).filter(|&i| out[i].1.is_some()).collect();
        order.sort_by_key(|&i| out[i].0.frames_ago);
        for &i in order.iter().take(PREVIEWS) {
            let Some(host) = out[i].1.clone() else { continue };
            let name = format!("{:x}.png", out[i].0.this);
            if picture(&host, &folder.join(&name)) {
                out[i].0.preview = Some(format!("textures-{frame}/{name}"));
            }
        }
    }
    out.into_iter().map(|t| t.0).collect()
}

/// Saves a picture of `host`'s smallest level of at least 64 texels a side.
fn picture(host: &crate::resource::HostTexture, path: &std::path::Path) -> bool {
    use std::sync::atomic::Ordering::Relaxed;
    let Some(h) = crate::host::host() else { return false };
    let texture = &host.texture;
    // SAFETY: a live resource.
    let d = unsafe { texture.GetDesc() };
    if d.Dimension != D3D12_RESOURCE_DIMENSION_TEXTURE2D || d.SampleDesc.Count > 1 {
        return false;
    }
    // Only textures the card never writes. A render target, depth or
    // unordered-access texture is read in a list of this function's own,
    // submitted at once outside the queues' order: the state the host
    // notes for it may be one the card has not reached yet (lists sent
    // ahead, the relay and second queues), and a state change on such a
    // texture rewrites its compression data. In this tree that drew black
    // lines across the picture and took the people's hair (drawn through
    // unordered access) after F11 (run 20261125-202248). Every target a
    // recorded frame draws is saved anyway (`crate::capture::dump_targets`).
    let written = D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET
        | D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL
        | D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS
        | D3D12_RESOURCE_FLAG_ALLOW_SIMULTANEOUS_ACCESS;
    if d.Flags.0 & written.0 != 0 {
        return false;
    }
    let mut mip = 0u32;
    while mip + 1 < u32::from(d.MipLevels) && ((d.Width >> (mip + 1)) >= 64 && (d.Height >> (mip + 1)) >= 64) {
        mip += 1;
    }
    let sub = mip;
    let mut footprint = D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default();
    let mut rows = 0u32;
    let mut total = 0u64;
    // SAFETY: a live texture's description; outputs for one subresource.
    unsafe {
        h.device.GetCopyableFootprints(&d, sub, 1, 0, Some(&mut footprint), Some(&mut rows), None, Some(&mut total));
    }
    if total == 0 || total > 64 << 20 {
        return false;
    }
    let heap = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_READBACK, ..Default::default() };
    let desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
        Width: total,
        Height: 1,
        DepthOrArraySize: 1,
        MipLevels: 1,
        SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        ..Default::default()
    };
    let state = D3D12_RESOURCE_STATES(crate::host::legal_state(texture, host.state.load(Relaxed)) as i32);
    let copy = D3D12_RESOURCE_STATE_COPY_SOURCE;
    let mut buffer: Option<ID3D12Resource> = None;
    // SAFETY: plain creation; an open list of ours; live resources.
    let bytes = unsafe {
        if h.device
            .CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc, D3D12_RESOURCE_STATE_COPY_DEST, None, &mut buffer)
            .is_err()
        {
            return false;
        }
        let Some(buffer) = buffer else { return false };
        let Ok(list) = h.new_list() else { return false };
        if state != copy {
            list.list.ResourceBarrier(&[crate::host::transition(texture, state, copy)]);
        }
        let to = D3D12_TEXTURE_COPY_LOCATION {
            pResource: std::mem::transmute_copy(&buffer),
            Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { PlacedFootprint: footprint },
        };
        let from = D3D12_TEXTURE_COPY_LOCATION {
            pResource: std::mem::transmute_copy(texture),
            Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: sub },
        };
        list.list.CopyTextureRegion(&to, 0, 0, 0, &from, None);
        if state != copy {
            list.list.ResourceBarrier(&[crate::host::transition(texture, copy, state)]);
        }
        list.close();
        h.execute(&[&list]);
        let mut data = std::ptr::null_mut();
        if buffer.Map(0, None, Some(&mut data)).is_err() {
            return false;
        }
        let v = std::slice::from_raw_parts(data.cast::<u8>(), total as usize).to_vec();
        buffer.Unmap(0, None);
        v
    };
    let f = footprint.Footprint;
    let (w, ht) = (f.Width as usize, f.Height as usize);
    let pitch = f.RowPitch as usize;
    let rgba = match f.Format.0 as u32 {
        70..=72 => blocks(&bytes, w, ht, pitch, 8, |b, out| bc1(b, out)),
        73..=75 => blocks(&bytes, w, ht, pitch, 16, |b, out| {
            bc1(&b[8..], out);
            // BC2: 4 bits of alpha a texel
            for (i, px) in out.iter_mut().enumerate() {
                px[3] = (b[i / 2] >> ((i % 2) * 4) & 0xf) * 17;
            }
        }),
        76..=78 => blocks(&bytes, w, ht, pitch, 16, |b, out| {
            bc1(&b[8..], out);
            let a = bc4(&b[..8]);
            for (px, a) in out.iter_mut().zip(a) {
                px[3] = a;
            }
        }),
        79..=81 => blocks(&bytes, w, ht, pitch, 8, |b, out| {
            for (px, v) in out.iter_mut().zip(bc4(b)) {
                *px = [v, v, v, 255];
            }
        }),
        82..=84 => blocks(&bytes, w, ht, pitch, 16, |b, out| {
            let (r, g) = (bc4(&b[..8]), bc4(&b[8..]));
            for (i, px) in out.iter_mut().enumerate() {
                *px = [r[i], g[i], 128, 255];
            }
        }),
        format => crate::capture::to_rgba(format, w, ht, pitch, &bytes),
    };
    let Some(rgba) = rgba else { return false };
    png(path, w as u32, ht as u32, &rgba)
}

/// Decodes 4x4 blocks of `size` bytes into RGBA rows.
fn blocks(bytes: &[u8], w: usize, h: usize, pitch: usize, size: usize, decode: impl Fn(&[u8], &mut [[u8; 4]; 16])) -> Option<Vec<u8>> {
    let mut out = vec![0u8; w * h * 4];
    for by in 0..h.div_ceil(4) {
        for bx in 0..w.div_ceil(4) {
            let at = by * pitch + bx * size;
            let b = bytes.get(at..at + size)?;
            let mut px = [[0u8; 4]; 16];
            decode(b, &mut px);
            for (i, p) in px.iter().enumerate() {
                let (x, y) = (bx * 4 + i % 4, by * 4 + i / 4);
                if x < w && y < h {
                    out[(y * w + x) * 4..][..4].copy_from_slice(p);
                }
            }
        }
    }
    Some(out)
}

fn rgb565(c: u16) -> [u8; 4] {
    let r = (c >> 11 & 31) as u32;
    let g = (c >> 5 & 63) as u32;
    let b = (c & 31) as u32;
    [(r * 255 / 31) as u8, (g * 255 / 63) as u8, (b * 255 / 31) as u8, 255]
}

fn bc1(b: &[u8], out: &mut [[u8; 4]; 16]) {
    let (c0, c1) = (u16::from_le_bytes([b[0], b[1]]), u16::from_le_bytes([b[2], b[3]]));
    let (p0, p1) = (rgb565(c0), rgb565(c1));
    let mix = |wa: u32, wb: u32| -> [u8; 4] {
        let m = |i: usize| ((u32::from(p0[i]) * wa + u32::from(p1[i]) * wb) / (wa + wb)) as u8;
        [m(0), m(1), m(2), 255]
    };
    let palette = if c0 > c1 { [p0, p1, mix(2, 1), mix(1, 2)] } else { [p0, p1, mix(1, 1), [0, 0, 0, 0]] };
    let bits = u32::from_le_bytes([b[4], b[5], b[6], b[7]]);
    for (i, px) in out.iter_mut().enumerate() {
        *px = palette[(bits >> (2 * i) & 3) as usize];
    }
}

fn bc4(b: &[u8]) -> [u8; 16] {
    let (a0, a1) = (u32::from(b[0]), u32::from(b[1]));
    let mut pal = [0u32; 8];
    pal[0] = a0;
    pal[1] = a1;
    if a0 > a1 {
        for i in 1..7 {
            pal[i + 1] = ((7 - i as u32) * a0 + i as u32 * a1) / 7;
        }
    } else {
        for i in 1..5 {
            pal[i + 1] = ((5 - i as u32) * a0 + i as u32 * a1) / 5;
        }
        pal[6] = 0;
        pal[7] = 255;
    }
    let mut bits = 0u64;
    for (i, &x) in b[2..8].iter().enumerate() {
        bits |= u64::from(x) << (8 * i);
    }
    let mut out = [0u8; 16];
    for (i, o) in out.iter_mut().enumerate() {
        *o = pal[(bits >> (3 * i) & 7) as usize] as u8;
    }
    out
}

/// Writes RGBA rows as a PNG (RGBA, 8 bits).
pub(crate) fn png(path: &std::path::Path, w: u32, h: u32, rgba: &[u8]) -> bool {
    fn crc(bytes: &[u8]) -> u32 {
        let mut c = 0xffff_ffffu32;
        for &b in bytes {
            c ^= u32::from(b);
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xedb8_8320 ^ (c >> 1) } else { c >> 1 };
            }
        }
        !c
    }
    fn chunk(png: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        png.extend((data.len() as u32).to_be_bytes());
        let start = png.len();
        png.extend_from_slice(kind);
        png.extend_from_slice(data);
        let c = crc(&png[start..]);
        png.extend(c.to_be_bytes());
    }
    let mut raw = Vec::with_capacity((w as usize * 4 + 1) * h as usize);
    for y in 0..h as usize {
        raw.push(0);
        raw.extend_from_slice(&rgba[y * w as usize * 4..][..w as usize * 4]);
    }
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut header = Vec::new();
    header.extend(w.to_be_bytes());
    header.extend(h.to_be_bytes());
    header.extend([8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &header);
    let mut compressed = vec![0u8; zlib_rs::compress_bound(raw.len())];
    let (idat, _) = zlib_rs::compress_slice(&mut compressed, &raw, zlib_rs::DeflateConfig::new(3));
    chunk(&mut out, b"IDAT", idat);
    chunk(&mut out, b"IEND", &[]);
    std::fs::write(path, out).is_ok()
}
