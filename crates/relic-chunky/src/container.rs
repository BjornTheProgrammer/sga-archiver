use std::io::{Read, Seek, SeekFrom, Write};

use anyhow::{bail, Result};
use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};

const MAGIC: &[u8; 16] = b"Relic Chunky\r\n\x1a\0";

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
