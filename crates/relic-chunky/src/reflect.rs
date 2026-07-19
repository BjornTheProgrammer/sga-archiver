//! Decoder for the **reflection**-serialized Relic Chunky variant used by
//! win-condition / `.rdo` files (as opposed to the RGD `KEYS`/`AEGD` variant
//! in [`crate::rgd`], used by plain attrib `.rgd` files).
//!
//! ## Format, as reverse-engineered from shipped win-condition `.bin` files
//!
//! A reflection file is a Relic Chunky container whose chunks are:
//!
//! | Chunk  | Kind   | Role                                                        |
//! |--------|--------|-------------------------------------------------------------|
//! | `RFCI` | data   | the root object's flat memory image: a fixed header of      |
//! |        |        | field slots, then a packed blob of the variable-length      |
//! |        |        | data (strings, etc.) those slots point at                   |
//! | `RFUP` | data   | small bookkeeping (version/patch), not decoded here         |
//! | `RNEW` | data   | object-allocation info, not decoded here                    |
//! | `RSHI` | data   | interned-string table: `count`, then `(u64 hash, u32 len,   |
//! |        |        | bytes)` entries. Holds enum values (e.g. `"nomad"`).        |
//! | `RFDB` | folder | the type database: a sequence of `RFTY` chunks              |
//! | `RFTY` | data   | one reflected C++ **type definition**: the type name, then  |
//! |        |        | its fields (each an `m_*` name plus a type name)            |
//! | `ROBJ` | data   | object instance table linking objects to their data         |
//! | `RERF` | data   | external references (e.g. PBG ids), not decoded here         |
//!
//! The type schema travels inside the file (`RFTY`), so the set of available
//! options is recoverable without any external schema. This module extracts
//! the schema (every type and its fields) plus every decoded string/enum
//! value, which is where win-condition configuration actually lives (starting
//! condition, scar file, front-end strings, images, option descriptors).
//!
//! Exact per-field scalar binding (walking each `RFTY` field offset into the
//! `RFCI` image) is intentionally not attempted: the fixed-header framing
//! varies by field type and is brittle across game versions. Strings, enum
//! values, and the schema are stable and cover the meaningful settings.

use std::io::{BufRead, Read, Seek};

use crate::chunky::{ChunkFile, ChunkType};

/// A reflected type and the field names it declares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReflectType {
    /// The C++ type name, e.g. `WinCondition` or
    /// `util::ReflectArray<WinCondition::StartingSquad,StdTraits>`.
    pub name: String,
    /// Declared members. Each entry is `(field_name, type_name)` where
    /// `type_name` is the reflected type token following the field name when
    /// one is present (primitives like `bool` sometimes omit it, in which
    /// case this is `None`).
    pub fields: Vec<(String, Option<String>)>,
}

/// One interned string from the `RSHI` table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InternedString {
    pub hash: u64,
    pub value: String,
}

/// The decoded, human-readable content of a reflection file.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReflectFile {
    /// Every reflected type in the file's type database (`RFDB`/`RFTY`).
    pub types: Vec<ReflectType>,
    /// Interned strings (`RSHI`) - enum values and other hash-referenced text.
    pub interned_strings: Vec<InternedString>,
    /// Null-terminated strings packed in the root object's image (`RFCI`):
    /// the configured string field values (scar file, fe images, name, ...).
    pub object_strings: Vec<String>,
}

/// Reads a printable-ASCII, length-prefixed (`u32` LE) string at `off`.
/// Returns the string and the number of bytes it occupied (prefix + content).
fn pascal_at(bytes: &[u8], off: usize) -> Option<(String, usize)> {
    let len = u32::from_le_bytes(bytes.get(off..off + 4)?.try_into().ok()?) as usize;
    if len == 0 || len > 512 || off + 4 + len > bytes.len() {
        return None;
    }
    let s = &bytes[off + 4..off + 4 + len];
    // Allow tab/newline inside, but require the run to be text.
    if s.iter().all(|&c| (0x20..=0x7e).contains(&c)) {
        Some((String::from_utf8_lossy(s).into_owned(), 4 + len))
    } else {
        None
    }
}

/// Extracts null-terminated printable-ASCII runs of at least `min_len` from a
/// packed string blob.
fn packed_strings(bytes: &[u8], min_len: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut start = None;
    for (i, &b) in bytes.iter().enumerate() {
        let printable = (0x20..=0x7e).contains(&b);
        match (printable, start) {
            (true, None) => start = Some(i),
            (false, Some(s)) => {
                if i - s >= min_len {
                    out.push(String::from_utf8_lossy(&bytes[s..i]).into_owned());
                }
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        if bytes.len() - s >= min_len {
            out.push(String::from_utf8_lossy(&bytes[s..]).into_owned());
        }
    }
    out
}

/// True if `s` looks like a C++ identifier / reflected type token (the shape
/// of every RFTY type and field name), used to reject stray printable runs
/// that happen to satisfy a length prefix.
fn is_type_token(s: &str) -> bool {
    let first = match s.chars().next() {
        Some(c) => c,
        None => return false,
    };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    // Field/type names use only these characters (templates, pointers,
    // namespaces, and the odd space inside `<...>`).
    s.chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | ':' | '<' | '>' | ',' | '*' | ' '))
}

/// Collects every length-prefixed identifier string in `bytes`, in order,
/// scanning at every byte offset (not just aligned) and skipping past each
/// match so nested/overlapping false positives are not double counted.
fn scan_type_tokens(bytes: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut off = 0;
    while off + 4 < bytes.len() {
        match pascal_at(bytes, off) {
            Some((s, consumed)) if is_type_token(&s) => {
                out.push(s);
                off += consumed;
            }
            _ => off += 1,
        }
    }
    out
}

/// Parses one `RFTY` chunk's bytes into a [`ReflectType`].
///
/// The chunk begins with the length-prefixed type name, followed by the field
/// records. Each record carries the field's length-prefixed `m_*` name and,
/// for non-primitive fields, a length-prefixed type token; primitives (e.g.
/// `bool`) reference their type by hash and omit the token. Rather than depend
/// on the exact (type-dependent) record framing, this collects the ordered
/// list of length-prefixed identifier strings: the first is the type name, a
/// string starting with `m_` opens a new field, and the next non-`m_` string
/// fills in that field's type.
fn parse_rfty(bytes: &[u8]) -> Option<ReflectType> {
    let mut iter = scan_type_tokens(bytes).into_iter();
    let name = iter.next()?;

    let mut fields: Vec<(String, Option<String>)> = Vec::new();
    for s in iter {
        if s.starts_with("m_") {
            fields.push((s, None));
        } else if let Some(last) = fields.last_mut() {
            if last.1.is_none() {
                last.1 = Some(s);
            }
        }
    }

    Some(ReflectType { name, fields })
}

/// Parses the `RSHI` interned-string table.
fn parse_rshi(bytes: &[u8]) -> Vec<InternedString> {
    let mut out = Vec::new();
    let mut off = 0usize;

    let read_u32 = |b: &[u8], o: usize| -> Option<u32> {
        b.get(o..o + 4).map(|s| u32::from_le_bytes(s.try_into().unwrap()))
    };
    let read_u64 = |b: &[u8], o: usize| -> Option<u64> {
        b.get(o..o + 8).map(|s| u64::from_le_bytes(s.try_into().unwrap()))
    };

    // The entry count is a u64 here (unlike the RGD KEYS chunk's u32 count).
    let Some(count) = read_u64(bytes, off) else {
        return out;
    };
    off += 8;

    for _ in 0..count {
        let Some(hash) = read_u64(bytes, off) else { break };
        off += 8;
        let Some(len) = read_u32(bytes, off) else { break };
        off += 4;
        let len = len as usize;
        let Some(raw) = bytes.get(off..off + len) else { break };
        off += len;
        out.push(InternedString {
            hash,
            value: String::from_utf8_lossy(raw).into_owned(),
        });
    }

    out
}

// Thin wrappers so the integration tests can exercise the parsing helpers
// without making the byte-level internals part of the public API.
#[doc(hidden)]
pub fn is_type_token_for_test(s: &str) -> bool {
    is_type_token(s)
}
#[doc(hidden)]
pub fn parse_rfty_for_test(bytes: &[u8]) -> Option<ReflectType> {
    parse_rfty(bytes)
}
#[doc(hidden)]
pub fn parse_rshi_for_test(bytes: &[u8]) -> Vec<InternedString> {
    parse_rshi(bytes)
}

impl ReflectFile {
    /// Decodes a reflection-serialized chunky file (win-condition `.rdo` /
    /// `.bin`). Returns [`None`] if the file has no `RFTY` type chunks (i.e.
    /// it is not a reflection file - callers can fall back to
    /// [`crate::rgd::RelicGameData::parse`] for RGD files).
    pub fn parse<R: Read + BufRead + Seek>(chunk_file: &mut ChunkFile<R>) -> Option<ReflectFile> {
        // Reuse the RGD module's folder-recursion so RFTY chunks nested inside
        // the RFDB folder are visited.
        let all = crate::rgd::RelicGameData::flatten_data_chunks(
            &mut chunk_file.reader,
            &chunk_file.chunks,
        )
        .ok()?;

        let mut file = ReflectFile::default();
        let mut saw_rfty = false;

        // Clone headers so we can borrow the reader mutably for extraction.
        let headers: Vec<_> = all
            .iter()
            .filter(|c| c.chunk_type == ChunkType::Data)
            .cloned()
            .collect();

        for chunk in &headers {
            let Ok(data) = chunk_file.extract_chunk_data(chunk) else {
                continue;
            };
            match chunk.name.as_str() {
                "RFTY" => {
                    saw_rfty = true;
                    if let Some(ty) = parse_rfty(&data) {
                        file.types.push(ty);
                    }
                }
                "RSHI" => file.interned_strings = parse_rshi(&data),
                // min length 6 keeps real values (paths, names, keys) while
                // dropping the short binary noise in the object's fixed header.
                "RFCI" => file.object_strings = packed_strings(&data, 6),
                _ => {}
            }
        }

        if saw_rfty { Some(file) } else { None }
    }

    /// Renders the decoded file as a human-readable report.
    pub fn to_report(&self) -> String {
        let mut out = String::new();

        out.push_str("== Interned strings (RSHI: enum values, hashed text) ==\n");
        if self.interned_strings.is_empty() {
            out.push_str("  (none)\n");
        } else {
            for s in &self.interned_strings {
                out.push_str(&format!("  0x{:016x}  {:?}\n", s.hash, s.value));
            }
        }

        out.push_str("\n== Object string values (RFCI: configured strings) ==\n");
        if self.object_strings.is_empty() {
            out.push_str("  (none)\n");
        } else {
            for s in &self.object_strings {
                out.push_str(&format!("  {s:?}\n"));
            }
        }

        out.push_str("\n== Type schema (RFDB/RFTY: all available fields) ==\n");
        for ty in &self.types {
            out.push_str(&format!("  {}\n", ty.name));
            for (field, field_type) in &ty.fields {
                match field_type {
                    Some(t) => out.push_str(&format!("    {field}: {t}\n")),
                    None => out.push_str(&format!("    {field}\n")),
                }
            }
        }

        out
    }
}
