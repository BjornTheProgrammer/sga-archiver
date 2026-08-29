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
    /// Base types this type derives from, as `(type hash, index)`. Empty for
    /// types with no bases (or non-struct kinds).
    #[serde(default)]
    pub bases: Vec<(u64, u32)>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FieldKind {
    /// 4-byte value written inline (float / int32 / uint32).
    Scalar,
    /// 8-byte value written inline (int64 / uint64).
    Scalar64,
    /// 1-byte value written inline (char / uint8).
    Scalar8,
    /// 1-byte flag written inline.
    Bool,
    /// Out-of-line string: `[i32 rel][pad][i32 len]` inline.
    Str,
    /// Interned enum: `u64` string hash written inline, child object in `.rdo`.
    Enum,
    /// Struct child written inside the parent's footprint at the field offset.
    Embed,
    /// `[i32 rel]` inline pointing at an out-of-line child.
    OffsetPointer,
    /// By-value array: `[i32 rel][pad][i32 count]`, elements packed out-of-line.
    Array,
    /// Pointer array: `[i32 rel][pad][i32 count]` to a block of 8-byte
    /// relative pointers, one per out-of-line element.
    PointerArray,
    /// Not written; occupies its footprint as zeros.
    Opaque,
}

pub fn is_enum_type(type_name: &str) -> bool {
    type_name.contains("FamilyManagerEnum") || type_name.contains("ReflectStringHash")
}

pub fn classify_field(type_name: &str, has_fields: impl Fn(&str) -> bool) -> FieldKind {
    if type_name.contains("ReflectArray") {
        if type_name.contains('*') {
            FieldKind::PointerArray
        } else {
            FieldKind::Array
        }
    } else if is_enum_type(type_name) {
        FieldKind::Enum
    } else if type_name.contains("ReflectString") {
        FieldKind::Str
    } else if type_name.contains("ReflectOffsetPointer") {
        FieldKind::OffsetPointer
    } else if type_name == "bool" {
        FieldKind::Bool
    } else if type_name == "float"
        || type_name.contains("int32")
        || type_name == "int"
        || type_name == "unsigned int"
    {
        FieldKind::Scalar
    } else if type_name.contains("int64")
        || type_name == "long long"
        || type_name == "unsigned long long"
    {
        FieldKind::Scalar64
    } else if type_name == "char" || type_name == "uint8_t" || type_name == "unsigned char" {
        FieldKind::Scalar8
    } else if has_fields(type_name) {
        FieldKind::Embed
    } else {
        FieldKind::Opaque
    }
}

pub fn array_element_type(array_type: &str) -> String {
    let inner = array_type
        .strip_prefix("util::ReflectArray<")
        .and_then(|s| s.strip_suffix('>'))
        .unwrap_or(array_type);
    let elem = inner.strip_suffix(",StdTraits").unwrap_or(inner);
    elem.trim().trim_end_matches('*').trim().to_string()
}

/// A forward-only cursor over a chunk payload. Every read advances the
/// position, so the layout of a chunk is expressed as a sequence of reads
/// rather than absolute offset arithmetic.
struct ByteReader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> ByteReader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        ByteReader { bytes, pos: 0 }
    }

    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let slice = self.bytes.get(self.pos..self.pos + n)?;
        self.pos += n;
        Some(slice)
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }

    fn skip(&mut self, n: usize) -> Option<()> {
        self.take(n).map(|_| ())
    }

    /// A length-prefixed ASCII string (`u32` length followed by the bytes).
    fn pascal(&mut self) -> Option<String> {
        let len = self.u32()? as usize;
        if len == 0 || len > 256 {
            return None;
        }
        let raw = self.take(len)?;
        if raw.iter().all(|&c| (0x20..=0x7e).contains(&c)) {
            Some(String::from_utf8_lossy(raw).into_owned())
        } else {
            None
        }
    }
}

/// Layout of an `RFTY` payload, read in order:
///   name (pascal), then a type header, then the field table.
/// The type header holds the type hash, instance size, and per-type trailer at
/// fixed positions, followed by a base-class list (`base_count` then that many
/// 12-byte entries) and a fixed pad before the field count. The field table
/// (`field_count` records) is already flattened to include inherited fields, so
/// the bases only need to be skipped, not modelled.
const AFTER_SIZE_TO_TRAILER: usize = 12;
const AFTER_TRAILER_TO_BASE_COUNT: usize = 20;
const AFTER_BASES_TO_FIELD_COUNT: usize = 24;

/// Each field record: name (pascal), its hash, the field type name, that type's
/// hash, the field's byte offset and size, then a fixed trailer.
const FIELD_TRAILER_LEN: usize = 64;

/// Counts above this mark a non-struct kind (template, enum, pointer) whose
/// header this parser does not model; such types carry no placeable fields.
const MAX_COUNT: u64 = 512;

pub fn parse_type(payload: &[u8]) -> Option<TypeDef> {
    let mut cursor = ByteReader::new(payload);
    let name = cursor.pascal()?;
    let hash = cursor.u64()?;
    let size = cursor.u32()?;

    cursor.skip(AFTER_SIZE_TO_TRAILER)?;
    let trailer = cursor.u32()?;
    cursor.skip(AFTER_TRAILER_TO_BASE_COUNT)?;

    let base_count = cursor.u64()?;
    if base_count > MAX_COUNT {
        return Some(TypeDef { name, hash, size, trailer, fields: Vec::new(), bases: Vec::new() });
    }
    // Each base entry is `[u64 hash][u32 index]` (12 bytes).
    let mut bases = Vec::with_capacity(base_count as usize);
    for _ in 0..base_count {
        let hash = cursor.u64()?;
        let index = cursor.u32()?;
        bases.push((hash, index));
    }
    cursor.skip(AFTER_BASES_TO_FIELD_COUNT)?;

    let field_count = cursor.u64()?;
    let mut fields = Vec::new();
    if field_count <= MAX_COUNT {
        for _ in 0..field_count {
            match read_field(&mut cursor, size) {
                Some(field) => fields.push(field),
                None => {
                    fields.clear();
                    break;
                }
            }
        }
    }

    Some(TypeDef {
        name,
        hash,
        size,
        trailer,
        fields,
        bases,
    })
}

fn read_field(cursor: &mut ByteReader, type_size: u32) -> Option<FieldDef> {
    let name = cursor.pascal()?;
    let name_hash = cursor.u64()?;
    let type_name = cursor.pascal()?;
    let type_hash = cursor.u64()?;
    let offset = cursor.u32()?;
    let size = cursor.u32()?;
    cursor.skip(FIELD_TRAILER_LEN)?;

    if !name.starts_with("m_") || offset >= type_size || size == 0 || size > type_size {
        return None;
    }

    Some(FieldDef {
        name,
        type_name,
        offset,
        size,
        name_hash,
        type_hash,
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

    pub fn by_hash(&self, hash: u64) -> Option<&TypeDef> {
        self.types.get(&hash)
    }

    pub fn by_name(&self, name: &str) -> Option<&TypeDef> {
        self.types.values().find(|t| t.name == name)
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
