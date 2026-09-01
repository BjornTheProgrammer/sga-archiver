use std::io::{BufReader, Cursor};

use sga::entries::{FileEncryptionType, HeaderReserved, SgaHeader};

fn sample(version: u16) -> SgaHeader {
    let v = version;
    let defaults = HeaderReserved::default();
    SgaHeader {
        magic: *b"_ARCHIVE",
        version,
        product: 0xBEEF,
        name: "Test Archive".into(),
        header_blob_offset: 0, // fixed up by roundtrip() after measuring
        header_blob_length: 96,
        data_offset: 428,
        data_blob_length: if v >= 8 { 12345 } else { 0 },
        toc_data_offset: 44,
        toc_data_count: 3,
        folder_data_offset: 100,
        folder_data_count: 17,
        file_data_offset: 200,
        file_data_count: 66,
        string_offset: 300,
        string_length: 1592,
        block_size: if v >= 7 { 262144 } else { 0 },
        header_encryption_type: if v >= 11 {
            FileEncryptionType::Aes128
        } else {
            FileEncryptionType::None
        },
        signature: if v >= 8 { [7u8; 256] } else { [0u8; 256] },
        file_hash_offset: if v >= 7 { 4400 } else { 0 },
        file_hash_length: if v >= 8 { 1420 } else { 0 },
        reserved: HeaderReserved {
            pre_name: if v < 6 { [0xAA; 16] } else { defaults.pre_name },
            post_name: if v < 6 {
                [0xBB; 16]
            } else {
                defaults.post_name
            },
            v5: if v == 5 {
                [0xC1C1, 0xC2C2]
            } else {
                defaults.v5
            },
            pad: if v < 11 { 0xD1D1 } else { defaults.pad },
            v11_one: defaults.v11_one,
        },
    }
}

fn roundtrip(version: u16) {
    let mut header = sample(version);
    let mut probe = Cursor::new(Vec::new());
    header.write_main_header(&mut probe).unwrap();
    header.header_blob_offset = probe.into_inner().len() as u64;

    let mut out = Cursor::new(Vec::new());
    header.write_main_header(&mut out).unwrap();
    header.write_index_table(&mut out).unwrap();
    let bytes = out.into_inner();

    let parsed = SgaHeader::parse(&mut BufReader::new(Cursor::new(&bytes))).unwrap();
    assert_eq!(
        parsed, header,
        "v{version}: parsed fields differ from written"
    );

    let mut again = Cursor::new(Vec::new());
    parsed.write_main_header(&mut again).unwrap();
    parsed.write_index_table(&mut again).unwrap();
    assert_eq!(
        again.into_inner(),
        bytes,
        "v{version}: write→read→write drifted"
    );
}

#[test]
fn every_version_round_trips() {
    for version in 3..=11 {
        roundtrip(version);
    }
}

#[test]
fn write_rejects_unsupported_versions() {
    for version in [0, 2, 12] {
        let mut header = sample(11);
        header.version = version;
        assert!(
            header
                .write_main_header(&mut Cursor::new(Vec::new()))
                .is_err()
        );
        assert!(
            header
                .write_index_table(&mut Cursor::new(Vec::new()))
                .is_err()
        );
    }
}

#[test]
fn parse_rejects_bad_magic_and_bad_version() {
    let mut bytes = b"NOT_MAGIissue".to_vec();
    bytes.resize(600, 0);
    assert!(SgaHeader::parse(&mut BufReader::new(Cursor::new(&bytes))).is_err());

    let mut bytes = b"_ARCHIVE".to_vec();
    bytes.extend_from_slice(&12u16.to_le_bytes());
    bytes.extend_from_slice(&0u16.to_le_bytes());
    bytes.resize(600, 0);
    assert!(SgaHeader::parse(&mut BufReader::new(Cursor::new(&bytes))).is_err());
}
