//! `Archive::read_file` documents itself as case-insensitive.
//!
//! It lowercased the query but compared against stored names verbatim, so a
//! mixed-case archive was unreachable by any spelling. Base-game and editor
//! archives are all-lowercase, which is why it went unnoticed.

use std::io::{BufReader, Cursor};

use sga::entires::{FileEncryptionType, FileStorageType, FileVerificationType};
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

#[test]
fn mixed_case_names_are_findable() {
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
    let parsed = Archive::read(&mut BufReader::new(Cursor::new(&bytes))).unwrap();

    for spelling in [
        "Art/House.rgm",
        "art/house.rgm",
        "ART/HOUSE.RGM",
        r"Art\House.rgm",
    ] {
        assert_eq!(
            parsed.read_file(spelling).as_deref(),
            Some(&b"geometry"[..]),
            "{spelling} did not resolve"
        );
    }

    // A name that genuinely is not there still reports absent.
    assert!(parsed.read_file("art/missing.rgm").is_none());
}
