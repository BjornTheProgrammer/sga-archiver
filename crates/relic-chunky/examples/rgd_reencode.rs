//! Round-trips a `.rgd` through decode → encode and reports where they differ,
//! validating the RGD encoder. Usage: `cargo run --example rgd_reencode -- x.rgd`

use std::io::BufReader;

use relic_chunky::chunky::ChunkFile;
use relic_chunky::rgd::RelicGameData;
use relic_chunky::rgd_write::write_rgd;

fn main() {
    let path = std::env::args().nth(1).unwrap();
    let original = std::fs::read(&path).unwrap();

    let mut chunk_file = ChunkFile::parse(BufReader::new(std::fs::File::open(&path).unwrap())).unwrap();
    let nodes = RelicGameData::parse(&mut chunk_file).unwrap();
    let encoded = write_rgd(&nodes).unwrap();

    if let Some(out) = std::env::args().nth(2) {
        std::fs::write(&out, &encoded).unwrap();
    }
    if encoded == original {
        println!("{path}: *** BYTE-IDENTICAL *** ({} bytes)", original.len());
        return;
    }
    println!("{path}: differ (orig {} vs {})", original.len(), encoded.len());
    let at = original.iter().zip(&encoded).position(|(a, b)| a != b).unwrap_or(original.len().min(encoded.len()));
    println!("  first diff @{at}: orig {:02x?} enc {:02x?}", &original.get(at..at + 12), &encoded.get(at..at + 12));
}
