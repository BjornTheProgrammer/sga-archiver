
use std::io::Cursor;

use relic_chunky::{container::Chunky, decompile::DecompiledReflect, reflect_write::recompile_bin};

const MOD_BIN: &[u8] = include_bytes!("mod.bin");

/// Read/write drift guard for reflection: decompiling a `.bin` to `.rdo` and
/// recompiling it must reproduce the original bytes. This is the exact path the
/// mod tooling uses (mod descriptors), and proves `decompile` and
/// `reflect_write` are exact inverses for it.
#[test]
fn reflection_bin_round_trips_byte_exact() {
    let chunky = Chunky::read(&mut Cursor::new(MOD_BIN)).unwrap();
    let decompiled = DecompiledReflect::parse(&chunky).unwrap();
    let rebuilt = recompile_bin(&decompiled.to_rdo_xml(), MOD_BIN).unwrap();
    assert_eq!(rebuilt, MOD_BIN, "decompile→recompile was not byte-exact");
}

fn decompile(bytes: &'static [u8]) -> DecompiledReflect {
    let chunky = Chunky::read(&mut Cursor::new(bytes)).unwrap();
    DecompiledReflect::parse(&chunky).expect("reflection file")
}

#[test]
fn mod_bin_structure_matches_source_rdo() {
    let d = decompile(MOD_BIN);

    assert_eq!(d.root_id, 14004954420306200179);
    let root = d.objects.iter().find(|o| o.id == d.root_id).unwrap();
    assert_eq!(d.types.get(&root.type_hash).unwrap().name, "Mod");

    let children: Vec<_> = d
        .objects
        .iter()
        .filter(|o| o.owner_id == d.root_id)
        .map(|o| o.id)
        .collect();
    assert!(children.contains(&14004954423660372099));
    assert!(children.contains(&14004954424942201991));
}

#[test]
fn mod_bin_rdo_contains_correct_values() {
    let xml = decompile(MOD_BIN).to_rdo_xml();

    assert!(xml.contains("Type=\"Mod\" Id=\"14004954420306200179\""));
    assert!(xml.contains("<DataProperty Name=\"m_locStringKey\" Type=\"Int32\" Value=\"1\"/>"));
    assert!(xml.contains("<DataProperty Name=\"m_locStringKey\" Type=\"Int32\" Value=\"2\"/>"));
    assert!(xml.contains("<DataProperty Name=\"m_modPart0\" Type=\"UInt32\" Value=\"1354613940\"/>"));
    assert!(xml.starts_with("<DataWarehouse>"));
}
