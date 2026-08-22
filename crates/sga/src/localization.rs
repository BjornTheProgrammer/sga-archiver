//! Compiles a locale CSV into a Relic `.ucs` string table (the `UCS` burner).
//!
//! The `.ucs` format (from the editor's `UCSWriter`): UTF-16LE with a BOM, one
//! line per string as `<id>\t<text>`, CRLF-terminated, text written verbatim
//! (the editor's default `escape: false` — literal `\r\n` in the source stays
//! literal). The source is a `<name>_<locale>.csv` with `ID` and `Text`
//! columns.

use anyhow::{anyhow, Result};

/// Compiles a locale CSV (UTF-8, `ID`/`Text` columns) into `.ucs` bytes.
pub fn compile_ucs(csv_bytes: &[u8]) -> Result<Vec<u8>> {
    // The editor writes CSVs with a UTF-8 BOM; strip it so the header parses.
    let csv_bytes = csv_bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(csv_bytes);

    let mut reader = csv::ReaderBuilder::new().has_headers(true).from_reader(csv_bytes);
    let headers = reader.headers()?.clone();
    let id_col = headers.iter().position(|h| h == "ID").ok_or_else(|| anyhow!("CSV has no 'ID' column"))?;
    let text_col = headers.iter().position(|h| h == "Text").ok_or_else(|| anyhow!("CSV has no 'Text' column"))?;

    let mut out = vec![0xFF, 0xFE]; // UTF-16LE BOM
    for record in reader.records() {
        let record = record?;
        // Skip rows without a numeric loc-string id (blank / non-string rows).
        let Some(id) = record.get(id_col).and_then(|s| s.trim().parse::<i64>().ok()) else {
            continue;
        };
        let text = record.get(text_col).unwrap_or("");
        for unit in format!("{id}\t{text}\r\n").encode_utf16() {
            out.extend_from_slice(&unit.to_le_bytes());
        }
    }
    Ok(out)
}
