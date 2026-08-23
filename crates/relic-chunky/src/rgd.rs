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

use binrw::BinRead;

use crate::container::Chunky;
use crate::records::KeyTable;

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
    /// Every variant, so `from_code` can map a wire code back to its variant
    /// without repeating the numeric values (they live only on the enum).
    const ALL: [RGDDataType; 7] = [
        RGDDataType::Float,
        RGDDataType::Int,
        RGDDataType::Boolean,
        RGDDataType::CString,
        RGDDataType::LocString,
        RGDDataType::List,
        RGDDataType::List2,
    ];

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

    /// The on-disk type code — the enum discriminant. This is the single source
    /// of truth for the numeric codes; read and write both go through it.
    pub fn code(self) -> i32 {
        self as i32
    }

    /// Maps an on-disk type code to its variant (or `None` if unknown).
    pub fn from_code(code: i32) -> Option<Self> {
        Self::ALL.into_iter().find(|ty| ty.code() == code)
    }
}

/// A game-data value, shared by the reader and the writer. `List` is the
/// engine's keyed table (type 100); `List2` is its ordered/reference list
/// (type 101).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum RGDValue {
    Float(f32),
    Int(i32),
    Boolean(bool),
    CString(String),
    LocString(String),
    List(Vec<RGDNode>),
    List2(Vec<RGDNode>),
}

impl RGDValue {
    pub fn data_type(&self) -> RGDDataType {
        match self {
            RGDValue::Float(_) => RGDDataType::Float,
            RGDValue::Int(_) => RGDDataType::Int,
            RGDValue::Boolean(_) => RGDDataType::Boolean,
            RGDValue::CString(_) => RGDDataType::CString,
            RGDValue::LocString(_) => RGDDataType::LocString,
            RGDValue::List(_) => RGDDataType::List,
            RGDValue::List2(_) => RGDDataType::List2,
        }
    }
}

/// One keyed game-data node. The reader resolves `key` from the `KEYS`
/// dictionary; the writer hashes it back.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RGDNode {
    pub key: String,
    pub value: RGDValue,
}

impl RGDNode {
    pub fn new(key: impl Into<String>, value: RGDValue) -> Self {
        RGDNode { key: key.into(), value }
    }
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
        Self::parse_aegd(aegd_data, &keys)
    }

    fn read_chunky_list<R: Read + Seek>(
        reader: &mut R,
        keys: &HashMap<u64, String>,
    ) -> Result<Vec<RGDNode>, RelicGameDataError> {
        let length = reader.read_u32::<LittleEndian>()? as usize;

        let mut index_entries = Vec::with_capacity(length.min(1024));
        for _ in 0..length {
            let key = reader.read_u64::<LittleEndian>()?;
            let data_type = reader.read_i32::<LittleEndian>()?;
            let data_offset = reader.read_i32::<LittleEndian>()?;
            index_entries.push((key, data_type, data_offset));
        }

        let data_start = reader.stream_position()?;

        let mut nodes = Vec::with_capacity(index_entries.len());
        for (key, data_type, offset) in index_entries {
            let position = data_start
                .checked_add_signed(offset as i64)
                .ok_or(RelicGameDataError::InvalidDataOffset { key, offset })?;
            reader.seek(SeekFrom::Start(position))?;

            nodes.push(RGDNode {
                key: Self::resolve_key(key, keys),
                value: Self::read_value(reader, data_type, keys)?,
            });
        }

        Ok(nodes)
    }

    fn read_value<R: Read + Seek>(
        reader: &mut R,
        data_type: i32,
        keys: &HashMap<u64, String>,
    ) -> Result<RGDValue, RelicGameDataError> {
        let Some(ty) = RGDDataType::from_code(data_type) else {
            if std::env::var_os("RGD_DEBUG_UNKNOWN_TYPE").is_some() {
                let position = reader.stream_position()?;
                let mut peek = [0u8; 256];
                let read = reader.read(&mut peek)?;
                reader.seek(SeekFrom::Start(position))?;
                eprintln!(
                    "unknown data type {data_type} at offset {position}, next {read} bytes: {:02x?}",
                    &peek[..read]
                );
            }
            return Err(RelicGameDataError::UnknownDataType(data_type));
        };
        Ok(match ty {
            RGDDataType::Float => RGDValue::Float(reader.read_f32::<LittleEndian>()?),
            RGDDataType::Int => RGDValue::Int(reader.read_i32::<LittleEndian>()?),
            RGDDataType::Boolean => RGDValue::Boolean(reader.read_u8()? != 0),
            RGDDataType::CString => RGDValue::CString(Self::read_cstring(reader)?),
            RGDDataType::LocString => RGDValue::LocString(Self::read_wstring(reader)?),
            RGDDataType::List => RGDValue::List(Self::read_chunky_list(reader, keys)?),
            RGDDataType::List2 => RGDValue::List2(Self::read_chunky_list(reader, keys)?),
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

    fn parse_aegd(
        data: &[u8],
        keys: &HashMap<u64, String>,
    ) -> Result<Vec<RGDNode>, RelicGameDataError> {
        let mut reader = Cursor::new(data);
        let _unknown = reader.read_u32::<LittleEndian>()?;
        Self::read_chunky_list(&mut reader, keys)
    }

    pub fn parse_keys(data: &[u8]) -> Result<HashMap<u64, String>, RelicGameDataError> {
        let table = KeyTable::read(&mut Cursor::new(data)).map_err(io::Error::other)?;
        let mut key_string_map = HashMap::new();
        for entry in table.keys {
            key_string_map.entry(entry.hash).or_insert(entry.value);
        }
        Ok(key_string_map)
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
            RGDValue::List(children) | RGDValue::List2(children) => {
                writer.write_event(Event::Start(element))?;
                for child in children {
                    write_node(writer, child)?;
                }
                writer.write_event(Event::End(BytesEnd::new("RGDNode")))?;
            }
            value => {
                let text = match value {
                    RGDValue::Float(value) => value.to_string(),
                    RGDValue::Int(value) => value.to_string(),
                    RGDValue::Boolean(value) => value.to_string(),
                    RGDValue::CString(value) => value.clone(),
                    RGDValue::LocString(value) => value.clone(),
                    RGDValue::List(_) | RGDValue::List2(_) => unreachable!("handled above"),
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
