//! Prints a file-offset-ordered map of a reflection `.bin` blob: every object
//! placement and every out-of-line string target, to reveal the writer's
//! placement discipline.
//! Usage: layout_map <file.bin> [max]

use std::fs;
use std::io::Cursor;

use anyhow::{anyhow, Result};
use relic_chunky::container::Chunky;
use relic_chunky::decompile::DecompiledReflect;
use relic_chunky::reflect_type::{classify_field, FieldKind};

fn i32_at(b: &[u8], off: usize) -> Option<i32> {
    b.get(off..off + 4).map(|s| i32::from_le_bytes(s.try_into().unwrap()))
}

fn main() -> Result<()> {
    let path = std::env::args().nth(1).ok_or_else(|| anyhow!("need .bin path"))?;
    let max: usize = std::env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(120);
    let orig = fs::read(&path)?;
    let chunky = Chunky::read(&mut Cursor::new(orig))?;
    let dec = DecompiledReflect::parse(&chunky).ok_or_else(|| anyhow!("not a reflection file"))?;

    let mut rows: Vec<(usize, String)> = Vec::new();
    for obj in &dec.objects {
        let ty = dec.types.get(&obj.type_hash);
        let tyname = ty.map(|t| t.name.as_str()).unwrap_or("?");
        let size = ty.map(|t| t.size).unwrap_or(0);
        rows.push((
            obj.data_offset,
            format!("OBJ  {tyname} id={} owner={} size={size:#x}", obj.id % 10000, obj.owner_id % 10000),
        ));
        let Some(ty) = ty else { continue };
        let base = obj.data_offset - dec.rfci_offset;
        for field in &ty.fields {
            let pos = base + field.offset as usize;
            match classify_field(&field.type_name, |_| false) {
                FieldKind::Str => {
                    let rel = i32_at(&dec.data, pos).unwrap_or(0);
                    let len = i32_at(&dec.data, pos + 8).unwrap_or(0);
                    if len > 0 {
                        let target = (pos as i64 + rel as i64) as usize + dec.rfci_offset;
                        rows.push((
                            target,
                            format!("STR  '{}' len={len} of {tyname}.{} (obj@{:#x})",
                                dec.data.get((target - dec.rfci_offset)..(target - dec.rfci_offset) + len as usize)
                                    .map(|b| String::from_utf8_lossy(b).into_owned())
                                    .unwrap_or_default(),
                                field.name, obj.data_offset),
                        ));
                    }
                }
                FieldKind::Array | FieldKind::PointerArray => {
                    let rel = i32_at(&dec.data, pos).unwrap_or(0);
                    let count = i32_at(&dec.data, pos + 8).unwrap_or(0);
                    if count > 0 {
                        let target = (pos as i64 + rel as i64) as usize + dec.rfci_offset;
                        rows.push((
                            target,
                            format!("ARR> {tyname}.{} n={count} (obj@{:#x})", field.name, obj.data_offset),
                        ));
                    }
                }
                _ => {}
            }
        }
    }
    rows.sort();
    for (off, desc) in rows.into_iter().take(max) {
        println!("{off:#06x} {desc}");
    }
    Ok(())
}
