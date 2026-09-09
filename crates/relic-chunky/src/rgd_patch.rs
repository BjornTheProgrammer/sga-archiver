//! Targeted edits to compiled RGD game data that leave every other byte alone.
//!
//! An RGD's `AEGD` chunk is `[u32 CRC32][node list]`, where the node list is
//! an index of `(key hash, data type, offset)` triples followed by the packed
//! values. The full decoder in [`rgd`](crate::rgd) resolves hashes through the
//! `KEYS` dictionary and is the right tool for reading; this module works on
//! the raw list so a single value can change without re-serializing anything
//! the parser did not fully understand.
//!
//! Every patch is guarded: the raw list is parsed and re-encoded first, and the
//! patch is refused unless that round-trip is byte-exact. The same guard is
//! applied to the whole Chunky container, so a patched file differs from its
//! source only in the values that were asked to change (and the CRC that
//! covers them).

use std::io::{Cursor, Read, Seek, SeekFrom};

use anyhow::{Context, Result, bail};
use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use flate2::Crc;

use crate::container::{Chunk, ChunkBody, ChunkKind, Chunky};

/// What a patch changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PatchReport {
    /// Values rewritten.
    pub replacements: usize,
    /// Size in bytes of the `AEGD` chunk before the patch.
    pub original_aegd_size: usize,
    /// Size in bytes of the `AEGD` chunk after the patch.
    pub patched_aegd_size: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RawValue {
    FourBytes {
        data_type: i32,
        bytes: [u8; 4],
    },
    Bool(u8),
    CString(Vec<u8>),
    LocString(Vec<u16>),
    List {
        data_type: i32,
        children: Vec<RawNode>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RawNode {
    pub key_hash: u64,
    pub value: RawValue,
}

impl RawValue {
    fn data_type(&self) -> i32 {
        match self {
            RawValue::FourBytes { data_type, .. } | RawValue::List { data_type, .. } => *data_type,
            RawValue::Bool(_) => 2,
            RawValue::CString(_) => 3,
            RawValue::LocString(_) => 4,
        }
    }

    fn align(&self) -> usize {
        match self {
            RawValue::FourBytes { .. } | RawValue::List { .. } => 4,
            RawValue::LocString(_) => 2,
            RawValue::Bool(_) | RawValue::CString(_) => 1,
        }
    }
}

fn read_cstring_bytes<R: Read>(reader: &mut R) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    loop {
        let byte = reader.read_u8()?;
        if byte == 0 {
            break;
        }
        bytes.push(byte);
    }
    Ok(bytes)
}

fn read_wstring_units<R: Read>(reader: &mut R) -> Result<Vec<u16>> {
    let mut units = Vec::new();
    loop {
        let unit = reader.read_u16::<LittleEndian>()?;
        if unit == 0 {
            break;
        }
        units.push(unit);
    }
    Ok(units)
}

pub(crate) fn read_raw_list<R: Read + Seek>(reader: &mut R) -> Result<Vec<RawNode>> {
    let count = reader.read_u32::<LittleEndian>()? as usize;
    let mut index = Vec::with_capacity(count);
    for _ in 0..count {
        index.push((
            reader.read_u64::<LittleEndian>()?,
            reader.read_i32::<LittleEndian>()?,
            reader.read_i32::<LittleEndian>()?,
        ));
    }
    let data_start = reader.stream_position()?;
    let mut nodes = Vec::with_capacity(count);
    for (key_hash, data_type, offset) in index {
        let position = data_start
            .checked_add_signed(offset as i64)
            .context("RGD value offset overflow")?;
        reader.seek(SeekFrom::Start(position))?;
        let value = match data_type {
            0 | 1 => {
                let mut bytes = [0u8; 4];
                reader.read_exact(&mut bytes)?;
                RawValue::FourBytes { data_type, bytes }
            }
            2 => RawValue::Bool(reader.read_u8()?),
            3 => RawValue::CString(read_cstring_bytes(reader)?),
            4 => RawValue::LocString(read_wstring_units(reader)?),
            100 | 101 => RawValue::List {
                data_type,
                children: read_raw_list(reader)?,
            },
            other => bail!("unsupported RGD data type {other}"),
        };
        nodes.push(RawNode { key_hash, value });
    }
    Ok(nodes)
}

fn write_raw_value(value: &RawValue) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    match value {
        RawValue::FourBytes { bytes, .. } => out.extend_from_slice(bytes),
        RawValue::Bool(value) => out.push(*value),
        RawValue::CString(bytes) => {
            out.extend_from_slice(bytes);
            out.push(0);
        }
        RawValue::LocString(units) => {
            for unit in units {
                out.write_u16::<LittleEndian>(*unit)?;
            }
            out.write_u16::<LittleEndian>(0)?;
        }
        RawValue::List { children, .. } => out = write_raw_list(children)?,
    }
    Ok(out)
}

pub(crate) fn write_raw_list(nodes: &[RawNode]) -> Result<Vec<u8>> {
    let values = nodes
        .iter()
        .map(|node| write_raw_value(&node.value))
        .collect::<Result<Vec<_>>>()?;
    let mut blob = Vec::new();
    let mut offsets = Vec::with_capacity(nodes.len());
    for (node, value) in nodes.iter().zip(&values) {
        while blob.len() % node.value.align() != 0 {
            blob.push(0);
        }
        offsets.push(blob.len() as i32);
        blob.extend_from_slice(value);
    }
    let mut out = Vec::new();
    out.write_u32::<LittleEndian>(nodes.len() as u32)?;
    for (node, offset) in nodes.iter().zip(offsets) {
        out.write_u64::<LittleEndian>(node.key_hash)?;
        out.write_i32::<LittleEndian>(node.value.data_type())?;
        out.write_i32::<LittleEndian>(offset)?;
    }
    out.extend_from_slice(&blob);
    Ok(out)
}

pub(crate) fn patch_cstrings(nodes: &mut [RawNode], old: &[u8], new: &[u8]) -> usize {
    let mut count = 0;
    for node in nodes {
        match &mut node.value {
            RawValue::CString(value) if value == old => {
                *value = new.to_vec();
                count += 1;
            }
            RawValue::List { children, .. } => {
                count += patch_cstrings(children, old, new);
            }
            _ => {}
        }
    }
    count
}

pub(crate) fn patch_f32s(
    nodes: &mut [RawNode],
    key_hash: u64,
    old: [u8; 4],
    new: [u8; 4],
) -> usize {
    let mut count = 0;
    for node in nodes {
        match &mut node.value {
            RawValue::FourBytes {
                data_type: 0,
                bytes,
            } if node.key_hash == key_hash && *bytes == old => {
                *bytes = new;
                count += 1;
            }
            RawValue::List { children, .. } => {
                count += patch_f32s(children, key_hash, old, new);
            }
            _ => {}
        }
    }
    count
}

/// Applies `edit` to the raw node list of the single `DATA AEGD` chunk,
/// refusing unless the list re-encodes byte-exactly first.
fn patch_aegd(
    chunks: &mut [Chunk],
    edit: &mut dyn FnMut(&mut Vec<RawNode>) -> usize,
    result: &mut Option<PatchReport>,
) -> Result<()> {
    for chunk in chunks {
        match &mut chunk.body {
            ChunkBody::Folder(children) => patch_aegd(children, edit, result)?,
            ChunkBody::Data(data)
                if chunk.kind == ChunkKind::Data && chunk.name.as_slice() == b"AEGD" =>
            {
                if result.is_some() {
                    bail!("more than one DATA AEGD chunk present");
                }
                if data.len() < 4 {
                    bail!("DATA AEGD chunk is too short");
                }
                let original_aegd_size = data.len();
                let original_list = &data[4..];
                let mut nodes = read_raw_list(&mut Cursor::new(original_list))?;
                if write_raw_list(&nodes)? != original_list {
                    bail!("refusing patch: parsed AEGD layout is not byte-exact on re-encode");
                }
                let replacements = edit(&mut nodes);
                let patched_list = write_raw_list(&nodes)?;
                let mut crc = Crc::new();
                crc.update(&patched_list);
                let mut patched = crc.sum().to_le_bytes().to_vec();
                patched.extend_from_slice(&patched_list);
                let patched_aegd_size = patched.len();
                *data = patched;
                *result = Some(PatchReport {
                    replacements,
                    original_aegd_size,
                    patched_aegd_size,
                });
            }
            ChunkBody::Data(_) => {}
        }
    }
    Ok(())
}

/// Replaces every CString value equal to `old` with `new`, anywhere in the
/// node tree, in an already parsed container.
pub fn patch_cstring(chunky: &mut Chunky, old: &[u8], new: &[u8]) -> Result<PatchReport> {
    if old.is_empty() || new.is_empty() || old.contains(&0) || new.contains(&0) {
        bail!("old and new CString values must be non-empty and contain no NUL bytes");
    }
    let mut result = None;
    patch_aegd(
        &mut chunky.chunks,
        &mut |nodes| patch_cstrings(nodes, old, new),
        &mut result,
    )?;
    result.context("No DATA AEGD chunk present")
}

/// Replaces every Float value under `key_hash` that equals `old` with `new`.
/// Integers and floats under other keys are left alone.
pub fn patch_f32(chunky: &mut Chunky, key_hash: u64, old: f32, new: f32) -> Result<PatchReport> {
    if !old.is_finite() || !new.is_finite() {
        bail!("old and new Float values must be finite");
    }
    let mut result = None;
    patch_aegd(
        &mut chunky.chunks,
        &mut |nodes| patch_f32s(nodes, key_hash, old.to_le_bytes(), new.to_le_bytes()),
        &mut result,
    )?;
    result.context("No DATA AEGD chunk present")
}

/// Parses `input` as a Chunky container, checks it re-encodes byte-exactly,
/// applies `patch`, and returns the patched file bytes.
fn patch_bytes(
    input: &[u8],
    patch: impl FnOnce(&mut Chunky) -> Result<PatchReport>,
) -> Result<(Vec<u8>, PatchReport)> {
    let mut chunky =
        Chunky::read(&mut Cursor::new(input)).context("failed to parse RGD container")?;
    let mut control = Vec::new();
    chunky.write(&mut control)?;
    if control != input {
        bail!("refusing patch: Chunky container is not byte-exact on re-encode");
    }
    let report = patch(&mut chunky)?;
    let mut bytes = Vec::new();
    chunky
        .write(&mut bytes)
        .context("failed to encode patched RGD")?;
    Ok((bytes, report))
}

/// [`patch_cstring`] over a whole RGD file's bytes, with the container guard.
pub fn patch_cstring_bytes(input: &[u8], old: &[u8], new: &[u8]) -> Result<(Vec<u8>, PatchReport)> {
    patch_bytes(input, |chunky| patch_cstring(chunky, old, new))
}

/// [`patch_f32`] over a whole RGD file's bytes, with the container guard.
pub fn patch_f32_bytes(
    input: &[u8],
    key_hash: u64,
    old: f32,
    new: f32,
) -> Result<(Vec<u8>, PatchReport)> {
    patch_bytes(input, |chunky| patch_f32(chunky, key_hash, old, new))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patches_exact_cstrings_recursively_without_touching_other_values() {
        let mut nodes = vec![RawNode {
            key_hash: 1,
            value: RawValue::List {
                data_type: 101,
                children: vec![
                    RawNode {
                        key_hash: 2,
                        value: RawValue::CString(b"short".to_vec()),
                    },
                    RawNode {
                        key_hash: 3,
                        value: RawValue::CString(b"shorter".to_vec()),
                    },
                    RawNode {
                        key_hash: 4,
                        value: RawValue::List {
                            data_type: 100,
                            children: vec![RawNode {
                                key_hash: 5,
                                value: RawValue::CString(b"short".to_vec()),
                            }],
                        },
                    },
                ],
            },
        }];

        let before = write_raw_list(&nodes).unwrap();
        assert_eq!(patch_cstrings(&mut nodes, b"short", b"owner:short"), 2);
        let after = write_raw_list(&nodes).unwrap();
        assert_ne!(before, after);
        let RawValue::List {
            data_type,
            children,
        } = &nodes[0].value
        else {
            panic!("outer list type changed");
        };
        assert_eq!(*data_type, 101);
        assert_eq!(
            children[0].value,
            RawValue::CString(b"owner:short".to_vec())
        );
        assert_eq!(children[1].value, RawValue::CString(b"shorter".to_vec()));
    }

    #[test]
    fn patches_exact_f32_values_without_touching_ints_or_other_floats() {
        let old = 750.0f32.to_le_bytes();
        let new = 3000.0f32.to_le_bytes();
        let mut nodes = vec![
            RawNode {
                key_hash: 1,
                value: RawValue::FourBytes {
                    data_type: 0,
                    bytes: old,
                },
            },
            RawNode {
                key_hash: 2,
                value: RawValue::FourBytes {
                    data_type: 1,
                    bytes: old,
                },
            },
            RawNode {
                key_hash: 3,
                value: RawValue::List {
                    data_type: 100,
                    children: vec![RawNode {
                        key_hash: 4,
                        value: RawValue::FourBytes {
                            data_type: 0,
                            bytes: 15.0f32.to_le_bytes(),
                        },
                    }],
                },
            },
        ];

        assert_eq!(patch_f32s(&mut nodes, 1, old, new), 1);
        assert_eq!(
            nodes[0].value,
            RawValue::FourBytes {
                data_type: 0,
                bytes: new
            }
        );
        assert_eq!(
            nodes[1].value,
            RawValue::FourBytes {
                data_type: 1,
                bytes: old
            }
        );
    }

    #[test]
    fn raw_list_round_trips_with_alignment_padding() {
        let nodes = vec![
            RawNode {
                key_hash: 9,
                value: RawValue::Bool(1),
            },
            RawNode {
                key_hash: 10,
                value: RawValue::CString(b"abc".to_vec()),
            },
            RawNode {
                key_hash: 11,
                value: RawValue::LocString(vec![0x41, 0x42]),
            },
            RawNode {
                key_hash: 12,
                value: RawValue::FourBytes {
                    data_type: 1,
                    bytes: 7u32.to_le_bytes(),
                },
            },
        ];
        let bytes = write_raw_list(&nodes).unwrap();
        let parsed = read_raw_list(&mut Cursor::new(&bytes)).unwrap();
        assert_eq!(parsed, nodes);
        assert_eq!(write_raw_list(&parsed).unwrap(), bytes);
    }

    #[test]
    fn container_patch_changes_only_the_aegd_chunk() {
        let list = write_raw_list(&[RawNode {
            key_hash: 42,
            value: RawValue::CString(b"eng_house".to_vec()),
        }])
        .unwrap();
        let mut crc = Crc::new();
        crc.update(&list);
        let mut aegd = crc.sum().to_le_bytes().to_vec();
        aegd.extend_from_slice(&list);
        let chunky = Chunky {
            major: 4,
            minor: 1,
            platform: 1,
            chunks: vec![
                Chunk {
                    kind: ChunkKind::Data,
                    name: *b"AEGD",
                    version: 3,
                    path: Vec::new(),
                    body: ChunkBody::Data(aegd),
                },
                Chunk {
                    kind: ChunkKind::Data,
                    name: *b"KEYS",
                    version: 1,
                    path: Vec::new(),
                    body: ChunkBody::Data(vec![0, 0, 0, 0]),
                },
            ],
        };
        let mut input = Vec::new();
        chunky.write(&mut input).unwrap();

        let (patched, report) =
            patch_cstring_bytes(&input, b"eng_house", b"owner:eng_house").unwrap();
        assert_eq!(report.replacements, 1);
        assert_eq!(report.patched_aegd_size, report.original_aegd_size + 6);
        let reparsed = Chunky::read(&mut Cursor::new(&patched)).unwrap();
        assert_eq!(reparsed.chunks[1], chunky.chunks[1]);
        let aegd = reparsed.chunks[0].data().unwrap();
        let nodes = read_raw_list(&mut Cursor::new(&aegd[4..])).unwrap();
        assert_eq!(
            nodes[0].value,
            RawValue::CString(b"owner:eng_house".to_vec())
        );
        let mut crc = Crc::new();
        crc.update(&aegd[4..]);
        assert_eq!(&aegd[..4], crc.sum().to_le_bytes());

        assert!(
            patch_cstring_bytes(&input, b"missing", b"x")
                .unwrap()
                .1
                .replacements
                == 0
        );
        assert!(patch_cstring_bytes(&input, b"", b"x").is_err());
    }
}
