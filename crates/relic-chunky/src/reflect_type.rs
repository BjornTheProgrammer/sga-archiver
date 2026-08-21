use std::collections::BTreeMap;
use std::io::{BufReader, Cursor, Read, Seek};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::container::{Chunk, ChunkBody, Chunky};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldDef {
    pub name: String,
    pub type_name: String,
    pub offset: u32,
    pub size: u32,
    pub name_hash: u64,
    pub type_hash: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypeDef {
    pub name: String,
    pub hash: u64,
    pub size: u32,
    pub trailer: u32,
    pub fields: Vec<FieldDef>,
}

fn u32_at(b: &[u8], o: usize) -> Option<u32> {
    b.get(o..o + 4).map(|s| u32::from_le_bytes(s.try_into().unwrap()))
}
fn u64_at(b: &[u8], o: usize) -> Option<u64> {
    b.get(o..o + 8).map(|s| u64::from_le_bytes(s.try_into().unwrap()))
}

fn read_pascal(b: &[u8], o: usize) -> Option<(String, usize)> {
    let len = u32_at(b, o)? as usize;
    if len == 0 || len > 256 || o + 4 + len > b.len() {
        return None;
    }
    let s = &b[o + 4..o + 4 + len];
    if s.iter().all(|&c| (0x20..=0x7e).contains(&c)) {
        Some((String::from_utf8_lossy(s).into_owned(), o + 4 + len))
    } else {
        None
    }
}

fn is_type_token(s: &str) -> bool {
    let first = s.chars().next();
    match first {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    s.chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | ':' | '<' | '>' | ',' | '*' | ' '))
}

const FIELD_TAIL: usize = 64;

pub fn parse_type(payload: &[u8]) -> Option<TypeDef> {
    let (name, after_name) = read_pascal(payload, 0)?;
    let hash = u64_at(payload, after_name)?;
    let size = u32_at(payload, after_name + 8)?;
    let trailer = u32_at(payload, after_name + 24).unwrap_or(0);

    let mut fields = Vec::new();
    let mut o = after_name + 8;
    while o + 4 < payload.len() {
        if let Some((field_name, e)) = read_pascal(payload, o) {
            if field_name.starts_with("m_") {
                if let Some(name_hash) = u64_at(payload, e) {
                    if let Some((type_name, te)) = read_pascal(payload, e + 8) {
                        if is_type_token(&type_name) && !type_name.starts_with("m_") {
                            if let (Some(type_hash), Some(offset), Some(fsize)) =
                                (u64_at(payload, te), u32_at(payload, te + 8), u32_at(payload, te + 12))
                            {
                                if offset < size && fsize > 0 && fsize <= size {
                                    fields.push(FieldDef {
                                        name: field_name,
                                        type_name,
                                        offset,
                                        size: fsize,
                                        name_hash,
                                        type_hash,
                                    });
                                    o = te + 16 + FIELD_TAIL;
                                    continue;
                                }
                            }
                        }
                    }
                }
            }
        }
        o += 1;
    }

    Some(TypeDef {
        name,
        hash,
        size,
        trailer,
        fields,
    })
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SchemaRegistry {
    pub types: BTreeMap<u64, TypeDef>,
}

impl SchemaRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_from_chunky(&mut self, chunky: &Chunky) {
        collect_rfty(&chunky.chunks, &mut self.types);
    }

    pub fn scan_reader<R: Read + Seek>(&mut self, reader: &mut R) -> bool {
        match Chunky::read(reader) {
            Ok(chunky) => {
                let before = self.types.len();
                self.add_from_chunky(&chunky);
                self.types.len() > before
            }
            Err(_) => false,
        }
    }

    pub fn scan_dir<P: AsRef<Path>>(&mut self, dir: P) -> std::io::Result<usize> {
        let mut scanned = 0;
        let mut stack = vec![dir.as_ref().to_path_buf()];
        while let Some(path) = stack.pop() {
            let entries = match std::fs::read_dir(&path) {
                Ok(e) => e,
                Err(_) => continue,
            };
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                if !is_chunky_file(&p) {
                    continue;
                }
                if let Ok(file) = std::fs::File::open(&p) {
                    let mut reader = BufReader::new(file);
                    if self.scan_reader(&mut reader) {
                        scanned += 1;
                    }
                }
            }
        }
        Ok(scanned)
    }
}

fn collect_rfty(chunks: &[Chunk], out: &mut BTreeMap<u64, TypeDef>) {
    for chunk in chunks {
        match &chunk.body {
            ChunkBody::Data(data) => {
                if &chunk.name == b"RFTY" {
                    if let Some(ty) = parse_type(data) {
                        out.entry(ty.hash).or_insert(ty);
                    }
                }
            }
            ChunkBody::Folder(children) => collect_rfty(children, out),
        }
    }
}

fn is_chunky_file(path: &Path) -> bool {
    let mut magic = [0u8; 16];
    match std::fs::File::open(path).and_then(|mut f| f.read_exact(&mut magic).map(|_| magic)) {
        Ok(m) => &m == b"Relic Chunky\r\n\x1a\0",
        Err(_) => false,
    }
}

pub fn parse_type_from_bytes(bytes: &[u8]) -> Option<TypeDef> {
    let mut cursor = Cursor::new(bytes);
    let chunky = Chunky::read(&mut cursor).ok()?;
    let mut map = BTreeMap::new();
    collect_rfty(&chunky.chunks, &mut map);
    map.into_values().next()
}
