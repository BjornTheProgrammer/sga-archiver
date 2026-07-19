use relic_chunky::reflect::{is_type_token_for_test, parse_rfty_for_test, parse_rshi_for_test};

/// Builds a length-prefixed (u32 LE) string as it appears in RFTY chunks.
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

    // Rejects things that can't be a C++ identifier / type token.
    assert!(!is_type_token_for_test(""));
    assert!(!is_type_token_for_test("1abc"));
    assert!(!is_type_token_for_test("has\ttab"));
    assert!(!is_type_token_for_test("has!bang"));
}

#[test]
fn parse_rfty_pairs_fields_with_types_and_handles_primitives() {
    // type name, then: m_name -> string type, m_devOnly (bool, no type token),
    // m_maxTeams -> int32_t. Interleave 8-byte "hashes" and offset words so the
    // scan has to skip non-string bytes, like a real RFTY chunk.
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&pascal("WinCondition"));
    bytes.extend_from_slice(&[0u8; 8]); // type hash
    bytes.extend_from_slice(&pascal("m_name"));
    bytes.extend_from_slice(&[0u8; 8]); // name hash
    bytes.extend_from_slice(&pascal("util::ReflectString<StdTraits>"));
    bytes.extend_from_slice(&[0u8; 8]); // type hash + offset
    bytes.extend_from_slice(&pascal("m_devOnly")); // primitive: no type token follows
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
    // count (u64) = 1, then hash (u64), len (u32) = 5, "nomad" - the exact
    // layout shipped in Corvinus Nomad FFA's RSHI chunk.
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
