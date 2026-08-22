//! Dumps the chunk tree of a `.rrgeom` (or any Relic Chunky), with data
//! previews, to reverse-engineer the geometry buffer layout.
//!
//! Usage: cargo run -p relic-chunky --example rrgeom_dump -- <file> [maxhex]

use std::env;
use std::fs::File;
use std::io::BufReader;

use anyhow::Result;
use relic_chunky::container::{Chunk, ChunkBody, Chunky};

fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    let path = &args[1];
    let maxhex: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(64);

    let mut r = BufReader::new(File::open(path)?);
    let chunky = Chunky::read(&mut r)?;
    println!(
        "Chunky major={} minor={} platform={}",
        chunky.major, chunky.minor, chunky.platform
    );
    for c in &chunky.chunks {
        dump(c, 0, maxhex);
    }
    Ok(())
}

fn dump(c: &Chunk, depth: usize, maxhex: usize) {
    let indent = "  ".repeat(depth);
    let path = if c.path.is_empty() {
        String::new()
    } else {
        format!(" path={:?}", String::from_utf8_lossy(&c.path))
    };
    match &c.body {
        ChunkBody::Folder(children) => {
            println!("{indent}FOLD {} v{}{}", c.name_str(), c.version, path);
            for child in children {
                dump(child, depth + 1, maxhex);
            }
        }
        ChunkBody::Data(data) => {
            println!(
                "{indent}DATA {} v{} len={}{}",
                c.name_str(),
                c.version,
                data.len(),
                path
            );
            preview(data, &indent, maxhex);
        }
    }
}

fn preview(data: &[u8], indent: &str, maxhex: usize) {
    let n = data.len().min(maxhex);
    for row in data[..n].chunks(16) {
        let hex: Vec<String> = row.iter().map(|b| format!("{b:02x}")).collect();
        let asc: String = row
            .iter()
            .map(|&b| if (0x20..0x7f).contains(&b) { b as char } else { '.' })
            .collect();
        println!("{indent}  {:<48} {}", hex.join(" "), asc);
    }
    // Also show as u32 LE for the first several words — useful for headers.
    let words = (n / 4).min(12);
    if words > 0 {
        let u: Vec<String> = (0..words)
            .map(|i| {
                let o = i * 4;
                u32::from_le_bytes(data[o..o + 4].try_into().unwrap()).to_string()
            })
            .collect();
        println!("{indent}  u32: {}", u.join(" "));
    }
    // And as f32 LE, for buffers that are vertex data.
    if words > 0 {
        let f: Vec<String> = (0..words)
            .map(|i| {
                let o = i * 4;
                format!("{:.3}", f32::from_le_bytes(data[o..o + 4].try_into().unwrap()))
            })
            .collect();
        println!("{indent}  f32: {}", f.join(" "));
    }
}
