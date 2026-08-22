use std::{
    collections::HashMap,
    io::{self, Cursor, Read, Seek, SeekFrom},
};

use byteorder::{LittleEndian, ReadBytesExt};
use quick_xml::{
    Writer,
    events::{BytesDecl, BytesEnd, BytesStart, Event},
};
use serde::Serialize;
use thiserror::Error;

use crate::container::Chunky;

#[derive(Debug)]
pub struct RelicGameData {}

#[derive(Error, Debug)]
pub enum RelicGameDataError {
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

    #[error("Unknown data type {0}")]
    UnknownDataType(i32),

    #[error("Entry for key {key} has out of bounds data offset {offset}")]
    InvalidDataOffset { key: u64, offset: i32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RGDDataType {
    Float = 0,
    Int = 1,
    Boolean = 2,
    CString = 3,
    LocString = 4,
    List = 100,
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

#[derive(Debug, Clone, PartialEq)]
pub enum RGDValue {
    Float(f32),
    Int(i32),
    Boolean(bool),
    CString(String),
    LocString(String),
    List(Vec<RGDEntry>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct RGDEntry {
    pub key_hash: u64,
    pub value: RGDValue,
}

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

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RGDNode {
    pub key: String,
    pub value: RGDNodeValue,
}

impl RelicGameData {
    pub fn parse(chunky: &Chunky) -> Result<Vec<RGDNode>, RelicGameDataError> {
        let mut keys_data = None;
        let mut aegd_data = None;

        for (chunk, _position) in chunky.data_chunks() {
            match chunk.name_str().as_str() {
                "KEYS" => {
                    if keys_data.is_some() {
                        return Err(RelicGameDataError::MultipleDataKeysChunks);
                    }
                    keys_data = chunk.data();
                }
                "AEGD" => {
                    if aegd_data.is_some() {
                        return Err(RelicGameDataError::MultipleDataAegdChunks);
                    }
                    aegd_data = chunk.data();
                }
                _ => {}
            }
        }

        let keys_data = keys_data.ok_or(RelicGameDataError::MissingDataKeysChunk)?;
        let aegd_data = aegd_data.ok_or(RelicGameDataError::MissingDataAegdChunk)?;

        let keys = Self::parse_keys(keys_data)?;
        let entries = Self::parse_aegd(aegd_data)?;

        Ok(Self::resolve_nodes(&entries, &keys))
    }

    fn read_chunky_list<R: Read + Seek>(
        reader: &mut R,
    ) -> Result<Vec<RGDEntry>, RelicGameDataError> {
        let length = reader.read_u32::<LittleEndian>()? as usize;

        let mut index_entries = Vec::with_capacity(length.min(1024));
        for _ in 0..length {
            let key = reader.read_u64::<LittleEndian>()?;
            let data_type = reader.read_i32::<LittleEndian>()?;
            let data_offset = reader.read_i32::<LittleEndian>()?;
            index_entries.push((key, data_type, data_offset));
        }

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

    fn parse_aegd(data: &[u8]) -> Result<Vec<RGDEntry>, RelicGameDataError> {
        let mut reader = Cursor::new(data);
        let _unknown = reader.read_u32::<LittleEndian>()?;
        Self::read_chunky_list(&mut reader)
    }

    pub fn parse_keys(data: &[u8]) -> Result<HashMap<u64, String>, RelicGameDataError> {
        let mut key_string_map = HashMap::new();
        let mut reader = Cursor::new(data);

        let count = reader.read_u32::<LittleEndian>()?;

        for _ in 0..count {
            let key = reader.read_u64::<LittleEndian>()?;
            let string_length = reader.read_u32::<LittleEndian>()?;

            let string = {
                let mut string_bytes = vec![0u8; string_length as usize];
                reader.read_exact(&mut string_bytes)?;
                String::from_utf8_lossy(&string_bytes).to_string()
            };

            key_string_map.entry(key).or_insert(string);
        }

        Ok(key_string_map)
    }

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

    fn resolve_key(key: u64, keys: &HashMap<u64, String>) -> String {
        keys.get(&key)
            .cloned()
            .unwrap_or_else(|| format!("unknown_{key}"))
    }
}

pub fn game_data_to_json(nodes: &[RGDNode]) -> Result<String, serde_json::Error> {
    #[derive(Serialize)]
    struct Root<'a> {
        data: &'a [RGDNode],
    }

    serde_json::to_string_pretty(&Root { data: nodes })
}

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

    Ok(String::from_utf8(writer.into_inner()).expect("quick-xml emits UTF-8"))
}
