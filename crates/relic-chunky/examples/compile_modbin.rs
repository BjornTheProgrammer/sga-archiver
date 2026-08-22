//! Compiles a reflection `.rdo` into a `.bin` and (optionally) compares against
//! a reference `.bin`. Used to validate the mod-descriptor burner.
//!
//! Usage: cargo run -p relic-chunky --example compile_modbin -- <in.rdo> <out.bin> [reference.bin]

use std::env;
use std::fs;

use anyhow::Result;
use relic_chunky::reflect_write::compile_bin;

fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    let rdo = fs::read_to_string(&args[1])?;
    let bin = compile_bin(&rdo)?;
    fs::write(&args[2], &bin)?;
    println!("compiled {} -> {} ({} bytes)", args[1], args[2], bin.len());

    if let Some(reference) = args.get(3) {
        let want = fs::read(reference)?;
        if want == bin {
            println!("*** BYTE-IDENTICAL to {reference} ***");
        } else {
            println!("differs from {reference}: {} vs {} bytes", want.len(), bin.len());
            let at = want.iter().zip(&bin).position(|(a, b)| a != b).unwrap_or(want.len().min(bin.len()));
            println!("first diff @{at}");
        }
    }
    Ok(())
}
