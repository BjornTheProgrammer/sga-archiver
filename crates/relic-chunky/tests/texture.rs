use std::io::{Cursor, Read};

use flate2::read::ZlibDecoder;
use image_dds::{ImageFormat, Surface};
use relic_chunky::container::{Chunk, ChunkBody, Chunky};
use relic_chunky::texture::compile_texture;

const W: u32 = 256;
const H: u32 = 256;

/// A synthetic image where every 4x4 block is a distinct solid colour. Solid
/// blocks encode near-losslessly, so any block-shift, channel swap, or layout
/// bug in the compiler shows up as a large PSNR drop.
fn synthetic_rgba() -> Vec<u8> {
    let mut rgba = vec![0u8; (W * H * 4) as usize];
    for by in 0..H / 4 {
        for bx in 0..W / 4 {
            let r = (bx * 37 + by * 17) as u8;
            let g = (bx * 53 + by * 29) as u8;
            let b = (bx * 97 + by * 61) as u8;
            for dy in 0..4 {
                for dx in 0..4 {
                    let p = (((by * 4 + dy) * W + (bx * 4 + dx)) * 4) as usize;
                    rgba[p..p + 4].copy_from_slice(&[r, g, b, 255]);
                }
            }
        }
    }
    rgba
}

fn encode_png(rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, W, H);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header().unwrap().write_image_data(rgba).unwrap();
    out
}

fn find<'a>(chunks: &'a [Chunk], name: &[u8; 4]) -> Option<&'a [u8]> {
    for chunk in chunks {
        if &chunk.name == name {
            return chunk.data();
        }
        if let ChunkBody::Folder(children) = &chunk.body {
            if let Some(found) = find(children, name) {
                return Some(found);
            }
        }
    }
    None
}

#[test]
fn texture_round_trips() {
    let source = synthetic_rgba();
    let rrtex = compile_texture(&encode_png(&source), "test").unwrap();

    let chunky = Chunky::read(&mut Cursor::new(&rrtex)).unwrap();
    let tman = find(&chunky.chunks, b"TMAN").expect("TMAN");
    let tdat = find(&chunky.chunks, b"TDAT").expect("TDAT");

    // Container header: format=BC7(6), correct dimensions, one mip.
    assert_eq!(u32::from_le_bytes(tman[0..4].try_into().unwrap()), 6, "format");
    assert_eq!(u32::from_le_bytes(tman[4..8].try_into().unwrap()), W, "width");
    assert_eq!(u32::from_le_bytes(tman[8..12].try_into().unwrap()), H, "height");
    assert_eq!(u32::from_le_bytes(tman[12..16].try_into().unwrap()), 1, "mip count");

    // Inflate the segments (compressed sizes from the TMAN table) into the
    // pixel payload.
    let segments = (tman.len() - 44 - 1) / 8;
    let mut pixels = Vec::new();
    let mut off = 4usize;
    for i in 0..segments {
        let comp =
            (u32::from_le_bytes(tman[44 + i * 8 + 4..44 + i * 8 + 8].try_into().unwrap()) >> 8)
                as usize;
        let mut raw = Vec::new();
        ZlibDecoder::new(&tdat[off..off + comp]).read_to_end(&mut raw).unwrap();
        pixels.extend_from_slice(&raw);
        off += comp;
    }

    // 16-byte surface descriptor: [0][width][height][bc7 byte count].
    assert_eq!(u32::from_le_bytes(pixels[0..4].try_into().unwrap()), 0);
    assert_eq!(u32::from_le_bytes(pixels[4..8].try_into().unwrap()), W);
    assert_eq!(u32::from_le_bytes(pixels[8..12].try_into().unwrap()), H);
    let bc7_len = u32::from_le_bytes(pixels[12..16].try_into().unwrap()) as usize;
    assert_eq!(bc7_len, pixels.len() - 16, "descriptor payload size");

    // Decode the BC7 (after the header) and check it matches the source.
    let surface = Surface {
        width: W,
        height: H,
        depth: 1,
        layers: 1,
        mipmaps: 1,
        image_format: ImageFormat::BC7RgbaUnorm,
        data: &pixels[16..],
    };
    let decoded = surface.decode_rgba8().unwrap();

    let mut sse = 0f64;
    for (a, b) in source.iter().zip(&decoded.data) {
        let e = *a as f64 - *b as f64;
        sse += e * e;
    }
    let mse = sse / source.len() as f64;
    let psnr = 10.0 * (255.0 * 255.0 / mse).log10();
    assert!(psnr > 40.0, "round-trip PSNR too low ({psnr:.1} dB) — layout/header/channel bug?");
}
