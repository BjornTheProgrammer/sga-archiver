
use std::collections::HashMap;

use crate::container::Chunky;
use crate::reflect_type::{parse_type, FieldDef, TypeDef};

fn u32_at(b: &[u8], off: usize) -> Option<u32> {
    b.get(off..off + 4).map(|s| u32::from_le_bytes(s.try_into().unwrap()))
}
fn i32_at(b: &[u8], off: usize) -> Option<i32> {
    b.get(off..off + 4).map(|s| i32::from_le_bytes(s.try_into().unwrap()))
}
fn u64_at(b: &[u8], off: usize) -> Option<u64> {
    b.get(off..off + 8).map(|s| u64::from_le_bytes(s.try_into().unwrap()))
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

        if !ty.bases.is_empty() {
            if let Some(value) = self.base_types_string(ty) {
                let ptab = "\t".repeat(depth + 1);
                out.push_str(&format!(
                    "{ptab}<DataProperty Name=\"#BaseTypes\" Type=\"String\" Value=\"{}\"/>\r\n",
                    xml_escape(&value)
                ));
            }
        }

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
        field: &FieldDef,
        depth: usize,
        used: &mut [bool],
        out: &mut String,
    ) {
        let tab = "\t".repeat(depth);
        let pos = base + field.offset as usize;
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

        if t.starts_with("util::ReflectArray") {
            let count = i32_at(&self.data, pos + 8).unwrap_or(0).max(0) as usize;
            let elem = array_element_type(t);
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

fn array_element_type(array_type: &str) -> String {
    let inner = array_type
        .strip_prefix("util::ReflectArray<")
        .and_then(|s| s.strip_suffix('>'))
        .unwrap_or(array_type);
    let elem = inner.strip_suffix(",StdTraits").unwrap_or(inner);
    elem.trim().trim_end_matches('*').trim().to_string()
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
