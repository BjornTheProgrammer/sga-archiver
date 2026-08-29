
use std::collections::{HashMap, HashSet};
use std::io::Cursor;

use binrw::BinRead;

use crate::container::Chunky;
use crate::rdo::{RdoGraph, RdoObject, RdoValue, ScalarType};
use crate::records::{InternedStringTable, ObjectTable};
use crate::reflect_type::{
    array_element_type, classify_field, is_enum_type, parse_type, FieldDef, FieldKind, TypeDef,
};

fn u32_at(b: &[u8], off: usize) -> Option<u32> {
    b.get(off..off + 4).map(|s| u32::from_le_bytes(s.try_into().unwrap()))
}
fn i32_at(b: &[u8], off: usize) -> Option<i32> {
    b.get(off..off + 4).map(|s| i32::from_le_bytes(s.try_into().unwrap()))
}
fn u64_at(b: &[u8], off: usize) -> Option<u64> {
    b.get(off..off + 8).map(|s| u64::from_le_bytes(s.try_into().unwrap()))
}
fn i64_at(b: &[u8], off: usize) -> Option<i64> {
    u64_at(b, off).map(|v| v as i64)
}

#[derive(Debug, Clone)]
pub struct ObjectRef {
    pub id: u64,
    pub type_hash: u64,
    pub owner_id: u64,
    pub data_offset: usize,
}

#[derive(Debug, Default)]
pub struct DecompiledReflect {
    pub types: HashMap<u64, TypeDef>,
    pub objects: Vec<ObjectRef>,
    pub data: Vec<u8>,
    pub rfci_offset: usize,
    pub interned: HashMap<u64, String>,
    pub root_id: u64,
}

fn parse_objects(bytes: &[u8]) -> Vec<ObjectRef> {
    let Ok(table) = ObjectTable::read(&mut Cursor::new(bytes)) else {
        return Vec::new();
    };
    table
        .records
        .into_iter()
        .map(|r| ObjectRef {
            id: r.id,
            type_hash: r.type_hash,
            owner_id: r.owner_id,
            data_offset: r.data_offset as usize,
        })
        .collect()
}

fn parse_interned(bytes: &[u8]) -> HashMap<u64, String> {
    InternedStringTable::read(&mut Cursor::new(bytes))
        .map(|table| table.strings.into_iter().map(|s| (s.hash, s.value)).collect())
        .unwrap_or_default()
}

struct Emit<'a> {
    by_offset: HashMap<usize, Vec<&'a ObjectRef>>,
    emitted: HashSet<u64>,
}

impl<'a> Emit<'a> {
    fn child_at(&self, file_pos: usize, owner: u64) -> Option<&'a ObjectRef> {
        self.by_offset
            .get(&file_pos)?
            .iter()
            .copied()
            .find(|o| o.owner_id == owner)
    }
}
impl DecompiledReflect {
    pub fn parse(chunky: &Chunky) -> Option<DecompiledReflect> {
        let mut out = DecompiledReflect::default();
        let mut saw_type = false;

        for (chunk, position) in chunky.data_chunks() {
            let Some(data) = chunk.data() else { continue };
            match chunk.name_str().as_str() {
                "RFTY" => {
                    if let Some(ty) = parse_type(data) {
                        saw_type = true;
                        out.types.insert(ty.hash, ty);
                    }
                }
                "ROBJ" => out.objects = parse_objects(data),
                "RFCI" => {
                    out.rfci_offset = position as usize;
                    out.data = data.to_vec();
                }
                "RSHI" => out.interned = parse_interned(data),
                "RNEW" => {
                    out.root_id = u64_at(data, 8).unwrap_or(0);
                }
                _ => {}
            }
        }

        if let Some(root) = out.objects.iter().find(|o| o.owner_id == 0) {
            out.root_id = root.id;
        }

        if saw_type { Some(out) } else { None }
    }

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

    fn base_types_string(&self, ty: &TypeDef) -> Option<String> {
        let mut s = String::new();
        for (hash, index) in &ty.bases {
            let name = self.types.get(hash)?.name.as_str();
            s.push_str(&format!("{name}#{index}|"));
        }
        Some(s)
    }

    fn base_of(&self, obj: &ObjectRef) -> usize {
        obj.data_offset.saturating_sub(self.rfci_offset)
    }

    fn has_fields(&self, type_name: &str) -> bool {
        self.types.values().any(|t| t.name == type_name && !t.fields.is_empty())
    }

    fn array_elements<'a>(
        &'a self,
        parent: &ObjectRef,
        file_pos: usize,
        rel: i32,
        count: usize,
        emit: &Emit<'a>,
    ) -> Option<Vec<&'a ObjectRef>> {
        if count == 0 {
            return Some(Vec::new());
        }
        let first = (file_pos as i64 + rel as i64) as usize;
        emit.child_at(first, parent.id)?;
        let mut elems: Vec<&ObjectRef> = self
            .objects
            .iter()
            .filter(|o| o.owner_id == parent.id && o.data_offset >= first)
            .collect();
        elems.sort_by_key(|o| o.data_offset);
        elems.truncate(count);
        (elems.len() == count && elems[0].data_offset == first).then_some(elems)
    }

    fn raw_array_bytes(&self, array_type: &str, pos: usize, rel: i32, count: usize) -> Option<Vec<u8>> {
        if count == 0 {
            return None;
        }
        let elem = array_element_type(array_type);
        let size = self.types.values().find(|t| t.name == elem)?.size as usize;
        let start = (pos as i64 + rel as i64) as usize;
        self.data.get(start..start + count * size).map(|b| b.to_vec())
    }

    fn pointer_targets<'a>(
        &self,
        pos: usize,
        rel: i32,
        count: usize,
        owner: u64,
        emit: &Emit<'a>,
    ) -> Option<Vec<&'a ObjectRef>> {
        if count == 0 {
            return Some(Vec::new());
        }
        let block = (pos as i64 + rel as i64) as usize;
        let mut targets = Vec::with_capacity(count);
        for j in 0..count {
            let ptr = block + j * 8;
            let rel2 = i64_at(&self.data, ptr)?;
            let target = (ptr as i64 + rel2) as usize + self.rfci_offset;
            targets.push(emit.child_at(target, owner)?);
        }
        Some(targets)
    }

    pub fn to_rdo_xml(&self) -> String {
        self.to_rdo_graph().to_xml()
    }

    pub fn to_rdo_graph(&self) -> RdoGraph {
        let mut by_offset: HashMap<usize, Vec<&ObjectRef>> = HashMap::new();
        for o in &self.objects {
            by_offset.entry(o.data_offset).or_default().push(o);
        }
        let mut emit = Emit { by_offset, emitted: HashSet::new() };
        let mut graph = RdoGraph::default();
        if let Some(root) = self.objects.iter().find(|o| o.id == self.root_id) {
            self.build_object(root, &mut emit, &mut graph);
        }
        graph
    }

    fn build_object(&self, obj: &ObjectRef, emit: &mut Emit, graph: &mut RdoGraph) {
        let Some(ty) = self.types.get(&obj.type_hash) else {
            return;
        };
        if !emit.emitted.insert(obj.id) {
            return;
        }
        let mut props: Vec<(String, RdoValue)> = Vec::new();
        if !ty.bases.is_empty() {
            if let Some(value) = self.base_types_string(ty) {
                props.push(("#BaseTypes".to_string(), RdoValue::String(value)));
            }
        }
        self.build_content(obj, ty, emit, graph, &mut props);
        graph.objects.push(RdoObject {
            id: obj.id,
            type_name: ty.name.clone(),
            owner_id: obj.owner_id,
            props,
        });
    }

    fn children_prop(
        &self,
        name: &str,
        children: &[&ObjectRef],
        emit: &mut Emit,
        graph: &mut RdoGraph,
        props: &mut Vec<(String, RdoValue)>,
    ) {
        props.push((name.to_string(), RdoValue::Children(children.iter().map(|c| c.id).collect())));
        for child in children {
            self.build_object(child, emit, graph);
        }
    }

    fn build_content(
        &self,
        obj: &ObjectRef,
        ty: &TypeDef,
        emit: &mut Emit,
        graph: &mut RdoGraph,
        props: &mut Vec<(String, RdoValue)>,
    ) {
        let base = self.base_of(obj);

        if is_enum_type(&ty.name) {
            let hash = u64_at(&self.data, base).unwrap_or(0);
            match self.interned.get(&hash) {
                Some(value) => props.push(("m_enumName".into(), RdoValue::String(value.clone()))),
                None => props.push((
                    "m_hashValue".into(),
                    RdoValue::Scalar { xml_type: ScalarType::UInt64, bits: hash },
                )),
            }
            return;
        }

        match classify_field(&ty.name, |n| self.has_fields(n)) {
            FieldKind::Str => {
                props.push(("m_value".into(), RdoValue::String(self.read_string(base))));
            }
            FieldKind::Array => {
                let rel = i32_at(&self.data, base).unwrap_or(0);
                let count = i32_at(&self.data, base + 8).unwrap_or(0).max(0) as usize;
                match self.array_elements(obj, obj.data_offset, rel, count, emit) {
                    Some(children) if !children.is_empty() => {
                        self.children_prop("m_elements", &children, emit, graph, props)
                    }
                    Some(_) => {}
                    None => {
                        if let Some(data) = self.raw_array_bytes(&ty.name, base, rel, count) {
                            props.push((
                                "m_elements".into(),
                                RdoValue::Bytes { count: count as u32, data },
                            ));
                        }
                    }
                }
            }
            FieldKind::PointerArray => {
                let rel = i32_at(&self.data, base).unwrap_or(0);
                let count = i32_at(&self.data, base + 8).unwrap_or(0).max(0) as usize;
                if let Some(children) = self.pointer_targets(base, rel, count, obj.id, emit) {
                    if !children.is_empty() {
                        self.children_prop("m_elements", &children, emit, graph, props);
                    }
                }
            }
            FieldKind::Opaque if ty.fields.is_empty() && ty.size > 0 => {
                let size = ty.size as usize;
                let data = self.data.get(base..base + size).unwrap_or_default().to_vec();
                props.push(("#Raw".into(), RdoValue::Bytes { count: size as u32, data }));
            }
            _ => {
                for field in &ty.fields {
                    self.build_property(obj, field, emit, graph, props);
                }
            }
        }
    }

    fn build_property(
        &self,
        parent: &ObjectRef,
        field: &FieldDef,
        emit: &mut Emit,
        graph: &mut RdoGraph,
        props: &mut Vec<(String, RdoValue)>,
    ) {
        let pos = self.base_of(parent) + field.offset as usize;
        let file_pos = parent.data_offset + field.offset as usize;
        let t = field.type_name.as_str();
        let name = field.name.clone();

        match classify_field(t, |n| self.has_fields(n)) {
            FieldKind::Scalar => {
                let bits = u32_at(&self.data, pos).unwrap_or(0) as u64;
                let xml_type = if t == "float" {
                    ScalarType::Float
                } else if t.contains("uint32") || t == "unsigned int" {
                    ScalarType::UInt32
                } else {
                    ScalarType::Int32
                };
                props.push((name, RdoValue::Scalar { xml_type, bits }));
            }
            FieldKind::Scalar64 => {
                let bits = u64_at(&self.data, pos).unwrap_or(0);
                let xml_type = if t.contains("uint64") || t.starts_with("unsigned") {
                    ScalarType::UInt64
                } else {
                    ScalarType::Int64
                };
                props.push((name, RdoValue::Scalar { xml_type, bits }));
            }
            FieldKind::Scalar8 => {
                let bits = self.data.get(pos).copied().unwrap_or(0) as u64;
                props.push((name, RdoValue::Scalar { xml_type: ScalarType::UInt8, bits }));
            }
            FieldKind::Bool => {
                props.push((name, RdoValue::Bool(self.data.get(pos).copied().unwrap_or(0) != 0)));
            }
            FieldKind::Str => {
                match emit.child_at(file_pos, parent.id) {
                    Some(child) => self.children_prop(&name, &[child], emit, graph, props),
                    None => props.push((name, RdoValue::String(self.read_string(pos)))),
                }
            }
            FieldKind::Embed | FieldKind::Enum => match emit.child_at(file_pos, parent.id) {
                Some(child) => self.children_prop(&name, &[child], emit, graph, props),
                None => props.push((name, RdoValue::Children(Vec::new()))),
            },
            FieldKind::OffsetPointer => {
                let rel = i32_at(&self.data, pos).unwrap_or(0);
                let target = (file_pos as i64 + rel as i64) as usize;
                match (rel != 0).then(|| emit.child_at(target, parent.id)).flatten() {
                    Some(child) => self.children_prop(&name, &[child], emit, graph, props),
                    None => props.push((name, RdoValue::Children(Vec::new()))),
                }
            }
            FieldKind::Array | FieldKind::PointerArray => {
                if let Some(child) = emit.child_at(file_pos, parent.id) {
                    self.children_prop(&name, &[child], emit, graph, props);
                    return;
                }
                let rel = i32_at(&self.data, pos).unwrap_or(0);
                let count = i32_at(&self.data, pos + 8).unwrap_or(0).max(0) as usize;
                if classify_field(t, |n| self.has_fields(n)) == FieldKind::PointerArray {
                    match self.pointer_targets(pos, rel, count, parent.id, emit) {
                        Some(children) if !children.is_empty() => {
                            self.children_prop(&name, &children, emit, graph, props)
                        }
                        _ => props.push((name, RdoValue::Children(Vec::new()))),
                    }
                    return;
                }
                match self.array_elements(parent, file_pos, rel, count, emit) {
                    Some(children) if !children.is_empty() => {
                        self.children_prop(&name, &children, emit, graph, props)
                    }
                    Some(_) => props.push((name, RdoValue::Children(Vec::new()))),
                    None => match self.raw_array_bytes(t, pos, rel, count) {
                        Some(data) => {
                            props.push((name, RdoValue::Bytes { count: count as u32, data }))
                        }
                        None => props.push((name, RdoValue::Children(Vec::new()))),
                    },
                }
            }
            FieldKind::Opaque => match emit.child_at(file_pos, parent.id) {
                Some(child) => self.children_prop(&name, &[child], emit, graph, props),
                None => props.push((name, RdoValue::Children(Vec::new()))),
            },
        }
    }

}
