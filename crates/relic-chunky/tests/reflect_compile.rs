use std::io::Cursor;
use std::path::PathBuf;

use relic_chunky::container::Chunky;
use relic_chunky::reflect_type::SchemaRegistry;
use relic_chunky::reflect_write::{compile_bin, compile_bin_from_schema, compile_reflect};

/// A `(reference .bin, source .rdo)` pair under `tests/fixtures`.
struct Fixture {
    bin: Vec<u8>,
    rdo: String,
}

fn fixture(dir: &str, bin_name: &str, rdo_name: &str) -> Fixture {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(dir);
    Fixture {
        bin: std::fs::read(base.join(bin_name)).expect("fixture .bin"),
        rdo: std::fs::read_to_string(base.join(rdo_name)).expect("fixture .rdo"),
    }
}

fn win_condition() -> Fixture {
    fixture("winconditions", "nomad anonymous.bin", "Nomad Anonymous.rdo")
}

fn mod_info() -> Fixture {
    fixture("info", "mod.bin", "mod.rdo")
}

fn assert_same(out: &[u8], bin: &[u8]) {
    assert_eq!(out.len(), bin.len(), "size: got {} want {}", out.len(), bin.len());
    if out != bin {
        let at = out.iter().zip(bin).position(|(a, b)| a != b).unwrap_or(out.len().min(bin.len()));
        panic!("first byte diff at {at}: got {:02x?} want {:02x?}", &out.get(at..at + 8), &bin.get(at..at + 8));
    }
}

/// Compiles a `.rdo` into a `.bin` using ONLY the schema bundled in the tool
/// (no reference `.bin`) — the proof that a mod tree needs no `.bin` at all.
fn assert_bundled_byte_exact(f: &Fixture) {
    assert_same(&compile_bin(&f.rdo).unwrap(), &f.bin);
}

/// Compiles a `.rdo` into a *complete* `.bin` using only the schema from the
/// reference (every chunk generated, none copied).
fn assert_full_byte_exact(f: &Fixture) {
    assert_same(&compile_bin_from_schema(&f.rdo, &f.bin).unwrap(), &f.bin);
}

/// Compiles a `.rdo` while reusing the reference's invariant chunks.
fn assert_reuse_byte_exact(f: &Fixture) {
    let reference = Chunky::read(&mut Cursor::new(&f.bin)).unwrap();
    let mut reg = SchemaRegistry::new();
    reg.add_from_chunky(&reference);
    let out = compile_reflect(&f.rdo, &reg, &reference).unwrap();
    let mut buf = Vec::new();
    out.write(&mut buf).unwrap();
    assert_same(&buf, &f.bin);
}

#[test]
fn win_condition_from_bundled_schema() {
    assert_bundled_byte_exact(&win_condition());
}

#[test]
fn mod_from_bundled_schema() {
    assert_bundled_byte_exact(&mod_info());
}

#[test]
fn win_condition_full_bin_byte_exact() {
    assert_full_byte_exact(&win_condition());
}

#[test]
fn mod_full_bin_byte_exact() {
    assert_full_byte_exact(&mod_info());
}

#[test]
fn win_condition_bin_compiles_byte_exact() {
    assert_reuse_byte_exact(&win_condition());
}

#[test]
fn mod_bin_compiles_byte_exact() {
    assert_reuse_byte_exact(&mod_info());
}
