use std::{
    collections::HashMap,
    io::{self, BufRead, Read, Seek, SeekFrom},
};

use byteorder::{LittleEndian, ReadBytesExt};
use quick_xml::{
    Writer,
    events::{BytesDecl, BytesEnd, BytesStart, Event},
};
use serde::Serialize;
use thiserror::Error;

use crate::chunky::{ChunkFile, ChunkHeader, ChunkType};

#[derive(Debug)]
pub struct RelicGameData {}

#[derive(Error, Debug)]
pub enum RelicGameDataError {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    #[error("chunk parse error: {0}")]
    Chunk(#[from] crate::chunky::DataStoreError),

    #[error("No DATA KEYS chunk present")]
    MissingDataKeysChunk,

    #[error("More than one DATA KEYS chunk present")]
    MultipleDataKeysChunks,

    #[error("No DATA AEGD chunk present")]
    MissingDataAegdChunk,

    #[error("More than one DATA AEGD chunk present")]
    MultipleDataAegdChunks,

    #[error("Unknown data type {0}")]
    UnknownDataType(i32),

    #[error("Entry for key {key} has out of bounds data offset {offset}")]
    InvalidDataOffset { key: u64, offset: i32 },
}

/// Data type of a DATA AEGD chunk entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RGDDataType {
    Float = 0,
    Int = 1,
    Boolean = 2,
    CString = 3,
    /// Null-terminated UTF-16LE string. Seen holding loc-string references
    /// (e.g. `$b5110754a5a76448b8648ef56c1eadc1b:38`), the same `$<hex>:<n>`
    /// shape as `Loc_FormatText` calls in SCAR. CString (3) is single-byte
    /// and used for plain identifiers/paths; this is its wide counterpart.
    LocString = 4,
    List = 100,
    /// A second tag that also encodes a list.
    List2 = 101,
}

impl RGDDataType {
    pub fn name(&self) -> &'static str {
        match self {
            RGDDataType::Float => "Float",
            RGDDataType::Int => "Int",
            RGDDataType::Boolean => "Boolean",
            RGDDataType::CString => "CString",
            RGDDataType::LocString => "LocString",
            RGDDataType::List | RGDDataType::List2 => "List",
        }
    }
}

/// Value of a DATA AEGD entry, with its keys still stored as hashes.
#[derive(Debug, Clone, PartialEq)]
pub enum RGDValue {
    Float(f32),
    Int(i32),
    Boolean(bool),
    CString(String),
    LocString(String),
    List(Vec<RGDEntry>),
}

/// Entry of a DATA AEGD chunk. The key's string representation can be found in
/// the DATA KEYS chunk; use [`RelicGameData::resolve_nodes`] to resolve it.
#[derive(Debug, Clone, PartialEq)]
pub struct RGDEntry {
    pub key_hash: u64,
    pub value: RGDValue,
}

/// Value of an [`RGDNode`], with its keys resolved against the DATA KEYS chunk.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum RGDNodeValue {
    Float(f32),
    Int(i32),
    Boolean(bool),
    CString(String),
    LocString(String),
    List(Vec<RGDNode>),
}

impl RGDNodeValue {
    pub fn data_type(&self) -> RGDDataType {
        match self {
            RGDNodeValue::Float(_) => RGDDataType::Float,
            RGDNodeValue::Int(_) => RGDDataType::Int,
            RGDNodeValue::Boolean(_) => RGDDataType::Boolean,
            RGDNodeValue::CString(_) => RGDDataType::CString,
            RGDNodeValue::LocString(_) => RGDDataType::LocString,
            RGDNodeValue::List(_) => RGDDataType::List,
        }
    }
}

/// Key-value node found in DATA AEGD chunks of Relic Game Data (RGD) files.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RGDNode {
    pub key: String,
    pub value: RGDNodeValue,
}

impl RelicGameData {
    /// Reads the nodes of a Relic Game Data (RGD) file, with every key resolved
    /// against the file's DATA KEYS chunk.
    pub fn parse<R: Read + BufRead + Seek>(
        chunk_file: &mut ChunkFile<R>,
    ) -> Result<Vec<RGDNode>, RelicGameDataError> {
        // KEYS/AEGD are not always top-level: win-condition RDO files (unlike
        // plain attrib .rgd files) nest them inside a FOLD chunk, so every
        // FOLD chunk's own children have to be walked too.
        let all_chunks = Self::flatten_data_chunks(&mut chunk_file.reader, &chunk_file.chunks)?;

        let mut keys_chunk_header = None;
        let mut kvs_chunk_header = None;

        for chunk in &all_chunks {
            if chunk.chunk_type == ChunkType::Data {
                if chunk.name == "KEYS" {
                    if keys_chunk_header.is_some() {
                        return Err(RelicGameDataError::MultipleDataKeysChunks);
                    }
                    keys_chunk_header = Some(chunk);
                } else if chunk.name == "AEGD" {
                    if kvs_chunk_header.is_some() {
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
        let entries = Self::parse_aegd(&mut chunk_file.reader, kvs_chunk_header)?;

        Ok(Self::resolve_nodes(&entries, &keys))
    }

    /// Recursively descends into every FOLD chunk in `chunks`, returning a
    /// flat list of every chunk found (FOLD chunks included, so callers can
    /// still tell where each DATA chunk came from if needed).
    pub fn flatten_data_chunks<R: Read + Seek>(
        reader: &mut R,
        chunks: &[ChunkHeader],
    ) -> Result<Vec<ChunkHeader>, RelicGameDataError> {
        let mut out = Vec::new();
        for chunk in chunks {
            if chunk.chunk_type == ChunkType::Folder {
                let end = chunk.data_position_start + chunk.length as u64;
                reader.seek(SeekFrom::Start(chunk.data_position_start))?;
                let children = Self::read_chunk_headers_bounded(reader, end)?;
                let nested = Self::flatten_data_chunks(reader, &children)?;
                out.push(chunk.clone());
                out.extend(nested);
            } else {
                out.push(chunk.clone());
            }
        }
        Ok(out)
    }

    /// Reads a sequence of sibling chunk headers starting at the reader's
    /// current position, stopping once `end` is reached. Mirrors the
    /// top-level loop in [`ChunkFile::parse`], but bounded to a single FOLD
    /// chunk's byte range instead of running to EOF.
    fn read_chunk_headers_bounded<R: Read + Seek>(
        reader: &mut R,
        end: u64,
    ) -> Result<Vec<ChunkHeader>, RelicGameDataError> {
        let mut chunks = Vec::new();
        loop {
            let position = reader.stream_position()?;
            if position >= end {
                break;
            }

            let chunk_header = ChunkHeader::parse(&mut *reader)?;
            reader.seek(SeekFrom::Start(
                chunk_header.data_position_start + chunk_header.length as u64,
            ))?;
            chunks.push(chunk_header);
        }
        Ok(chunks)
    }

    fn read_chunky_list<R: Read + Seek>(
        reader: &mut R,
    ) -> Result<Vec<RGDEntry>, RelicGameDataError> {
        let length = reader.read_u32::<LittleEndian>()? as usize;

        // `length` is read straight from the file, so only reserve a sane amount
        // up front. A truncated or corrupt list fails on read below instead.
        let mut index_entries = Vec::with_capacity(length.min(1024));
        for _ in 0..length {
            let key = reader.read_u64::<LittleEndian>()?;
            let data_type = reader.read_i32::<LittleEndian>()?;
            let data_offset = reader.read_i32::<LittleEndian>()?;
            index_entries.push((key, data_type, data_offset));
        }

        // Entry offsets are relative to the end of the index.
        let data_start = reader.stream_position()?;

        let mut entries = Vec::with_capacity(index_entries.len());
        for (key, data_type, offset) in index_entries {
            let position = data_start
                .checked_add_signed(offset as i64)
                .ok_or(RelicGameDataError::InvalidDataOffset { key, offset })?;
            reader.seek(SeekFrom::Start(position))?;

            entries.push(RGDEntry {
                key_hash: key,
                value: Self::read_value(reader, data_type)?,
            });
        }

        Ok(entries)
    }

    fn read_value<R: Read + Seek>(
        reader: &mut R,
        data_type: i32,
    ) -> Result<RGDValue, RelicGameDataError> {
        Ok(match data_type {
            0 => RGDValue::Float(reader.read_f32::<LittleEndian>()?),
            1 => RGDValue::Int(reader.read_i32::<LittleEndian>()?),
            2 => RGDValue::Boolean(reader.read_u8()? != 0),
            3 => RGDValue::CString(Self::read_cstring(reader)?),
            4 => RGDValue::LocString(Self::read_wstring(reader)?),
            100 | 101 => RGDValue::List(Self::read_chunky_list(reader)?),
            other => {
                if std::env::var_os("RGD_DEBUG_UNKNOWN_TYPE").is_some() {
                    let position = reader.stream_position()?;
                    let mut peek = [0u8; 256];
                    let read = reader.read(&mut peek)?;
                    reader.seek(SeekFrom::Start(position))?;
                    eprintln!(
                        "unknown data type {other} at offset {position}, next {read} bytes: {:02x?}",
                        &peek[..read]
                    );
                }
                return Err(RelicGameDataError::UnknownDataType(other));
            }
        })
    }

    fn read_cstring<R: Read>(reader: &mut R) -> Result<String, RelicGameDataError> {
        let mut bytes = Vec::new();
        loop {
            let byte = reader.read_u8()?;
            if byte == 0 {
                break;
            }
            bytes.push(byte);
        }

        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    /// Reads a null-terminated UTF-16LE string (data type 4, "LocString").
    fn read_wstring<R: Read>(reader: &mut R) -> Result<String, RelicGameDataError> {
        let mut units = Vec::new();
        loop {
            let unit = reader.read_u16::<LittleEndian>()?;
            if unit == 0 {
                break;
            }
            units.push(unit);
        }

        Ok(String::from_utf16_lossy(&units))
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

            // The first string wins if a hash is listed more than once.
            key_string_map.entry(key).or_insert(string);
        }

        Ok(key_string_map)
    }

    /// Resolves the key hash of every entry, recursively, against the DATA KEYS
    /// chunk. Each entry keeps its own key: a list's children are resolved with
    /// their own hashes, not the hash of the list they sit in.
    pub fn resolve_nodes(entries: &[RGDEntry], keys: &HashMap<u64, String>) -> Vec<RGDNode> {
        entries
            .iter()
            .map(|entry| RGDNode {
                key: Self::resolve_key(entry.key_hash, keys),
                value: match &entry.value {
                    RGDValue::Float(value) => RGDNodeValue::Float(*value),
                    RGDValue::Int(value) => RGDNodeValue::Int(*value),
                    RGDValue::Boolean(value) => RGDNodeValue::Boolean(*value),
                    RGDValue::CString(value) => RGDNodeValue::CString(value.clone()),
                    RGDValue::LocString(value) => RGDNodeValue::LocString(value.clone()),
                    RGDValue::List(children) => {
                        RGDNodeValue::List(Self::resolve_nodes(children, keys))
                    }
                },
            })
            .collect()
    }

    /// Keys that are not present in the DATA KEYS chunk are kept as their raw
    /// hash so no data is silently dropped.
    fn resolve_key(key: u64, keys: &HashMap<u64, String>) -> String {
        keys.get(&key)
            .cloned()
            .unwrap_or_else(|| format!("unknown_{key}"))
    }
}

/// Encodes RGD nodes as a JSON string.
pub fn game_data_to_json(nodes: &[RGDNode]) -> Result<String, serde_json::Error> {
    #[derive(Serialize)]
    struct Root<'a> {
        data: &'a [RGDNode],
    }

    serde_json::to_string_pretty(&Root { data: nodes })
}

/// Encodes RGD nodes as an XML string.
pub fn game_data_to_xml(nodes: &[RGDNode]) -> Result<String, quick_xml::Error> {
    fn write_node(writer: &mut Writer<Vec<u8>>, node: &RGDNode) -> Result<(), quick_xml::Error> {
        let mut element = BytesStart::new("RGDNode");
        element.push_attribute(("Key", node.key.as_str()));
        element.push_attribute(("Type", node.value.data_type().name()));

        match &node.value {
            RGDNodeValue::List(children) => {
                writer.write_event(Event::Start(element))?;
                for child in children {
                    write_node(writer, child)?;
                }
                writer.write_event(Event::End(BytesEnd::new("RGDNode")))?;
            }
            value => {
                let text = match value {
                    RGDNodeValue::Float(value) => value.to_string(),
                    RGDNodeValue::Int(value) => value.to_string(),
                    RGDNodeValue::Boolean(value) => value.to_string(),
                    RGDNodeValue::CString(value) => value.clone(),
                    RGDNodeValue::LocString(value) => value.clone(),
                    RGDNodeValue::List(_) => unreachable!("handled above"),
                };
                element.push_attribute(("Value", text.as_str()));
                writer.write_event(Event::Empty(element))?;
            }
        }

        Ok(())
    }

    let mut writer = Writer::new_with_indent(Vec::new(), b' ', 2);
    writer.write_event(Event::Decl(BytesDecl::new("1.0", Some("utf-8"), None)))?;
    writer.write_event(Event::Start(BytesStart::new("Root")))?;
    for node in nodes {
        write_node(&mut writer, node)?;
    }
    writer.write_event(Event::End(BytesEnd::new("Root")))?;

    // quick-xml only ever writes UTF-8, so this cannot fail.
    Ok(String::from_utf8(writer.into_inner()).expect("quick-xml emits UTF-8"))
}
