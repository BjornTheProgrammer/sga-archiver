use sga::localization::compile_ucs;

/// UTF-16LE with a BOM, matching the `.ucs` encoding.
fn utf16le(s: &str) -> Vec<u8> {
    let mut out = vec![0xFF, 0xFE];
    for unit in s.encode_utf16() {
        out.extend_from_slice(&unit.to_le_bytes());
    }
    out
}

#[test]
fn ucs_compiles_byte_exact() {
    // `Text` is not the first column (index lookup by header), row 2 exercises
    // CSV quoting (embedded comma) and literal `\r\n` escapes that must survive
    // verbatim into the `.ucs`.
    let csv = "ID,Pipeline,Text\r\n1,,Hello\r\n2,,\"a,b\\r\\nc\"\r\n";
    let ucs = compile_ucs(csv.as_bytes()).unwrap();
    assert_eq!(ucs, utf16le("1\tHello\r\n2\ta,b\\r\\nc\r\n"));
}

#[test]
fn ucs_skips_rows_without_numeric_id() {
    let csv = "ID,Text\n1,keep\n,blank-skipped\nx,nonnumeric-skipped\n2,also-keep\n";
    let ucs = compile_ucs(csv.as_bytes()).unwrap();
    assert_eq!(ucs, utf16le("1\tkeep\r\n2\talso-keep\r\n"));
}

#[test]
fn ucs_handles_utf8_bom() {
    let csv = "\u{feff}ID,Text\n1,x\n";
    let ucs = compile_ucs(csv.as_bytes()).unwrap();
    assert_eq!(ucs, utf16le("1\tx\r\n"));
}
