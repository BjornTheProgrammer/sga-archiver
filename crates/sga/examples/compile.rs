fn main() -> anyhow::Result<()> {
    let source = std::env::args().nth(1).unwrap();
    let out = std::env::args().nth(2).unwrap();
    sga::compile(&source, &out)?;
    println!("compiled {} -> {}", source, out);
    Ok(())
}
