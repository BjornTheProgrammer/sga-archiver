//! Prints every RFTY TypeDef in a reflection `.bin`: name, size, field count.
//! Usage: type_table <file.bin> [filter]

use std::fs;
use std::io::Cursor;

use anyhow::{anyhow, Result};
use relic_chunky::container::Chunky;
use relic_chunky::decompile::DecompiledReflect;

fn main() -> Result<()> {
    let path = std::env::args().nth(1).ok_or_else(|| anyhow!("need .bin path"))?;
    let filter = std::env::args().nth(2).unwrap_or_default();
    let orig = fs::read(&path)?;
    let chunky = Chunky::read(&mut Cursor::new(orig))?;
    let dec = DecompiledReflect::parse(&chunky).ok_or_else(|| anyhow!("not a reflection file"))?;
    let mut types: Vec<_> = dec.types.values().collect();
    types.sort_by(|a, b| a.name.cmp(&b.name));
    for ty in types {
        if !filter.is_empty() && !ty.name.contains(&filter) {
            continue;
        }
        println!("{} size={:#x} fields={} trailer={:#x}", ty.name, ty.size, ty.fields.len(), ty.trailer);
    }
    Ok(())
}
