use std::{env, fs, io::BufReader};

use relic_chunky::{chunky::ChunkFile, decompile::DecompiledReflect};

fn main() {
    let path = env::args().nth(1).expect("usage: dump_decompile <file.bin>");
    let file = fs::File::open(&path).expect("open");
    let mut cf = ChunkFile::parse(BufReader::new(file)).expect("parse");
    let d = DecompiledReflect::parse(&mut cf).expect("reflection file");

    println!("root_id = 0x{:016x}", d.root_id);
    println!("\n== objects (ROBJ order) ==");
    for o in &d.objects {
        let tn = d.types.get(&o.type_hash).map(|t| t.name.as_str()).unwrap_or("?");
        println!(
            "  id=0x{:016x} type={:<24} owner=0x{:016x} data_off={}",
            o.id, tn, o.owner_id, o.data_offset
        );
    }

    println!("\n== types ==");
    let mut types: Vec<_> = d.types.values().collect();
    types.sort_by_key(|t| t.name.clone());
    for t in types {
        if t.fields.is_empty() {
            continue;
        }
        let bases: Vec<String> = t
            .bases
            .iter()
            .map(|(h, i)| format!("0x{h:016x}#{i}"))
            .collect();
        println!(
            "  {} (hash=0x{:016x} size={}) bases=[{}]",
            t.name,
            t.hash,
            t.size,
            bases.join(", ")
        );
        for f in &t.fields {
            println!("    +{:<3} size {:<2} {}: {}", f.offset, f.size, f.name, f.type_name);
        }
    }

    println!("\n== RFCI data ({} bytes) ==", d.data.len());
    println!("\n== interned (RSHI) ==");
    for (h, s) in &d.interned {
        println!("  0x{h:016x} {s:?}");
    }
}
