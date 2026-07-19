use byteorder::{LittleEndian, ReadBytesExt};
use std::io::{self, Read, Seek, SeekFrom};
use thiserror::Error;

#[derive(Debug, Clone)]
pub struct ChunkyFileHeader {
    pub major: u16,
    pub minor: u16,
    pub platform: u32,
}

impl ChunkyFileHeader {
    pub fn parse<R: Read>(mut reader: R) -> Result<Self, DataStoreError> {
        let mut magic = [0u8; 16];
        reader.read_exact(&mut magic)?;

        if &magic != b"Relic Chunky\r\n\x1A\0" {
            return Err(DataStoreError::InvalidMagic);
        }

        let major = reader.read_u16::<LittleEndian>()?;
        let minor = reader.read_u16::<LittleEndian>()?;
        let platform = reader.read_u32::<LittleEndian>()?;

        Ok(ChunkyFileHeader {
            major,
            minor,
            platform,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ChunkType {
    Data,
    Folder,
    Unknown(String),
}

#[derive(Debug, Clone)]
pub struct ChunkHeader {
    pub chunk_type: ChunkType,
    pub name: String,
    pub version: u32,
    pub length: u32,
    pub path: String,
    pub data_position_start: u64,
}

#[derive(Error, Debug)]
pub enum DataStoreError {
    #[error("invalid magic word in header")]
    InvalidMagic,
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
}

impl ChunkHeader {
    pub fn parse<R: Read + Seek>(mut reader: R) -> Result<Self, DataStoreError> {
        let chunk_type = {
            let mut type_bytes = [0u8; 4];
            reader.read_exact(&mut type_bytes)?;
            match &type_bytes {
                b"DATA" => ChunkType::Data,
                b"FOLD" => ChunkType::Folder,
                _ => {
                    let s = String::from_utf8_lossy(&type_bytes).to_string();
                    ChunkType::Unknown(s)
                }
            }
        };

        let name = {
            let mut type_bytes = [0u8; 4];
            reader.read_exact(&mut type_bytes)?;
            String::from_utf8_lossy(&type_bytes).to_string()
        };

        let version = reader.read_u32::<LittleEndian>()?;

        let length = reader.read_u32::<LittleEndian>()?;

        let path_length = reader.read_u32::<LittleEndian>()? as usize;
        let mut path_bytes = vec![0u8; path_length];
        reader.read_exact(&mut path_bytes)?;
        let path = String::from_utf8_lossy(&path_bytes).to_string();

        let current_pos = reader.seek(SeekFrom::Current(0))?;

        Ok(Self {
            chunk_type,
            name,
            version,
            length,
            path,
            data_position_start: current_pos,
        })
    }
}

#[derive(Debug)]
pub struct ChunkFile<R: Read + Seek> {
    pub header: ChunkyFileHeader,
    pub chunks: Vec<ChunkHeader>,
    pub reader: R,
}

impl<R: Read + Seek> ChunkFile<R> {
    pub fn parse(mut reader: R) -> Result<Self, DataStoreError> {
        let header = ChunkyFileHeader::parse(&mut reader)?;
        let mut chunks = Vec::new();

        loop {
            match ChunkHeader::parse(&mut reader) {
                Ok(chunk_header) => {
                    reader.seek(SeekFrom::Start(
                        chunk_header.data_position_start + chunk_header.length as u64,
                    ))?;
                    chunks.push(chunk_header);
                }
                Err(DataStoreError::Io(ref e)) if e.kind() == io::ErrorKind::UnexpectedEof => {
                    break;
                }
                Err(e) => return Err(e),
            }
        }

        Ok(ChunkFile {
            header,
            chunks,
            reader,
        })
    }

    pub fn extract_chunk_data(&mut self, chunk: &ChunkHeader) -> Result<Vec<u8>, DataStoreError> {
        self.reader
            .seek(SeekFrom::Start(chunk.data_position_start))?;
        let mut data = vec![0u8; chunk.length as usize];
        self.reader.read_exact(&mut data)?;
        Ok(data)
    }
}
