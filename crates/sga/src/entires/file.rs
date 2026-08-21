use binrw::binrw;

use crate::utils::{parse_wide, write_wide};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileVerificationType {
    None,
    CRC,
    CRCBlocks,
    MD5Blocks,
    SHA1Blocks,
}

impl FileVerificationType {
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

    pub fn to_u8(&self) -> u8 {
        match self {
            FileVerificationType::None => 0,
            FileVerificationType::CRC => 1,
            FileVerificationType::CRCBlocks => 2,
            FileVerificationType::MD5Blocks => 3,
            FileVerificationType::SHA1Blocks => 4,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileStorageType {
    Store,
    StreamCompress,
    BufferCompress,
    StreamCompressBrotli,
    BufferCompressBrotli,
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

    pub fn to_u8(&self) -> u8 {
        match self {
            FileStorageType::Store => 0,
            FileStorageType::StreamCompress => 1,
            FileStorageType::BufferCompress => 2,
            FileStorageType::StreamCompressBrotli => 3,
            FileStorageType::BufferCompressBrotli => 4,
            FileStorageType::Unknown(n) => *n,
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

    pub fn to_u8(&self) -> u8 {
        match self {
            FileEncryptionType::None => 0,
            FileEncryptionType::Aes128 => 1,
            FileEncryptionType::Unknown(n) => *n,
        }
    }

    pub fn is_encrypted(&self) -> bool {
        !matches!(self, FileEncryptionType::None)
    }
}

#[binrw]
#[brw(little, import(version: u16))]
#[derive(Debug, Clone)]
pub struct SgaFileEntry {
    pub name_offset: u32,

    #[brw(if(version >= 8))]
    pub hash_offset: u32,

    #[br(parse_with = parse_wide, args(version))]
    #[bw(write_with = write_wide, args(version))]
    pub data_offset: u64,

    pub compressed_length: u32,

    pub uncompressed_size: u32,

    #[brw(if(version >= 4 && version < 10))]
    pub unknown: u32,

    pub verification_byte: u8,

    pub storage_byte: u8,

    #[brw(if(version >= 6))]
    pub crc: u32,

    #[brw(if(version == 7))]
    pub hash_offset_v7: u32,
}
