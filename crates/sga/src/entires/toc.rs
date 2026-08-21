use std::io::{BufRead, Read};

use thiserror::Error;

use crate::utils::{read_fixed_string, read_index};

/// Table of contents entry of an SGA archive.
#[derive(Debug, Clone)]
pub struct SgaToC {
    /// Alias of the table of contents.
    pub alias: String,

    /// Name of the table of contents.
    pub name: String,

    /// Index of the first child folder.
    pub folder_start_index: u32,

    /// Index past the last child folder.
    pub folder_end_index: u32,

    /// Index of the first child file.
    pub file_start_index: u32,

    /// Index past the last child file.
    pub file_end_index: u32,

    /// Index of the root folder.
    pub folder_root_index: u32,
}

#[derive(Error, Debug)]
pub enum SgaTocParseError {
    #[error("Failed to parse alias: `{0}`")]
    FailedToParseAlias(String),
    #[error("Failed to parse name: `{0}`")]
    FailedToParseName(String),
    #[error("Failed to parse number: `{0}`")]
    FailedToParseNumber(String),
}

impl SgaToC {
    pub fn parse<T: Read + BufRead>(reader: &mut T, version: u16) -> Result<Self, SgaTocParseError> {
        let alias = read_fixed_string(reader, 64, 1)
            .map_err(|err| SgaTocParseError::FailedToParseAlias(err.to_string()))?;

        let name = read_fixed_string(reader, 64, 1)
            .map_err(|err| SgaTocParseError::FailedToParseName(err.to_string()))?;

        let idx = |reader: &mut T| read_index(reader, version)
            .map_err(|err| SgaTocParseError::FailedToParseNumber(err.to_string()));
        let folder_start_index = idx(reader)?;
        let folder_end_index = idx(reader)?;
        let file_start_index = idx(reader)?;
        let file_end_index = idx(reader)?;
        let folder_root_index = idx(reader)?;

        Ok(Self {
            alias,
            name,
            folder_start_index,
            folder_end_index,
            file_start_index,
            file_end_index,
            folder_root_index,
        })
    }
}
