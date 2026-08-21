use std::fs::File;
use std::io::BufReader;
use sga::Archive;

fn main() -> anyhow::Result<()> {
    let src = std::env::args().nth(1).unwrap();
    let out = std::env::args().nth(2).unwrap();
    let mut r = BufReader::new(File::open(&src)?);
    let archive = Archive::read(&mut r)?;
    let mut w = File::create(&out)?;
    archive.write(&mut w)?;
    println!("wrote {}", out);
    Ok(())
}
