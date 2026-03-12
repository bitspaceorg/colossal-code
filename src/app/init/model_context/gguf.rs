use std::collections::HashMap;
use std::convert::TryFrom;
use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::path::Path;

pub fn context_length_from_gguf(path: &Path) -> Option<usize> {
    let file = File::open(path).ok()?;
    let mut reader = BufReader::new(file);

    let mut magic = [0u8; 4];
    reader.read_exact(&mut magic).ok()?;
    if &magic != b"GGUF" {
        return None;
    }

    let _version = read_u32(&mut reader).ok()?;
    let _tensor_count = read_u64(&mut reader).ok()?;
    let metadata_count = read_u64(&mut reader).ok()?;

    let mut architecture: Option<String> = None;
    let mut context_lengths: HashMap<String, usize> = HashMap::new();

    for _ in 0..metadata_count {
        let key = match read_string(&mut reader) {
            Ok(value) => value,
            Err(_) => return None,
        };
        let value_type = match read_u32(&mut reader) {
            Ok(value) => value,
            Err(_) => return None,
        };

        if key == "general.architecture" {
            match read_string_value(&mut reader, value_type) {
                Ok(Some(value)) => {
                    architecture = Some(value);
                }
                Ok(None) => {}
                Err(_) => return None,
            }
        } else if key.ends_with(".context_length") {
            match read_integer_value(&mut reader, value_type) {
                Ok(Some(value)) => {
                    context_lengths.insert(key.clone(), value);
                }
                Ok(None) => {}
                Err(_) => return None,
            }
        } else if skip_value(&mut reader, value_type).is_err() {
            return None;
        }

        if let Some(arch) = &architecture {
            let key_name = format!("{arch}.context_length");
            if let Some(length) = context_lengths.get(&key_name) {
                return Some(*length);
            }
        }
    }

    if let Some(arch) = architecture {
        let key_name = format!("{arch}.context_length");
        if let Some(length) = context_lengths.get(&key_name) {
            return Some(*length);
        }
    }

    None
}

fn read_u32<R: Read>(reader: &mut R) -> io::Result<u32> {
    let mut buf = [0u8; 4];
    reader.read_exact(&mut buf)?;
    Ok(u32::from_le_bytes(buf))
}

fn read_u64<R: Read>(reader: &mut R) -> io::Result<u64> {
    let mut buf = [0u8; 8];
    reader.read_exact(&mut buf)?;
    Ok(u64::from_le_bytes(buf))
}

fn read_string<R: Read>(reader: &mut R) -> io::Result<String> {
    let len = read_u64(reader)?;
    let len_usize = usize::try_from(len).map_err(|_| io::ErrorKind::InvalidData)?;
    let mut buf = vec![0u8; len_usize];
    reader.read_exact(&mut buf)?;
    String::from_utf8(buf).map_err(|_| io::ErrorKind::InvalidData.into())
}

fn read_string_value<R: Read + Seek>(
    reader: &mut R,
    value_type: u32,
) -> io::Result<Option<String>> {
    if value_type != 8 {
        skip_value(reader, value_type)?;
        return Ok(None);
    }
    read_string(reader).map(Some)
}

fn read_integer_value<R: Read + Seek>(
    reader: &mut R,
    value_type: u32,
) -> io::Result<Option<usize>> {
    match value_type {
        0 => Ok(Some(reader.read_u8()? as usize)),
        1 => {
            let mut buf = [0u8; 1];
            reader.read_exact(&mut buf)?;
            let value = i8::from_le_bytes(buf);
            if value >= 0 {
                Ok(Some(value as usize))
            } else {
                Ok(None)
            }
        }
        2 => {
            let mut buf = [0u8; 2];
            reader.read_exact(&mut buf)?;
            Ok(Some(u16::from_le_bytes(buf) as usize))
        }
        3 => {
            let mut buf = [0u8; 2];
            reader.read_exact(&mut buf)?;
            let value = i16::from_le_bytes(buf);
            if value >= 0 {
                Ok(Some(value as usize))
            } else {
                Ok(None)
            }
        }
        4 => Ok(Some(read_u32(reader)? as usize)),
        5 => {
            let mut buf = [0u8; 4];
            reader.read_exact(&mut buf)?;
            let value = i32::from_le_bytes(buf);
            if value >= 0 {
                Ok(Some(value as usize))
            } else {
                Ok(None)
            }
        }
        10 => Ok(usize::try_from(read_u64(reader)?).ok()),
        11 => {
            let mut buf = [0u8; 8];
            reader.read_exact(&mut buf)?;
            let value = i64::from_le_bytes(buf);
            if value >= 0 {
                Ok(usize::try_from(value as u64).ok())
            } else {
                Ok(None)
            }
        }
        _ => {
            skip_value(reader, value_type)?;
            Ok(None)
        }
    }
}

fn skip_value<R: Read + Seek>(reader: &mut R, value_type: u32) -> io::Result<()> {
    match value_type {
        0 | 1 | 7 => skip_bytes(reader, 1),
        2 | 3 => skip_bytes(reader, 2),
        4..=6 => skip_bytes(reader, 4),
        8 => {
            let len = read_u64(reader)?;
            skip_bytes(reader, len)
        }
        9 => {
            let inner_type = read_u32(reader)?;
            let len = read_u64(reader)?;
            for _ in 0..len {
                skip_value(reader, inner_type)?;
            }
            Ok(())
        }
        10..=12 => skip_bytes(reader, 8),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unknown metadata value type {value_type}"),
        )),
    }
}

fn skip_bytes<R: Read + Seek>(reader: &mut R, len: u64) -> io::Result<()> {
    reader.seek(SeekFrom::Current(len as i64))?;
    Ok(())
}

trait ReadExt: Read {
    fn read_u8(&mut self) -> io::Result<u8> {
        let mut buf = [0u8; 1];
        self.read_exact(&mut buf)?;
        Ok(buf[0])
    }
}

impl<T: Read> ReadExt for T {}
