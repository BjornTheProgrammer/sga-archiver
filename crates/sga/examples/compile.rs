fn main() -> anyhow::Result<()> {
    let base = std::env::args().nth(1).unwrap();
    let assets = std::env::args().nth(2).unwrap();
    let out = std::env::args().nth(3).unwrap();
    let n = sga::compile(&base, &assets, &out)?;
    println!("recompiled {} source files -> {}", n, out);
    Ok(())
}
