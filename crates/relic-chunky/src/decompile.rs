//! Reconstructs the editor-source `<DataWarehouse>` `.rdo` XML from a compiled
//! reflection `.bin` file (see [`crate::reflect`] for the chunk overview).
//!
//! ## Model (reverse-engineered against matched `.bin`/`.rdo` pairs)
//!
//! * `RFTY` type records carry, per field: the field name, an 8-byte name
//!   hash, the field's type token, an 8-byte type hash, then a `u32` byte
//!   offset and a `u32` size within the owning object. The type record header
//!   carries the type's own 8-byte hash and total object size.
//! * `RNEW` names the root object (its id + type hash).
//! * `ROBJ` is the object table: for every object, its id, type hash, owning
//!   object id, and its base offset into the `RFCI` data pool.
//! * `RFCI` is the flat field-data pool: each object's scalar/string fields
//!   live at `base + field.offset`. Strings are an offset-pointer + length
//!   into a packed blob at the tail of `RFCI`. Enum/`ReflectStringHash` fields
//!   are an 8-byte hash resolved against `RSHI`.
//!
//! The reconstruction walks the root object, emits each field as a
//! `<DataProperty>`, and recurses into owned child objects.

use std::collections::HashMap;
use std::io::{BufRead, Read, Seek};

use crate::chunky::{ChunkFile, ChunkType};

fn u32_at(b: &[u8], off: usize) -> Option<u32> {
    b.get(off..off + 4).map(|s| u32::from_le_bytes(s.try_into().unwrap()))
}
fn i32_at(b: &[u8], off: usize) -> Option<i32> {
    b.get(off..off + 4).map(|s| i32::from_le_bytes(s.try_into().unwrap()))
}
fn u64_at(b: &[u8], off: usize) -> Option<u64> {
    b.get(off..off + 8).map(|s| u64::from_le_bytes(s.try_into().unwrap()))
}

/// One declared field of a reflected type.
#[derive(Debug, Clone)]
pub struct Field {
    pub name: String,
    pub type_name: String,
    pub offset: usize,
    pub size: usize,
}

/// A reflected type: its name, 8-byte hash, total object size, fields, and the
/// base types it derives from (as `(base_type_hash, index)`; empty for types
/// with no base). `bases` is filled by a post-parse pass once every type hash
/// is known (see [`DecompiledReflect::resolve_bases`]).
#[derive(Debug, Clone)]
pub struct TypeDef {
    pub name: String,
    pub hash: u64,
    pub size: usize,
    pub fields: Vec<Field>,
    pub bases: Vec<(u64, u32)>,
    /// The record bytes between the type header and the first field, where the
    /// base-type table lives. Scanned in [`DecompiledReflect::resolve_bases`].
    header_region: Vec<u8>,
}

/// An object instance from the `ROBJ` table.
#[derive(Debug, Clone)]
pub struct ObjectRef {
    pub id: u64,
    pub type_hash: u64,
    pub owner_id: u64,
    pub data_offset: usize,
}

/// Everything needed to reconstruct the `.rdo`.
#[derive(Debug, Default)]
pub struct DecompiledReflect {
    /// Types keyed by their 8-byte hash.
    pub types: HashMap<u64, TypeDef>,
    /// Objects keyed by id, in `ROBJ` order.
    pub objects: Vec<ObjectRef>,
    /// The `RFCI` flat data pool.
    pub data: Vec<u8>,
    /// File offset where the `RFCI` chunk's data begins; object `data_offset`s
    /// in `ROBJ` are file-relative, so an object's base within [`Self::data`]
    /// is `data_offset - rfci_offset`.
    pub rfci_offset: usize,
    /// Interned strings from `RSHI`, keyed by hash.
    pub interned: HashMap<u64, String>,
    /// Root object id (from `RNEW`).
    pub root_id: u64,
}

fn is_type_token(s: &str) -> bool {
    let first = match s.chars().next() {
        Some(c) => c,
        None => return false,
    };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    s.chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | ':' | '<' | '>' | ',' | '*' | ' '))
}

/// Reads a length-prefixed printable string at `off`, returning it plus the
/// byte offset immediately after its content.
fn read_pascal(bytes: &[u8], off: usize) -> Option<(String, usize)> {
    let len = u32_at(bytes, off)? as usize;
    if len == 0 || len > 512 || off + 4 + len > bytes.len() {
        return None;
    }
    let s = &bytes[off + 4..off + 4 + len];
    if s.iter().all(|&c| (0x20..=0x7e).contains(&c)) {
        Some((String::from_utf8_lossy(s).into_owned(), off + 4 + len))
    } else {
        None
    }
}

/// Parses one `RFTY` chunk into a [`TypeDef`]. The records are tightly packed
/// (no 4-byte alignment): the leading length-prefixed type name is immediately
/// followed by the 8-byte type hash and a `u32` object size. Each field record
/// is `[name][name-hash(8)][type-token][type-hash(8)][offset(4)][size(4)]`.
fn parse_type(bytes: &[u8]) -> Option<TypeDef> {
    let (name, after_name) = read_pascal(bytes, 0)?;
    let hash = u64_at(bytes, after_name)?;
    let size = u32_at(bytes, after_name + 8).unwrap_or(0) as usize;

    let mut fields = Vec::new();
    let mut first_field_off = bytes.len();
    let mut i = after_name;
    while i + 4 < bytes.len() {
        match read_pascal(bytes, i) {
            Some((s, end)) if s.starts_with("m_") => {
                first_field_off = first_field_off.min(i);
                // [name][name-hash(8)][type-token][type-hash(8)][off(4)][size(4)]
                let after_name_hash = end + 8;
                match read_pascal(bytes, after_name_hash) {
                    Some((t, tend))
                        if is_type_token(&t) && !t.starts_with("m_") =>
                    {
                        let after_type_hash = tend + 8;
                        let offset = u32_at(bytes, after_type_hash).unwrap_or(0) as usize;
                        let fsize = u32_at(bytes, after_type_hash + 4).unwrap_or(0) as usize;
                        fields.push(Field {
                            name: s,
                            type_name: t,
                            offset,
                            size: fsize,
                        });
                        // Jump past this whole record so the name-hash / type
                        // bytes aren't rescanned as spurious fields.
                        i = after_type_hash + 8;
                    }
                    // No inline type token (unexpected in practice): skip the
                    // name and keep scanning.
                    _ => i = end,
                }
            }
            _ => i += 1,
        }
    }

    // The base-type table sits between the type header (name + 8-byte hash +
    // 4-byte size) and the first field. Kept raw and resolved later, once all
    // type hashes are known.
    let header_start = (after_name + 12).min(bytes.len());
    let header_region = bytes.get(header_start..first_field_off).unwrap_or(&[]).to_vec();

    Some(TypeDef {
        name,
        hash,
        size,
        fields,
        bases: Vec::new(),
        header_region,
    })
}

/// Parses the `ROBJ` object table. Each 36-byte record is
/// `[id(8)][type_hash(8)][data_offset(4)][pad(4)][owner_id(8)][field_hash(4)]`;
/// the root record's owner slot is zero.
fn parse_objects(bytes: &[u8]) -> Vec<ObjectRef> {
    let mut out = Vec::new();
    let count = u32_at(bytes, 0).unwrap_or(0) as usize;
    let mut p = 8;
    for _ in 0..count {
        let (Some(id), Some(type_hash), Some(data_offset)) =
            (u64_at(bytes, p), u64_at(bytes, p + 8), u32_at(bytes, p + 16))
        else {
            break;
        };
        let owner_id = u64_at(bytes, p + 24).unwrap_or(0);
        out.push(ObjectRef {
            id,
            type_hash,
            owner_id,
            data_offset: data_offset as usize,
        });
        p += 36;
    }
    out
}

/// Parses `RSHI` (u64 count, then `(u64 hash, u32 len, bytes)` entries).
fn parse_interned(bytes: &[u8]) -> HashMap<u64, String> {
    let mut out = HashMap::new();
    let count = u64_at(bytes, 0).unwrap_or(0);
    let mut p = 8;
    for _ in 0..count {
        let (Some(hash), Some(len)) = (u64_at(bytes, p), u32_at(bytes, p + 8)) else {
            break;
        };
        p += 12;
        let len = len as usize;
        let Some(raw) = bytes.get(p..p + len) else { break };
        p += len;
        out.insert(hash, String::from_utf8_lossy(raw).into_owned());
    }
    out
}

impl DecompiledReflect {
    /// Loads all chunks needed for reconstruction. Returns [`None`] if the file
    /// is not a reflection file.
    pub fn parse<R: Read + BufRead + Seek>(chunk_file: &mut ChunkFile<R>) -> Option<DecompiledReflect> {
        let all = crate::rgd::RelicGameData::flatten_data_chunks(
            &mut chunk_file.reader,
            &chunk_file.chunks,
        )
        .ok()?;

        let headers: Vec<_> = all
            .iter()
            .filter(|c| c.chunk_type == ChunkType::Data)
            .cloned()
            .collect();

        let mut out = DecompiledReflect::default();
        let mut saw_type = false;

        for chunk in &headers {
            let Ok(data) = chunk_file.extract_chunk_data(chunk) else {
                continue;
            };
            match chunk.name.as_str() {
                "RFTY" => {
                    if let Some(ty) = parse_type(&data) {
                        saw_type = true;
                        out.types.insert(ty.hash, ty);
                    }
                }
                "ROBJ" => out.objects = parse_objects(&data),
                "RFCI" => {
                    out.rfci_offset = chunk.data_position_start as usize;
                    out.data = data;
                }
                "RSHI" => out.interned = parse_interned(&data),
                "RNEW" => {
                    // [count(4)][pad(4)][root id(8)][root type hash(8)]...
                    out.root_id = u64_at(&data, 8).unwrap_or(0);
                }
                _ => {}
            }
        }

        out.resolve_bases();

        // The true root is the unique object with no owner. RNEW's first entry
        // is not reliably the root in complex files, so prefer the ownerless
        // object and fall back to RNEW only if none is found.
        if let Some(root) = out.objects.iter().find(|o| o.owner_id == 0) {
            out.root_id = root.id;
        }

        if saw_type { Some(out) } else { None }
    }

    /// Reads a scalar field value from the data pool at `pos`, formatting it
    /// for the `.rdo` XML. Returns the XML `Type` and `Value`.
    fn scalar(&self, pos: usize, type_name: &str) -> Option<(&'static str, String)> {
        match type_name {
            "bool" => Some(("Bool", (*self.data.get(pos)? != 0).to_string())),
            "int32_t" => Some(("Int32", i32_at(&self.data, pos)?.to_string())),
            "uint32_t" => Some(("UInt32", u32_at(&self.data, pos)?.to_string())),
            "int64_t" => Some(("Int64", (u64_at(&self.data, pos)? as i64).to_string())),
            "uint64_t" => Some(("UInt64", u64_at(&self.data, pos)?.to_string())),
            _ => None,
        }
    }

    /// Reads a `util::ReflectString<StdTraits>` at `pos`: an 8-byte
    /// offset-pointer (relative to its own position) plus a 4-byte length,
    /// pointing at the packed char blob. Returns the decoded string.
    fn read_string(&self, pos: usize) -> String {
        let rel = match i32_at(&self.data, pos) {
            Some(v) => v as i64,
            None => return String::new(),
        };
        let len = i32_at(&self.data, pos + 8).unwrap_or(0);
        if len <= 0 {
            return String::new();
        }
        let start = pos as i64 + rel;
        if start < 0 {
            return String::new();
        }
        let (start, len) = (start as usize, len as usize);
        match self.data.get(start..start + len) {
            Some(b) => String::from_utf8_lossy(b).into_owned(),
            None => String::new(),
        }
    }

    /// Fills each type's `bases` by scanning its header region for 8-byte
    /// values that match a known type hash. Base-type hashes are 64-bit, so a
    /// spurious match is astronomically unlikely; the 4 bytes preceding each
    /// hash are its display index (`#0`, ...). Runs once all types are known.
    fn resolve_bases(&mut self) {
        let known: std::collections::HashSet<u64> = self.types.keys().copied().collect();
        let mut resolved: HashMap<u64, Vec<(u64, u32)>> = HashMap::new();

        for ty in self.types.values() {
            let region = &ty.header_region;
            let mut bases = Vec::new();
            let mut i = 0;
            while i + 8 <= region.len() {
                let candidate = u64::from_le_bytes(region[i..i + 8].try_into().unwrap());
                if candidate != ty.hash && known.contains(&candidate) {
                    let index = if i >= 4 {
                        u32::from_le_bytes(region[i - 4..i].try_into().unwrap())
                    } else {
                        0
                    };
                    bases.push((candidate, index));
                    i += 8;
                } else {
                    i += 1;
                }
            }
            if !bases.is_empty() {
                resolved.insert(ty.hash, bases);
            }
        }

        for (hash, bases) in resolved {
            if let Some(ty) = self.types.get_mut(&hash) {
                ty.bases = bases;
            }
        }
    }

    /// Builds a type's `#BaseTypes` string (`Base#index|` per base, in order).
    /// Returns [`None`] if any base hash does not resolve to a known type, so a
    /// misread never emits a partial/garbled annotation.
    fn base_types_string(&self, ty: &TypeDef) -> Option<String> {
        let mut s = String::new();
        for (hash, index) in &ty.bases {
            let name = self.types.get(hash)?.name.as_str();
            s.push_str(&format!("{name}#{index}|"));
        }
        Some(s)
    }

    /// An object's base offset within [`Self::data`]: its `ROBJ` `data_offset`
    /// is file-relative, so subtract where the `RFCI` chunk data begins.
    fn base_of(&self, obj: &ObjectRef) -> usize {
        obj.data_offset.saturating_sub(self.rfci_offset)
    }

    /// Reconstructs the `<DataWarehouse>` `.rdo` XML for the root object.
    pub fn to_rdo_xml(&self) -> String {
        let mut out = String::from("<DataWarehouse>\r\n");
        if let Some(root) = self.objects.iter().find(|o| o.id == self.root_id) {
            if let Some(ty) = self.types.get(&root.type_hash) {
                out.push_str(&format!("\t<!--{}/-->\r\n", ty.name));
                self.emit_object(root, 1, out.len(), &mut out);
            }
        }
        out.push_str("</DataWarehouse>\r\n");
        out
    }

    /// Emits one `<DataObject>` and, recursively, its object-typed fields. Each
    /// object reads its own field data at [`Self::base_of`].
    fn emit_object(&self, obj: &ObjectRef, depth: usize, _pos: usize, out: &mut String) {
        let Some(ty) = self.types.get(&obj.type_hash) else {
            return;
        };
        let base = self.base_of(obj);
        let tab = "\t".repeat(depth);
        if obj.owner_id == 0 {
            out.push_str(&format!(
                "{tab}<DataObject Name=\"\" Type=\"{}\" Id=\"{}\">\r\n",
                ty.name, obj.id
            ));
        } else {
            out.push_str(&format!(
                "{tab}<DataObject Name=\"\" Type=\"{}\" Id=\"{}\" OwnerId=\"{}\">\r\n",
                ty.name, obj.id, obj.owner_id
            ));
        }

        // Derived types carry a synthetic "#BaseTypes" property (the editor's
        // base-class annotation) as their first property, e.g.
        // "WinCondition::OptionUIDescriptor#0|".
        if !ty.bases.is_empty() {
            if let Some(value) = self.base_types_string(ty) {
                let ptab = "\t".repeat(depth + 1);
                out.push_str(&format!(
                    "{ptab}<DataProperty Name=\"#BaseTypes\" Type=\"String\" Value=\"{}\"/>\r\n",
                    xml_escape(&value)
                ));
            }
        }

        // Per-parent record of which owned children are already consumed, so
        // repeated same-type fields and array elements map in ROBJ order.
        let mut used = vec![false; self.objects.len()];
        for field in &ty.fields {
            self.emit_property(obj, base, field, depth + 1, &mut used, out);
        }

        out.push_str(&format!("{tab}</DataObject>\r\n"));
    }

    fn emit_property(
        &self,
        parent: &ObjectRef,
        base: usize,
        field: &Field,
        depth: usize,
        used: &mut [bool],
        out: &mut String,
    ) {
        let tab = "\t".repeat(depth);
        let pos = base + field.offset;
        let t = field.type_name.as_str();

        if let Some((xml_type, value)) = self.scalar(pos, t) {
            out.push_str(&format!(
                "{tab}<DataProperty Name=\"{}\" Type=\"{xml_type}\" Value=\"{}\"/>\r\n",
                field.name,
                xml_escape(&value)
            ));
            return;
        }

        if t.starts_with("util::ReflectString") {
            let s = self.read_string(pos);
            out.push_str(&format!(
                "{tab}<DataProperty Name=\"{}\" Type=\"String\" Value=\"{}\"/>\r\n",
                field.name,
                xml_escape(&s)
            ));
            return;
        }

        // Enum / string-hash: resolve the 8-byte hash against RSHI.
        if t.starts_with("FamilyManagerEnum") || t.contains("ReflectStringHash") {
            let hash = u64_at(&self.data, pos).unwrap_or(0);
            let value = self.interned.get(&hash).cloned().unwrap_or_default();
            out.push_str(&format!(
                "{tab}<DataProperty Name=\"{}\" Type=\"String\" Value=\"{}\"/>\r\n",
                field.name,
                xml_escape(&value)
            ));
            return;
        }

        // Arrays: read the element count, then emit that many owned children of
        // the element type (each with its own data via base_of).
        if t.starts_with("util::ReflectArray") {
            let count = i32_at(&self.data, pos + 8).unwrap_or(0).max(0) as usize;
            let elem = array_element_type(t);
            // A pointer element type (`Base *`) is polymorphic: the actual
            // elements are subtypes, so match owned children by owner rather
            // than by exact type.
            let polymorphic = t.contains('*');
            let children = self.take_children(parent.id, &elem, count, polymorphic, used);
            if children.is_empty() {
                out.push_str(&format!(
                    "{tab}<DataProperty Name=\"{}\" Type=\"Object\"/>\r\n",
                    field.name
                ));
            } else {
                self.emit_object_property(&field.name, &children, depth, out);
            }
            return;
        }

        // Single embedded object.
        if let Some((idx, child)) = self.find_child(parent.id, t, used) {
            used[idx] = true;
            let child = child.clone();
            self.emit_object_property(&field.name, &[child], depth, out);
        } else {
            out.push_str(&format!(
                "{tab}<DataProperty Name=\"{}\" Type=\"Object\"/>\r\n",
                field.name
            ));
        }
    }

    /// Emits an object-typed `<DataProperty>` containing one `<DataValue>` per
    /// child id followed by the child `<DataObject>`s (the array/single-object
    /// layout used by the editor's `.rdo`).
    fn emit_object_property(
        &self,
        field_name: &str,
        children: &[ObjectRef],
        depth: usize,
        out: &mut String,
    ) {
        let tab = "\t".repeat(depth);
        let ctab = "\t".repeat(depth + 1);
        out.push_str(&format!(
            "{tab}<DataProperty Name=\"{field_name}\" Type=\"Object\">\r\n"
        ));
        // The <DataValue> names each element by its actual type (for
        // polymorphic arrays this is the concrete subtype).
        for child in children {
            let type_name = self
                .types
                .get(&child.type_hash)
                .map(|t| t.name.as_str())
                .unwrap_or("");
            out.push_str(&format!(
                "{ctab}<DataValue Name=\"{type_name}\">{}</DataValue>\r\n",
                child.id
            ));
        }
        for child in children {
            self.emit_object(child, depth + 1, 0, out);
        }
        out.push_str(&format!("{tab}</DataProperty>\r\n"));
    }

    /// Consumes up to `count` unused owned children, marking them used, returned
    /// in `ROBJ` order. When `polymorphic`, matches any owned child (the element
    /// type is a base-class pointer); otherwise matches by exact type name.
    fn take_children(
        &self,
        owner_id: u64,
        type_name: &str,
        count: usize,
        polymorphic: bool,
        used: &mut [bool],
    ) -> Vec<ObjectRef> {
        let mut out = Vec::new();
        for (i, o) in self.objects.iter().enumerate() {
            if out.len() >= count {
                break;
            }
            let type_ok = polymorphic
                || self.types.get(&o.type_hash).is_some_and(|t| t.name == type_name);
            if !used[i] && o.owner_id == owner_id && type_ok {
                used[i] = true;
                out.push(o.clone());
            }
        }
        out
    }

    /// Finds the next unused owned child object of the given type name.
    fn find_child(
        &self,
        owner_id: u64,
        type_name: &str,
        used: &[bool],
    ) -> Option<(usize, &ObjectRef)> {
        self.objects.iter().enumerate().find(|(i, o)| {
            !used[*i]
                && o.owner_id == owner_id
                && self
                    .types
                    .get(&o.type_hash)
                    .is_some_and(|t| t.name == type_name)
        })
    }
}

/// Extracts the element type from a `util::ReflectArray<ELEM,StdTraits>` type
/// name, dropping a trailing pointer marker: `ReflectArray<Foo *,StdTraits>`
/// -> `Foo`. Handles nested templates in `ELEM` by matching the outermost
/// `,StdTraits>` suffix.
fn array_element_type(array_type: &str) -> String {
    let inner = array_type
        .strip_prefix("util::ReflectArray<")
        .and_then(|s| s.strip_suffix('>'))
        .unwrap_or(array_type);
    // Drop the trailing ",StdTraits" (the allocator traits parameter).
    let elem = inner.strip_suffix(",StdTraits").unwrap_or(inner);
    elem.trim().trim_end_matches('*').trim().to_string()
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
