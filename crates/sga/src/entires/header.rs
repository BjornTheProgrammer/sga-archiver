use std::io::{self, BufRead, Read, Seek, SeekFrom, Write};

use binrw::{binrw, BinRead, BinWrite};
use thiserror::Error;

use crate::entires::FileEncryptionType;
use crate::utils::{name_units, parse_index, parse_wide, utf16_name, write_index, write_wide};

#[derive(Debug, Clone, PartialEq)]
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

    pub reserved: HeaderReserved,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HeaderReserved {
    /// 16-byte block before the name (v<6; likely a checksum).
    pub pre_name: [u8; 16],
    /// 16-byte block after the name (v<6; likely a checksum).
    pub post_name: [u8; 16],
    /// Two trailing words after the pad (v5 only).
    pub v5: [u32; 2],
    /// Word where v11 puts its encryption pair (v<11; observed as 1).
    pub pad: u32,
    /// Half-word before the encryption type (v11+; observed always 1).
    pub v11_one: u16,
}

impl Default for HeaderReserved {
    fn default() -> Self {
        HeaderReserved {
            pre_name: [0; 16],
            post_name: [0; 16],
            v5: [0; 2],
            // Both observed as 1 in editor/game archives.
            pad: 1,
            v11_one: 1,
        }
    }
}

#[binrw]
#[brw(little, import(version: u16))]
#[derive(Debug, Clone)]
struct MainHeaderRest {
    #[br(if(version < 6, [0u8; 16]))]
    #[bw(if(version < 6))]
    pre_name: [u8; 16],

    name: [u16; 64],

    #[br(if(version < 6, [0u8; 16]))]
    #[bw(if(version < 6))]
    post_name: [u8; 16],

    #[brw(if(version >= 9))]
    header_blob_offset64: u64,

    #[brw(if(version == 8))]
    header_blob_offset32: u32,

    header_blob_length: u32,

    #[br(parse_with = parse_wide, args(version))]
    #[bw(write_with = write_wide, args(version))]
    data_offset: u64,

    #[brw(if(version >= 9))]
    data_blob_length64: u64,

    #[brw(if(version == 5))]
    header_blob_offset_v5: u32,

    #[brw(if(version == 8))]
    data_blob_length32: u32,

    #[brw(if(version >= 11))]
    v11_one: u16,

    #[brw(if(version >= 11))]
    encryption: u16,

    #[brw(if(version < 11))]
    pad: u32,

    #[br(if(version == 5, [0u32; 2]))]
    #[bw(if(version == 5))]
    v5_extra: [u32; 2],

    #[br(if(version >= 8, [0u8; 256]))]
    #[bw(if(version >= 8))]
    signature: [u8; 256],
}

#[binrw]
#[brw(little, import(version: u16))]
#[derive(Debug, Clone)]
struct IndexTable {
    toc_data_offset: u32,
    #[br(parse_with = parse_index, args(version))]
    #[bw(write_with = write_index, args(version))]
    toc_data_count: u32,

    folder_data_offset: u32,
    #[br(parse_with = parse_index, args(version))]
    #[bw(write_with = write_index, args(version))]
    folder_data_count: u32,

    file_data_offset: u32,
    #[br(parse_with = parse_index, args(version))]
    #[bw(write_with = write_index, args(version))]
    file_data_count: u32,

    string_offset: u32,
    #[br(parse_with = parse_index, args(version))]
    #[bw(write_with = write_index, args(version))]
    string_length: u32,

    #[brw(if(version >= 7))]
    file_hash_offset: u32,

    #[brw(if(version >= 8))]
    file_hash_length: u32,

    #[brw(if(version >= 7))]
    block_size: u32,
}

#[derive(Error, Debug)]
pub enum SgaHeaderParseError {
    #[error("bad magic: expected \"_ARCHIVE\", found {0:?}")]
    MagicValueImproper(String),
    #[error("unsupported SGA archive version `{0}` (this crate handles versions 3 through 11)")]
    UnsupportedVersion(u16),
    #[error("failed to read header: {0}")]
    Io(#[from] io::Error),
    #[error("failed to parse header: {0}")]
    Binrw(#[from] binrw::Error),
}

fn check_version(version: u16) -> Result<(), SgaHeaderParseError> {
    if (3..=11).contains(&version) {
        Ok(())
    } else {
        Err(SgaHeaderParseError::UnsupportedVersion(version))
    }
}

impl SgaHeader {
    pub fn parse<T: Read + BufRead + Seek>(reader: &mut T) -> Result<Self, SgaHeaderParseError> {
        let mut magic = [0u8; 8];
        reader.read_exact(&mut magic)?;
        if &magic != b"_ARCHIVE" {
            return Err(SgaHeaderParseError::MagicValueImproper(
                String::from_utf8_lossy(&magic).into_owned(),
            ));
        }
        let mut word = [0u8; 2];
        reader.read_exact(&mut word)?;
        let version = u16::from_le_bytes(word);
        reader.read_exact(&mut word)?;
        let product = u16::from_le_bytes(word);
        check_version(version)?;

        let main = MainHeaderRest::read_args(reader, (version,))?;

        // v9+ and v8 store the header blob offset directly, v5 stores it after
        // the data offset; older versions place the blob right where the main
        // header ends.
        let stored_offset = match version {
            9.. => Some(main.header_blob_offset64),
            8 => Some(main.header_blob_offset32 as u64),
            5 => Some(main.header_blob_offset_v5 as u64),
            _ => None,
        };
        let header_blob_offset = match stored_offset {
            Some(offset) => offset,
            None => reader.stream_position()?,
        };
        reader.seek(SeekFrom::Start(header_blob_offset))?;

        let index = IndexTable::read_args(reader, (version,))?;

        Ok(SgaHeader {
            magic,
            version,
            product,
            name: utf16_name(&main.name),
            header_blob_offset,
            header_blob_length: main.header_blob_length,
            data_offset: main.data_offset,
            data_blob_length: if version >= 9 {
                main.data_blob_length64
            } else {
                main.data_blob_length32 as u64
            },
            toc_data_offset: index.toc_data_offset,
            toc_data_count: index.toc_data_count,
            folder_data_offset: index.folder_data_offset,
            folder_data_count: index.folder_data_count,
            file_data_offset: index.file_data_offset,
            file_data_count: index.file_data_count,
            string_offset: index.string_offset,
            string_length: index.string_length,
            block_size: index.block_size,
            header_encryption_type: FileEncryptionType::from_u8(main.encryption as u8),
            signature: main.signature,
            file_hash_offset: index.file_hash_offset,
            file_hash_length: index.file_hash_length,
            reserved: {
                let defaults = HeaderReserved::default();
                HeaderReserved {
                    pre_name: if version < 6 { main.pre_name } else { defaults.pre_name },
                    post_name: if version < 6 { main.post_name } else { defaults.post_name },
                    v5: if version == 5 { main.v5_extra } else { defaults.v5 },
                    pad: if version < 11 { main.pad } else { defaults.pad },
                    v11_one: if version >= 11 { main.v11_one } else { defaults.v11_one },
                }
            },
        })
    }

    fn main_rest(&self) -> MainHeaderRest {
        MainHeaderRest {
            pre_name: self.reserved.pre_name,
            name: name_units(&self.name),
            post_name: self.reserved.post_name,
            header_blob_offset64: self.header_blob_offset,
            header_blob_offset32: self.header_blob_offset as u32,
            header_blob_length: self.header_blob_length,
            data_offset: self.data_offset,
            data_blob_length64: self.data_blob_length,
            header_blob_offset_v5: self.header_blob_offset as u32,
            data_blob_length32: self.data_blob_length as u32,
            v11_one: self.reserved.v11_one,
            encryption: self.header_encryption_type.to_u8() as u16,
            pad: self.reserved.pad,
            v5_extra: self.reserved.v5,
            signature: self.signature,
        }
    }

    fn index_table(&self) -> IndexTable {
        IndexTable {
            toc_data_offset: self.toc_data_offset,
            toc_data_count: self.toc_data_count,
            folder_data_offset: self.folder_data_offset,
            folder_data_count: self.folder_data_count,
            file_data_offset: self.file_data_offset,
            file_data_count: self.file_data_count,
            string_offset: self.string_offset,
            string_length: self.string_length,
            file_hash_offset: self.file_hash_offset,
            file_hash_length: self.file_hash_length,
            block_size: self.block_size,
        }
    }

    pub fn write_main_header<W: Write + Seek>(&self, writer: &mut W) -> io::Result<()> {
        check_version(self.version).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        writer.write_all(&self.magic)?;
        writer.write_all(&self.version.to_le_bytes())?;
        writer.write_all(&self.product.to_le_bytes())?;
        self.main_rest()
            .write_args(writer, (self.version,))
            .map_err(io::Error::other)?;
        Ok(())
    }

    pub fn write_index_table<W: Write + Seek>(&self, writer: &mut W) -> io::Result<()> {
        check_version(self.version).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        self.index_table()
            .write_args(writer, (self.version,))
            .map_err(io::Error::other)?;
        Ok(())
    }
}
