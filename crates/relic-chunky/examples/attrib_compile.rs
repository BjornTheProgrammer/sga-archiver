//! Compiles an attribute XML and diffs against a reference .rgd.
//! Usage: cargo run --example attrib_compile -- source.xml reference.rgd
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let xml = std::fs::read_to_string(&a[1]).unwrap();
    let out = relic_chunky::attrib::compile_attrib(&xml).unwrap();
    let refr = std::fs::read(&a[2]).unwrap();
    if out == refr { println!("*** BYTE-IDENTICAL *** ({} bytes)", refr.len()); return; }
    println!("differ (mine {} vs ref {})", out.len(), refr.len());
    let at = out.iter().zip(&refr).position(|(x,y)|x!=y).unwrap_or(out.len().min(refr.len()));
    println!("first diff @{at}: mine {:02x?} ref {:02x?}", &out.get(at..at+12), &refr.get(at..at+12));
}
