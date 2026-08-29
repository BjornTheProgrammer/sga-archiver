//! Compares two `.sga` archives member-by-member: stored length, storage type,
//! and whether the stored bytes match. Localizes packing divergence.
//! Usage: sga_diff <a.sga> <b.sga>

use std::fs::File;
use std::io::BufReader;

use anyhow::{anyhow, Result};
use sga::{Archive, FileEntry, Folder};

fn collect<'a>(folder: &'a Folder, prefix: &str, out: &mut Vec<(String, &'a FileEntry)>) {
    for f in &folder.files {
        out.push((format!("{prefix}/{}", f.name), f));
    }
    for c in &folder.folders {
        collect(c, &format!("{prefix}/{}", c.name), out);
    }
}

fn entries(archive: &Archive) -> Vec<(String, &FileEntry)> {
    let mut out = Vec::new();
    for toc in &archive.tocs {
        collect(&toc.root, &toc.alias.clone(), &mut out);
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn main() -> Result<()> {
    let a_path = std::env::args().nth(1).ok_or_else(|| anyhow!("need two .sga paths"))?;
    let b_path = std::env::args().nth(2).ok_or_else(|| anyhow!("need two .sga paths"))?;
    let a = Archive::read(&mut BufReader::new(File::open(&a_path)?))?;
    let b = Archive::read(&mut BufReader::new(File::open(&b_path)?))?;

    let ea = entries(&a);
    let eb = entries(&b);
    println!("{}: {} files, {}: {} files", a_path, ea.len(), b_path, eb.len());

    for (name, fa) in &ea {
        let Some((_, fb)) = eb.iter().find(|(n, _)| n == name) else {
            println!("only in A: {name}");
            continue;
        };
        let mut notes = Vec::new();
        if fa.storage_type != fb.storage_type {
            notes.push(format!("storage {:?} vs {:?}", fa.storage_type, fb.storage_type));
        }
        if fa.stored_data.len() != fb.stored_data.len() {
            notes.push(format!("stored len {} vs {}", fa.stored_data.len(), fb.stored_data.len()));
        } else if fa.stored_data != fb.stored_data {
            let at = fa.stored_data.iter().zip(&fb.stored_data).position(|(x, y)| x != y).unwrap();
            notes.push(format!("stored bytes differ from +{at} (len {})", fa.stored_data.len()));
        }
        if fa.crc != fb.crc {
            notes.push(format!("crc {:#x} vs {:#x}", fa.crc, fb.crc));
        }
        if fa.data_order != fb.data_order {
            notes.push(format!("data order {:?} vs {:?}", fa.data_order, fb.data_order));
        }
        if !notes.is_empty() {
            println!("{name}: {}", notes.join("; "));
        }
    }
    for (name, _) in &eb {
        if !ea.iter().any(|(n, _)| n == name) {
            println!("only in B: {name}");
        }
    }

    if std::env::args().nth(3).as_deref() == Some("--tree") {
        fn tree(folder: &Folder, depth: usize) {
            let pad = "  ".repeat(depth);
            for f in &folder.files {
                println!("{pad}F {}", f.name);
            }
            for c in &folder.folders {
                println!("{pad}D {}/", c.name);
                tree(c, depth + 1);
            }
        }
        for (label, ar) in [("A", &a), ("B", &b)] {
            println!("== {label} tree (stored order):");
            for toc in &ar.tocs {
                println!("TOC {}", toc.alias);
                tree(&toc.root, 1);
            }
        }
    }

    if std::env::args().nth(3).as_deref() == Some("--order") {
        for (label, list) in [("A", &ea), ("B", &eb)] {
            println!("== {label} data order:");
            let mut by_order: Vec<_> = list.clone();
            by_order.sort_by_key(|(_, f)| f.data_order);
            for (name, f) in by_order {
                println!("  {:>9} {name}", f.data_order.map(|o| o.to_string()).unwrap_or_default());
            }
        }
    }
    Ok(())
}
