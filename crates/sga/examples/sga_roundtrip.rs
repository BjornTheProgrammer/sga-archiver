//! Read→write round-trip check: parses an `.sga` and rewrites it; reports
//! whether the output is byte-identical.
//! Usage: sga_roundtrip <file.sga>

use std::fs;
use std::io::{BufReader, Cursor};

use anyhow::{Result, anyhow};
use sga::Archive;

fn main() -> Result<()> {
    let path = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow!("need .sga path"))?;
    let orig = fs::read(&path)?;
    let archive = Archive::read(&mut BufReader::new(Cursor::new(&orig)))?;
    let mut out = Cursor::new(Vec::new());
    archive.write(&mut out)?;
    let rebuilt = out.into_inner();
    if rebuilt == orig {
        println!("BYTE-IDENTICAL ({} bytes): {path}", orig.len());
    } else {
        let at = orig
            .iter()
            .zip(&rebuilt)
            .position(|(a, b)| a != b)
            .unwrap_or(orig.len().min(rebuilt.len()));
        println!(
            "DIFFERS: {path} (orig {} vs rebuilt {}, first diff @{at})",
            orig.len(),
            rebuilt.len()
        );
    }
    Ok(())
}
