fn main() -> anyhow::Result<()> {
    let name = std::env::args().nth(1).unwrap();
    let dir = std::env::args().nth(2).unwrap();
    let out = std::env::args().nth(3).unwrap();
    sga::pack_dir(&name, &dir, &out)?;
    println!("packed {}", out);
    Ok(())
}
