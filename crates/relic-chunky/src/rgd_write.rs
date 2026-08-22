//! Encodes a Relic game-data (`.rgd`) node tree back into bytes — the inverse
//! of [`crate::rgd`]. The container is a Relic Chunky with two `DATA` chunks:
//! `AEGD` (`[u32 CRC32][node list]`) and `KEYS` (the hash→string dictionary).

use std::collections::HashSet;
use std::io::Write;

use anyhow::Result;
use byteorder::{LittleEndian, WriteBytesExt};
use flate2::Crc;

use crate::container::{Chunk, ChunkBody, ChunkKind, Chunky};
use crate::hash::dictionary_hash;
use crate::rgd::{RGDNode, RGDNodeValue};

const AEGD_VERSION: u32 = 3;
const KEYS_VERSION: u32 = 1;

/// Serializes a game-data node tree into `.rgd` bytes.
pub fn write_rgd(nodes: &[RGDNode]) -> Result<Vec<u8>> {
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
        chunks: vec![data_chunk(b"AEGD", AEGD_VERSION, aegd), data_chunk(b"KEYS", KEYS_VERSION, keys_data)],
    };
    let mut out = Vec::new();
    chunky.write(&mut out)?;
    Ok(out)
}

/// Serializes one node list: a `[count]` header, an index table of
/// `{key hash, type, data offset}`, then the values, all sorted by key hash.
fn write_list(nodes: &[RGDNode]) -> Vec<u8> {
    let mut sorted: Vec<&RGDNode> = nodes.iter().collect();
    sorted.sort_by_key(|n| dictionary_hash(&n.key));

    let values: Vec<Vec<u8>> = sorted.iter().map(|n| write_value(&n.value)).collect();

    // Lay out the value data blob, aligning each value to its type's natural
    // read size (4 for Float/Int/List, 2 for LocString, 1 for Bool/CString).
    let mut blob = Vec::new();
    let mut offsets = Vec::with_capacity(sorted.len());
    for (node, value) in sorted.iter().zip(&values) {
        let align = value_align(&node.value);
        while blob.len() % align != 0 {
            blob.push(0);
        }
        offsets.push(blob.len() as i32);
        blob.extend_from_slice(value);
    }

    let mut out = Vec::new();
    out.write_u32::<LittleEndian>(sorted.len() as u32).unwrap();
    for ((node, _), &offset) in sorted.iter().zip(&values).zip(&offsets) {
        out.write_u64::<LittleEndian>(dictionary_hash(&node.key)).unwrap();
        out.write_i32::<LittleEndian>(node.value.data_type() as i32).unwrap();
        out.write_i32::<LittleEndian>(offset).unwrap();
    }
    out.extend_from_slice(&blob);
    out
}

/// A value's alignment in the data blob, matching the reader's typed access.
fn value_align(value: &RGDNodeValue) -> usize {
    match value {
        RGDNodeValue::Float(_) | RGDNodeValue::Int(_) | RGDNodeValue::List(_) => 4,
        RGDNodeValue::LocString(_) => 2,
        RGDNodeValue::Boolean(_) | RGDNodeValue::CString(_) => 1,
    }
}

fn write_value(value: &RGDNodeValue) -> Vec<u8> {
    match value {
        RGDNodeValue::Float(f) => f.to_le_bytes().to_vec(),
        RGDNodeValue::Int(i) => i.to_le_bytes().to_vec(),
        RGDNodeValue::Boolean(b) => vec![u8::from(*b)],
        RGDNodeValue::CString(s) => {
            let mut v = s.as_bytes().to_vec();
            v.push(0);
            v
        }
        RGDNodeValue::LocString(s) => {
            let mut v = Vec::new();
            for unit in s.encode_utf16() {
                v.extend_from_slice(&unit.to_le_bytes());
            }
            v.extend_from_slice(&[0, 0]);
            v
        }
        RGDNodeValue::List(children) => write_list(children),
    }
}

/// Gathers every distinct key in the tree as `(hash, string)`, sorted by string
/// (the order the editor's `KEYS` chunk uses).
fn gather_keys(nodes: &[RGDNode]) -> Vec<(u64, String)> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    fn walk(nodes: &[RGDNode], out: &mut Vec<(u64, String)>, seen: &mut HashSet<u64>) {
        for node in nodes {
            let hash = dictionary_hash(&node.key);
            if seen.insert(hash) {
                out.push((hash, node.key.clone()));
            }
            if let RGDNodeValue::List(children) = &node.value {
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
