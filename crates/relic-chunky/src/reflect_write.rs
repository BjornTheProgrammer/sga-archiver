use std::collections::HashMap;
use std::io::{Cursor, Write};

use anyhow::{anyhow, bail, Result};
use byteorder::{LittleEndian, WriteBytesExt};
use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;

use crate::container::{Chunk, ChunkBody, ChunkKind, Chunky};
use crate::reflect_type::SchemaRegistry;

/// The reflection data blob (`RFCI`) is the first chunk, so its bytes begin
/// after the file header (24) and the chunk header (20).
const RFCI_FILE_OFFSET: u32 = 44;
/// The blob opens with a reserved word before the root object's data.
const RFCI_PREFIX: u32 = 4;

// ---------------------------------------------------------------------------
// `.rdo` object graph
// ---------------------------------------------------------------------------

/// One `<DataObject>` from the `.rdo`, with its scalar/bool/string fields keyed
/// by field name and its object-valued fields kept as ordered `(field, id)`
/// pairs (a field may name several children when it is an array).
#[derive(Debug, Default)]
struct RdoObject {
    id: u64,
    type_name: String,
    owner_id: u64,
    scalars: HashMap<String, u32>,
    bools: HashMap<String, bool>,
    strings: HashMap<String, String>,
    children: Vec<(String, u64)>,
}

impl RdoObject {
    fn child_ids(&self, field: &str) -> Vec<u64> {
        self.children
            .iter()
            .filter(|(name, _)| name == field)
            .map(|(_, id)| *id)
            .collect()
    }
}

/// Resolves the XML entities that appear in `.rdo` attribute values (notably
/// `&lt;`/`&gt;` in template type names) so names match the schema.
fn unescape(raw: &str) -> String {
    raw.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

fn attr(tag: &BytesStart, name: &str) -> Option<String> {
    tag.attributes().flatten().find_map(|a| {
        if a.key.as_ref() == name.as_bytes() {
            Some(unescape(&String::from_utf8_lossy(&a.value)))
        } else {
            None
        }
    })
}

/// Stores a leaf `<DataProperty>` on `object` according to its declared type.
fn store_scalar(object: &mut RdoObject, prop_type: &str, name: String, value: &str) {
    match prop_type {
        "Bool" => {
            object.bools.insert(name, value.eq_ignore_ascii_case("true"));
        }
        "String" => {
            object.strings.insert(name, value.to_string());
        }
        "Float" => {
            object.scalars.insert(name, value.parse::<f32>().unwrap_or(0.0).to_bits());
        }
        "Int32" => {
            object.scalars.insert(name, value.parse::<i32>().unwrap_or(0) as u32);
        }
        _ => {
            object.scalars.insert(name, value.parse::<u32>().unwrap_or(0));
        }
    }
}

fn parse_rdo(xml: &str) -> Result<Vec<RdoObject>> {
    let mut reader = Reader::from_str(xml);
    let mut objects: Vec<RdoObject> = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    // Open object-valued properties: (owning object index, field name). A
    // `<DataObject>` seen while one is open is a child of that object via that
    // field; the stack handles nested object properties.
    let mut open_fields: Vec<(usize, String)> = Vec::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(tag)) if tag.name().as_ref() == b"DataObject" => {
                let id = attr(&tag, "Id").and_then(|s| s.parse().ok()).unwrap_or(0);
                let idx = objects.len();
                objects.push(RdoObject {
                    id,
                    type_name: attr(&tag, "Type").unwrap_or_default(),
                    owner_id: attr(&tag, "OwnerId").and_then(|s| s.parse().ok()).unwrap_or(0),
                    ..Default::default()
                });
                if let Some((parent, field)) = open_fields.last() {
                    objects[*parent].children.push((field.clone(), id));
                }
                stack.push(idx);
            }
            Ok(Event::Start(tag)) if tag.name().as_ref() == b"DataProperty" => {
                if attr(&tag, "Type").as_deref() == Some("Object") {
                    if let (Some(&current), Some(field)) = (stack.last(), attr(&tag, "Name")) {
                        open_fields.push((current, field));
                    }
                }
            }
            Ok(Event::Empty(tag)) if tag.name().as_ref() == b"DataProperty" => {
                let prop_type = attr(&tag, "Type").unwrap_or_default();
                // A self-closing object property is just an empty array; there
                // is no value to store.
                if prop_type == "Object" {
                    continue;
                }
                if let (Some(&current), Some(name), Some(value)) =
                    (stack.last(), attr(&tag, "Name"), attr(&tag, "Value"))
                {
                    store_scalar(&mut objects[current], &prop_type, name, &value);
                }
            }
            Ok(Event::End(tag)) if tag.name().as_ref() == b"DataObject" => {
                stack.pop();
            }
            Ok(Event::End(tag)) if tag.name().as_ref() == b"DataProperty" => {
                open_fields.pop();
            }
            Ok(Event::Eof) => break,
            Err(err) => bail!("failed to parse .rdo xml: {err}"),
            _ => {}
        }
    }

    Ok(objects)
}

// ---------------------------------------------------------------------------
// Field classification and alignment
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Scalar,
    Bool,
    Str,
    Enum,
    Embed,
    OffsetPointer,
    Array,
    PointerArray,
    Opaque,
}

fn is_enum(type_name: &str) -> bool {
    type_name.contains("FamilyManagerEnum") || type_name.contains("ReflectStringHash")
}

fn classify(type_name: &str, reg: &SchemaRegistry) -> Kind {
    if type_name.contains("ReflectArray") {
        if type_name.contains('*') {
            Kind::PointerArray
        } else {
            Kind::Array
        }
    } else if is_enum(type_name) {
        Kind::Enum
    } else if type_name.contains("ReflectString") {
        Kind::Str
    } else if type_name.contains("ReflectOffsetPointer") {
        Kind::OffsetPointer
    } else if type_name == "bool" {
        Kind::Bool
    } else if type_name == "float"
        || type_name.contains("int32")
        || type_name.contains("uint32")
    {
        Kind::Scalar
    } else if reg.by_name(type_name).is_some_and(|t| !t.fields.is_empty()) {
        Kind::Embed
    } else {
        Kind::Opaque
    }
}

/// Alignment demanded by a single field, from its type name.
fn field_align(type_name: &str) -> u32 {
    if type_name.contains("ReflectOffsetPointer")
        || type_name.contains("ReflectString")
        || type_name.contains("ReflectArray")
        || type_name.contains('*')
        || type_name.contains("int64")
        || type_name.contains("uint64")
    {
        8
    } else if type_name == "float" || type_name.contains("int32") || type_name.contains("uint32") {
        4
    } else {
        1
    }
}

/// Alignment of a whole struct: the widest alignment of its fields. Types with
/// no schema fields fall back to pointer alignment.
fn type_align(reg: &SchemaRegistry, type_name: &str) -> u32 {
    match reg.by_name(type_name) {
        Some(ty) if !ty.fields.is_empty() => {
            ty.fields.iter().map(|f| field_align(&f.type_name)).max().unwrap_or(1)
        }
        _ => 8,
    }
}

/// Round a blob-relative offset up so that its file position (`+44`) is
/// `align`-aligned.
fn align_file(rel: u32, align: u32) -> u32 {
    (((RFCI_FILE_OFFSET + rel) + align - 1) & !(align - 1)) - RFCI_FILE_OFFSET
}

use crate::hash::city_hash64;

// ---------------------------------------------------------------------------
// Serializer
// ---------------------------------------------------------------------------

/// A piece of an object placed out-of-line, ordered by `eff` (its field's blob
/// offset). All of an object's out-of-line pieces are emitted in descending
/// field-offset order, sharing the single append cursor.
enum Item {
    Str { eff: u32, fpos: u32, val: String },
    Array { eff: u32, fpos: u32, children: Vec<u64> },
    Pointers { eff: u32, fpos: u32, children: Vec<u64> },
    OffsetPointer { eff: u32, fpos: u32, children: Vec<u64> },
}

impl Item {
    fn eff(&self) -> u32 {
        match self {
            Item::Str { eff, .. }
            | Item::Array { eff, .. }
            | Item::Pointers { eff, .. }
            | Item::OffsetPointer { eff, .. } => *eff,
        }
    }
}

/// Read-only inputs shared by the recursive placement.
struct Ctx<'a> {
    objects: &'a [RdoObject],
    by_id: &'a HashMap<u64, usize>,
    registry: &'a SchemaRegistry,
}

impl<'a> Ctx<'a> {
    fn get(&self, id: u64) -> Result<usize> {
        self.by_id.get(&id).copied().ok_or_else(|| anyhow!("child object {id} not found"))
    }
    fn align_of(&self, id: u64) -> u32 {
        type_align(self.registry, &self.objects[self.by_id[&id]].type_name)
    }
}

/// One 8-byte relative pointer written into the blob (a `ReflectArray<X*>`
/// entry). These drive the relocation table (`RFUP`) and the allocation table
/// (`RNEW`).
struct PtrRec {
    /// File offset of the pointer field.
    file_off: u32,
    /// Placed target object.
    target: usize,
    /// Blob offset of the target object's data.
    target_off: u32,
}

/// The growing object-data blob plus placement bookkeeping.
struct State {
    blob: Vec<u8>,
    /// Logical end of written data (the blob is grown to match).
    len: u32,
    /// Append cursor for out-of-line data.
    next: u32,
    /// Objects in the order they are placed, as `(object index, blob offset)`.
    order: Vec<(usize, u32)>,
    /// Interned enum / string-hash values, in first-seen order.
    rshi: Vec<(u64, String)>,
    /// Every 8-byte relative pointer written, in emission order.
    pointers: Vec<PtrRec>,
}

impl State {
    fn ensure(&mut self, end: u32) {
        if self.blob.len() < end as usize {
            self.blob.resize(end as usize, 0);
        }
        if end > self.len {
            self.len = end;
        }
    }
    fn w32(&mut self, off: u32, v: u32) {
        self.ensure(off + 4);
        self.blob[off as usize..off as usize + 4].copy_from_slice(&v.to_le_bytes());
    }
    fn wi32(&mut self, off: u32, v: i32) {
        self.w32(off, v as u32);
    }
    fn w64(&mut self, off: u32, v: u64) {
        self.ensure(off + 8);
        self.blob[off as usize..off as usize + 8].copy_from_slice(&v.to_le_bytes());
    }
    fn wi64(&mut self, off: u32, v: i64) {
        self.w64(off, v as u64);
    }
    fn w8(&mut self, off: u32, v: u8) {
        self.ensure(off + 1);
        self.blob[off as usize] = v;
    }
    fn wbytes(&mut self, off: u32, bytes: &[u8]) {
        self.ensure(off + bytes.len() as u32);
        self.blob[off as usize..off as usize + bytes.len()].copy_from_slice(bytes);
    }
    /// Advance the append cursor to the next `align`-aligned free position.
    fn append(&mut self, align: u32) -> u32 {
        self.next = align_file(self.next.max(self.len), align);
        self.next
    }
    fn intern(&mut self, hash: u64, value: String) {
        if !self.rshi.iter().any(|(h, _)| *h == hash) {
            self.rshi.push((hash, value));
        }
    }
}

/// Writes an object's inline data at `off`, recursing into embedded structs and
/// enums (which live inside the parent's footprint), and collecting every
/// out-of-line field into `out` with its blob offset for later placement.
fn inline_write(st: &mut State, ctx: &Ctx, idx: usize, off: u32, out: &mut Vec<Item>) {
    let object = &ctx.objects[idx];
    st.order.push((idx, off));
    let size = ctx.registry.by_name(&object.type_name).map(|t| t.size).unwrap_or(8);
    st.ensure(off + size);

    if is_enum(&object.type_name) {
        let name = object.strings.get("m_enumName").cloned().unwrap_or_default();
        let hash = city_hash64(name.to_lowercase().as_bytes());
        st.w64(off, hash);
        st.intern(hash, name);
        return;
    }

    let Some(ty) = ctx.registry.by_name(&object.type_name) else {
        return;
    };

    for field in &ty.fields {
        let fpos = off + field.offset;
        match classify(&field.type_name, ctx.registry) {
            Kind::Scalar => st.w32(fpos, object.scalars.get(&field.name).copied().unwrap_or(0)),
            Kind::Bool => st.w8(fpos, object.bools.get(&field.name).copied().unwrap_or(false) as u8),
            Kind::Str => out.push(Item::Str {
                eff: fpos,
                fpos,
                val: object.strings.get(&field.name).cloned().unwrap_or_default(),
            }),
            Kind::Embed | Kind::Enum => {
                if let Some(&child) = object.child_ids(&field.name).first() {
                    if let Ok(ci) = ctx.get(child) {
                        inline_write(st, ctx, ci, fpos, out);
                    }
                }
            }
            Kind::Array => out.push(Item::Array {
                eff: fpos,
                fpos,
                children: object.child_ids(&field.name),
            }),
            Kind::PointerArray => out.push(Item::Pointers {
                eff: fpos,
                fpos,
                children: object.child_ids(&field.name),
            }),
            Kind::OffsetPointer => out.push(Item::OffsetPointer {
                eff: fpos,
                fpos,
                children: object.child_ids(&field.name),
            }),
            Kind::Opaque => {}
        }
    }
}

/// Places an object at `off`: writes its inline data, then emits its
/// out-of-line fields (strings and arrays) in descending field-offset order,
/// recursing into referenced objects.
fn place(st: &mut State, ctx: &Ctx, idx: usize, off: u32) -> Result<()> {
    let mut out = Vec::new();
    inline_write(st, ctx, idx, off, &mut out);
    out.sort_by(|a, b| b.eff().cmp(&a.eff()));

    for item in out {
        match item {
            Item::Str { fpos, val, .. } => {
                let at = st.next.max(st.len);
                st.next = at;
                if !val.is_empty() {
                    st.wbytes(at, val.as_bytes());
                }
                // reserve the value plus a NUL terminator, even when empty
                st.ensure(at + val.len() as u32 + 1);
                st.wi32(fpos, at as i32 - fpos as i32);
                st.wi32(fpos + 8, val.len() as i32);
            }
            Item::Array { fpos, children, .. } => {
                if children.is_empty() {
                    st.wi32(fpos, 0);
                    st.wi32(fpos + 8, 0);
                    continue;
                }
                let first = st.append(ctx.align_of(children[0]));
                for &child in &children {
                    let at = st.append(ctx.align_of(child));
                    place(st, ctx, ctx.get(child)?, at)?;
                }
                st.wi32(fpos, first as i32 - fpos as i32);
                st.wi32(fpos + 8, children.len() as i32);
            }
            Item::Pointers { fpos, children, .. } => {
                if children.is_empty() {
                    st.wi32(fpos, 0);
                    st.wi32(fpos + 8, 0);
                    continue;
                }
                // The editor keeps pointer-array targets in an unordered_map
                // keyed by object id; their placement order is the bucket order
                // (id % 8 for the default table), and the pointers themselves
                // are written in reverse of that order.
                let mut ordered = children.clone();
                ordered.sort_by_key(|id| *id % 8);
                let block = st.append(8);
                st.next = block + ordered.len() as u32 * 8;
                st.ensure(st.next);
                let mut targets = Vec::with_capacity(ordered.len());
                for &child in &ordered {
                    let at = st.append(ctx.align_of(child));
                    targets.push(at);
                    place(st, ctx, ctx.get(child)?, at)?;
                }
                for j in 0..targets.len() {
                    let k = targets.len() - 1 - j;
                    let target_off = targets[k];
                    let ptr = block + j as u32 * 8;
                    st.wi64(ptr, target_off as i64 - ptr as i64);
                    st.pointers.push(PtrRec {
                        file_off: RFCI_FILE_OFFSET + ptr,
                        target: ctx.get(ordered[k])?,
                        target_off,
                    });
                }
                st.wi32(fpos, block as i32 - fpos as i32);
                st.wi32(fpos + 8, ordered.len() as i32);
            }
            Item::OffsetPointer { fpos, children, .. } => {
                let Some(&child) = children.first() else {
                    st.wi32(fpos, 0);
                    continue;
                };
                let at = st.append(ctx.align_of(child));
                place(st, ctx, ctx.get(child)?, at)?;
                st.wi32(fpos, at as i32 - fpos as i32);
            }
        }
    }

    Ok(())
}

/// Runs the placement for a whole `.rdo` graph, returning the populated state
/// and the root object's index.
fn place_all(objects: &[RdoObject], registry: &SchemaRegistry) -> Result<(State, usize)> {
    let by_id: HashMap<u64, usize> =
        objects.iter().enumerate().map(|(i, o)| (o.id, i)).collect();
    let root = objects
        .iter()
        .position(|o| o.owner_id == 0)
        .ok_or_else(|| anyhow!("no root object (owner id 0) in .rdo"))?;
    let ctx = Ctx { objects, by_id: &by_id, registry };
    let root_size = registry
        .by_name(&objects[root].type_name)
        .ok_or_else(|| anyhow!("root type not in registry: {}", objects[root].type_name))?
        .size;
    let mut st = State {
        blob: vec![0u8; RFCI_PREFIX as usize],
        len: RFCI_PREFIX,
        next: RFCI_PREFIX + root_size,
        order: Vec::new(),
        rshi: Vec::new(),
        pointers: Vec::new(),
    };
    place(&mut st, &ctx, root, RFCI_PREFIX)?;
    st.blob.truncate(st.len as usize);
    Ok((st, root))
}

fn build_robj(st: &State, objects: &[RdoObject], registry: &SchemaRegistry) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    out.write_u32::<LittleEndian>(st.order.len() as u32)?;
    out.write_u32::<LittleEndian>(0)?;
    for &(idx, off) in &st.order {
        let object = &objects[idx];
        let ty = registry
            .by_name(&object.type_name)
            .ok_or_else(|| anyhow!("type not in registry: {}", object.type_name))?;
        out.write_u64::<LittleEndian>(object.id)?;
        out.write_u64::<LittleEndian>(ty.hash)?;
        out.write_u32::<LittleEndian>(RFCI_FILE_OFFSET + off)?;
        out.write_u32::<LittleEndian>(0)?;
        out.write_u64::<LittleEndian>(object.owner_id)?;
        out.write_u32::<LittleEndian>(ty.trailer)?;
    }
    Ok(out)
}

/// The relocation table: the sorted file offsets of every 8-byte relative
/// pointer, so the loader can fix them up after the blob moves.
fn build_rfup(st: &State) -> Vec<u8> {
    let mut offsets: Vec<u32> = st.pointers.iter().map(|p| p.file_off).collect();
    offsets.sort_unstable();
    let mut out = Vec::new();
    out.extend_from_slice(&(offsets.len() as u64).to_le_bytes());
    for off in offsets {
        out.extend_from_slice(&(off as u64).to_le_bytes());
    }
    out
}

/// The allocation table: one 29-byte record per heap-allocated object — every
/// pointer-array target, then the root last.
fn build_rnew(
    st: &State,
    objects: &[RdoObject],
    registry: &SchemaRegistry,
    root: usize,
) -> Result<Vec<u8>> {
    let mut records: Vec<(usize, u32)> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for pointer in &st.pointers {
        if seen.insert(objects[pointer.target].id) {
            records.push((pointer.target, RFCI_FILE_OFFSET + pointer.target_off));
        }
    }
    records.push((root, RFCI_FILE_OFFSET + RFCI_PREFIX));

    let mut out = Vec::new();
    out.write_u64::<LittleEndian>(records.len() as u64)?;
    for (idx, data_off) in records {
        let object = &objects[idx];
        let ty = registry
            .by_name(&object.type_name)
            .ok_or_else(|| anyhow!("type not in registry: {}", object.type_name))?;
        out.write_u64::<LittleEndian>(object.id)?;
        out.write_u64::<LittleEndian>(ty.hash)?;
        out.write_u64::<LittleEndian>(data_off as u64)?;
        out.write_u32::<LittleEndian>(ty.trailer)?;
        out.write_u8(u8::from(object.owner_id == 0))?;
    }
    Ok(out)
}

fn build_rshi(st: &State) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    out.write_u32::<LittleEndian>(st.rshi.len() as u32)?;
    out.write_u32::<LittleEndian>(0)?;
    for (hash, value) in &st.rshi {
        out.write_u64::<LittleEndian>(*hash)?;
        out.write_u32::<LittleEndian>(value.len() as u32)?;
        out.write_all(value.as_bytes())?;
    }
    Ok(out)
}

fn replace_chunk(chunks: &mut [Chunk], name: &[u8; 4], data: Vec<u8>) -> bool {
    for chunk in chunks {
        match &mut chunk.body {
            ChunkBody::Data(existing) if &chunk.name == name => {
                *existing = data;
                return true;
            }
            ChunkBody::Folder(children) => {
                if replace_chunk(children, name, data.clone()) {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

/// Compiles a `.rdo` object graph into a reflection `.bin`, reusing the type
/// schema and the invariant chunks (`RFTY`, `RNEW`, ...) from `reference` and
/// rebuilding the object data (`RFCI`), object table (`ROBJ`) and, when the
/// graph interns any values, the string-hash table (`RSHI`).
pub fn compile_reflect(rdo_xml: &str, registry: &SchemaRegistry, reference: &Chunky) -> Result<Chunky> {
    let objects = parse_rdo(rdo_xml)?;
    let (st, _root) = place_all(&objects, registry)?;

    let robj = build_robj(&st, &objects, registry)?;
    // Only rewrite RSHI when the graph actually interns values; otherwise leave
    // the reference table untouched.
    let rshi = (!st.rshi.is_empty()).then(|| build_rshi(&st)).transpose()?;
    let rfci = st.blob;

    let mut out = reference.clone();
    if !replace_chunk(&mut out.chunks, b"RFCI", rfci) {
        bail!("reference .bin has no RFCI chunk");
    }
    if !replace_chunk(&mut out.chunks, b"ROBJ", robj) {
        bail!("reference .bin has no ROBJ chunk");
    }
    if let Some(rshi) = rshi {
        replace_chunk(&mut out.chunks, b"RSHI", rshi);
    }
    Ok(out)
}

/// Version stamped on the reflection chunks (`RFCI`, `RNEW`, `ROBJ`, ...).
const REFLECT_CHUNK_VERSION: u32 = 4131415;

fn data_chunk(name: &[u8; 4], body: Vec<u8>) -> Chunk {
    Chunk {
        kind: ChunkKind::Data,
        name: *name,
        version: REFLECT_CHUNK_VERSION,
        path: Vec::new(),
        body: ChunkBody::Data(body),
    }
}

/// The schema chunk (`RFDB` folder) of a reflection container, holding the
/// `SIZE` count and the `RFTY` type definitions.
pub fn schema_chunk(chunky: &Chunky) -> Option<&Chunk> {
    chunky.chunks.iter().find(|c| &c.name == b"RFDB")
}

/// Compiles a `.rdo` graph into a complete reflection container, generating
/// every chunk from the `.rdo` and splicing in `schema` (an `RFDB` folder that
/// supplies the invariant engine type definitions). Unlike [`compile_reflect`],
/// this needs no reference `.bin` — only the schema, which is the same for every
/// artifact of a given root type.
pub fn compile_reflect_full(
    rdo_xml: &str,
    registry: &SchemaRegistry,
    schema: &Chunk,
) -> Result<Chunky> {
    let objects = parse_rdo(rdo_xml)?;
    let (st, root) = place_all(&objects, registry)?;

    let robj = build_robj(&st, &objects, registry)?;
    let rshi = build_rshi(&st)?;
    let rfup = build_rfup(&st);
    let rnew = build_rnew(&st, &objects, registry, root)?;
    let rfci = st.blob;

    Ok(Chunky {
        major: 4,
        minor: 0,
        platform: 1,
        chunks: vec![
            data_chunk(b"RFCI", rfci),
            data_chunk(b"RFUP", rfup),
            data_chunk(b"RNEW", rnew),
            data_chunk(b"RSHI", rshi),
            schema.clone(),
            data_chunk(b"ROBJ", robj),
            data_chunk(b"RERF", vec![0u8; 8]),
        ],
    })
}

/// Recompiles a reflection `.bin` from its `.rdo` source, taking the type
/// schema and invariant chunks from the existing `.bin` bytes. Returns the new
/// `.bin` bytes, which are byte-identical to `reference_bin` when the `.rdo` is
/// unchanged.
pub fn recompile_bin(rdo_xml: &str, reference_bin: &[u8]) -> Result<Vec<u8>> {
    let reference = Chunky::read(&mut Cursor::new(reference_bin))?;
    let mut registry = SchemaRegistry::new();
    registry.add_from_chunky(&reference);
    let out = compile_reflect(rdo_xml, &registry, &reference)?;
    let mut buf = Vec::new();
    out.write(&mut buf)?;
    Ok(buf)
}

/// Compiles a reflection `.bin` from its `.rdo` source and a standalone schema
/// container (an `RFDB`-bearing `.bin` for the same root type). The schema
/// supplies the engine type definitions; the mod source needs no compiled
/// `.bin` of its own.
pub fn compile_bin_from_schema(rdo_xml: &str, schema_bin: &[u8]) -> Result<Vec<u8>> {
    let schema_container = Chunky::read(&mut Cursor::new(schema_bin))?;
    let mut registry = SchemaRegistry::new();
    registry.add_from_chunky(&schema_container);
    let schema = schema_chunk(&schema_container)
        .ok_or_else(|| anyhow!("schema container has no RFDB chunk"))?;
    let out = compile_reflect_full(rdo_xml, &registry, schema)?;
    let mut buf = Vec::new();
    out.write(&mut buf)?;
    Ok(buf)
}

/// Extracts a reusable schema resource from a compiled reflection `.bin`,
/// returning the root object's type name and a minimal container that carries
/// just the schema (`RFDB`). Used to regenerate the bundled schema library.
pub fn extract_schema(bin: &[u8]) -> Result<(String, Vec<u8>)> {
    let container = Chunky::read(&mut Cursor::new(bin))?;
    let mut registry = SchemaRegistry::new();
    registry.add_from_chunky(&container);

    let robj = container
        .chunks
        .iter()
        .find_map(|c| (c.name == *b"ROBJ").then(|| c.data()).flatten())
        .ok_or_else(|| anyhow!("schema source has no ROBJ chunk"))?;
    let root_hash = root_type_hash(robj)?;
    let root_type = registry
        .by_hash(root_hash)
        .map(|t| t.name.clone())
        .ok_or_else(|| anyhow!("root type hash {root_hash:#x} not found in schema"))?;

    let schema = schema_chunk(&container)
        .ok_or_else(|| anyhow!("schema source has no RFDB chunk"))?;
    let minimal = Chunky {
        major: container.major,
        minor: container.minor,
        platform: container.platform,
        chunks: vec![schema.clone()],
    };
    let mut buf = Vec::new();
    minimal.write(&mut buf)?;
    Ok((root_type, buf))
}

/// The type hash of the root object (owner id 0) from an `ROBJ` chunk body.
fn root_type_hash(robj: &[u8]) -> Result<u64> {
    let count = u32::from_le_bytes(robj.get(0..4).ok_or_else(|| anyhow!("short ROBJ"))?.try_into().unwrap());
    for i in 0..count as usize {
        let base = 8 + i * 36;
        let owner = u64::from_le_bytes(
            robj.get(base + 24..base + 32).ok_or_else(|| anyhow!("short ROBJ"))?.try_into().unwrap(),
        );
        if owner == 0 {
            let hash = u64::from_le_bytes(
                robj.get(base + 8..base + 16).ok_or_else(|| anyhow!("short ROBJ"))?.try_into().unwrap(),
            );
            return Ok(hash);
        }
    }
    bail!("no root object (owner id 0) in ROBJ")
}

/// The type of the root object (the one with owner id 0) declared in a `.rdo`.
pub fn root_type_of(rdo_xml: &str) -> Result<String> {
    let objects = parse_rdo(rdo_xml)?;
    objects
        .iter()
        .find(|o| o.owner_id == 0)
        .map(|o| o.type_name.clone())
        .ok_or_else(|| anyhow!("no root object (owner id 0) in .rdo"))
}

/// Compiles a `.rdo` into a complete reflection `.bin` using the schema bundled
/// in the tool for the `.rdo`'s root type — no reference `.bin` required.
pub fn compile_bin(rdo_xml: &str) -> Result<Vec<u8>> {
    let root_type = root_type_of(rdo_xml)?;
    let schema = crate::schema_lib::schema_for(&root_type)
        .ok_or_else(|| anyhow!("no bundled schema for root type '{root_type}'"))?;
    compile_bin_from_schema(rdo_xml, schema)
}
