use std::fs::File;
use std::io::BufReader;
use relic_chunky::container::Chunky;

fn dump(chunks: &[relic_chunky::container::Chunk], depth: usize) {
    for c in chunks {
        let indent = "  ".repeat(depth);
        match &c.body {
            relic_chunky::container::ChunkBody::Data(d) =>
                println!("{}{:?} {} v{} ({} bytes)", indent, c.kind, c.name_str(), c.version, d.len()),
            relic_chunky::container::ChunkBody::Folder(kids) => {
                println!("{}{:?} {} v{} [folder]", indent, c.kind, c.name_str(), c.version);
                dump(kids, depth + 1);
            }
        }
    }
}

fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).unwrap();
    let mut r = BufReader::new(File::open(&path)?);
    let chunky = Chunky::read(&mut r)?;
    println!("major={} minor={} platform={}", chunky.major, chunky.minor, chunky.platform);
    dump(&chunky.chunks, 0);
    let mut out = Vec::new();
    chunky.write(&mut out)?;
    let orig = std::fs::read(&path)?;
    println!("--- roundtrip: orig={} written={} identical={}", orig.len(), out.len(), orig == out);
    Ok(())
}
