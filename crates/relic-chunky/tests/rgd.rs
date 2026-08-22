use std::io::Cursor;

use relic_chunky::{
    container::Chunky,
    rgd::{RGDNode, RGDNodeValue, RelicGameData, game_data_to_json, game_data_to_xml},
};

const FIXTURE: &[u8] = include_bytes!("weapon_war_elephant_spear_3_sul.rgd");

fn parse_fixture() -> Vec<RGDNode> {
    let chunky = Chunky::read(&mut Cursor::new(FIXTURE)).unwrap();
    RelicGameData::parse(&chunky).unwrap()
}

fn child<'a>(nodes: &'a [RGDNode], key: &str) -> &'a RGDNode {
    nodes
        .iter()
        .find(|node| node.key == key)
        .unwrap_or_else(|| panic!("no node with key '{key}'"))
}

fn list<'a>(nodes: &'a [RGDNode], key: &str) -> &'a [RGDNode] {
    match &child(nodes, key).value {
        RGDNodeValue::List(children) => children,
        other => panic!("expected '{key}' to be a list, got {other:?}"),
    }
}

#[test]
fn parses_top_level_nodes() {
    let nodes = parse_fixture();

    let keys: Vec<&str> = nodes.iter().map(|node| node.key.as_str()).collect();
    assert_eq!(keys, ["default", "instance_version", "campaign"]);

    assert_eq!(
        child(&nodes, "instance_version").value,
        RGDNodeValue::Int(2)
    );
}

#[test]
fn resolves_scalar_values() {
    let nodes = parse_fixture();
    let weapon_bag = list(list(&nodes, "default"), "weapon_bag");

    assert_eq!(
        child(weapon_bag, "name").value,
        RGDNodeValue::CString("Spearman Weapon".to_string())
    );
    assert_eq!(
        child(weapon_bag, "weapon_class").value,
        RGDNodeValue::CString("cdn_2h_spear".to_string())
    );
    assert_eq!(
        child(list(&nodes, "default"), "pbgid").value,
        RGDNodeValue::Int(144673)
    );
}

/// Nested entries used to inherit their parent's key hash, which collapsed every
/// sibling in a list onto the same key.
#[test]
fn nested_siblings_keep_their_own_keys() {
    let nodes = parse_fixture();
    let weapon_bag = list(list(&nodes, "default"), "weapon_bag");

    let colour = list(weapon_bag, "ui_map_colour");
    let keys: Vec<&str> = colour.iter().map(|node| node.key.as_str()).collect();
    assert_eq!(keys, ["red", "green", "blue", "alpha"]);

    let fog_of_war = list(weapon_bag, "fog_of_war");
    let keys: Vec<&str> = fog_of_war.iter().map(|node| node.key.as_str()).collect();
    assert_eq!(keys, ["reveal_self_on_attack", "reveal_target_on_hit"]);
    assert!(
        fog_of_war
            .iter()
            .all(|node| node.value == RGDNodeValue::Boolean(true))
    );
}

/// Values in real files contain backslash paths, so encoding has to survive a
/// round trip through a real JSON parser.
#[test]
fn json_round_trips_special_characters() {
    let awkward = "quote\" backslash\\ path\\races\\melee tab\t control\u{1}";
    let nodes = vec![RGDNode {
        key: awkward.to_string(),
        value: RGDNodeValue::CString(awkward.to_string()),
    }];

    let json = game_data_to_json(&nodes).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

    assert_eq!(parsed["data"][0]["key"], awkward);
    assert_eq!(parsed["data"][0]["value"], awkward);
}

#[test]
fn json_encodes_values_as_native_types() {
    let nodes = vec![
        RGDNode {
            key: "a_float".to_string(),
            value: RGDNodeValue::Float(1.5),
        },
        RGDNode {
            key: "an_int".to_string(),
            value: RGDNodeValue::Int(7),
        },
        RGDNode {
            key: "a_bool".to_string(),
            value: RGDNodeValue::Boolean(true),
        },
        RGDNode {
            key: "a_list".to_string(),
            value: RGDNodeValue::List(vec![RGDNode {
                key: "nested".to_string(),
                value: RGDNodeValue::CString("x".to_string()),
            }]),
        },
    ];

    let parsed: serde_json::Value =
        serde_json::from_str(&game_data_to_json(&nodes).unwrap()).unwrap();
    let data = &parsed["data"];

    assert_eq!(data[0]["value"], 1.5);
    assert_eq!(data[1]["value"], 7);
    assert_eq!(data[2]["value"], true);
    assert_eq!(data[3]["value"][0]["key"], "nested");
    assert_eq!(data[3]["value"][0]["value"], "x");
}

#[test]
fn xml_escapes_special_characters() {
    let nodes = vec![RGDNode {
        key: "a&b<c>".to_string(),
        value: RGDNodeValue::CString("quote\"".to_string()),
    }];

    let xml = game_data_to_xml(&nodes).unwrap();
    assert!(xml.contains("a&amp;b&lt;c&gt;"), "{xml}");
    assert!(xml.contains("&quot;"), "{xml}");
}

#[test]
fn encodes_fixture() {
    let nodes = parse_fixture();

    let json = game_data_to_json(&nodes).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["data"][0]["key"], "default");
    assert_eq!(parsed["data"][1]["value"], 2);

    let xml = game_data_to_xml(&nodes).unwrap();
    assert!(xml.starts_with("<?xml version=\"1.0\" encoding=\"utf-8\"?>"));
    assert!(xml.ends_with("</Root>"));
    assert!(
        xml.contains("<RGDNode Key=\"weapon_class\" Type=\"CString\" Value=\"cdn_2h_spear\"/>")
    );
}

#[test]
fn missing_keys_chunk_is_an_error() {
    // A chunky file with a valid header but no chunks at all.
    let mut bytes = b"Relic Chunky\r\n\x1A\0".to_vec();
    bytes.extend_from_slice(&4u16.to_le_bytes()); // major
    bytes.extend_from_slice(&1u16.to_le_bytes()); // minor
    bytes.extend_from_slice(&1u32.to_le_bytes()); // platform

    let chunky = Chunky::read(&mut Cursor::new(bytes)).unwrap();
    assert!(RelicGameData::parse(&chunky).is_err());
}
