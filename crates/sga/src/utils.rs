use std::io::{self, ErrorKind, Read};
use binrw::{BinRead, BinResult, BinWrite};
use byteorder::{LittleEndian, ReadBytesExt};

pub fn read_index<R: Read>(reader: &mut R, version: u16) -> io::Result<u32> {
    if version < 6 {
        Ok(reader.read_u16::<LittleEndian>()? as u32)
    } else {
        reader.read_u32::<LittleEndian>()
    }
}

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

/// Reads a fixed section from the buffer.
/// if char_size is greater than 1, then it reads char_count * char_size bytes.
pub fn read_fixed_string<R: Read>(reader: &mut R, char_count: usize, char_size: usize) -> io::Result<String> {
    let total_bytes = char_count * char_size;
    let mut buffer = vec![0u8; total_bytes];
    reader.read_exact(&mut buffer)?;

    let mut effective_char_count = char_count;

    for i in 0..char_count {
        let slice = &buffer[i * char_size..(i + 1) * char_size];
        if slice.iter().all(|&b| b == 0) {
            effective_char_count = i;
            break;
        }
    }

    let string_bytes = &buffer[..effective_char_count * char_size];

    let result = match char_size {
        1 => String::from_utf8(string_bytes.to_vec())
            .map_err(|_| io::Error::new(ErrorKind::InvalidData, "Invalid UTF-8")),
        2 => {
            use std::slice;
            if string_bytes.len() % 2 != 0 {
                return Err(io::Error::new(ErrorKind::InvalidData, "Odd number of bytes for UTF-16"));
            }
            let u16_slice: &[u16] = unsafe {
                slice::from_raw_parts(string_bytes.as_ptr() as *const u16, string_bytes.len() / 2)
            };
            String::from_utf16(u16_slice)
                .map_err(|_| io::Error::new(ErrorKind::InvalidData, "Invalid UTF-16"))
        },
        _ => return Err(io::Error::new(ErrorKind::InvalidInput, "Unsupported char_size")),
    };

    result
}