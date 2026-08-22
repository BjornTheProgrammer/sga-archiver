//! Dumps each file's storage/verification/crc so we can see how the editor
//! packages a given file vs how our writer does.
//! Usage: filemeta <archive.sga> [name-substring]

use std::io::BufReader;
use std::fs::File;

use sga::{Archive, Folder};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let filter = args.get(2).map(|s| s.to_lowercase());
    let ar = Archive::read(&mut BufReader::new(File::open(&args[1])?))?;
    println!("version={} tocs={}", ar.version, ar.tocs.len());
    for toc in &ar.tocs {
        let (mut nf, mut nd) = (0usize, 0usize);
        count(&toc.root, &mut nf, &mut nd);
        println!("--- TOC alias={:?} name={:?}  ({nf} files, {nd} folders) ---", toc.alias, toc.name);
        walk(&toc.root, String::new(), filter.as_deref());
    }
    Ok(())
}

fn count(folder: &Folder, nf: &mut usize, nd: &mut usize) {
    *nf += folder.files.len();
    *nd += folder.folders.len();
    for c in &folder.folders { count(c, nf, nd); }
}

fn walk(folder: &Folder, path: String, filter: Option<&str>) {
    for f in &folder.files {
        let full = if path.is_empty() { f.name.clone() } else { format!("{path}/{}", f.name) };
        if filter.map(|s| full.to_lowercase().contains(s)).unwrap_or(true) {
            // Recompute a zlib CRC-32 over both the decoded and the stored bytes
            // to see which (if either) matches the crc field the editor stored.
            let dec = f.decoded().unwrap_or_default();
            let mut c1 = flate2::Crc::new(); c1.update(&dec);
            let mut c2 = flate2::Crc::new(); c2.update(&f.stored_data);
            println!(
                "{:<58} store={:?} verify={:?} crc={:08x} crc32(decoded)={:08x} crc32(stored)={:08x}",
                full, f.storage_type, f.verification_type, f.crc, c1.sum(), c2.sum()
            );
        }
    }
    for child in &folder.folders {
        let p = if path.is_empty() { child.name.clone() } else { format!("{path}/{}", child.name) };
        walk(child, p, filter);
    }
}
