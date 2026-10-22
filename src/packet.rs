//! Temporal units as read from a file, and adding and removing their OBUs.

use crate::bits::write_leb128;
use crate::error::{Error, ErrorKind, Result};
use crate::obu::{OBU_METADATA, OBU_SEQUENCE_HEADER, OBU_TEMPORAL_DELIMITER, RawObu, read_obu, split_obus};

/// One temporal unit: the OBUs of everything shown at one point in time.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Packet {
  /// The presentation time stamp, in the time base of the reader.
  pub pts: u64,
  /// The OBUs. Every OBU read from Annex B or low overhead framing has a size field.
  pub data: Vec<u8>,
  /// The number of OBUs in each frame unit, for packets read from Annex B.
  pub units: Option<Vec<usize>>,
}

impl Packet {
  /// Splits the packet into its OBUs.
  pub fn obus(&self) -> Result<Vec<RawObu>> {
    split_obus(&self.data)
  }

  // Every OBU as its own byte vector with a size field.
  fn pieces(&self) -> Result<Vec<Vec<u8>>> {
    let obus = self.obus()?;
    if let Some(units) = &self.units
      && units.iter().sum::<usize>() != obus.len()
    {
      return Err(Error::bitstream("the temporal unit does not match its frame units"));
    }
    Ok(
      obus
        .iter()
        .map(|obu| {
          if obu.header.has_size {
            return self.data[obu.range()].to_vec();
          }
          let mut piece = Vec::with_capacity(obu.range().len() + 8);
          piece.push(self.data[obu.prefix.start] | 0x02);
          piece.extend_from_slice(&self.data[obu.prefix.start + 1..obu.prefix.end]);
          write_leb128(&mut piece, obu.payload.len() as u64);
          piece.extend_from_slice(&self.data[obu.payload.clone()]);
          piece
        })
        .collect(),
    )
  }

  /// Inserts a whole OBU, with a size field, before the OBU at `index`, or at the end when
  /// `index` is the number of OBUs. In Annex B framing it joins the frame unit of the OBU it
  /// is inserted before.
  pub fn insert_obu(&mut self, index: usize, obu: &[u8]) -> Result<()> {
    self.insert_obus(index, &[obu])
  }

  /// Inserts several whole OBUs, in order, before the OBU at `index`.
  pub fn insert_obus(&mut self, index: usize, obus: &[&[u8]]) -> Result<()> {
    let mut pieces = self.pieces()?;
    if index > pieces.len() {
      return Err(Error::new(
        ErrorKind::InvalidArgument,
        format!("OBU index {index} is past the end of a temporal unit with {} OBUs", pieces.len()),
      ));
    }
    for obu in obus {
      let raw = read_obu(obu, 0)?;
      if !raw.header.has_size || raw.range().end != obu.len() {
        return Err(Error::new(
          ErrorKind::InvalidArgument,
          "an inserted OBU must be one OBU with a size field",
        ));
      }
    }
    if let Some(units) = &mut self.units {
      if units.is_empty() {
        units.push(0);
      }
      let mut start = 0;
      let mut target = units.len() - 1;
      for (i, &count) in units.iter().enumerate() {
        if index < start + count {
          target = i;
          break;
        }
        start += count;
      }
      units[target] += obus.len();
    }
    pieces.splice(index..index, obus.iter().map(|obu| obu.to_vec()));
    self.data = pieces.concat();
    Ok(())
  }

  /// Removes every OBU for which `keep` returns false and returns how many were removed.
  /// `keep` gets the located OBU and the packet data it points into. Frame units left
  /// empty are dropped.
  pub fn retain_obus(&mut self, mut keep: impl FnMut(&RawObu, &[u8]) -> bool) -> Result<usize> {
    let obus = self.obus()?;
    let pieces = self.pieces()?;
    let kept: Vec<bool> = obus.iter().map(|obu| keep(obu, &self.data)).collect();
    let removed = kept.iter().filter(|&&kept| !kept).count();
    if removed == 0 {
      return Ok(0);
    }
    if let Some(units) = &mut self.units {
      let mut next = kept.iter();
      for count in units.iter_mut() {
        *count = next.by_ref().take(*count).filter(|&&kept| kept).count();
      }
      units.retain(|&count| count > 0);
    }
    self.data = pieces
      .into_iter()
      .zip(kept)
      .filter_map(|(piece, kept)| kept.then_some(piece))
      .collect::<Vec<_>>()
      .concat();
    Ok(removed)
  }

  /// Where new metadata OBUs go: after the temporal delimiter, sequence headers and metadata
  /// already there, and before the first frame.
  pub fn metadata_index(&self) -> Result<usize> {
    let obus = self.obus()?;
    Ok(
      obus
        .iter()
        .position(|obu| !matches!(obu.header.obu_type, OBU_TEMPORAL_DELIMITER | OBU_SEQUENCE_HEADER | OBU_METADATA))
        .unwrap_or(obus.len()),
    )
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::obu::{OBU_FRAME, OBU_PADDING, make_obu, obu_type_of};

  fn types(packet: &Packet) -> Vec<u8> {
    packet.obus().unwrap().iter().map(|obu| obu.header.obu_type).collect()
  }

  fn sample(units: Option<Vec<usize>>) -> Packet {
    let mut data = make_obu(OBU_TEMPORAL_DELIMITER, None, &[]);
    data.extend(make_obu(OBU_SEQUENCE_HEADER, None, &[0xaa]));
    data.extend(make_obu(OBU_FRAME, None, &[0xbb, 0xcc]));
    data.extend(make_obu(OBU_FRAME, None, &[0xdd]));
    Packet { pts: 3, data, units }
  }

  #[test]
  fn inserts_metadata_before_the_first_frame() {
    let mut packet = sample(Some(vec![3, 1]));
    let index = packet.metadata_index().unwrap();
    assert_eq!(index, 2);
    packet.insert_obu(index, &make_obu(OBU_METADATA, None, &[0x04, 0x80])).unwrap();
    assert_eq!(types(&packet), vec![2, 1, 5, 6, 6]);
    assert_eq!(packet.units, Some(vec![4, 1]));
    assert_eq!(packet.metadata_index().unwrap(), 3);
    packet.insert_obu(5, &make_obu(OBU_PADDING, None, &[])).unwrap();
    assert_eq!(packet.units, Some(vec![4, 2]));
    assert!(packet.insert_obu(9, &make_obu(OBU_PADDING, None, &[])).is_err());
    assert!(packet.insert_obu(0, &[0x78, 0x00, 0x78, 0x00]).is_err());
  }

  #[test]
  fn removes_obus_and_empty_frame_units() {
    let mut packet = sample(Some(vec![3, 1]));
    let removed = packet
      .retain_obus(|obu, data| obu_type_of(data[obu.prefix.start]) != OBU_FRAME || obu.payload.len() != 1)
      .unwrap();
    assert_eq!(removed, 1);
    assert_eq!(types(&packet), vec![2, 1, 6]);
    assert_eq!(packet.units, Some(vec![3]));
    assert_eq!(packet.retain_obus(|_, _| true).unwrap(), 0);
  }

  #[test]
  fn gives_unsized_obus_a_size_when_editing() {
    let mut data = make_obu(OBU_TEMPORAL_DELIMITER, None, &[]);
    data.extend([(OBU_FRAME << 3), 0x11, 0x22]);
    let mut packet = Packet { pts: 0, data, units: None };
    packet.insert_obu(2, &make_obu(OBU_PADDING, None, &[0x01])).unwrap();
    let obus = packet.obus().unwrap();
    assert_eq!(obus.len(), 3);
    assert!(obus.iter().all(|obu| obu.header.has_size));
    assert_eq!(&packet.data[obus[1].payload.clone()], &[0x11, 0x22]);
  }

  #[test]
  fn rejects_units_that_do_not_match() {
    let mut packet = sample(Some(vec![1]));
    assert!(packet.insert_obu(0, &make_obu(OBU_PADDING, None, &[])).is_err());
  }
}
