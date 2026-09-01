//! `Archive::read_file` lowercased its query and compared against stored names
//! verbatim, so a mixed-case archive was unreachable by any spelling at all.
//!
//! The repair is to compare as stored rather than to fold both sides: the SGA
//! format does not define case-insensitive lookup, and folding it would be a
//! Windows assumption baked into a crate that builds on Linux too.

use std::io::{BufReader, Cursor};

use sga::entries::{FileEncryptionType, FileStorageType, FileVerificationType};
use sga::{Archive, FileEntry, Folder, Toc, TocLayout};

fn stored(name: &str, data: &[u8]) -> FileEntry {
    FileEntry {
        name: name.to_string(),
        stored_data: data.to_vec(),
        uncompressed_size: data.len() as u32,
        storage_type: FileStorageType::Store,
        encryption_type: FileEncryptionType::None,
        verification_type: FileVerificationType::None,
        crc: {
            let mut crc = flate2::Crc::new();
            crc.update(data);
            crc.amount()
        },
        data_order: None,
    }
}

/// An archive holding one mixed-case file, written and read back.
fn packed() -> Archive {
    let archive = Archive {
        header_reserved: Default::default(),
        name: "probe".into(),
        version: 10,
        product: 0,
        block_size: 262_144,
        header_encryption_type: FileEncryptionType::None,
        signature: [0; 256],
        layout: TocLayout::Modern,
        tocs: vec![Toc {
            alias: "data".into(),
            name: "data".into(),
            root: Folder {
                name: String::new(),
                files: vec![],
                folders: vec![Folder {
                    name: "Art".into(),
                    files: vec![stored("House.rgm", b"geometry")],
                    folders: vec![],
                }],
            },
        }],
    };

    let mut bytes = Vec::new();
    archive.write(&mut Cursor::new(&mut bytes)).unwrap();
    Archive::read(&mut BufReader::new(Cursor::new(&bytes))).unwrap()
}

#[test]
fn names_are_matched_exactly_as_stored() {
    let parsed = packed();

    // Matched exactly. The archive format does not fold case and neither do
    // case-sensitive filesystems, so neither does this crate.
    for spelling in ["Art/House.rgm", r"Art\House.rgm"] {
        assert_eq!(
            parsed.read_file(spelling).as_deref(),
            Some(&b"geometry"[..]),
            "{spelling} did not resolve"
        );
    }
    for wrong_case in ["art/house.rgm", "ART/HOUSE.RGM", "Art/house.rgm"] {
        assert!(
            parsed.read_file(wrong_case).is_none(),
            "{wrong_case} resolved, but names are compared as stored"
        );
    }

    // A name that genuinely is not there still reports absent.
    assert!(parsed.read_file("art/missing.rgm").is_none());
}

#[test]
fn the_entry_is_reachable_without_decoding_it() {
    let parsed = packed();
    let entry = parsed.file("Art/House.rgm").expect("entry not found");
    assert_eq!(entry.name, "House.rgm");
    assert_eq!(entry.uncompressed_size, 8);
    assert!(parsed.file("Art/missing.rgm").is_none());
}

#[test]
fn try_read_file_separates_absent_from_undecodable() {
    let parsed = packed();
    assert_eq!(
        parsed.try_read_file("Art/House.rgm").unwrap().as_deref(),
        Some(&b"geometry"[..])
    );
    // Absent is Ok(None), not an error.
    assert!(parsed.try_read_file("Art/missing.rgm").unwrap().is_none());
}

#[test]
fn every_file_comes_back_with_the_toc_holding_it() {
    let parsed = packed();
    let all = parsed.files().collect::<Vec<_>>();
    assert_eq!(all.len(), 1);
    let (toc, path, entry) = &all[0];
    assert_eq!(toc.alias, "data");
    assert_eq!(path, r"Art\House.rgm");
    assert_eq!(entry.name, "House.rgm");
}
