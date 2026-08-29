
use std::io::Cursor;

use relic_chunky::attrib::compile_attrib;
use relic_chunky::container::Chunky;
use relic_chunky::rgd::{RGDNode, RGDValue, RelicGameData};

fn find<'a>(nodes: &'a [RGDNode], key: &str) -> &'a RGDValue {
    &nodes.iter().find(|n| n.key == key).unwrap_or_else(|| panic!("missing key {key}")).value
}

#[test]
fn instance_xml_round_trips_through_rgd() {
    let xml = r#"<instance>
        <group name="stats">
            <float name="radius" value="2.5"/>
            <int name="count" value="7"/>
            <bool name="enabled" value="True"/>
            <file name="icon" value="icons\thing"/>
            <locstring name="title" value="11190330"/>
        </group>
        <list name="entries">
            <instance_reference name="target" value="sbps\races\core\unit"/>
        </list>
    </instance>"#;

    let rgd = compile_attrib(xml).unwrap();
    let chunky = Chunky::read(&mut Cursor::new(&rgd)).unwrap();
    let nodes = RelicGameData::parse(&chunky).unwrap();

    assert_eq!(find(&nodes, "instance_version"), &RGDValue::Int(2));
    let RGDValue::List(default) = find(&nodes, "default") else {
        panic!("default is not a List");
    };
    let RGDValue::List(stats) = find(default, "stats") else {
        panic!("stats is not a List");
    };
    assert_eq!(find(stats, "radius"), &RGDValue::Float(2.5));
    assert_eq!(find(stats, "count"), &RGDValue::Int(7));
    assert_eq!(find(stats, "enabled"), &RGDValue::Boolean(true));
    assert_eq!(find(stats, "icon"), &RGDValue::CString("icons\\thing".into()));
    assert_eq!(find(stats, "title"), &RGDValue::Int(11190330));

    let RGDValue::List2(entries) = find(default, "entries") else {
        panic!("entries is not a List2");
    };
    let RGDValue::List2(target) = find(entries, "target") else {
        panic!("target is not a List2");
    };
    assert_eq!(find(target, "$PBGNAME"), &RGDValue::CString("unit".into()));
    assert_eq!(find(target, "$PBGMAP"), &RGDValue::CString("sbps\\races\\core".into()));
}

#[test]
fn mod_xml_compiles() {
    let xml = r#"<mod guid="50bdc4b41a3f4794936d8c6d0b2898a0" override_instances="False"/>"#;
    let rgd = compile_attrib(xml).unwrap();
    let chunky = Chunky::read(&mut Cursor::new(&rgd)).unwrap();
    let nodes = RelicGameData::parse(&chunky).unwrap();
    assert_eq!(find(&nodes, "override_instances"), &RGDValue::Boolean(false));
    assert_eq!(
        find(&nodes, "id"),
        &RGDValue::CString("50bdc4b41a3f4794936d8c6d0b2898a0".into())
    );
}

#[test]
fn unknown_elements_and_bad_values_are_errors() {
    let unknown = r#"<instance><group name="g"><widget name="x" value="1"/></group></instance>"#;
    assert!(compile_attrib(unknown).is_err(), "unknown tag must not be dropped silently");

    let bad_float = r#"<instance><float name="r" value="fast"/></instance>"#;
    assert!(compile_attrib(bad_float).is_err(), "unparseable float must error");
}
