//! The lazy reader must describe an archive exactly as the eager one does,
//! hand back the same decoded bytes on demand, refuse encrypted members
//! rather than leak ciphertext, and catch stored-byte corruption.

use std::io::{BufReader, Cursor};

use sga::entries::{FileEncryptionType, FileStorageType, FileVerificationType, HeaderReserved};
use sga::{Archive, ArchiveReader, EncryptedMember, FileEntry, Folder, Toc, TocLayout};

fn empty(version: u16) -> Archive {
    Archive {
        header_reserved: HeaderReserved::default(),
        name: "0123456789abcdef0123456789abcdef".into(),
        version,
        product: 0,
        block_size: 8,
        header_encryption_type: FileEncryptionType::None,
        signature: [0; 256],
        layout: TocLayout::Modern,
        tocs: Vec::new(),
    }
}

/// Stored, compressed and hashed members across two TOCs, written to bytes.
fn sample() -> (Archive, Vec<u8>) {
    let mut archive = empty(10);
    archive.upsert_stored_in(
        "data",
        "art/house.rgm",
        b"geometry that is long enough to compress".to_vec(),
    );
    archive.upsert_stored_in("data", "art/house.rrtex", b"texture".to_vec());
    archive.upsert_stored_in("data", "scar/main.scar", b"-- lua".to_vec());
    archive.upsert_stored_in("attrib", "attrib/unit.rgd", b"rgd".to_vec());
    // BufferCompress the .rgm, leave the rest Store with SHA1 blocks.
    archive.repackage_art().unwrap();
    let mut bytes = Vec::new();
    archive.write(&mut Cursor::new(&mut bytes)).unwrap();
    (archive, bytes)
}

fn open(bytes: &[u8]) -> ArchiveReader<BufReader<Cursor<&[u8]>>> {
    ArchiveReader::new(BufReader::new(Cursor::new(bytes))).unwrap()
}

#[test]
fn index_matches_eager_read() {
    let (archive, bytes) = sample();
    let reader = open(&bytes);
    let index = reader.index();
    assert_eq!(index.header.name, archive.name);
    assert_eq!(index.header.version, 10);
    assert_eq!(index.file_count(), 4);
    assert_eq!(index.tocs.len(), 2);

    let eager = Archive::read(&mut BufReader::new(Cursor::new(&bytes))).unwrap();
    let eager_paths = eager
        .files()
        .map(|(toc, path, _)| format!("{}:{path}", toc.alias))
        .collect::<Vec<_>>();
    let lazy_paths = index
        .files()
        .map(|(toc, path, _)| format!("{}:{path}", toc.alias))
        .collect::<Vec<_>>();
    assert_eq!(lazy_paths, eager_paths);
    assert!(lazy_paths.contains(&"data:art\\house.rgm".to_string()));
    assert!(lazy_paths.contains(&"attrib:attrib\\unit.rgd".to_string()));
}

#[test]
fn reads_members_on_demand_in_either_separator() {
    let (_, bytes) = sample();
    let mut reader = open(&bytes);
    let (toc, file) = reader.index().file("art/house.rgm").unwrap();
    assert_eq!(toc.alias, "data");
    assert_eq!(file.storage_type, FileStorageType::BufferCompress);
    assert_eq!(file.verification_type, FileVerificationType::None);
    assert!(file.stored_size > 0);

    let decoded = reader.read_file("art\\house.rgm").unwrap().unwrap();
    assert_eq!(decoded, b"geometry that is long enough to compress");
    let stored = reader.read_file("scar/main.scar").unwrap().unwrap();
    assert_eq!(stored, b"-- lua");
    assert!(reader.read_file("scar/missing.scar").unwrap().is_none());
}

#[test]
fn stored_bytes_are_returned_verbatim() {
    let (_, bytes) = sample();
    let mut reader = open(&bytes);
    let file = reader.index().file("art/house.rgm").unwrap().1.clone();
    let stored = reader.read_stored(&file).unwrap();
    assert_eq!(stored.len(), file.stored_size as usize);
    assert_ne!(stored, b"geometry that is long enough to compress");
}

#[test]
fn encrypted_members_are_an_error_not_ciphertext() {
    let mut archive = empty(10);
    let secret = FileEntry {
        name: "secret.bin".into(),
        stored_data: b"ciphertext".to_vec(),
        uncompressed_size: 10,
        storage_type: FileStorageType::Store,
        encryption_type: FileEncryptionType::Aes128,
        verification_type: FileVerificationType::None,
        crc: 0,
        data_order: None,
    };
    archive.tocs.push(Toc {
        alias: "data".into(),
        name: "data".into(),
        root: Folder {
            name: String::new(),
            files: vec![secret],
            folders: vec![],
        },
    });
    let mut bytes = Vec::new();
    archive.write(&mut Cursor::new(&mut bytes)).unwrap();

    let mut reader = open(&bytes);
    let file = reader.index().file("secret.bin").unwrap().1.clone();
    assert_eq!(file.encryption_type, FileEncryptionType::Aes128);
    let error = reader.read(&file).unwrap_err();
    let encrypted = error
        .downcast_ref::<EncryptedMember>()
        .expect("typed error");
    assert_eq!(encrypted.path, "secret.bin");
    assert_eq!(encrypted.encryption, FileEncryptionType::Aes128);
    // The stored bytes remain reachable when a caller asks for exactly those.
    assert_eq!(reader.read_stored(&file).unwrap(), b"ciphertext");

    // The eager path refuses the same way.
    let eager = Archive::read(&mut BufReader::new(Cursor::new(&bytes))).unwrap();
    let error = eager.file("secret.bin").unwrap().decoded().unwrap_err();
    assert!(error.downcast_ref::<EncryptedMember>().is_some());
}

#[test]
fn verify_passes_a_clean_archive_and_catches_corruption() {
    let (_, bytes) = sample();
    let report = open(&bytes).verify().unwrap();
    assert_eq!(report.checked_files, 4);
    assert!(report.verified(), "{report:?}");
    assert_eq!(report.unhashed_files, 0);

    // Flip one byte inside the stored .scar member: both its CRC and its
    // SHA1 block hash must now disagree with what the archive recorded.
    let mut corrupted = bytes.clone();
    let file = open(&bytes)
        .index()
        .file("scar/main.scar")
        .unwrap()
        .1
        .clone();
    corrupted[file.data_offset as usize] ^= 0xFF;
    let report = open(&corrupted).verify().unwrap();
    assert!(!report.verified());
    assert_eq!(report.crc_mismatches.len(), 1);
    assert_eq!(report.crc_mismatches[0].path, "scar\\main.scar");
    assert_eq!(report.crc_mismatches[0].toc, "data");
    assert_eq!(report.sha1_mismatches.len(), 1);
    assert_eq!(report.sha1_mismatches[0].check, "sha1_blocks");
}

#[test]
fn eager_read_still_round_trips_byte_exactly() {
    let (_, bytes) = sample();
    let eager = Archive::read(&mut BufReader::new(Cursor::new(&bytes))).unwrap();
    let mut again = Vec::new();
    eager.write(&mut Cursor::new(&mut again)).unwrap();
    assert_eq!(again, bytes);
}
