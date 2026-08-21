use relic_chunky::reflect::{is_type_token_for_test, parse_rfty_for_test, parse_rshi_for_test};

fn pascal(s: &str) -> Vec<u8> {
    let mut out = (s.len() as u32).to_le_bytes().to_vec();
    out.extend_from_slice(s.as_bytes());
    out
}

#[test]
fn is_type_token_accepts_identifiers_and_templates() {
    assert!(is_type_token_for_test("WinCondition"));
    assert!(is_type_token_for_test("m_startingCondition"));
    assert!(is_type_token_for_test(
        "util::ReflectArray<WinCondition::StartingSquad,StdTraits>"
    ));
    assert!(is_type_token_for_test("int32_t"));

    assert!(!is_type_token_for_test(""));
    assert!(!is_type_token_for_test("1abc"));
    assert!(!is_type_token_for_test("has\ttab"));
    assert!(!is_type_token_for_test("has!bang"));
}

#[test]
fn parse_rfty_pairs_fields_with_types_and_handles_primitives() {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&pascal("WinCondition"));
    bytes.extend_from_slice(&[0u8; 8]);
    bytes.extend_from_slice(&pascal("m_name"));
    bytes.extend_from_slice(&[0u8; 8]);
    bytes.extend_from_slice(&pascal("util::ReflectString<StdTraits>"));
    bytes.extend_from_slice(&[0u8; 8]);
    bytes.extend_from_slice(&pascal("m_devOnly"));
    bytes.extend_from_slice(&[0u8; 8]);
    bytes.extend_from_slice(&pascal("m_maxTeams"));
    bytes.extend_from_slice(&[0u8; 8]);
    bytes.extend_from_slice(&pascal("int32_t"));

    let ty = parse_rfty_for_test(&bytes).expect("parses");
    assert_eq!(ty.name, "WinCondition");
    assert_eq!(
        ty.fields,
        vec![
            ("m_name".to_string(), Some("util::ReflectString<StdTraits>".to_string())),
            ("m_devOnly".to_string(), None),
            ("m_maxTeams".to_string(), Some("int32_t".to_string())),
        ]
    );
}

#[test]
fn parse_rshi_reads_u64_count_and_entries() {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&1u64.to_le_bytes());
    bytes.extend_from_slice(&0x8c623eca289a7e3bu64.to_le_bytes());
    bytes.extend_from_slice(&5u32.to_le_bytes());
    bytes.extend_from_slice(b"nomad");

    let strings = parse_rshi_for_test(&bytes);
    assert_eq!(strings.len(), 1);
    assert_eq!(strings[0].hash, 0x8c623eca289a7e3b);
    assert_eq!(strings[0].value, "nomad");
}
