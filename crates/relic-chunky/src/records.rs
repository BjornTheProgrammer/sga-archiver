//! Flat, count-prefixed reflection tables expressed as `binrw` structs, so the
//! same definition drives both parsing (decompile) and writing (reflect_write)
//! — no more hand-rolled, drift-prone read/write pairs.

use binrw::binrw;

/// One `ROBJ` record (36 bytes): an object's id, its type hash, the offset of
/// its data within the `RFCI` blob, a reserved word, its owning object's id, and
/// the type's trailer word. Decompile ignores `trailer`; the writer emits it.
#[binrw]
#[brw(little)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectRecord {
    pub id: u64,
    pub type_hash: u64,
    pub data_offset: u32,
    #[brw(pad_before = 4)]
    pub owner_id: u64,
    pub trailer: u32,
}

/// The `ROBJ` chunk body: a `u32` record count, a reserved `u32`, then the
/// records.
#[binrw]
#[brw(little)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectTable {
    #[br(temp)]
    #[bw(calc = records.len() as u32)]
    count: u32,
    #[br(temp)]
    #[bw(calc = 0u32)]
    _reserved: u32,
    #[br(count = count)]
    pub records: Vec<ObjectRecord>,
}

/// A `(u64 hash, length-prefixed ASCII string)` pair — the shared record shape
/// of both the reflection `RSHI` interned-string table and the RGD `KEYS`
/// dictionary.
#[binrw]
#[brw(little)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HashedString {
    pub hash: u64,
    #[br(temp)]
    #[bw(calc = value.len() as u32)]
    len: u32,
    #[br(count = len, map = |b: Vec<u8>| String::from_utf8_lossy(&b).into_owned())]
    #[bw(map = |s: &String| s.as_bytes().to_vec())]
    pub value: String,
}

/// The `RSHI` chunk body: a `u32` count, a reserved `u32`, then the records.
#[binrw]
#[brw(little)]
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct InternedStringTable {
    #[br(temp)]
    #[bw(calc = strings.len() as u32)]
    count: u32,
    #[br(temp)]
    #[bw(calc = 0u32)]
    _reserved: u32,
    #[br(count = count)]
    pub strings: Vec<HashedString>,
}

/// The RGD `KEYS` chunk body: a `u32` count then the records (no reserved word,
/// unlike `RSHI`).
#[binrw]
#[brw(little)]
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct KeyTable {
    #[br(temp)]
    #[bw(calc = keys.len() as u32)]
    count: u32,
    #[br(count = count)]
    pub keys: Vec<HashedString>,
}
