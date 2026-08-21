use std::collections::HashMap;

use anyhow::{anyhow, bail, Result};
use byteorder::{LittleEndian, WriteBytesExt};
use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;

use crate::container::{Chunk, ChunkBody, Chunky};
use crate::reflect_type::{SchemaRegistry, TypeDef};

/// The reflection data blob (`RFCI`) is the first chunk, so its bytes begin
/// after the file header (24) and the chunk header (20).
const RFCI_FILE_OFFSET: u32 = 44;
/// The blob opens with a reserved word before the root object's data.
const RFCI_PREFIX: u32 = 4;

#[derive(Debug, Default)]
struct RdoObject {
    id: u64,
    type_name: String,
    owner_id: u64,
    scalars: Vec<(String, u32)>,
    children: Vec<(String, u64)>,
}

fn attr(tag: &BytesStart, name: &str) -> Option<String> {
    tag.attributes().flatten().find_map(|a| {
        if a.key.as_ref() == name.as_bytes() {
            Some(String::from_utf8_lossy(&a.value).into_owned())
        } else {
            None
        }
    })
}

fn scalar_bits(prop_type: &str, value: &str) -> u32 {
    match prop_type {
        "Int32" => value.parse::<i32>().unwrap_or(0) as u32,
        _ => value.parse::<u32>().unwrap_or(0),
    }
}

fn parse_rdo(xml: &str) -> Result<Vec<RdoObject>> {
    let mut reader = Reader::from_str(xml);
    let mut objects: Vec<RdoObject> = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    let mut pending_child_field: Option<(usize, String)> = None;

    loop {
        match reader.read_event() {
            Ok(Event::Start(tag)) if tag.name().as_ref() == b"DataObject" => {
                let id = attr(&tag, "Id").and_then(|s| s.parse().ok()).unwrap_or(0);
                let idx = objects.len();
                objects.push(RdoObject {
                    id,
                    type_name: attr(&tag, "Type").unwrap_or_default(),
                    owner_id: attr(&tag, "OwnerId").and_then(|s| s.parse().ok()).unwrap_or(0),
                    scalars: Vec::new(),
                    children: Vec::new(),
                });
                if let Some((parent, field)) = pending_child_field.take() {
                    objects[parent].children.push((field, id));
                }
                stack.push(idx);
            }
            Ok(Event::Start(tag)) if tag.name().as_ref() == b"DataProperty" => {
                if attr(&tag, "Type").as_deref() == Some("Object") {
                    if let (Some(&current), Some(field)) = (stack.last(), attr(&tag, "Name")) {
                        pending_child_field = Some((current, field));
                    }
                }
            }
            Ok(Event::Empty(tag)) if tag.name().as_ref() == b"DataProperty" => {
                if let (Some(&current), Some(field), Some(value)) =
                    (stack.last(), attr(&tag, "Name"), attr(&tag, "Value"))
                {
                    let prop_type = attr(&tag, "Type").unwrap_or_default();
                    objects[current]
                        .scalars
                        .push((field, scalar_bits(&prop_type, &value)));
                }
            }
            Ok(Event::End(tag)) if tag.name().as_ref() == b"DataObject" => {
                stack.pop();
            }
            Ok(Event::Eof) => break,
            Err(err) => bail!("failed to parse .rdo xml: {err}"),
            _ => {}
        }
    }

    Ok(objects)
}

fn field(ty: &TypeDef, name: &str) -> Option<(u32, u32)> {
    ty.fields.iter().find(|f| f.name == name).map(|f| (f.offset, f.size))
}

struct Serializer<'a> {
    objects: &'a [RdoObject],
    id_index: HashMap<u64, usize>,
    registry: &'a SchemaRegistry,
    blob: Vec<u8>,
    order: Vec<(usize, u32)>,
}

impl<'a> Serializer<'a> {
    fn place(&mut self, index: usize, blob_offset: u32) -> Result<()> {
        let object = &self.objects[index];
        let ty = self
            .registry
            .by_name(&object.type_name)
            .ok_or_else(|| anyhow!("type not in registry: {}", object.type_name))?;

        self.order.push((index, blob_offset));
        let end = (blob_offset + ty.size) as usize;
        if self.blob.len() < end {
            self.blob.resize(end, 0);
        }

        for (name, bits) in &object.scalars {
            let (offset, _size) = field(ty, name)
                .ok_or_else(|| anyhow!("{}.{} not in schema", object.type_name, name))?;
            let at = (blob_offset + offset) as usize;
            self.blob[at..at + 4].copy_from_slice(&bits.to_le_bytes());
        }

        let mut children: Vec<(u64, u32)> = object
            .children
            .iter()
            .filter_map(|(field_name, child_id)| {
                field(ty, field_name).map(|(offset, _)| (*child_id, offset))
            })
            .collect();
        children.sort_by_key(|&(_, offset)| offset);

        for (child_id, offset) in children {
            let child = *self
                .id_index
                .get(&child_id)
                .ok_or_else(|| anyhow!("child object {child_id} not found"))?;
            self.place(child, blob_offset + offset)?;
        }

        Ok(())
    }

    fn robj(&self) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        out.write_u32::<LittleEndian>(self.order.len() as u32)?;
        out.write_u32::<LittleEndian>(0)?;
        for &(index, blob_offset) in &self.order {
            let object = &self.objects[index];
            let ty = self.registry.by_name(&object.type_name).unwrap();
            out.write_u64::<LittleEndian>(object.id)?;
            out.write_u64::<LittleEndian>(ty.hash)?;
            out.write_u32::<LittleEndian>(RFCI_FILE_OFFSET + blob_offset)?;
            out.write_u32::<LittleEndian>(0)?;
            out.write_u64::<LittleEndian>(object.owner_id)?;
            out.write_u32::<LittleEndian>(ty.trailer)?;
        }
        Ok(out)
    }
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
/// rebuilding only the object data (`RFCI`) and object table (`ROBJ`).
pub fn compile_reflect(rdo_xml: &str, registry: &SchemaRegistry, reference: &Chunky) -> Result<Chunky> {
    let objects = parse_rdo(rdo_xml)?;
    let id_index: HashMap<u64, usize> =
        objects.iter().enumerate().map(|(i, o)| (o.id, i)).collect();
    let root = objects
        .iter()
        .position(|o| o.owner_id == 0)
        .ok_or_else(|| anyhow!("no root object (owner id 0) in .rdo"))?;

    let mut serializer = Serializer {
        objects: &objects,
        id_index,
        registry,
        blob: vec![0u8; RFCI_PREFIX as usize],
        order: Vec::new(),
    };
    serializer.place(root, RFCI_PREFIX)?;

    let robj = serializer.robj()?;
    let rfci = serializer.blob;

    let mut out = reference.clone();
    if !replace_chunk(&mut out.chunks, b"RFCI", rfci) {
        bail!("reference .bin has no RFCI chunk");
    }
    if !replace_chunk(&mut out.chunks, b"ROBJ", robj) {
        bail!("reference .bin has no ROBJ chunk");
    }
    Ok(out)
}
