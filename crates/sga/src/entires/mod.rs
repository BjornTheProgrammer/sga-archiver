mod header;
use anyhow::Result;
pub use header::*;

mod toc;
pub use toc::*;

mod folder;
pub use folder::*;

mod file;
pub use file::*;

use std::io::{BufRead, Read, Seek, SeekFrom};

#[derive(Debug)]
pub struct SgaEntries {
    pub header: SgaHeader,
    pub tocs: Vec<SgaToC>,
    pub folders: Vec<SgaFolderEntry>,
    pub files: Vec<SgaFileEntry>,
}

impl SgaEntries {
    pub fn new<T: Read + BufRead + Seek>(reader: &mut T) -> Result<Self> {
        let header = SgaHeader::parse(reader)?;

        
        reader.seek(SeekFrom::Start(
            header.header_blob_offset + header.toc_data_offset as u64,
        ))?;

        let mut table_of_contents = Vec::with_capacity(header.toc_data_count as usize);
        for _ in 0..header.toc_data_count {
            table_of_contents.push(SgaToC::parse(reader, header.version)?);
        }


        reader.seek(SeekFrom::Start(
            header.header_blob_offset + header.folder_data_offset as u64,
        ))?;

        let mut folders = Vec::with_capacity(header.folder_data_count as usize);
        for _ in 0..header.folder_data_count {
            folders.push(SgaFolderEntry::parse(reader, header.version)?);
        }


        reader.seek(SeekFrom::Start(
            header.header_blob_offset + header.file_data_offset as u64,
        ))?;

        let mut files = Vec::with_capacity(header.file_data_count as usize);
        for _ in 0..header.file_data_count {
            files.push(SgaFileEntry::parse(reader, header.version)?);
        }

        Ok(Self {
            header,
            tocs: table_of_contents,
            folders,
            files,
        })
    }
}
