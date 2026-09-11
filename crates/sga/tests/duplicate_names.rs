//! Two members with the same name in one folder — a cooked texture and its
//! packed variant — must each keep their own bytes through a write.
use std::io::{BufReader, Cursor};

use sga::entries::{FileEncryptionType, HeaderReserved};
use sga::{Archive, ArchiveReader, FileEntry, Folder, TocLayout};

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { 0xEDB8_8320 ^ (crc >> 1) } else { crc >> 1 };
        }
    }
    !crc
}

#[test]
fn duplicate_member_names_keep_their_own_bytes_and_crcs() {
    let bytes = {
        let mut archive = Archive::read(&mut BufReader::new(Cursor::new(&sample_bytes()))).unwrap();
        // Add a second `a.rrtex` beside the first, with different contents.
        fn holding<'a>(folder: &'a mut Folder, name: &str) -> Option<&'a mut Folder> {
            if folder.files.iter().any(|f| f.name == name) {
                return Some(folder);
            }
            folder.folders.iter_mut().find_map(|f| holding(f, name))
        }
        let folder = holding(&mut archive.tocs[0].root, "a.rrtex").expect("folder holding a.rrtex");
        let first = folder.files.iter().find(|f| f.name == "a.rrtex").unwrap().clone();
        folder.files.push(FileEntry {
            stored_data: b"second texture with other bytes".to_vec(),
            uncompressed_size: 31,
            crc: crc32(b"second texture with other bytes"),
            data_order: None,
            ..first
        });
        let mut out = Vec::new();
        archive.write(&mut Cursor::new(&mut out)).unwrap();
        out
    };
    let mut reader = ArchiveReader::new(BufReader::new(Cursor::new(bytes.as_slice()))).unwrap();
    let index = reader.index().clone();
    let entries: Vec<_> = index
        .files()
        .filter(|(_, rel, _)| rel.ends_with("a.rrtex"))
        .map(|(_, _, file)| file.clone())
        .collect();
    assert_eq!(entries.len(), 2, "both members survive the write");
    let mut seen = Vec::new();
    for file in &entries {
        let stored = reader.read_stored(file).unwrap();
        assert_eq!(crc32(&stored), file.crc, "{}: bytes match the entry's crc", file.name);
        seen.push(stored);
    }
    assert_ne!(seen[0], seen[1], "the two members keep different bytes");
}

fn sample_bytes() -> Vec<u8> {
    let mut archive = Archive {
        header_reserved: HeaderReserved::default(),
        name: "0123456789abcdef0123456789abcdef".into(),
        version: 10,
        product: 0,
        block_size: 8,
        header_encryption_type: FileEncryptionType::None,
        signature: [0; 256],
        layout: TocLayout::Modern,
        tocs: Vec::new(),
    };
    archive.upsert_stored_in("data", "art/a.rrtex", b"first texture".to_vec());
    let mut out = Vec::new();
    archive.write(&mut Cursor::new(&mut out)).unwrap();
    out
}
