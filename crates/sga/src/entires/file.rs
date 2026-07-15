use std::io::{BufRead, Read};

use sga_macros::read_field;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum SgaFileEntryParseError {
    #[error("Failed to parse number `{0}`")]
    FailedToParseNumber(String),
    #[error("Failed to read byte value `{0}`")]
    FailedToParseByte(String),
    #[error("Failed to parse storage type `{0}`")]
    FailedToParseStorageType(String),
    #[error("Failed to parse verification type `{0}`")]
    FailedToParseVerificationType(String),
}

/// Describes how a file is verified when it's loaded.
#[derive(Debug, Clone)]
pub enum FileVerificationType {
    /// No verification.
    None,

    /// CRC verification.
    CRC,

    /// CRC verification for blocks.
    CRCBlocks,

    /// MD5 verification for blocks.
    MD5Blocks,

    /// SHA1 verification for blocks.
    SHA1Blocks,
}

impl FileVerificationType {
    /// Parses a byte into a `FileVerificationType`.
    pub fn from_u8(value: u8) -> Result<Self, String> {
        match value {
            0 => Ok(Self::None),
            1 => Ok(Self::CRC),
            2 => Ok(Self::CRCBlocks),
            3 => Ok(Self::MD5Blocks),
            4 => Ok(Self::SHA1Blocks),
            _ => Err("Invalid file verification type".into()),
        }
    }

    /// Converts the `FileVerificationType` into its corresponding byte value.
    pub fn to_u8(self) -> u8 {
        match self {
            FileVerificationType::None => 0,
            FileVerificationType::CRC => 1,
            FileVerificationType::CRCBlocks => 2,
            FileVerificationType::MD5Blocks => 3,
            FileVerificationType::SHA1Blocks => 4,
        }
    }
}

/// Describes how a file is stored within an SGA archive.
#[derive(Debug, Clone)]
pub enum FileStorageType {
    /// Stored plainly.
    Store,

    /// Stored compressed (stream compression).
    StreamCompress,

    /// Stored compressed (buffer compression).
    BufferCompress,

    /// Stored compressed (stream compression with Brotli).
    StreamCompressBrotli,

    /// Stored compressed (buffer compression with Brotli).
    BufferCompressBrotli,

    /// Encrypted with AES-128 (a high nibble of `1` on the on-disk byte). The
    /// decryption key is not present in the SGA archive, the game, or any of
    /// its official tooling, so this crate can identify these files but not
    /// decrypt them — the underlying compression scheme (the low nibble) is
    /// unrecoverable without the key anyway, so it isn't retained. See
    /// `docs/storage-type-18.md` for the full writeup.
    Aes128Encrypted,

    /// Unknown storage type.
    Unknown(u8),
}

impl FileStorageType {
    /// Parses a byte into a `FileStorageType`. The low nibble is the
    /// compression scheme; a high nibble of `1` marks the whole byte as
    /// `Aes128Encrypted`.
    pub fn from_u8(value: u8) -> Self {
        match value >> 4 {
            0 => match value {
                0 => Self::Store,
                1 => Self::StreamCompress,
                2 => Self::BufferCompress,
                3 => Self::StreamCompressBrotli,
                4 => Self::BufferCompressBrotli,
                _ => Self::Unknown(value),
            },
            1 => Self::Aes128Encrypted,
            _ => Self::Unknown(value),
        }
    }

    pub fn to_u8(self) -> u8 {
        match self {
            FileStorageType::Store => 0,
            FileStorageType::StreamCompress => 1,
            FileStorageType::BufferCompress => 2,
            FileStorageType::StreamCompressBrotli => 3,
            FileStorageType::BufferCompressBrotli => 4,
            FileStorageType::Aes128Encrypted => 0x10,
            FileStorageType::Unknown(n) => n,
        }
    }
}

/// File entry of an SGA archive.
#[derive(Debug, Clone)]
pub struct SgaFileEntry {
    /// Offset of the file's name in the SGA archive's string blob.
    pub name_offset: u32,

    /// Offset of the file's hash in the SGA archive's hash blob.
    pub hash_offset: u32,

    /// Offset of the file's data in the SGA archive's data blob.
    pub data_offset: u64,

    /// Size of the file in bytes as stored in the SGA archive.
    pub compressed_length: u32,

    /// Size of the file in bytes when uncompressed.
    pub uncompressed_size: u32,

    /// How the file should be verified when loaded.
    pub verification_type: FileVerificationType,

    /// How the file's data is stored.
    pub storage_type: FileStorageType,

    /// CRC32 checksum of the file's data.
    pub crc: u32,
}

impl SgaFileEntry {
    pub fn parse<T: Read + BufRead>(reader: &mut T) -> Result<Self, SgaFileEntryParseError> {
        let name_offset = read_field!(reader, SgaFileEntryParseError::FailedToParseNumber, u32)?;
        let hash_offset = read_field!(reader, SgaFileEntryParseError::FailedToParseNumber, u32)?;
        let data_offset = read_field!(reader, SgaFileEntryParseError::FailedToParseNumber, u64)?;
        let compressed_length = read_field!(reader, SgaFileEntryParseError::FailedToParseNumber, u32)?;
        let uncompressed_size = read_field!(reader, SgaFileEntryParseError::FailedToParseNumber, u32)?;
        let verification_type_byte = read_field!(reader, SgaFileEntryParseError::FailedToParseByte, u8)?;
        let storage_type_byte = read_field!(reader, SgaFileEntryParseError::FailedToParseByte, u8)?;
        let crc = read_field!(reader, SgaFileEntryParseError::FailedToParseNumber, u32)?;


        let verification_type = FileVerificationType::from_u8(verification_type_byte).map_err(|err| SgaFileEntryParseError::FailedToParseVerificationType(err.to_string()))?;
        let storage_type = FileStorageType::from_u8(storage_type_byte);

        Ok(Self {
            name_offset,
            hash_offset,
            data_offset,
            compressed_length,
            uncompressed_size,
            verification_type,
            storage_type,
            crc,
        })
    }
}
