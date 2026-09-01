use binrw::{BinRead, BinResult, BinWrite};

/// Index fields (counts and lengths) are u16 before archive version 6 and u32
/// from then on.
#[binrw::parser(reader, endian)]
pub fn parse_index(version: u16) -> BinResult<u32> {
    if version < 6 {
        Ok(u16::read_options(reader, endian, ())? as u32)
    } else {
        u32::read_options(reader, endian, ())
    }
}

#[binrw::writer(writer, endian)]
pub fn write_index(value: &u32, version: u16) -> BinResult<()> {
    if version < 6 {
        (*value as u16).write_options(writer, endian, ())
    } else {
        value.write_options(writer, endian, ())
    }
}

/// Offset fields widen from u32 to u64 at archive version 9.
#[binrw::parser(reader, endian)]
pub fn parse_wide(version: u16) -> BinResult<u64> {
    if version >= 9 {
        u64::read_options(reader, endian, ())
    } else {
        Ok(u32::read_options(reader, endian, ())? as u64)
    }
}

#[binrw::writer(writer, endian)]
pub fn write_wide(value: &u64, version: u16) -> BinResult<()> {
    if version >= 9 {
        value.write_options(writer, endian, ())
    } else {
        (*value as u32).write_options(writer, endian, ())
    }
}

pub fn utf16_name(units: &[u16; 64]) -> String {
    let len = units
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(units.len());
    String::from_utf16_lossy(&units[..len])
}

pub fn name_units(name: &str) -> [u16; 64] {
    let mut units = [0u16; 64];
    for (slot, unit) in units.iter_mut().zip(name.encode_utf16()) {
        *slot = unit;
    }
    units
}
