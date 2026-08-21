use std::io::{BufRead, Read, Seek, SeekFrom};

use sga_macros::read_field;
use thiserror::Error;

use crate::{entires::FileEncryptionType, utils::{read_fixed_string, read_index}};

/// Header of an SGA archive.
#[derive(Debug, Clone)]
pub struct SgaHeader {
    /// Magic value of an SGA archive. Should be "_ARCHIVE".
    pub magic: [u8; 8], // "_ARCHIVE" is 8 bytes

    /// Archive version.
    pub version: u16,

    /// Product id.
    pub product: u16,

    /// Name of the archive.
    pub name: String,

    /// Offset where the archive's header blob starts.
    pub header_blob_offset: u64,

    /// Size of the archive's header blob in bytes.
    pub header_blob_length: u32,

    /// Offset where the archive's data blob starts.
    pub data_offset: u64,

    /// Size of the archive's data blob in bytes.
    pub data_blob_length: u64,

    /// Offset relative to HeaderBlobOffset where the archive's table of contents data starts.
    pub toc_data_offset: u32,

    /// Number of tocs at the TocDataOffset.
    pub toc_data_count: u32,

    /// Offset relative to HeaderBlobOffset where the archive's folder data starts.
    pub folder_data_offset: u32,

    /// Number of folders at FolderDataOffset.
    pub folder_data_count: u32,

    /// Offset relative to HeaderBlobOffset where the archive's file data starts.
    pub file_data_offset: u32,

    /// Number of files at FileDataOffset.
    pub file_data_count: u32,

    /// Offset relative to HeaderBlobOffset where the archive's string data starts.
    pub string_offset: u32,

    /// Size of the archive's string data in bytes.
    pub string_length: u32,

    /// Block size of the archive.
    pub block_size: u32,

    pub header_encryption_type: FileEncryptionType,

    /// 2048-bit (256 byte) signature of the archive.
    /// Probably using PKCS#1 in official archives.
    /// Also validated in the game by XORing together 16 byte chunks and comparing against known values.
    pub signature: [u8; 256],

    /// Offset relative to HeaderBlobOffset where the archive's file hash starts.
    pub file_hash_offset: u32,

    /// Size of the archive's file hash in bytes.
    pub file_hash_length: u32,
}

#[derive(Error, Debug)]
pub enum SgaHeaderParseError {
    #[error("Magic value of an SGA archive. Should be \"_ARCHIVE\": `{0}`")]
    MagicValueImproper(String),
    #[error("Failed to parse a number from stream: `{0}`")]
    FailedToParseNumber(String),
    #[error("Failed to parse name from stream: `{0}`")]
    FailedToName(String),
    #[error("Failed to parse signature from stream: `{0}`")]
    SignatureValueImproper(String),
    #[error("Failed to seek position from stream: `{0}`")]
    SeekError(String),
    #[error("Unsupported SGA archive version `{0}` (this crate reads versions 3 through 11)")]
    UnsupportedVersion(u16),
}

impl SgaHeader {
    pub fn parse<T: Read + BufRead + Seek>(reader: &mut T) -> Result<Self, SgaHeaderParseError> {
        let mut magic = [0u8; 8];
        reader.read_exact(&mut magic).map_err(|_| {
            SgaHeaderParseError::MagicValueImproper("Failed to read 8 bytes for magic".to_string())
        })?;

        if &magic != b"_ARCHIVE" {
            let magic_as_str = std::str::from_utf8(&magic).unwrap();
            let header = SgaHeaderParseError::MagicValueImproper(format!(
                "Found magic value of {}",
                magic_as_str
            ));
            return Err(header);
        }

        let version = read_field!(reader, SgaHeaderParseError::FailedToParseNumber, u16)?;
        let product = read_field!(reader, SgaHeaderParseError::FailedToParseNumber, u16)?;

        if version < 3 || version > 11 {
            return Err(SgaHeaderParseError::UnsupportedVersion(version));
        }

        if version < 6 {
            reader.seek_relative(16).map_err(|err| SgaHeaderParseError::SeekError(err.to_string()))?;
        }

        let name =
            read_fixed_string(reader, 64, 2)
                .map_err(|err| SgaHeaderParseError::FailedToName(err.to_string()))?;

        if version < 6 {
            reader.seek_relative(16).map_err(|err| SgaHeaderParseError::SeekError(err.to_string()))?;
        }

        let mut header_blob_offset: Option<u64> = if version >= 9 {
            Some(read_field!(reader, SgaHeaderParseError::FailedToParseNumber, u64)?)
        } else if version >= 8 {
            Some(read_field!(reader, SgaHeaderParseError::FailedToParseNumber, u32)? as u64)
        } else {
            None
        };

        let header_blob_length = read_field!(reader, SgaHeaderParseError::FailedToParseNumber, u32)?;

        let data_offset;
        let mut data_blob_length: u64 = 0;
        if version >= 9 {
            data_offset = read_field!(reader, SgaHeaderParseError::FailedToParseNumber, u64)?;
            data_blob_length = read_field!(reader, SgaHeaderParseError::FailedToParseNumber, u64)?;
        } else if version == 5 {
            data_offset = read_field!(reader, SgaHeaderParseError::FailedToParseNumber, u32)? as u64;
            header_blob_offset = Some(read_field!(reader, SgaHeaderParseError::FailedToParseNumber, u32)? as u64);
        } else {
            data_offset = read_field!(reader, SgaHeaderParseError::FailedToParseNumber, u32)? as u64;
            if version >= 8 {
                data_blob_length = read_field!(reader, SgaHeaderParseError::FailedToParseNumber, u32)? as u64;
            }
        }

        let mut header_encryption_type = FileEncryptionType::None;
        if version >= 11 {
            let _ = read_field!(reader, SgaHeaderParseError::FailedToParseNumber, u16); // Always 1
            let header_encryption_byte = read_field!(reader, SgaHeaderParseError::FailedToParseNumber, u16)?;
            header_encryption_type = FileEncryptionType::from_u8(header_encryption_byte as u8);
        } else {
            let _ = read_field!(reader, SgaHeaderParseError::FailedToParseNumber, u32);
        }

        if version == 5 {
            let _ = read_field!(reader, SgaHeaderParseError::FailedToParseNumber, u32);
            let _ = read_field!(reader, SgaHeaderParseError::FailedToParseNumber, u32);
        }

        let mut signature = [0u8; 256];
        if version >= 8 {
            reader.read_exact(&mut signature).map_err(|_| {
                SgaHeaderParseError::SignatureValueImproper(
                    "Failed to read 256 bytes for signature".to_string(),
                )
            })?;
        }

        let header_blob_offset = match header_blob_offset {
            Some(offset) => offset,
            None => reader.stream_position().map_err(|err| SgaHeaderParseError::SeekError(err.to_string()))?,
        };

        reader.seek(SeekFrom::Start(header_blob_offset)).map_err(|err| SgaHeaderParseError::SeekError(err.to_string()))?;

        // BaseStream.Seek((long)blobOffset, SeekOrigin.Begin);

        let index = |reader: &mut T| read_index(reader, version)
            .map_err(|err| SgaHeaderParseError::FailedToParseNumber(err.to_string()));

        let toc_data_offset = read_field!(reader, SgaHeaderParseError::FailedToParseNumber, u32)?;
        let toc_data_count = index(reader)?;

        let folder_data_offset = read_field!(reader, SgaHeaderParseError::FailedToParseNumber, u32)?;
        let folder_data_count = index(reader)?;

        let file_data_offset = read_field!(reader, SgaHeaderParseError::FailedToParseNumber, u32)?;
        let file_data_count = index(reader)?;

        let string_offset = read_field!(reader, SgaHeaderParseError::FailedToParseNumber, u32)?;
        let string_length = index(reader)?;

        let mut file_hash_offset = 0u32;
        let mut file_hash_length = 0u32;
        let mut block_size = 0u32;
        if version >= 7 {
            file_hash_offset = read_field!(reader, SgaHeaderParseError::FailedToParseNumber, u32)?;
            if version >= 8 {
                file_hash_length = read_field!(reader, SgaHeaderParseError::FailedToParseNumber, u32)?;
            }
            block_size = read_field!(reader, SgaHeaderParseError::FailedToParseNumber, u32)?;
        }

        Ok(Self {
            magic,
            version,
            product,
            name,
            header_blob_offset,
            header_blob_length,
            data_offset,
            data_blob_length,
            toc_data_offset,
            toc_data_count,
            folder_data_offset,
            folder_data_count,
            file_data_offset,
            file_data_count,
            string_offset,
            string_length,
            file_hash_offset,
            file_hash_length,
            block_size,
            header_encryption_type,
            signature,
        })
    }
}
