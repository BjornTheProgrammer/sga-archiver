//! Compiles a PNG into a Relic `.rrtex` texture (the `RRTextureBurner`).
//!
//! The container is a Relic Chunky: `TSET → DATA + TXTR(name) → DXTC → TMAN +
//! TDAT`. The pixels are a single-mip **BC7** surface, sliced into 64 KB
//! segments that are each zlib-compressed and concatenated in `TDAT`; `TMAN`
//! carries the dimensions and a per-segment `[raw<<8][compressed<<8]` table.
//!
//! The editor's `bc7e` encoder and zlib can't be reproduced byte-for-byte, so
//! the output is a *functionally valid* BC7 texture, not byte-identical to the
//! editor's.

use anyhow::{bail, Result};
use flate2::write::ZlibEncoder;
use flate2::Compression;
use intel_tex_2::{bc7, RgbaSurface};
use std::io::Write;

use crate::container::{Chunk, ChunkBody, ChunkKind, Chunky};

/// BC7 pixel data is stored in 64 KB segments (a 256×256-texel tile of BC7).
const SEGMENT_SIZE: usize = 65536;
/// `TMAN`/`TDAT` tag the format with this code (BC7).
const FORMAT_BC7: u32 = 6;

/// Compiles PNG bytes into a `.rrtex`. `name` becomes the `TXTR` chunk's texture
/// name (the source file stem).
pub fn compile_texture(png_bytes: &[u8], name: &str) -> Result<Vec<u8>> {
    let (rgba, width, height) = decode_png(png_bytes)?;
    if width % 4 != 0 || height % 4 != 0 {
        bail!("texture {width}x{height} must have dimensions that are multiples of 4");
    }

    let surface = RgbaSurface { width, height, stride: width * 4, data: &rgba };
    let blocks = bc7::compress_blocks(&bc7::opaque_basic_settings(), &surface);

    // The pixel data starts with a 16-byte surface descriptor the engine skips
    // before reading the (linear, row-major) BC7 blocks:
    //   [u32 0][u32 width][u32 height][u32 bc7_byte_count]
    let mut pixels = Vec::with_capacity(16 + blocks.len());
    pixels.extend_from_slice(&0u32.to_le_bytes());
    pixels.extend_from_slice(&width.to_le_bytes());
    pixels.extend_from_slice(&height.to_le_bytes());
    pixels.extend_from_slice(&(blocks.len() as u32).to_le_bytes());
    pixels.extend_from_slice(&blocks);

    // Slice the surface into 64 KB segments, zlib each.
    let mut segments: Vec<(usize, Vec<u8>)> = Vec::new();
    for raw in pixels.chunks(SEGMENT_SIZE) {
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::best());
        encoder.write_all(raw)?;
        segments.push((raw.len(), encoder.finish()?));
    }

    let chunky = Chunky {
        major: 4,
        minor: 0,
        platform: 1,
        chunks: vec![build_tset(name, build_tman(width, height, &segments), build_tdat(&segments))],
    };
    let mut out = Vec::new();
    chunky.write(&mut out)?;
    Ok(out)
}

fn decode_png(bytes: &[u8]) -> Result<(Vec<u8>, u32, u32)> {
    let mut decoder = png::Decoder::new(bytes);
    // Expand paletted / low-bit-depth images to straight samples.
    decoder.set_transformations(png::Transformations::EXPAND);
    let mut reader = decoder.read_info()?;
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf)?;
    buf.truncate(info.buffer_size());

    if info.bit_depth != png::BitDepth::Eight {
        bail!("only 8-bit PNGs are supported (got {:?})", info.bit_depth);
    }

    let pixels = (info.width * info.height) as usize;
    let mut rgba = vec![0u8; pixels * 4];
    match info.color_type {
        png::ColorType::Rgba => rgba.copy_from_slice(&buf[..pixels * 4]),
        png::ColorType::Rgb => {
            for i in 0..pixels {
                rgba[i * 4..i * 4 + 3].copy_from_slice(&buf[i * 3..i * 3 + 3]);
                rgba[i * 4 + 3] = 255;
            }
        }
        png::ColorType::GrayscaleAlpha => {
            for i in 0..pixels {
                let g = buf[i * 2];
                rgba[i * 4..i * 4 + 3].copy_from_slice(&[g, g, g]);
                rgba[i * 4 + 3] = buf[i * 2 + 1];
            }
        }
        png::ColorType::Grayscale => {
            for i in 0..pixels {
                let g = buf[i];
                rgba[i * 4..i * 4 + 4].copy_from_slice(&[g, g, g, 255]);
            }
        }
        other => bail!("unsupported PNG color type {other:?}"),
    }
    Ok((rgba, info.width, info.height))
}

/// Builds the `TMAN` metadata: a fixed header (format, dimensions, one mip) plus
/// a per-segment `[raw<<8][compressed<<8]` table and a trailing byte.
fn build_tman(width: u32, height: u32, segments: &[(usize, Vec<u8>)]) -> Vec<u8> {
    let mut t = Vec::new();
    let put = |t: &mut Vec<u8>, v: u32| t.extend_from_slice(&v.to_le_bytes());
    put(&mut t, FORMAT_BC7);
    put(&mut t, width);
    put(&mut t, height);
    put(&mut t, 1); // mip count
    put(&mut t, 2); // texture-descriptor constants (BC7 2D)
    put(&mut t, 28);
    put(&mut t, 1);
    put(&mut t, 2);
    put(&mut t, 256); // segment tile dimensions (256x256 texels = 64 KB)
    put(&mut t, 256);
    put(&mut t, segments.len() as u32 * 256); // padded surface size / 256
    for (raw, compressed) in segments {
        put(&mut t, (*raw as u32) << 8);
        put(&mut t, (compressed.len() as u32) << 8);
    }
    t.push(0);
    t
}

/// Builds `TDAT`: a format-code prefix followed by the concatenated zlib
/// segment streams.
fn build_tdat(segments: &[(usize, Vec<u8>)]) -> Vec<u8> {
    let mut d = Vec::new();
    d.extend_from_slice(&FORMAT_BC7.to_le_bytes());
    for (_, compressed) in segments {
        d.extend_from_slice(compressed);
    }
    d
}

fn build_tset(name: &str, tman: Vec<u8>, tdat: Vec<u8>) -> Chunk {
    let dxtc = folder(b"DXTC", 6, Vec::new(), vec![data(b"TMAN", 2, tman), data(b"TDAT", 1, tdat)]);
    let mut txtr_path = name.as_bytes().to_vec();
    txtr_path.push(0); // texture name is NUL-terminated in the chunk path
    let txtr = folder(b"TXTR", 2, txtr_path, vec![dxtc]);
    folder(b"TSET", 1, Vec::new(), vec![data(b"DATA", 4, Vec::new()), txtr])
}

fn data(name: &[u8; 4], version: u32, body: Vec<u8>) -> Chunk {
    Chunk { kind: ChunkKind::Data, name: *name, version, path: Vec::new(), body: ChunkBody::Data(body) }
}

fn folder(name: &[u8; 4], version: u32, path: Vec<u8>, children: Vec<Chunk>) -> Chunk {
    Chunk { kind: ChunkKind::Folder, name: *name, version, path, body: ChunkBody::Folder(children) }
}
