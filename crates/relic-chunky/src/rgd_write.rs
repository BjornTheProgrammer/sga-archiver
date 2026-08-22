//! Encodes a Relic game-data (`.rgd`) node tree into bytes — the inverse of
//! [`crate::rgd`]. The container is a Relic Chunky with two `DATA` chunks:
//! `AEGD` (`[u32 CRC32][node list]`) and `KEYS` (the hash->string dictionary).

use std::collections::HashSet;
use std::io::Write;

use anyhow::Result;
use byteorder::{LittleEndian, WriteBytesExt};
use flate2::Crc;

use crate::container::{Chunk, ChunkBody, ChunkKind, Chunky};
use crate::hash::dictionary_hash;

/// A game-data value. `List` is the engine's keyed table (RGD type 100); `List2`
/// is its ordered/reference list (type 101).
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Float(f32),
    Int(i32),
    Bool(bool),
    CString(String),
    LocString(String),
    List(Vec<Node>),
    List2(Vec<Node>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub key: String,
    pub value: Value,
}

impl Node {
    pub fn new(key: impl Into<String>, value: Value) -> Self {
        Node { key: key.into(), value }
    }
}

impl Value {
    fn type_code(&self) -> i32 {
        match self {
            Value::Float(_) => 0,
            Value::Int(_) => 1,
            Value::Bool(_) => 2,
            Value::CString(_) => 3,
            Value::LocString(_) => 4,
            Value::List(_) => 100,
            Value::List2(_) => 101,
        }
    }

    /// Alignment in the data blob, matching the reader's typed access.
    fn align(&self) -> usize {
        match self {
            Value::Float(_) | Value::Int(_) | Value::List(_) | Value::List2(_) => 4,
            Value::LocString(_) => 2,
            Value::Bool(_) | Value::CString(_) => 1,
        }
    }
}

/// Serializes a game-data node tree into `.rgd` bytes.
pub fn write_rgd(nodes: &[Node]) -> Result<Vec<u8>> {
    let node_bytes = write_list(nodes);
    let mut crc = Crc::new();
    crc.update(&node_bytes);
    let mut aegd = crc.sum().to_le_bytes().to_vec();
    aegd.extend_from_slice(&node_bytes);

    let keys = gather_keys(nodes);
    let mut keys_data = Vec::new();
    keys_data.write_u32::<LittleEndian>(keys.len() as u32)?;
    for (hash, key) in &keys {
        keys_data.write_u64::<LittleEndian>(*hash)?;
        keys_data.write_u32::<LittleEndian>(key.len() as u32)?;
        keys_data.write_all(key.as_bytes())?;
    }

    let chunky = Chunky {
        major: 4,
        minor: 0,
        platform: 1,
        chunks: vec![data_chunk(b"AEGD", 3, aegd), data_chunk(b"KEYS", 1, keys_data)],
    };
    let mut out = Vec::new();
    chunky.write(&mut out)?;
    Ok(out)
}

/// Serializes one node list: a `[count]` header, an index table of
/// `{key hash, type, data offset}`, then the values — sorted by key hash, with
/// each value aligned to its type.
fn write_list(nodes: &[Node]) -> Vec<u8> {
    let mut sorted: Vec<&Node> = nodes.iter().collect();
    sorted.sort_by_key(|n| dictionary_hash(&n.key));

    let values: Vec<Vec<u8>> = sorted.iter().map(|n| write_value(&n.value)).collect();

    let mut blob = Vec::new();
    let mut offsets = Vec::with_capacity(sorted.len());
    for (node, value) in sorted.iter().zip(&values) {
        let align = node.value.align();
        while blob.len() % align != 0 {
            blob.push(0);
        }
        offsets.push(blob.len() as i32);
        blob.extend_from_slice(value);
    }

    let mut out = Vec::new();
    out.write_u32::<LittleEndian>(sorted.len() as u32).unwrap();
    for (node, &offset) in sorted.iter().zip(&offsets) {
        out.write_u64::<LittleEndian>(dictionary_hash(&node.key)).unwrap();
        out.write_i32::<LittleEndian>(node.value.type_code()).unwrap();
        out.write_i32::<LittleEndian>(offset).unwrap();
    }
    out.extend_from_slice(&blob);
    out
}

fn write_value(value: &Value) -> Vec<u8> {
    match value {
        Value::Float(f) => f.to_le_bytes().to_vec(),
        Value::Int(i) => i.to_le_bytes().to_vec(),
        Value::Bool(b) => vec![u8::from(*b)],
        Value::CString(s) => {
            let mut v = s.as_bytes().to_vec();
            v.push(0);
            v
        }
        Value::LocString(s) => {
            let mut v = Vec::new();
            for unit in s.encode_utf16() {
                v.extend_from_slice(&unit.to_le_bytes());
            }
            v.extend_from_slice(&[0, 0]);
            v
        }
        Value::List(children) | Value::List2(children) => write_list(children),
    }
}

/// Gathers every distinct key as `(hash, string)`, sorted by string (the order
/// the editor's `KEYS` chunk uses).
fn gather_keys(nodes: &[Node]) -> Vec<(u64, String)> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    fn walk(nodes: &[Node], out: &mut Vec<(u64, String)>, seen: &mut HashSet<u64>) {
        for node in nodes {
            let hash = dictionary_hash(&node.key);
            if seen.insert(hash) {
                out.push((hash, node.key.clone()));
            }
            if let Value::List(children) | Value::List2(children) = &node.value {
                walk(children, out, seen);
            }
        }
    }
    walk(nodes, &mut out, &mut seen);
    out.sort_by(|a, b| a.1.cmp(&b.1));
    out
}

fn data_chunk(name: &[u8; 4], version: u32, body: Vec<u8>) -> Chunk {
    Chunk {
        kind: ChunkKind::Data,
        name: *name,
        version,
        path: Vec::new(),
        body: ChunkBody::Data(body),
    }
}
