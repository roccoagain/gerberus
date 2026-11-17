//! Altium's record streams: OLE compound files whose streams hold `|KEY=VALUE|...` records.

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;

/// One record. Keys are upper-cased, since files write `Location.X` and `LOCATION.X` alike.
#[derive(Debug, Clone, Default)]
pub struct Record {
    pub fields: HashMap<String, String>,
}

impl Record {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.fields.get(key).map(String::as_str)
    }

    pub fn text(&self, key: &str) -> &str {
        self.get(key).unwrap_or("")
    }

    pub fn int(&self, key: &str) -> i64 {
        self.get(key).and_then(|v| v.trim().parse().ok()).unwrap_or(0)
    }

    pub fn flag(&self, key: &str) -> bool {
        matches!(self.get(key), Some(v) if v.eq_ignore_ascii_case("T") || v.eq_ignore_ascii_case("TRUE") || v == "1")
    }

    /// The `RECORD=` type, or -1 for the header.
    pub fn kind(&self) -> i64 {
        self.get("RECORD").and_then(|v| v.trim().parse().ok()).unwrap_or(-1)
    }

    /// `OWNERINDEX`, if written.
    pub fn owner(&self) -> Option<usize> {
        self.get("OWNERINDEX")
            .and_then(|v| v.trim().parse::<i64>().ok())
            .and_then(|v| usize::try_from(v).ok())
    }

    /// A coordinate in hundred-thousandths of an Altium unit (10 mil): the whole part and its `_FRAC`.
    /// Values are clamped to a sheet far larger than any Altium allows, so arithmetic on them can't overflow.
    pub fn coord(&self, key: &str) -> i64 {
        const LIMIT: i64 = 1_000_000;
        self.int(key).clamp(-LIMIT, LIMIT) * 100_000 + self.int(&format!("{key}_FRAC")).clamp(-99_999, 99_999)
    }
}

/// Opens an OLE compound file and reads one stream.
pub fn read_stream(path: &Path, stream: &str) -> Result<Option<Vec<u8>>, String> {
    let mut file = cfb::open(path).map_err(|e| format!("{}: not an Altium binary file ({e})", path.display()))?;
    let name = format!("/{stream}");
    if !file.is_stream(&name) {
        return Ok(None);
    }
    let mut data = Vec::new();
    file.open_stream(&name)
        .and_then(|mut s| s.read_to_end(&mut data))
        .map_err(|e| format!("{}: {stream}: {e}", path.display()))?;
    Ok(Some(data))
}

/// Windows-1252 bytes 0x80..0x9F that differ from Latin-1.
const CP1252: [char; 32] = [
    '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8d}', 'Ž', '\u{8f}', '\u{90}', '‘', '’',
    '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9d}', 'ž', 'Ÿ',
];

fn decode(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => bytes
            .iter()
            .map(|&b| {
                if (0x80..0xA0).contains(&b) {
                    CP1252[(b - 0x80) as usize]
                } else {
                    b as char
                }
            })
            .collect(),
    }
}

/// Parses `|KEY=VALUE|KEY=VALUE` text.
pub fn parse_text(bytes: &[u8]) -> Record {
    let bytes = bytes.strip_suffix(&[0]).unwrap_or(bytes);
    let mut fields = HashMap::new();
    let mut unicode = Vec::new();
    for pair in bytes.split(|&b| b == b'|') {
        let Some(eq) = pair.iter().position(|&b| b == b'=') else {
            continue;
        };
        let key = decode(&pair[..eq]).trim().to_ascii_uppercase();
        if key.is_empty() {
            continue;
        }
        if let Some(k) = key.strip_prefix("%UTF8%") {
            unicode.push((k.to_string(), String::from_utf8_lossy(&pair[eq + 1..]).into_owned()));
        } else {
            fields.insert(key, decode(&pair[eq + 1..]));
        }
    }
    // A value the code page can't hold is written again in UTF-8, and that copy is the text.
    fields.extend(unicode);
    Record { fields }
}

fn pascal(data: &[u8], pos: &mut usize) -> Option<String> {
    let len = *data.get(*pos)? as usize;
    let s = data.get(*pos + 1..*pos + 1 + len)?;
    *pos += 1 + len;
    Some(decode(s))
}

/// Newer files write some pins as binary records. Layout as read by KiCad's Altium importer.
fn parse_binary_pin(data: &[u8]) -> Option<Record> {
    let mut f = HashMap::new();
    let i32_at = |p: usize| {
        data.get(p..p + 4)
            .and_then(|b| b.try_into().ok())
            .map(i32::from_le_bytes)
    };
    let i16_at = |p: usize| {
        data.get(p..p + 2)
            .and_then(|b| b.try_into().ok())
            .map(i16::from_le_bytes)
    };
    if i32_at(0)? != 2 {
        return None;
    }
    f.insert("RECORD".into(), "2".into());
    f.insert("OWNERPARTID".into(), i16_at(5)?.to_string());
    f.insert("OWNERPARTDISPLAYMODE".into(), data.get(7)?.to_string());
    let mut pos = 12;
    pascal(data, &mut pos)?; // description
    pos += 1;
    f.insert("ELECTRICAL".into(), data.get(pos)?.to_string());
    f.insert("PINCONGLOMERATE".into(), data.get(pos + 1)?.to_string());
    f.insert("PINLENGTH".into(), i16_at(pos + 2)?.to_string());
    f.insert("LOCATION.X".into(), i16_at(pos + 4)?.to_string());
    f.insert("LOCATION.Y".into(), i16_at(pos + 6)?.to_string());
    pos += 12;
    f.insert("NAME".into(), pascal(data, &mut pos)?);
    f.insert("DESIGNATOR".into(), pascal(data, &mut pos)?);
    Some(Record { fields: f })
}

/// Splits a record stream. Each record is a 3-byte little-endian length, a type byte (0 text, 1 binary) and
/// the data.
pub fn parse_stream(data: &[u8]) -> Result<Vec<Record>, String> {
    let mut out = Vec::new();
    let mut pos = 0;
    while pos + 4 <= data.len() {
        let len = u32::from_le_bytes([data[pos], data[pos + 1], data[pos + 2], 0]) as usize;
        let kind = data[pos + 3];
        pos += 4;
        let body = data
            .get(pos..pos + len)
            .ok_or_else(|| format!("record at byte {} runs past the end of the stream", pos - 4))?;
        pos += len;
        match kind {
            0 => out.push(parse_text(body)),
            1 => out.push(parse_binary_pin(body).ok_or_else(|| format!("unreadable binary record at byte {pos}"))?),
            k => return Err(format!("unknown record type {k} at byte {}", pos - len - 4)),
        }
    }
    Ok(out)
}
