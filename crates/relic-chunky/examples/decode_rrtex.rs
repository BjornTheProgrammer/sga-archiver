//! Decodes a `.rrtex`'s BC7 surface back to a PNG, for visually verifying the
//! texture compiler. Usage: `cargo run --example decode_rrtex -- in.rrtex out.png`

use std::io::{Cursor, Read};

use flate2::read::ZlibDecoder;
use image_dds::{ImageFormat, Surface};
use relic_chunky::container::{Chunk, ChunkBody, Chunky};

fn find<'a>(chunks: &'a [Chunk], name: &[u8; 4]) -> Option<&'a Chunk> {
    for chunk in chunks {
        if &chunk.name == name {
            return Some(chunk);
        }
        if let ChunkBody::Folder(children) = &chunk.body {
            if let Some(found) = find(children, name) {
                return Some(found);
            }
        }
    }
    None
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (input, output) = (&args[1], &args[2]);

    let bytes = std::fs::read(input).unwrap();
    let chunky = Chunky::read(&mut Cursor::new(&bytes)).unwrap();
    let tman = find(&chunky.chunks, b"TMAN").unwrap().data().unwrap();
    let tdat = find(&chunky.chunks, b"TDAT").unwrap().data().unwrap();

    let width = u32::from_le_bytes(tman[4..8].try_into().unwrap());
    let height = u32::from_le_bytes(tman[8..12].try_into().unwrap());
    let segments = (tman.len() - 44 - 1) / 8;

    // Inflate each zlib segment (compressed size from the TMAN table) into the
    // full BC7 surface.
    let mut bc7 = Vec::new();
    let mut off = 4usize; // skip the TDAT format prefix
    for i in 0..segments {
        let comp = (u32::from_le_bytes(tman[44 + i * 8 + 4..44 + i * 8 + 8].try_into().unwrap())
            >> 8) as usize;
        let mut decoder = ZlibDecoder::new(&tdat[off..off + comp]);
        let mut raw = Vec::new();
        decoder.read_to_end(&mut raw).unwrap();
        bc7.extend_from_slice(&raw);
        off += comp;
    }

    let surface = Surface {
        width,
        height,
        depth: 1,
        layers: 1,
        mipmaps: 1,
        image_format: ImageFormat::BC7RgbaUnorm,
        data: bc7,
    };
    let rgba = surface.decode_rgba8().unwrap();

    let file = std::io::BufWriter::new(std::fs::File::create(output).unwrap());
    let mut encoder = png::Encoder::new(file, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header().unwrap().write_image_data(&rgba.data).unwrap();

    println!("decoded {width}x{height} BC7 ({segments} segments) -> {output}");
}
