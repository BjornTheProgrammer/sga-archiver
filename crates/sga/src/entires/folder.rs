use std::io::{BufRead, Read};

use sga_macros::read_field;
use thiserror::Error;

use crate::utils::read_index;

/// Folder entry of an SGA archive.
#[derive(Debug, Clone)]
pub struct SgaFolderEntry {
    /// Offset of the folder's name in the SGA archive's string blob.
    pub name_offset: u32,

    /// Index of the first child folder.
    pub folder_start_index: u32,

    /// Index past the last child folder.
    pub folder_end_index: u32,

    /// Index of the first child file.
    pub file_start_index: u32,

    /// Index past the last child file.
    pub file_end_index: u32,
}

#[derive(Error, Debug)]
pub enum SgaFolderEntryParseError {
    #[error("Failed to read number from stream `{0}`")]
    FailedToParseNumber(String),
}

impl SgaFolderEntry {
    pub fn parse<T: Read + BufRead>(reader: &mut T, version: u16) -> Result<Self, SgaFolderEntryParseError> {
        let name_offset = read_field!(reader, SgaFolderEntryParseError::FailedToParseNumber, u32)?;
        let idx = |reader: &mut T| read_index(reader, version)
            .map_err(|err| SgaFolderEntryParseError::FailedToParseNumber(err.to_string()));
        let folder_start_index = idx(reader)?;
        let folder_end_index = idx(reader)?;
        let file_start_index = idx(reader)?;
        let file_end_index = idx(reader)?;

        Ok(Self {
            name_offset,
            folder_start_index,
            folder_end_index,
            file_start_index,
            file_end_index,
        })
    }
}
