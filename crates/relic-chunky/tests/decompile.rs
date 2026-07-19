//! Validates the reflection `.bin` -> `.rdo` decompiler against fixtures whose
//! source values are known from the matching editor `.rdo` files.

use std::io::{BufReader, Cursor};

use relic_chunky::{chunky::ChunkFile, decompile::DecompiledReflect};

const MOD_BIN: &[u8] = include_bytes!("mod.bin");

fn decompile(bytes: &'static [u8]) -> DecompiledReflect {
    let mut cf = ChunkFile::parse(BufReader::new(Cursor::new(bytes))).unwrap();
    DecompiledReflect::parse(&mut cf).expect("reflection file")
}

#[test]
fn mod_bin_structure_matches_source_rdo() {
    let d = decompile(MOD_BIN);

    // Root object id and type, from the source mod.rdo.
    assert_eq!(d.root_id, 14004954420306200179);
    let root = d.objects.iter().find(|o| o.id == d.root_id).unwrap();
    assert_eq!(d.types.get(&root.type_hash).unwrap().name, "Mod");

    // Two owned ReflectLocString children with the exact ids from the source.
    let children: Vec<_> = d
        .objects
        .iter()
        .filter(|o| o.owner_id == d.root_id)
        .map(|o| o.id)
        .collect();
    assert!(children.contains(&14004954423660372099)); // m_name
    assert!(children.contains(&14004954424942201991)); // m_description
}

#[test]
fn mod_bin_rdo_contains_correct_values() {
    let xml = decompile(MOD_BIN).to_rdo_xml();

    // The reconstructed DataWarehouse must carry the source's exact values:
    // the mod GUID split across modPart0-3 and the two loc-string keys.
    assert!(xml.contains("Type=\"Mod\" Id=\"14004954420306200179\""));
    assert!(xml.contains("<DataProperty Name=\"m_locStringKey\" Type=\"Int32\" Value=\"1\"/>"));
    assert!(xml.contains("<DataProperty Name=\"m_locStringKey\" Type=\"Int32\" Value=\"2\"/>"));
    assert!(xml.contains("<DataProperty Name=\"m_modPart0\" Type=\"UInt32\" Value=\"1354613940\"/>"));
    assert!(xml.starts_with("<DataWarehouse>"));
}
