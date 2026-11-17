//! Reads which net each pad is on from a `.PcbDoc`, the netlist Altium compiled from the schematics. Tests use
//! it as the reference for the schematic reader.

use std::path::Path;

use super::records::{parse_text, read_stream};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PadNet {
    pub reference: String,
    pub pad: String,
    pub net: String,
}

/// `|KEY=VALUE` records each behind a 4-byte length, as in `Nets6/Data` and `Components6/Data`.
fn text_records(path: &Path, stream: &str) -> Result<Vec<super::records::Record>, String> {
    let data = read_stream(path, stream)?.ok_or_else(|| format!("{}: no {stream} stream", path.display()))?;
    let mut out = Vec::new();
    let mut pos = 0;
    while let Some(len) = le_u32(&data, pos) {
        pos += 4;
        let body = data
            .get(pos..pos + len)
            .ok_or_else(|| format!("{}: {stream} is cut short", path.display()))?;
        pos += len;
        out.push(parse_text(body));
    }
    Ok(out)
}

fn le_u32(d: &[u8], p: usize) -> Option<usize> {
    d.get(p..p + 4)
        .and_then(|b| b.try_into().ok())
        .map(|b| u32::from_le_bytes(b) as usize)
}

fn le_i16(d: &[u8], p: usize) -> Option<i16> {
    d.get(p..p + 2).and_then(|b| b.try_into().ok()).map(i16::from_le_bytes)
}

/// The designator each component carries on the board, by component index.
///
/// `Components6` records only the schematic designator, which every channel of a multi-channel sheet shares.
/// The designator Altium gave each channel (`IC49A`, `R5_CH2`) is the component's designator text in `Texts6`:
/// the text primitive (type 5) owned by that component that starts with the schematic designator. Each record
/// is a type byte, a length-prefixed header (component index at byte 7) and a length-prefixed Pascal string.
fn board_designators(path: &Path, sources: &[String]) -> Result<Vec<String>, String> {
    let mut out = sources.to_vec();
    let Some(texts) = read_stream(path, "Texts6/Data")? else {
        return Ok(out);
    };
    let mut exact = vec![false; sources.len()];
    let mut pos = 0;
    while pos + 5 <= texts.len() {
        let kind = texts[pos];
        let Some(hlen) = le_u32(&texts, pos + 1) else { break };
        let header = texts.get(pos + 5..pos + 5 + hlen).unwrap_or(&[]);
        pos += 5 + hlen;
        let Some(tlen) = le_u32(&texts, pos) else { break };
        let block = texts.get(pos + 4..pos + 4 + tlen).unwrap_or(&[]);
        pos += 4 + tlen;
        if kind != 5 || block.is_empty() {
            continue;
        }
        let Some(comp) = le_i16(header, 7)
            .and_then(|c| usize::try_from(c).ok())
            .filter(|&c| c < sources.len())
        else {
            continue;
        };
        let n = block[0] as usize;
        let text = String::from_utf8_lossy(block.get(1..1 + n).unwrap_or(&[])).into_owned();
        let source = &sources[comp];
        if &text == source {
            exact[comp] = true;
            out[comp] = text;
        } else if !exact[comp]
            && let Some(suffix) = text.strip_prefix(source.as_str())
            // Only a channel suffix may follow: letters, digits, `_` and the characters Altium counts past Z.
            && suffix.starts_with(|c: char| c.is_ascii_alphabetic() || "_[\\]^`".contains(c))
            && suffix.chars().all(|c| c.is_ascii_alphanumeric() || "_[\\]^`".contains(c))
        {
            out[comp] = text;
        }
    }
    Ok(out)
}

/// Every component pad that is on a net.
///
/// `Pads6/Data` is binary: each pad is a type byte (2) and six length-prefixed blocks. The first holds the pad
/// name as a Pascal string; the fifth holds the net index at byte 3 and the component index at byte 7, both
/// little-endian `i16`, -1 when unset.
pub fn load_pad_nets(path: &Path) -> Result<Vec<PadNet>, String> {
    let nets: Vec<String> = text_records(path, "Nets6/Data")?
        .iter()
        .map(|r| r.text("NAME").to_string())
        .collect();
    let sources: Vec<String> = text_records(path, "Components6/Data")?
        .iter()
        .map(|r| r.text("SOURCEDESIGNATOR").to_string())
        .collect();
    let refs = board_designators(path, &sources)?;
    let data = read_stream(path, "Pads6/Data")?.ok_or_else(|| format!("{}: no Pads6 stream", path.display()))?;
    let bad = || format!("{}: unreadable Pads6 stream", path.display());
    let mut out = Vec::new();
    let mut pos = 0;
    while pos < data.len() {
        if data[pos] != 2 {
            return Err(bad());
        }
        pos += 1;
        let mut blocks = Vec::new();
        for _ in 0..6 {
            let len = le_u32(&data, pos).ok_or_else(bad)?;
            pos += 4;
            blocks.push(data.get(pos..pos + len).ok_or_else(bad)?);
            pos += len;
        }
        let name_len = *blocks[0].first().ok_or_else(bad)? as usize;
        let pad = String::from_utf8_lossy(blocks[0].get(1..1 + name_len).ok_or_else(bad)?).into_owned();
        let (Some(net), Some(comp)) = (le_i16(blocks[4], 3), le_i16(blocks[4], 7)) else {
            return Err(bad());
        };
        let (Ok(net), Ok(comp)) = (usize::try_from(net), usize::try_from(comp)) else {
            continue;
        };
        if let (Some(net), Some(reference)) = (nets.get(net), refs.get(comp)) {
            out.push(PadNet {
                reference: reference.clone(),
                pad,
                net: net.clone(),
            });
        }
    }
    Ok(out)
}
