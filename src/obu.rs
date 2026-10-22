//! OBU headers, and splitting and writing the OBUs of a temporal unit.

use crate::bits::{read_leb128, write_leb128};
use crate::error::{Error, Result};
use std::ops::Range;

/// `OBU_SEQUENCE_HEADER`
pub const OBU_SEQUENCE_HEADER: u8 = 1;
/// `OBU_TEMPORAL_DELIMITER`
pub const OBU_TEMPORAL_DELIMITER: u8 = 2;
/// `OBU_FRAME_HEADER`
pub const OBU_FRAME_HEADER: u8 = 3;
/// `OBU_TILE_GROUP`
pub const OBU_TILE_GROUP: u8 = 4;
/// `OBU_METADATA`
pub const OBU_METADATA: u8 = 5;
/// `OBU_FRAME`
pub const OBU_FRAME: u8 = 6;
/// `OBU_REDUNDANT_FRAME_HEADER`
pub const OBU_REDUNDANT_FRAME_HEADER: u8 = 7;
/// `OBU_TILE_LIST`
pub const OBU_TILE_LIST: u8 = 8;
/// `OBU_PADDING`
pub const OBU_PADDING: u8 = 15;

/// The optional second byte of an OBU header.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ObuExtension {
  /// `temporal_id`, from 0 to 7.
  pub temporal_id: u8,
  /// `spatial_id`, from 0 to 3.
  pub spatial_id: u8,
}

impl ObuExtension {
  /// Reads an extension byte.
  pub fn from_byte(byte: u8) -> Self {
    ObuExtension {
      temporal_id: byte >> 5,
      spatial_id: (byte >> 3) & 3,
    }
  }

  /// Writes the extension byte.
  pub fn to_byte(self) -> u8 {
    ((self.temporal_id & 7) << 5) | ((self.spatial_id & 3) << 3)
  }
}

/// A parsed OBU header.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ObuHeader {
  /// `obu_type`, one of the `OBU_` constants or a reserved value.
  pub obu_type: u8,
  /// The extension byte, when `obu_extension_flag` is set.
  pub extension: Option<ObuExtension>,
  /// `obu_has_size_field`
  pub has_size: bool,
}

/// The byte length of an OBU header, 2 when the first byte announces an extension byte and 1 otherwise.
pub fn header_length(first: u8) -> usize {
  if first & 0x04 != 0 { 2 } else { 1 }
}

/// The `obu_type` in the first byte of an OBU header.
pub fn obu_type_of(first: u8) -> u8 {
  (first >> 3) & 0x0f
}

/// One OBU inside a byte slice, located but not parsed beyond its header.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawObu {
  /// The parsed header.
  pub header: ObuHeader,
  /// The header bytes, without the size field.
  pub prefix: Range<usize>,
  /// The payload bytes, after the size field.
  pub payload: Range<usize>,
}

impl RawObu {
  /// The whole OBU: header, size field and payload.
  pub fn range(&self) -> Range<usize> {
    self.prefix.start..self.payload.end
  }
}

/// Reads the OBU that starts at `pos`. An OBU without a size field runs to the end of `data`.
pub fn read_obu(data: &[u8], pos: usize) -> Result<RawObu> {
  let Some(&first) = data.get(pos) else {
    return Err(Error::bitstream("truncated OBU header"));
  };
  if first & 0x80 != 0 {
    return Err(Error::bitstream("invalid OBU header"));
  }
  let mut cursor = pos + 1;
  let mut extension = None;
  if first & 0x04 != 0 {
    let Some(&byte) = data.get(cursor) else {
      return Err(Error::bitstream("truncated OBU extension header"));
    };
    extension = Some(ObuExtension::from_byte(byte));
    cursor += 1;
  }
  let prefix = pos..cursor;
  let has_size = first & 0x02 != 0;
  let size = if has_size {
    let (size, used) = read_leb128(&data[cursor..])?;
    cursor += used;
    usize::try_from(size).map_err(|_| Error::bitstream("OBU size exceeds the packet"))?
  } else {
    data.len() - cursor
  };
  let Some(end) = cursor.checked_add(size).filter(|&end| end <= data.len()) else {
    return Err(Error::bitstream("OBU size exceeds the packet"));
  };
  Ok(RawObu {
    header: ObuHeader {
      obu_type: obu_type_of(first),
      extension,
      has_size,
    },
    prefix,
    payload: cursor..end,
  })
}

/// Iterates over the OBUs of a temporal unit. Stops after the first error.
pub struct ObuIter<'a> {
  data: &'a [u8],
  pos: usize,
  failed: bool,
}

impl Iterator for ObuIter<'_> {
  type Item = Result<RawObu>;

  fn next(&mut self) -> Option<Self::Item> {
    if self.failed || self.pos >= self.data.len() {
      return None;
    }
    match read_obu(self.data, self.pos) {
      Ok(obu) => {
        self.pos = obu.payload.end;
        Some(Ok(obu))
      }
      Err(error) => {
        self.failed = true;
        Some(Err(error))
      }
    }
  }
}

/// Iterates over the OBUs in `data`.
pub fn iter_obus(data: &[u8]) -> ObuIter<'_> {
  ObuIter {
    data,
    pos: 0,
    failed: false,
  }
}

/// Splits `data` into its OBUs.
pub fn split_obus(data: &[u8]) -> Result<Vec<RawObu>> {
  iter_obus(data).collect()
}

/// Appends an OBU with a size field.
pub fn write_obu(out: &mut Vec<u8>, obu_type: u8, extension: Option<ObuExtension>, payload: &[u8]) {
  let mut first = ((obu_type & 0x0f) << 3) | 0x02;
  if extension.is_some() {
    first |= 0x04;
  }
  out.push(first);
  if let Some(extension) = extension {
    out.push(extension.to_byte());
  }
  write_leb128(out, payload.len() as u64);
  out.extend_from_slice(payload);
}

/// Returns an OBU with a size field.
pub fn make_obu(obu_type: u8, extension: Option<ObuExtension>, payload: &[u8]) -> Vec<u8> {
  let mut out = Vec::with_capacity(payload.len() + 10);
  write_obu(&mut out, obu_type, extension, payload);
  out
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn writes_and_reads_obus() {
    let extension = ObuExtension {
      temporal_id: 5,
      spatial_id: 2,
    };
    let mut data = make_obu(OBU_TEMPORAL_DELIMITER, None, &[]);
    write_obu(&mut data, OBU_METADATA, Some(extension), &[1, 2, 3]);
    assert_eq!(data, vec![0x12, 0x00, 0x2e, 0xb0, 0x03, 1, 2, 3]);
    let obus = split_obus(&data).unwrap();
    assert_eq!(obus.len(), 2);
    assert_eq!(obus[0].header.obu_type, OBU_TEMPORAL_DELIMITER);
    assert_eq!(obus[1].header.extension, Some(extension));
    assert_eq!(obus[1].prefix, 2..4);
    assert_eq!(obus[1].payload, 5..8);
    assert_eq!(obus[1].range(), 2..8);
  }

  #[test]
  fn unsized_obu_runs_to_the_end() {
    let obu = read_obu(&[0x30, 0xaa, 0xbb], 0).unwrap();
    assert!(!obu.header.has_size);
    assert_eq!(obu.payload, 1..3);
  }

  #[test]
  fn rejects_damaged_headers() {
    assert!(read_obu(&[0x92, 0x00], 0).is_err());
    assert!(read_obu(&[0x2a, 0x05, 0x00], 0).is_err());
    assert!(read_obu(&[0x2e], 0).is_err());
    let results: Vec<_> = iter_obus(&[0x12, 0x00, 0x2a, 0x09]).collect();
    assert_eq!(results.len(), 2);
    assert!(results[1].is_err());
  }
}
