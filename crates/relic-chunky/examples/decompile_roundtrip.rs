//! Round-trips a reflection `.bin`: decompile -> `.rdo` -> recompile (against
//! the original as schema reference) -> `.bin'`, and reports fidelity. Any diff
//! is a gap in the decompiler (or recompiler).
//! Usage: decompile_roundtrip <file.bin>

use std::fs;
use std::io::Cursor;

use anyhow::{anyhow, Result};
use relic_chunky::container::Chunky;
use relic_chunky::decompile::DecompiledReflect;
use relic_chunky::reflect_write::recompile_bin;

fn main() -> Result<()> {
    let path = std::env::args().nth(1).ok_or_else(|| anyhow!("need .bin path"))?;
    let orig = fs::read(&path)?;

    let chunky = Chunky::read(&mut Cursor::new(orig.clone()))?;
    let dec = DecompiledReflect::parse(&chunky).ok_or_else(|| anyhow!("not a reflection file"))?;
    let rdo = dec.to_rdo_xml();
    fs::write(format!("{path}.roundtrip.rdo"), &rdo)?;
    println!("decompiled: {} objects, {} types, rdo {} bytes", dec.objects.len(), dec.types.len(), rdo.len());

    let rebuilt = recompile_bin(&rdo, &orig)?;
    if rebuilt == orig {
        println!("*** BYTE-IDENTICAL round-trip ({} bytes) ***", orig.len());
        return Ok(());
    }
    println!("differ: orig {} vs rebuilt {}", orig.len(), rebuilt.len());
    let at = orig.iter().zip(&rebuilt).position(|(a, b)| a != b).unwrap_or(orig.len().min(rebuilt.len()));
    let lo = at.saturating_sub(4);
    println!("first diff @{at}");
    println!("  orig   : {:02x?}", &orig.get(lo..lo + 24));
    println!("  rebuilt: {:02x?}", &rebuilt.get(lo..lo + 24));
    Ok(())
}
