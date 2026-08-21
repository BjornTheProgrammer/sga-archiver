//! Engine reflection schemas bundled with the tool.
//!
//! A reflection `.bin` is ~93% invariant engine type schema (`RFDB`); only a
//! small part is mod content. Bundling the schema here — keyed by the root
//! object type — lets a `.rdo` compile into a complete `.bin` without any
//! compiled `.bin` living in the mod source tree.
//!
//! Regenerate these files with the CLI's `--dump-schema-lib` from existing
//! `.bin`s when new root types are needed, and add a match arm below.

/// Returns the bundled schema container for a root type, or `None` when the
/// tool carries no schema for it.
pub fn schema_for(root_type: &str) -> Option<&'static [u8]> {
    match sanitize(root_type).as_str() {
        "WinCondition" => Some(include_bytes!("../schemas/WinCondition.schema")),
        "Mod" => Some(include_bytes!("../schemas/Mod.schema")),
        _ => None,
    }
}

/// Reduces a type name to the file-name form used for its schema resource
/// (template punctuation like `<`/`>`/`:` becomes `_`).
fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}
