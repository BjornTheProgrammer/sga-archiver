use std::{
    collections::HashMap,
    io::{self, BufRead, Read, Seek, SeekFrom},
};

use byteorder::{LittleEndian, ReadBytesExt};
use thiserror::Error;

use crate::chunky::{ChunkFile, ChunkHeader, ChunkType};

#[derive(Debug)]
pub struct RelicGameData {}

#[derive(Error, Debug)]
pub enum RelicGameDataError {
    // #[error("data store disconnected")]
    // Disconnect(),
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    #[error("No DATA KEYS chunk present")]
    MissingDataKeysChunk,

    #[error("More than one DATA KEYS chunk present")]
    MultipleDataKeysChunks,

    #[error("No DATA AEGD chunk present")]
    MissingDataAegdChunk,

    #[error("More than one DATA AEGD chunk present")]
    MultipleDataAegdChunks,

    #[error("Invalid data type")]
    InvalidDataType(#[from] Box<dyn std::error::Error>),

    #[error("Unknown data type {0}")]
    UnknownDataType(i32),
}

#[derive(Debug, Clone)]
pub enum RGDValue {
    Float(f32),
    Int(i32),
    Boolean(bool),
    CString(String),
    List(Vec<RGDEntry>),
}

#[derive(Debug, Clone)]
pub struct RGDEntry {
    pub key_hash: u64,
    pub value: RGDValue,
}

#[derive(Debug, Clone)]
pub struct RGDNode {
    pub key: String,
    pub value: RGDValue,
}

impl RelicGameData {
    pub fn parse<R: Read + BufRead + Seek>(
        chunk_file: &mut ChunkFile<R>,
    ) -> Result<Vec<RGDNode>, RelicGameDataError> {
        let mut keys_chunk_header = None;
        let mut kvs_chunk_header = None;

        for chunk in &chunk_file.chunks {
            if chunk.chunk_type == ChunkType::Data {
                if chunk.name == "KEYS" {
                    if let Some(_) = keys_chunk_header {
                        return Err(RelicGameDataError::MultipleDataKeysChunks);
                    }
                    keys_chunk_header = Some(chunk);
                } else if chunk.name == "AEGD" {
                    if let Some(_) = kvs_chunk_header {
                        return Err(RelicGameDataError::MultipleDataAegdChunks);
                    }
                    kvs_chunk_header = Some(chunk);
                }
            }
        }

        let keys_chunk_header = match keys_chunk_header {
            Some(keys) => keys,
            None => return Err(RelicGameDataError::MissingDataKeysChunk),
        };

        let kvs_chunk_header = match kvs_chunk_header {
            Some(kvs) => kvs,
            None => return Err(RelicGameDataError::MissingDataAegdChunk),
        };

        let keys = Self::parse_keys(&mut chunk_file.reader, keys_chunk_header)?;
        println!("keys: {:?}", keys);

        let entries = Self::parse_aegd(&mut chunk_file.reader, kvs_chunk_header)?;
        println!("entries: {:?}", entries);

        Ok(Self::resolve_nodes(&entries, &keys))
    }

    fn read_chunky_list<R: Read + Seek>(
        reader: &mut R,
    ) -> Result<Vec<RGDEntry>, RelicGameDataError> {
        let length = reader.read_u32::<LittleEndian>()? as usize;

        let mut index_entries = Vec::with_capacity(length);
        for _ in 0..length {
            let key = reader.read_u64::<LittleEndian>()?;
            let data_type = reader.read_i32::<LittleEndian>()?;
            let data_offset = reader.read_i32::<LittleEndian>()?;
            index_entries.push((key, data_type, data_offset));
        }

        let data_start = reader.stream_position()?;

        let mut entries = Vec::with_capacity(length);
        for (key, data_type, offset) in index_entries {
            reader.seek(SeekFrom::Start(data_start + offset as u64))?;
            let value = match data_type {
                0 => RGDValue::Float(reader.read_f32::<LittleEndian>()?),
                1 => RGDValue::Int(reader.read_i32::<LittleEndian>()?),
                2 => RGDValue::Boolean(reader.read_u8()? != 0),
                3 => {
                    let mut bytes = Vec::new();
                    loop {
                        let b = reader.read_u8()?;
                        if b == 0 {
                            break;
                        }
                        bytes.push(b);
                    }
                    RGDValue::CString(String::from_utf8_lossy(&bytes).into_owned())
                }
                100 | 101 => RGDValue::List(Self::read_chunky_list(reader)?),
                other => return Err(RelicGameDataError::UnknownDataType(other)),
            };
            entries.push(RGDEntry {
                key_hash: key,
                value,
            });
        }

        Ok(entries)
    }

    fn parse_aegd<R: Read + Seek>(
        reader: &mut R,
        chunk: &ChunkHeader,
    ) -> Result<Vec<RGDEntry>, RelicGameDataError> {
        reader.seek(SeekFrom::Start(chunk.data_position_start))?;
        let _unknown = reader.read_u32::<LittleEndian>()?;
        Self::read_chunky_list(reader)
    }

    pub fn parse_keys<R: Read + Seek>(
        reader: &mut R,
        chunk: &ChunkHeader,
    ) -> Result<HashMap<u64, String>, RelicGameDataError> {
        let mut key_string_map = HashMap::new();
        reader.seek(SeekFrom::Start(chunk.data_position_start))?;

        let count = reader.read_u32::<LittleEndian>()?;

        for _ in 0..count {
            let key = reader.read_u64::<LittleEndian>()?;
            let string_length = reader.read_u32::<LittleEndian>()?;

            let string = {
                let mut string_bytes = vec![0u8; string_length as usize];
                reader.read_exact(&mut string_bytes)?;
                String::from_utf8_lossy(&string_bytes).to_string()
            };

            key_string_map.insert(key, string);
        }

        Ok(key_string_map)
    }

    fn resolve_nodes(entries: &[RGDEntry], keys: &HashMap<u64, String>) -> Vec<RGDNode> {
        entries
            .iter()
            .map(|entry| {
                let key = keys
                    .get(&entry.key_hash)
                    .cloned()
                    .unwrap_or_else(|| format!("unknown_{}", entry.key_hash));
                let value = match &entry.value {
                    RGDValue::List(children) => {
                        let resolved: Vec<RGDEntry> = Self::resolve_nodes(children, keys)
                            .into_iter()
                            .map(|node| RGDEntry {
                                key_hash: entry.key_hash,
                                value: node.value,
                            })
                            .collect();
                        RGDValue::List(resolved)
                    }
                    other => other.clone(),
                };
                RGDNode { key, value }
            })
            .collect()
    }
}
