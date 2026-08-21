use std::io::Cursor;
use relic_chunky::container::Chunky;
use relic_chunky::reflect_type::SchemaRegistry;
use relic_chunky::reflect_write::compile_reflect;

#[test]
fn mod_bin_compiles_byte_exact() {
    let bin_path = "C:/Users/Bjorn/Documents/Nomad Anonymous/prebuilt/info/mod.bin";
    let rdo_path = "C:/Users/Bjorn/Documents/Nomad Anonymous/assets/mod.rdo";
    let Ok(bin) = std::fs::read(bin_path) else { eprintln!("skip: no mod.bin"); return; };
    let reference = Chunky::read(&mut Cursor::new(&bin)).unwrap();
    let mut reg = SchemaRegistry::new();
    reg.add_from_chunky(&reference);
    let rdo = std::fs::read_to_string(rdo_path).unwrap();
    let out = compile_reflect(&rdo, &reg, &reference).unwrap();
    let mut buf = Vec::new();
    out.write(&mut buf).unwrap();
    assert_eq!(buf.len(), bin.len(), "size: got {} want {}", buf.len(), bin.len());
    if buf != bin {
        let at = buf.iter().zip(&bin).position(|(a,b)| a!=b).unwrap_or(buf.len().min(bin.len()));
        panic!("first byte diff at {at}: got {:02x?} want {:02x?}", &buf.get(at..at+8), &bin.get(at..at+8));
    }
}
