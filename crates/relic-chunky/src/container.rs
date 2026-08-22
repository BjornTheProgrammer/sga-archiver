use std::io::{Cursor, Read, Seek, SeekFrom, Write};

use anyhow::{bail, Result};
use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};

const MAGIC: &[u8; 16] = b"Relic Chunky\r\n\x1a\0";

/// Rewrites length-prefixed ASCII string tokens inside every `DATA` payload of a
/// Relic Chunky. Each `(old, new)` is matched as `[u32 len][bytes]` and swapped;
/// chunk and folder lengths are recomputed on re-serialization. Returns the new
/// bytes and the number of tokens replaced. This is how a base-path reference
/// (e.g. `art\scenario\eng_house_age2\eng_house_age2`) is retargeted without
/// hand-fixing the container's length fields.
pub fn replace_length_prefixed_strings(bytes: &[u8], subs: &[(&str, &str)]) -> Result<(Vec<u8>, usize)> {
    let mut chunky = Chunky::read(&mut Cursor::new(bytes))?;
    let tokens: Vec<(Vec<u8>, Vec<u8>)> =
        subs.iter().map(|(o, n)| (length_prefixed(o), length_prefixed(n))).collect();
    let mut count = 0;
    for chunk in &mut chunky.chunks {
        count += patch_tokens(chunk, &tokens);
    }
    let mut out = Vec::new();
    chunky.write(&mut out)?;
    Ok((out, count))
}

fn length_prefixed(s: &str) -> Vec<u8> {
    let mut v = (s.len() as u32).to_le_bytes().to_vec();
    v.extend_from_slice(s.as_bytes());
    v
}

fn patch_tokens(chunk: &mut Chunk, tokens: &[(Vec<u8>, Vec<u8>)]) -> usize {
    match &mut chunk.body {
        ChunkBody::Folder(children) => children.iter_mut().map(|c| patch_tokens(c, tokens)).sum(),
        ChunkBody::Data(data) => {
            let mut n = 0;
            for (old, new) in tokens {
                let mut from = 0;
                while let Some(pos) = find_sub(&data[from..], old) {
                    let at = from + pos;
                    data.splice(at..at + old.len(), new.iter().copied());
                    from = at + new.len();
                    n += 1;
                }
            }
            n
        }
    }
}

fn find_sub(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChunkKind {
    Data,
    Folder,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChunkBody {
    Data(Vec<u8>),
    Folder(Vec<Chunk>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    pub kind: ChunkKind,
    pub name: [u8; 4],
    pub version: u32,
    pub path: Vec<u8>,
    pub body: ChunkBody,
}

impl Chunk {
    pub fn name_str(&self) -> String {
        String::from_utf8_lossy(&self.name).into_owned()
    }

    pub fn data(&self) -> Option<&[u8]> {
        match &self.body {
            ChunkBody::Data(bytes) => Some(bytes),
            ChunkBody::Folder(_) => None,
        }
    }

    pub fn children(&self) -> Option<&[Chunk]> {
        match &self.body {
            ChunkBody::Folder(children) => Some(children),
            ChunkBody::Data(_) => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunky {
    pub major: u16,
    pub minor: u16,
    pub platform: u32,
    pub chunks: Vec<Chunk>,
}

impl Chunky {
    pub fn read<R: Read + Seek>(reader: &mut R) -> Result<Chunky> {
        let mut magic = [0u8; 16];
        reader.read_exact(&mut magic)?;
        if &magic != MAGIC {
            bail!("not a Relic Chunky file");
        }

        let major = reader.read_u16::<LittleEndian>()?;
        let minor = reader.read_u16::<LittleEndian>()?;
        let platform = reader.read_u32::<LittleEndian>()?;

        let end = reader.seek(SeekFrom::End(0))?;
        reader.seek(SeekFrom::Start(24))?;
        let chunks = read_chunks(reader, end)?;

        Ok(Chunky {
            major,
            minor,
            platform,
            chunks,
        })
    }

    pub fn write<W: Write>(&self, writer: &mut W) -> Result<()> {
        writer.write_all(MAGIC)?;
        writer.write_u16::<LittleEndian>(self.major)?;
        writer.write_u16::<LittleEndian>(self.minor)?;
        writer.write_u32::<LittleEndian>(self.platform)?;
        for chunk in &self.chunks {
            write_chunk(writer, chunk)?;
        }
        Ok(())
    }

    /// Every `Data` chunk in the tree, depth-first in file order, paired with the
    /// absolute file offset where its body begins. The layout is contiguous, so
    /// these positions are computed from chunk sizes — matching what the flat
    /// reader used to expose for reflection/RGD offset math.
    pub fn data_chunks(&self) -> Vec<(&Chunk, u64)> {
        let mut out = Vec::new();
        collect_data_chunks(&self.chunks, FILE_HEADER_LEN, &mut out);
        out
    }
}

/// Size of the Relic Chunky file header (16-byte magic + major/minor/platform).
const FILE_HEADER_LEN: u64 = 24;
/// Fixed part of a chunk header: kind, name, version, length, path length.
const CHUNK_HEADER_FIXED: u64 = 20;

fn collect_data_chunks<'a>(chunks: &'a [Chunk], mut pos: u64, out: &mut Vec<(&'a Chunk, u64)>) -> u64 {
    for chunk in chunks {
        let body_pos = pos + CHUNK_HEADER_FIXED + chunk.path.len() as u64;
        pos = match &chunk.body {
            ChunkBody::Data(data) => {
                out.push((chunk, body_pos));
                body_pos + data.len() as u64
            }
            ChunkBody::Folder(children) => collect_data_chunks(children, body_pos, out),
        };
    }
    pos
}

fn read_chunks<R: Read + Seek>(reader: &mut R, limit: u64) -> Result<Vec<Chunk>> {
    let mut chunks = Vec::new();

    while reader.stream_position()? + 20 <= limit {
        let mut kind_bytes = [0u8; 4];
        reader.read_exact(&mut kind_bytes)?;
        let kind = match &kind_bytes {
            b"DATA" => ChunkKind::Data,
            b"FOLD" => ChunkKind::Folder,
            other => bail!("unknown chunk type {:?}", other),
        };

        let mut name = [0u8; 4];
        reader.read_exact(&mut name)?;
        let version = reader.read_u32::<LittleEndian>()?;
        let length = reader.read_u32::<LittleEndian>()? as u64;
        let path_len = reader.read_u32::<LittleEndian>()? as usize;
        let mut path = vec![0u8; path_len];
        reader.read_exact(&mut path)?;

        let data_start = reader.stream_position()?;
        let body = match kind {
            ChunkKind::Folder => ChunkBody::Folder(read_chunks(reader, data_start + length)?),
            ChunkKind::Data => {
                let mut data = vec![0u8; length as usize];
                reader.read_exact(&mut data)?;
                ChunkBody::Data(data)
            }
        };
        reader.seek(SeekFrom::Start(data_start + length))?;

        chunks.push(Chunk {
            kind,
            name,
            version,
            path,
            body,
        });
    }

    Ok(chunks)
}

fn write_chunk<W: Write>(writer: &mut W, chunk: &Chunk) -> Result<()> {
    let kind = match chunk.kind {
        ChunkKind::Data => b"DATA",
        ChunkKind::Folder => b"FOLD",
    };
    writer.write_all(kind)?;
    writer.write_all(&chunk.name)?;
    writer.write_u32::<LittleEndian>(chunk.version)?;

    let body_bytes = match &chunk.body {
        ChunkBody::Data(data) => data.clone(),
        ChunkBody::Folder(children) => {
            let mut buffer = Vec::new();
            for child in children {
                write_chunk(&mut buffer, child)?;
            }
            buffer
        }
    };

    writer.write_u32::<LittleEndian>(body_bytes.len() as u32)?;
    writer.write_u32::<LittleEndian>(chunk.path.len() as u32)?;
    writer.write_all(&chunk.path)?;
    writer.write_all(&body_bytes)?;
    Ok(())
}
