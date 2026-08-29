
use std::collections::HashSet;

use anyhow::{bail, Result};
use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;

#[derive(Debug, Clone, PartialEq)]
pub struct RdoObject {
    pub id: u64,
    pub type_name: String,
    pub owner_id: u64,
    pub props: Vec<(String, RdoValue)>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RdoValue {
    Scalar { xml_type: ScalarType, bits: u64 },
    Bool(bool),
    String(String),
    Bytes { count: u32, data: Vec<u8> },
    Children(Vec<u64>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScalarType {
    Float,
    Int32,
    UInt32,
    Int64,
    UInt64,
    UInt8,
}

impl ScalarType {
    pub fn as_str(self) -> &'static str {
        match self {
            ScalarType::Float => "Float",
            ScalarType::Int32 => "Int32",
            ScalarType::UInt32 => "UInt32",
            ScalarType::Int64 => "Int64",
            ScalarType::UInt64 => "UInt64",
            ScalarType::UInt8 => "UInt8",
        }
    }

    fn text(self, bits: u64) -> String {
        match self {
            ScalarType::Float => f32::from_bits(bits as u32).to_string(),
            ScalarType::Int32 => (bits as u32 as i32).to_string(),
            ScalarType::UInt32 => (bits as u32).to_string(),
            ScalarType::Int64 => (bits as i64).to_string(),
            ScalarType::UInt64 => bits.to_string(),
            ScalarType::UInt8 => (bits as u8).to_string(),
        }
    }

    fn bits(self, text: &str) -> u64 {
        match self {
            ScalarType::Float => text.parse::<f32>().unwrap_or(0.0).to_bits() as u64,
            ScalarType::Int32 => text.parse::<i32>().unwrap_or(0) as u32 as u64,
            _ => text.parse::<u64>().or_else(|_| text.parse::<i64>().map(|v| v as u64)).unwrap_or(0),
        }
    }
}

impl RdoObject {
    fn value(&self, name: &str) -> Option<&RdoValue> {
        self.props.iter().find(|(n, _)| n == name).map(|(_, v)| v)
    }

    pub fn scalar(&self, name: &str) -> Option<u64> {
        match self.value(name)? {
            RdoValue::Scalar { bits, .. } => Some(*bits),
            _ => None,
        }
    }

    pub fn bool(&self, name: &str) -> Option<bool> {
        match self.value(name)? {
            RdoValue::Bool(value) => Some(*value),
            _ => None,
        }
    }

    pub fn string(&self, name: &str) -> Option<&str> {
        match self.value(name)? {
            RdoValue::String(value) => Some(value),
            _ => None,
        }
    }

    pub fn raw(&self, name: &str) -> Option<(u32, &[u8])> {
        match self.value(name)? {
            RdoValue::Bytes { count, data } => Some((*count, data)),
            _ => None,
        }
    }

    pub fn child_ids(&self, name: &str) -> Vec<u64> {
        match self.value(name) {
            Some(RdoValue::Children(ids)) => ids.clone(),
            _ => Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct RdoGraph {
    pub objects: Vec<RdoObject>,
}

impl RdoGraph {
    pub fn root(&self) -> Option<&RdoObject> {
        self.objects.iter().find(|o| o.owner_id == 0)
    }

    fn by_id(&self, id: u64) -> Option<&RdoObject> {
        self.objects.iter().find(|o| o.id == id)
    }
}

// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------

impl RdoGraph {
    pub fn to_xml(&self) -> String {
        let mut out = String::from("<DataWarehouse>\r\n");
        if let Some(root) = self.root() {
            out.push_str(&format!("\t<!--{}/-->\r\n", root.type_name));
            let mut emitted = HashSet::new();
            self.emit_object(root, 1, &mut emitted, &mut out);
        }
        out.push_str("</DataWarehouse>\r\n");
        out
    }

    fn emit_object(&self, object: &RdoObject, depth: usize, emitted: &mut HashSet<u64>, out: &mut String) {
        if !emitted.insert(object.id) {
            return;
        }
        let tab = "\t".repeat(depth);
        if object.owner_id == 0 {
            out.push_str(&format!(
                "{tab}<DataObject Name=\"\" Type=\"{}\" Id=\"{}\">\r\n",
                escape(&object.type_name),
                object.id
            ));
        } else {
            out.push_str(&format!(
                "{tab}<DataObject Name=\"\" Type=\"{}\" Id=\"{}\" OwnerId=\"{}\">\r\n",
                escape(&object.type_name),
                object.id,
                object.owner_id
            ));
        }
        for (name, value) in &object.props {
            self.emit_prop(name, value, depth + 1, emitted, out);
        }
        out.push_str(&format!("{tab}</DataObject>\r\n"));
    }

    fn emit_prop(&self, name: &str, value: &RdoValue, depth: usize, emitted: &mut HashSet<u64>, out: &mut String) {
        let tab = "\t".repeat(depth);
        match value {
            RdoValue::Scalar { xml_type, bits } => out.push_str(&format!(
                "{tab}<DataProperty Name=\"{}\" Type=\"{}\" Value=\"{}\"/>\r\n",
                escape(name),
                xml_type.as_str(),
                xml_type.text(*bits)
            )),
            RdoValue::Bool(v) => out.push_str(&format!(
                "{tab}<DataProperty Name=\"{}\" Type=\"Bool\" Value=\"{v}\"/>\r\n",
                escape(name)
            )),
            RdoValue::String(v) => out.push_str(&format!(
                "{tab}<DataProperty Name=\"{}\" Type=\"String\" Value=\"{}\"/>\r\n",
                escape(name),
                escape(v)
            )),
            RdoValue::Bytes { count, data } => out.push_str(&format!(
                "{tab}<DataProperty Name=\"{}\" Type=\"Bytes\" Count=\"{count}\" Value=\"{}\"/>\r\n",
                escape(name),
                hex_encode(data)
            )),
            RdoValue::Children(ids) if ids.is_empty() => out.push_str(&format!(
                "{tab}<DataProperty Name=\"{}\" Type=\"Object\"/>\r\n",
                escape(name)
            )),
            RdoValue::Children(ids) => {
                let ctab = "\t".repeat(depth + 1);
                out.push_str(&format!(
                    "{tab}<DataProperty Name=\"{}\" Type=\"Object\">\r\n",
                    escape(name)
                ));
                for id in ids {
                    let type_name = self.by_id(*id).map(|o| o.type_name.as_str()).unwrap_or("");
                    out.push_str(&format!(
                        "{ctab}<DataValue Name=\"{}\">{id}</DataValue>\r\n",
                        escape(type_name)
                    ));
                }
                for id in ids {
                    if let Some(child) = self.by_id(*id) {
                        self.emit_object(child, depth + 1, emitted, out);
                    }
                }
                out.push_str(&format!("{tab}</DataProperty>\r\n"));
            }
        }
    }
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn hex_decode(s: &str) -> Vec<u8> {
    s.as_bytes()
        .chunks(2)
        .filter_map(|pair| {
            let hi = (*pair.first()? as char).to_digit(16)?;
            let lo = (*pair.get(1)? as char).to_digit(16)?;
            Some((hi * 16 + lo) as u8)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------

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

fn scalar_value(prop_type: &str, value: &str) -> RdoValue {
    let xml_type = match prop_type {
        "Float" => ScalarType::Float,
        "Int32" => ScalarType::Int32,
        "UInt32" => ScalarType::UInt32,
        "Int64" => ScalarType::Int64,
        "UInt8" => ScalarType::UInt8,
        _ => ScalarType::UInt64,
    };
    RdoValue::Scalar { xml_type, bits: xml_type.bits(value) }
}

/// Parses a `.rdo` document into the flat object graph. `<DataValue>` lines
/// are the authoritative child references (and child order); nested
/// `<DataObject>` elements define the objects themselves.
pub fn parse_rdo(xml: &str) -> Result<RdoGraph> {
    let mut reader = Reader::from_str(xml);
    let mut objects: Vec<RdoObject> = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    let mut open_props: Vec<(usize, usize)> = Vec::new();
    let mut in_data_value = false;

    loop {
        match reader.read_event() {
            Ok(Event::Start(tag)) if tag.name().as_ref() == b"DataObject" => {
                let idx = objects.len();
                objects.push(RdoObject {
                    id: attr(&tag, "Id").and_then(|s| s.parse().ok()).unwrap_or(0),
                    type_name: attr(&tag, "Type").unwrap_or_default(),
                    owner_id: attr(&tag, "OwnerId").and_then(|s| s.parse().ok()).unwrap_or(0),
                    props: Vec::new(),
                });
                stack.push(idx);
            }
            Ok(Event::Start(tag)) if tag.name().as_ref() == b"DataProperty" => {
                if attr(&tag, "Type").as_deref() == Some("Object") {
                    if let (Some(&current), Some(name)) = (stack.last(), attr(&tag, "Name")) {
                        objects[current].props.push((name, RdoValue::Children(Vec::new())));
                        open_props.push((current, objects[current].props.len() - 1));
                    }
                }
            }
            Ok(Event::Start(tag)) if tag.name().as_ref() == b"DataValue" => {
                let _ = tag;
                in_data_value = true;
            }
            Ok(Event::Text(text)) if in_data_value => {
                if let (Some(&(obj, prop)), Ok(id)) = (
                    open_props.last(),
                    String::from_utf8_lossy(text.as_ref()).trim().parse::<u64>(),
                ) {
                    if let RdoValue::Children(ids) = &mut objects[obj].props[prop].1 {
                        ids.push(id);
                    }
                }
            }
            Ok(Event::End(tag)) if tag.name().as_ref() == b"DataValue" => {
                in_data_value = false;
            }
            Ok(Event::Empty(tag)) if tag.name().as_ref() == b"DataProperty" => {
                let prop_type = attr(&tag, "Type").unwrap_or_default();
                let Some(&current) = stack.last() else { continue };
                let Some(name) = attr(&tag, "Name") else { continue };
                if prop_type == "Object" {
                    objects[current].props.push((name, RdoValue::Children(Vec::new())));
                    continue;
                }
                let Some(value) = attr(&tag, "Value") else { continue };
                let parsed = match prop_type.as_str() {
                    "Bool" => RdoValue::Bool(value.eq_ignore_ascii_case("true")),
                    "String" => RdoValue::String(value),
                    "Bytes" => RdoValue::Bytes {
                        count: attr(&tag, "Count").and_then(|s| s.parse().ok()).unwrap_or(0),
                        data: hex_decode(&value),
                    },
                    other => scalar_value(other, &value),
                };
                objects[current].props.push((name, parsed));
            }
            Ok(Event::End(tag)) if tag.name().as_ref() == b"DataObject" => {
                stack.pop();
            }
            Ok(Event::End(tag)) if tag.name().as_ref() == b"DataProperty" => {
                open_props.pop();
            }
            Ok(Event::Eof) => break,
            Err(err) => bail!("failed to parse .rdo xml: {err}"),
            _ => {}
        }
    }

    Ok(RdoGraph { objects })
}
