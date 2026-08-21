use std::{ffi::CString, io::{self, BufRead, ErrorKind, Read}};
use anyhow::Result;
use byteorder::{LittleEndian, ReadBytesExt};

pub fn read_index<R: Read>(reader: &mut R, version: u16) -> io::Result<u32> {
    if version < 6 {
        Ok(reader.read_u16::<LittleEndian>()? as u32)
    } else {
        reader.read_u32::<LittleEndian>()
    }
}

/// Reads a c string from the current position in the buffer.
pub fn read_c_string<R: Read +  BufRead>(reader: &mut R) -> Result<String> {
    let mut cstring_vec = vec![];
    reader.read_until(b'\0', &mut cstring_vec)?;

    let parsed_string = CString::from_vec_with_nul(cstring_vec)?.into_string()?;

    Ok(parsed_string)
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