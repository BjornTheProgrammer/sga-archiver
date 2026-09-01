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

fn sample(layout: TocLayout) -> Archive {
    let root = Folder {
        name: String::new(),
        files: vec![stored("top.txt", b"top")],
        folders: vec![
            Folder {
                name: "a".into(),
                files: vec![stored("main.scar", b"aa")],
                folders: vec![Folder {
                    name: "deep".into(),
                    files: vec![stored("leaf.txt", b"leaf")],
                    folders: vec![],
                }],
            },
            Folder {
                name: "b".into(),
                files: vec![stored("main.scar", b"bb")],
                folders: vec![],
            },
        ],
    };
    Archive {
        name: "0123456789abcdef0123456789abcdef".into(),
        version: 11,
        product: 0,
        block_size: 0,
        header_encryption_type: FileEncryptionType::None,
        signature: [0u8; 256],
        header_reserved: sga::entries::HeaderReserved::default(),
        layout,
        tocs: vec![Toc {
            alias: "data".into(),
            name: "data".into(),
            root,
        }],
    }
}

fn roundtrip(layout: TocLayout) {
    let archive = sample(layout);
    let mut out = Cursor::new(Vec::new());
    archive.write(&mut out).unwrap();
    let bytes = out.into_inner();

    let reread = Archive::read(&mut BufReader::new(Cursor::new(&bytes))).unwrap();
    assert_eq!(reread.layout, layout, "layout not detected on read");

    let mut out2 = Cursor::new(Vec::new());
    reread.write(&mut out2).unwrap();
    assert_eq!(out2.into_inner(), bytes, "{layout:?}: read→write drifted");
}

#[test]
fn legacy_layout_round_trips_and_is_detected() {
    roundtrip(TocLayout::Legacy);
}

#[test]
fn modern_layout_round_trips_and_is_detected() {
    roundtrip(TocLayout::Modern);
}

#[test]
fn layouts_differ_on_branching_trees() {
    let mut legacy = Cursor::new(Vec::new());
    sample(TocLayout::Legacy).write(&mut legacy).unwrap();
    let mut modern = Cursor::new(Vec::new());
    sample(TocLayout::Modern).write(&mut modern).unwrap();
    assert_ne!(legacy.into_inner(), modern.into_inner());
}
