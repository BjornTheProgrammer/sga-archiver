//! Compiles attribute-editor XML into Relic game data (`.rgd`) — the `Mod
//! Attributes` burner.
//!
//! Two kinds of source: the root `<mod>` file (mod metadata) and `<instance>`
//! files (attribute instances). Element→node rules, reverse-engineered from the
//! editor output:
//! - `<group>` / `<enum_table>` → List (type 100)
//! - `<list>` → List2 (type 101)
//! - `<template_reference value=T>` → List `{$REF=T, …children}`
//! - `<instance_reference value="a\b\name">` → List2 `{$PBGNAME=name, $PBGMAP=a\b}`
//! - `<locstring value=V mod=G>` → LocString `$G:V`; without `mod` → Int(V)
//! - `<float>/<int>/<bool>` → scalars; `<file>` → CString; `<uniqueid>` → Int

use std::collections::HashMap;

use anyhow::{anyhow, bail, Result};
use quick_xml::events::Event;
use quick_xml::Reader;

use crate::rgd::{RGDNode, RGDValue};
use crate::rgd_write::write_rgd;

/// Compiles an attribute `.xml` (root `<mod>` or `<instance>`) into `.rgd` bytes.
pub fn compile_attrib(xml: &str) -> Result<Vec<u8>> {
    let root = parse_tree(xml)?;
    let nodes = match root.tag.as_str() {
        "mod" => mod_nodes(&root),
        "instance" => instance_nodes(&root),
        other => bail!("unexpected attribute root element <{other}>"),
    };
    write_rgd(&nodes)
}

/// Root `<mod guid=… override_instances=…>` → `{override_instances, id}`.
fn mod_nodes(root: &Elem) -> Vec<RGDNode> {
    vec![
        RGDNode::new("override_instances", RGDValue::Boolean(parse_bool(root.attr("override_instances")))),
        RGDNode::new("id", RGDValue::CString(root.attr("guid").to_string())),
    ]
}

/// `<instance>` → `{default: {<groups…>, pbgid}, instance_version: 2}`.
fn instance_nodes(root: &Elem) -> Vec<RGDNode> {
    let default_children = root.children.iter().filter_map(transform).collect();
    vec![
        RGDNode::new("default", RGDValue::List(default_children)),
        RGDNode::new("instance_version", RGDValue::Int(2)),
    ]
}

/// Transforms one source element into a game-data node.
fn transform(elem: &Elem) -> Option<RGDNode> {
    let key = elem.attr("name").to_string();
    let value = elem.attr("value");
    let node = |v: RGDValue| RGDNode::new(key.clone(), v);
    Some(match elem.tag.as_str() {
        "group" | "enum_table" => node(RGDValue::List(children(elem))),
        "list" => node(RGDValue::List2(children(elem))),
        "template_reference" => {
            let mut items = vec![RGDNode::new("$REF", RGDValue::CString(value.to_string()))];
            items.extend(children(elem));
            node(RGDValue::List(items))
        }
        "instance_reference" => {
            let (map, name) = value.rsplit_once('\\').unwrap_or(("", value));
            node(RGDValue::List2(vec![
                RGDNode::new("$PBGNAME", RGDValue::CString(name.to_string())),
                RGDNode::new("$PBGMAP", RGDValue::CString(map.to_string())),
            ]))
        }
        "float" => node(RGDValue::Float(value.parse().unwrap_or(0.0))),
        "int" => node(RGDValue::Int(value.parse().unwrap_or(0))),
        "bool" => node(RGDValue::Boolean(parse_bool(value))),
        "file" => node(RGDValue::CString(value.to_string())),
        "uniqueid" => node(RGDValue::Int(value.parse().unwrap_or(0))),
        "locstring" => {
            if let Some(guid) = elem.attrs.get("mod") {
                node(RGDValue::LocString(format!("${guid}:{value}")))
            } else {
                node(RGDValue::Int(value.parse().unwrap_or(0)))
            }
        }
        _ => return None,
    })
}

fn children(elem: &Elem) -> Vec<RGDNode> {
    elem.children.iter().filter_map(transform).collect()
}

fn parse_bool(s: &str) -> bool {
    s.eq_ignore_ascii_case("true")
}

// ---------------------------------------------------------------------------
// Minimal XML element tree
// ---------------------------------------------------------------------------

struct Elem {
    tag: String,
    attrs: HashMap<String, String>,
    children: Vec<Elem>,
}

impl Elem {
    fn attr(&self, name: &str) -> &str {
        self.attrs.get(name).map(String::as_str).unwrap_or("")
    }
}

fn attrs_of(tag: &quick_xml::events::BytesStart) -> HashMap<String, String> {
    tag.attributes()
        .flatten()
        .map(|a| {
            let key = String::from_utf8_lossy(a.key.as_ref()).into_owned();
            let val = a
                .unescape_value()
                .map(|v| v.into_owned())
                .unwrap_or_else(|_| String::from_utf8_lossy(&a.value).into_owned());
            (key, val)
        })
        .collect()
}

fn parse_tree(xml: &str) -> Result<Elem> {
    let mut reader = Reader::from_str(xml);
    let mut stack: Vec<Elem> = Vec::new();
    let mut root: Option<Elem> = None;

    loop {
        match reader.read_event()? {
            Event::Start(tag) => stack.push(Elem {
                tag: String::from_utf8_lossy(tag.name().as_ref()).into_owned(),
                attrs: attrs_of(&tag),
                children: Vec::new(),
            }),
            Event::Empty(tag) => {
                let elem = Elem {
                    tag: String::from_utf8_lossy(tag.name().as_ref()).into_owned(),
                    attrs: attrs_of(&tag),
                    children: Vec::new(),
                };
                match stack.last_mut() {
                    Some(parent) => parent.children.push(elem),
                    None => root = Some(elem),
                }
            }
            Event::End(_) => {
                let elem = stack.pop().ok_or_else(|| anyhow!("unbalanced XML"))?;
                match stack.last_mut() {
                    Some(parent) => parent.children.push(elem),
                    None => root = Some(elem),
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    root.ok_or_else(|| anyhow!("empty attribute XML"))
}
