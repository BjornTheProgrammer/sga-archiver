use std::{io::{BufRead, Read, Seek, SeekFrom}, sync::{Arc, Mutex}};

use anyhow::Result;
use brotli::Decompressor;
use flate2::read::DeflateDecoder;

use crate::{entires::{FileEncryptionType, FileStorageType, SgaEntries, SgaFileEntry}, utils::read_c_string};

use super::FolderNode;

/// Represents a file in the file system.
#[derive(Debug, Clone)]
pub struct FileNode {
    /// The name of the file
    pub name: String,

    /// The parent of the file
    pub parent: Arc<Mutex<FolderNode>>,

    data_position: u64,
    data_length: usize,
    data_uncompressed_length: usize,

    /// The type of file storage it is
    pub storage_type: FileStorageType,

    pub encryption_type: FileEncryptionType,
}

impl FileNode {
    /// Instantiate a new file node
    pub fn new<U: AsRef<str>>(name: U, data_position: u64, data_length: usize, data_uncompressed_length: usize, storage_type: FileStorageType, encryption_type: FileEncryptionType, parent: Arc<Mutex<FolderNode>>) -> Self {
        Self {
            name: name.as_ref().to_string(),
            parent: parent,

            data_position,
            data_length,
            data_uncompressed_length,

            storage_type,
            encryption_type,
        }
    }

    /// Reads the data from the file and returns it as a vector.
    ///
    /// `FileStorageType::Aes128Encrypted` files can't actually be decrypted
    /// by this crate (see `docs/storage-type-18.md`), so the bytes returned
    /// for them are still ciphertext.
    pub fn read_data<T: Read + Seek>(&self, reader: &mut T) -> Result<Vec<u8>> {
        reader.seek(SeekFrom::Start(self.data_position))?;

        if self.encryption_type.is_encrypted() {
            let mut data = vec![0u8; self.data_length];
            reader.read_exact(&mut data)?;
            return Ok(data);
        }

        match &self.storage_type {
            FileStorageType::Store | FileStorageType::Unknown(_) => {
                let mut data = vec![0u8; self.data_length as usize];
                reader.read_exact(&mut data)?;
                Ok(data)
            },
            FileStorageType::StreamCompress | FileStorageType::BufferCompress => {
                reader.seek_relative(2)?;
                let mut deflate_stream = DeflateDecoder::new(reader);
                let mut decoded = vec![0u8; self.data_uncompressed_length as usize];
                deflate_stream.read_exact(&mut decoded)?;
                Ok(decoded)
            },
            FileStorageType::StreamCompressBrotli | FileStorageType::BufferCompressBrotli => {
                let mut brotli_stream = Decompressor::new(reader, 4096); // 4KB buffer
                let mut decoded = vec![0u8; self.data_uncompressed_length as usize];
                brotli_stream.read_exact(&mut decoded)?;
                Ok(decoded)
            },
        }
    }

    /// Constructs a file node from an SgaFileEntry
    /// Files are always expected to have a parent, that is why it is a required field here
    pub fn from_file_entry<T: Read + BufRead + Seek>(reader: &mut T, entries: &SgaEntries, file_entry: SgaFileEntry, parent: Arc<Mutex<FolderNode>>) -> anyhow::Result<Self> {
        reader.seek(SeekFrom::Start(entries.header.header_blob_offset + entries.header.string_offset as u64 + file_entry.name_offset as u64))?;
        let file_name = read_c_string(reader)?;

        Ok(Self {
            name: file_name,
            data_position: entries.header.data_offset + file_entry.data_offset,
            data_length: file_entry.compressed_length as usize,
            data_uncompressed_length: file_entry.uncompressed_size as usize,
            storage_type: file_entry.storage_type,
            encryption_type: file_entry.encryption_type,
            parent
        })
    }
}
