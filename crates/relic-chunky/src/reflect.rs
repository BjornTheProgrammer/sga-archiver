
use std::io::{BufRead, Read, Seek};

use crate::chunky::{ChunkFile, ChunkType};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReflectType {
    pub name: String,
    pub fields: Vec<(String, Option<String>)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InternedString {
    pub hash: u64,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReflectFile {
    pub types: Vec<ReflectType>,
    pub interned_strings: Vec<InternedString>,
    pub object_strings: Vec<String>,
}

fn pascal_at(bytes: &[u8], off: usize) -> Option<(String, usize)> {
    let len = u32::from_le_bytes(bytes.get(off..off + 4)?.try_into().ok()?) as usize;
    if len == 0 || len > 512 || off + 4 + len > bytes.len() {
        return None;
    }
    let s = &bytes[off + 4..off + 4 + len];
    if s.iter().all(|&c| (0x20..=0x7e).contains(&c)) {
        Some((String::from_utf8_lossy(s).into_owned(), 4 + len))
    } else {
        None
    }
}

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

fn is_type_token(s: &str) -> bool {
    let first = match s.chars().next() {
        Some(c) => c,
        None => return false,
    };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    s.chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | ':' | '<' | '>' | ',' | '*' | ' '))
}

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

fn parse_rshi(bytes: &[u8]) -> Vec<InternedString> {
    let mut out = Vec::new();
    let mut off = 0usize;

    let read_u32 = |b: &[u8], o: usize| -> Option<u32> {
        b.get(o..o + 4).map(|s| u32::from_le_bytes(s.try_into().unwrap()))
    };
    let read_u64 = |b: &[u8], o: usize| -> Option<u64> {
        b.get(o..o + 8).map(|s| u64::from_le_bytes(s.try_into().unwrap()))
    };

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
    pub fn parse<R: Read + BufRead + Seek>(chunk_file: &mut ChunkFile<R>) -> Option<ReflectFile> {
        let all = crate::rgd::RelicGameData::flatten_data_chunks(
            &mut chunk_file.reader,
            &chunk_file.chunks,
        )
        .ok()?;

        let mut file = ReflectFile::default();
        let mut saw_rfty = false;

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
                "RFCI" => file.object_strings = packed_strings(&data, 6),
                _ => {}
            }
        }

        if saw_rfty { Some(file) } else { None }
    }

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
