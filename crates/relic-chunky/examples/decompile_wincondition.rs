use std::{env, fs, io::BufReader};

use relic_chunky::{chunky::ChunkFile, reflect::ReflectFile};

fn main() {
    let path = env::args()
        .nth(1)
        .expect("usage: decompile_wincondition <win-condition .bin/.rdo>");

    let file = fs::File::open(&path).expect("open input file");
    let mut chunk_file = ChunkFile::parse(BufReader::new(file)).expect("parse chunky container");

    match ReflectFile::parse(&mut chunk_file) {
        Some(reflect) => {
            println!("# Decompiled reflection file: {path}\n");
            print!("{}", reflect.to_report());
        }
        None => {
            eprintln!(
                "'{path}' has no RFTY type chunks - it is not a reflection file.\n\
                 If it is a plain attrib .rgd file, decode it with the sga-unpacker CLI instead."
            );
            std::process::exit(1);
        }
    }
}
