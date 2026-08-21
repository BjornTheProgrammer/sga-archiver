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
#[derive(Debug, Clone, PartialEq, Eq)]
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

    /// Unknown storage type.
    Unknown(u8),
}

impl FileStorageType {
    pub fn from_u8(value: u8) -> Self {
        match value {
            0 => Self::Store,
            1 => Self::StreamCompress,
            2 => Self::BufferCompress,
            3 => Self::StreamCompressBrotli,
            4 => Self::BufferCompressBrotli,
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
            FileStorageType::Unknown(n) => n,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileEncryptionType {
    None,

    Aes128,

    Unknown(u8),
}

impl FileEncryptionType {
    pub fn from_u8(value: u8) -> Self {
        match value {
            0 => Self::None,
            1 => Self::Aes128,
            _ => Self::Unknown(value),
        }
    }

    pub fn to_u8(self) -> u8 {
        match self {
            FileEncryptionType::None => 0,
            FileEncryptionType::Aes128 => 1,
            FileEncryptionType::Unknown(n) => n,
        }
    }

    pub fn is_encrypted(&self) -> bool {
        !matches!(self, FileEncryptionType::None)
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

    pub encryption_type: FileEncryptionType,

    /// CRC32 checksum of the file's data.
    pub crc: u32,
}

impl SgaFileEntry {
    pub fn parse<T: Read + BufRead>(reader: &mut T, version: u16) -> Result<Self, SgaFileEntryParseError> {
        let name_offset = read_field!(reader, SgaFileEntryParseError::FailedToParseNumber, u32)?;

        let mut hash_offset = 0u32;
        if version >= 8 {
            hash_offset = read_field!(reader, SgaFileEntryParseError::FailedToParseNumber, u32)?;
        }

        let data_offset = if version >= 9 {
            read_field!(reader, SgaFileEntryParseError::FailedToParseNumber, u64)?
        } else {
            read_field!(reader, SgaFileEntryParseError::FailedToParseNumber, u32)? as u64
        };

        let compressed_length = read_field!(reader, SgaFileEntryParseError::FailedToParseNumber, u32)?;
        let uncompressed_size = read_field!(reader, SgaFileEntryParseError::FailedToParseNumber, u32)?;

        if version >= 4 && version < 10 {
            let _ = read_field!(reader, SgaFileEntryParseError::FailedToParseNumber, u32);
        }

        let verification_type = if version >= 7 {
            let verification_type_byte = read_field!(reader, SgaFileEntryParseError::FailedToParseByte, u8)?;
            FileVerificationType::from_u8(verification_type_byte)
                .map_err(|err| SgaFileEntryParseError::FailedToParseVerificationType(err.to_string()))?
        } else {
            let _ = read_field!(reader, SgaFileEntryParseError::FailedToParseByte, u8);
            FileVerificationType::None
        };

        let storage_type_byte = read_field!(reader, SgaFileEntryParseError::FailedToParseByte, u8)?;
        let (storage_type, encryption_type) = if version >= 10 {
            (
                FileStorageType::from_u8(storage_type_byte & 0x0F),
                FileEncryptionType::from_u8(storage_type_byte >> 4),
            )
        } else {
            (FileStorageType::from_u8(storage_type_byte), FileEncryptionType::None)
        };

        let mut crc = 0u32;
        if version >= 6 {
            crc = read_field!(reader, SgaFileEntryParseError::FailedToParseNumber, u32)?;
        }

        if version == 7 {
            hash_offset = read_field!(reader, SgaFileEntryParseError::FailedToParseNumber, u32)?;
        }

        Ok(Self {
            name_offset,
            hash_offset,
            data_offset,
            compressed_length,
            uncompressed_size,
            verification_type,
            storage_type,
            encryption_type,
            crc,
        })
    }
}
