//! Reads an .sga and writes it straight back, reporting byte-fidelity.
//! Usage: roundtrip <archive.sga> [out.sga]
use std::io::Cursor;
fn main(){
    let a:Vec<String>=std::env::args().collect();
    let orig=std::fs::read(&a[1]).unwrap();
    let arch=sga::read_archive(&a[1]).unwrap();
    let mut buf=Vec::new();
    arch.write(&mut Cursor::new(&mut buf)).unwrap();
    if let Some(o)=a.get(2){ std::fs::write(o,&buf).unwrap(); }
    if buf==orig { println!("*** BYTE-IDENTICAL *** ({} bytes)",orig.len()); return; }
    println!("differ: orig {} vs repacked {}",orig.len(),buf.len());
    let at=orig.iter().zip(&buf).position(|(x,y)|x!=y).unwrap_or(orig.len().min(buf.len()));
    println!("first diff @{at}: orig {:02x?} new {:02x?}",&orig.get(at..at+16),&buf.get(at..at+16));
}
