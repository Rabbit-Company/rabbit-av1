//! Reading and writing IVF, low overhead (Section 5) and Annex B streams.

use crate::bits::{read_leb128, write_leb128};
use crate::error::{Error, ErrorKind, Result};
use crate::ivf::{HEADER_SIZE, IvfReader, ivf_header, read_error};
use crate::obu::{OBU_SEQUENCE_HEADER, OBU_TEMPORAL_DELIMITER, header_length, iter_obus, obu_type_of, split_obus};
use crate::packet::Packet;
use crate::sequence::SequenceHeader;
use std::collections::VecDeque;
use std::io::{BufRead, Read, Write};

const MAX_UNIT: u64 = 1 << 28;
/// The frame rate assumed for raw streams that do not state one.
pub const DEFAULT_FPS: (u32, u32) = (24000, 1001);

/// How AV1 data is laid out in a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Framing {
  /// IVF: a file header and a length and time stamp before every temporal unit.
  Ivf,
  /// Low overhead bitstream format (Section 5): OBUs with size fields, each temporal unit
  /// starting with a temporal delimiter. Usually stored as `.obu`.
  LowOverhead,
  /// Length delimited bitstream format (Annex B).
  AnnexB,
}

trait PacketSource {
  fn next_packet(&mut self) -> Result<Option<Packet>>;
}

impl<R: Read> PacketSource for IvfReader<R> {
  fn next_packet(&mut self) -> Result<Option<Packet>> {
    IvfReader::next_packet(self)
  }
}

struct Closed;

impl PacketSource for Closed {
  fn next_packet(&mut self) -> Result<Option<Packet>> {
    Ok(None)
  }
}

fn read_byte(input: &mut impl Read) -> Result<Option<u8>> {
  let mut byte = [0u8];
  loop {
    match input.read(&mut byte) {
      Ok(0) => return Ok(None),
      Ok(_) => return Ok(Some(byte[0])),
      Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
      Err(error) => return Err(Error::io("failed to read the stream", error)),
    }
  }
}

// Reads a leb128 value from a stream. None means the stream ended before it began.
fn read_leb128_from(input: &mut impl Read) -> Result<Option<u64>> {
  let mut value = 0u64;
  for i in 0..8 {
    let Some(byte) = read_byte(input)? else {
      if i == 0 {
        return Ok(None);
      }
      return Err(Error::bitstream("the stream ends inside a size field"));
    };
    value |= ((byte & 0x7f) as u64) << (i * 7);
    if byte & 0x80 == 0 {
      return Ok(Some(value));
    }
  }
  Err(Error::bitstream("invalid size field"))
}

fn read_exact_unit(input: &mut impl Read, size: u64) -> Result<Vec<u8>> {
  if size > MAX_UNIT {
    return Err(Error::bitstream(format!("a unit claims {size} bytes, the stream is damaged")));
  }
  let mut data = vec![0u8; size as usize];
  input
    .read_exact(&mut data)
    .map_err(|error| read_error("the stream ends inside a unit", error))?;
  Ok(data)
}

struct LowOverheadSource<R: Read> {
  input: R,
  carry: Option<Vec<u8>>,
  index: u64,
}

impl<R: Read> LowOverheadSource<R> {
  fn next_obu(&mut self) -> Result<Option<Vec<u8>>> {
    let Some(first) = read_byte(&mut self.input)? else {
      return Ok(None);
    };
    let mut obu = vec![first];
    if first & 0x04 != 0 {
      let extension = read_byte(&mut self.input)?.ok_or_else(|| Error::bitstream("the stream ends inside an OBU header"))?;
      obu.push(extension);
    }
    if first & 0x02 == 0 {
      return Err(Error::bitstream("an OBU in the low overhead stream has no size field"));
    }
    let size = read_leb128_from(&mut self.input)?.ok_or_else(|| Error::bitstream("the stream ends inside an OBU header"))?;
    write_leb128(&mut obu, size);
    obu.extend(read_exact_unit(&mut self.input, size)?);
    Ok(Some(obu))
  }
}

impl<R: Read> PacketSource for LowOverheadSource<R> {
  fn next_packet(&mut self) -> Result<Option<Packet>> {
    let mut data = self.carry.take().unwrap_or_default();
    while let Some(obu) = self.next_obu()? {
      let is_delimiter = obu_type_of(obu[0]) == OBU_TEMPORAL_DELIMITER;
      if is_delimiter && !data.is_empty() {
        self.carry = Some(obu);
        break;
      }
      data.extend(obu);
    }
    if data.is_empty() {
      return Ok(None);
    }
    self.index += 1;
    Ok(Some(Packet {
      pts: self.index - 1,
      data,
      units: None,
    }))
  }
}

// OBUs without a size field get one, so every packet looks the same as one from IVF.
struct AnnexBSource<R: Read> {
  input: R,
  index: u64,
}

impl<R: Read> PacketSource for AnnexBSource<R> {
  fn next_packet(&mut self) -> Result<Option<Packet>> {
    let Some(size) = read_leb128_from(&mut self.input)? else {
      return Ok(None);
    };
    let unit = read_exact_unit(&mut self.input, size)?;
    let mut data = Vec::with_capacity(unit.len() + 16);
    let mut units = Vec::new();
    let mut pos = 0;
    while pos < unit.len() {
      let (frame_size, used) = read_leb128(&unit[pos..])?;
      pos += used;
      let end = pos + frame_size as usize;
      if end > unit.len() {
        return Err(Error::bitstream("a frame unit is larger than its temporal unit"));
      }
      let mut count = 0;
      while pos < end {
        let (obu_size, used) = read_leb128(&unit[pos..end])?;
        pos += used;
        let obu_end = pos + obu_size as usize;
        if obu_end > end || obu_size == 0 {
          return Err(Error::bitstream("an OBU is larger than its frame unit"));
        }
        let obu = &unit[pos..obu_end];
        let head = header_length(obu[0]).min(obu.len());
        if obu[0] & 0x02 != 0 {
          data.extend_from_slice(obu);
        } else {
          data.push(obu[0] | 0x02);
          data.extend_from_slice(&obu[1..head]);
          write_leb128(&mut data, (obu.len() - head) as u64);
          data.extend_from_slice(&obu[head..]);
        }
        count += 1;
        pos = obu_end;
      }
      units.push(count);
    }
    self.index += 1;
    Ok(Some(Packet {
      pts: self.index - 1,
      data,
      units: Some(units),
    }))
  }
}

/// Recognises the framing of raw AV1 from the first bytes of a file. `None` means the data
/// is something else, such as Matroska or MP4.
pub fn detect(start: &[u8]) -> Option<Framing> {
  if start.starts_with(b"DKIF") {
    return Some(Framing::Ivf);
  }
  let delimiter = match start.first() {
    Some(0x12) => start.get(1) == Some(&0),
    Some(0x16) => start.get(2) == Some(&0),
    _ => false,
  };
  if delimiter {
    return Some(Framing::LowOverhead);
  }
  let (tu, a) = read_leb128(start).ok()?;
  let (fu, b) = read_leb128(start.get(a..)?).ok()?;
  let (obu, c) = read_leb128(start.get(a + b..)?).ok()?;
  let header = *start.get(a + b + c)?;
  let obu_type = obu_type_of(header);
  let plausible = fu + b as u64 <= tu && obu + c as u64 <= fu && obu > 0 && header & 0x81 == 0 && (obu_type == 1 || obu_type == 2);
  plausible.then_some(Framing::AnnexB)
}

/// Reads temporal units from IVF, low overhead or Annex B data.
pub struct VideoReader {
  source: Box<dyn PacketSource>,
  pending: VecDeque<Packet>,
  /// The framing of the input.
  pub framing: Framing,
  /// The time base denominator. For raw streams, the frame rate numerator.
  pub rate: u32,
  /// The time base numerator. For raw streams, the frame rate denominator.
  pub scale: u32,
  sequence: Option<SequenceHeader>,
  header: [u8; HEADER_SIZE],
}

impl VideoReader {
  /// Detects the framing and reads up to the first temporal unit.
  ///
  /// Raw streams have no time stamps, so packets get their index as time stamp and the time base
  /// becomes the frame rate: `fps`, else the one in the sequence header, else [`DEFAULT_FPS`].
  /// IVF keeps its own time base, and `fps` only changes the header that [`ivf_header`](Self::ivf_header) returns.
  pub fn open(mut input: Box<dyn BufRead>, fps: Option<(u32, u32)>) -> Result<VideoReader> {
    let start = input
      .fill_buf()
      .map_err(|error| Error::io("failed to read the input", error))?
      .to_vec();
    let framing = detect(&start).ok_or_else(|| Error::new(ErrorKind::UnsupportedInput, "the input is not an AV1 stream"))?;
    let (mut source, rate, scale, header): (Box<dyn PacketSource>, _, _, _) = if framing == Framing::Ivf {
      let reader = IvfReader::new(input)?;
      let mut header = reader.header;
      if let Some((num, den)) = fps {
        header[16..20].copy_from_slice(&num.to_le_bytes());
        header[20..24].copy_from_slice(&den.to_le_bytes());
      }
      let (rate, scale) = (reader.rate, reader.scale);
      (Box::new(reader), rate, scale, header)
    } else {
      let source: Box<dyn PacketSource> = match framing {
        Framing::AnnexB => Box::new(AnnexBSource { input, index: 0 }),
        _ => Box::new(LowOverheadSource {
          input,
          carry: None,
          index: 0,
        }),
      };
      (source, 0, 0, [0u8; HEADER_SIZE])
    };
    let first = source.next_packet()?;
    let sequence = first.as_ref().and_then(|packet| first_sequence_header(&packet.data));
    let mut reader = VideoReader {
      source,
      pending: first.into_iter().collect(),
      framing,
      rate,
      scale,
      sequence,
      header,
    };
    if framing != Framing::Ivf {
      let (rate, scale) = fps
        .or_else(|| reader.sequence.as_ref().and_then(|seq| seq.frame_rate))
        .unwrap_or(DEFAULT_FPS);
      let (width, height) = reader
        .sequence
        .as_ref()
        .map_or((0, 0), |seq| (seq.max_frame_width, seq.max_frame_height));
      let clamp = |value: u32| value.min(u16::MAX as u32) as u16;
      reader.rate = rate;
      reader.scale = scale;
      reader.header = ivf_header(clamp(width), clamp(height), rate, scale, 0);
    }
    Ok(reader)
  }

  /// Reads the next temporal unit. `None` at the end of the stream.
  pub fn next_packet(&mut self) -> Result<Option<Packet>> {
    match self.pending.pop_front() {
      Some(packet) => Ok(Some(packet)),
      None => self.source.next_packet(),
    }
  }

  /// The sequence header of the first temporal unit, when it has one.
  pub fn sequence(&self) -> Option<&SequenceHeader> {
    self.sequence.as_ref()
  }

  /// Converts a packet time stamp to units of `1 / units_per_second` seconds, rounding up.
  pub fn timestamp(&self, pts: u64, units_per_second: u64) -> u64 {
    let numerator = pts as u128 * self.scale as u128 * units_per_second as u128;
    numerator.div_ceil(self.rate as u128) as u64
  }

  /// An IVF file header for writing the stream to IVF.
  pub fn ivf_header(&self) -> &[u8; HEADER_SIZE] {
    &self.header
  }

  /// Stops reading. Later calls to [`next_packet`](Self::next_packet) return `None`, and the
  /// input is dropped, which closes a pipe.
  pub fn close(&mut self) {
    self.pending.clear();
    self.source = Box::new(Closed);
  }
}

fn first_sequence_header(data: &[u8]) -> Option<SequenceHeader> {
  for obu in iter_obus(data) {
    let obu = obu.ok()?;
    if obu.header.obu_type == OBU_SEQUENCE_HEADER {
      return SequenceHeader::parse(&data[obu.payload]).ok();
    }
  }
  None
}

/// Writes a temporal unit to a low overhead stream, adding a temporal delimiter when it has none.
pub fn write_low_overhead(output: &mut impl Write, data: &[u8]) -> Result<()> {
  let starts_with_delimiter = data.first().is_some_and(|&first| obu_type_of(first) == OBU_TEMPORAL_DELIMITER);
  let result = if starts_with_delimiter {
    output.write_all(data)
  } else {
    output.write_all(&[0x12, 0x00]).and_then(|()| output.write_all(data))
  };
  result.map_err(|error| Error::io("failed to write the stream", error))
}

/// Writes a temporal unit to an Annex B stream. `units` is the number of OBUs in each frame unit.
pub fn write_annexb(output: &mut impl Write, data: &[u8], units: &[usize]) -> Result<()> {
  let obus = split_obus(data)?;
  if units.iter().sum::<usize>() != obus.len() {
    return Err(Error::bitstream("the temporal unit no longer matches its frame units"));
  }
  let mut temporal = Vec::with_capacity(data.len() + 32);
  let mut next = obus.iter();
  for &count in units {
    let mut frame = Vec::new();
    for obu in next.by_ref().take(count) {
      write_leb128(&mut frame, obu.range().len() as u64);
      frame.extend_from_slice(&data[obu.range()]);
    }
    write_leb128(&mut temporal, frame.len() as u64);
    temporal.extend(frame);
  }
  let mut out = Vec::with_capacity(temporal.len() + 8);
  write_leb128(&mut out, temporal.len() as u64);
  out.extend(temporal);
  output
    .write_all(&out)
    .map_err(|error| Error::io("failed to write the stream", error))
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::io::Cursor;

  fn reader(bytes: Vec<u8>) -> Box<dyn BufRead> {
    Box::new(Cursor::new(bytes))
  }

  #[test]
  fn recognises_framings() {
    assert_eq!(detect(b"DKIF\0\0 \0AV01"), Some(Framing::Ivf));
    assert_eq!(detect(&[0x12, 0x00, 0x0a, 0x0b]), Some(Framing::LowOverhead));
    assert_eq!(
      detect(&[0x0a, 0x09, 0x01, 0x10, 0x07, 0x08, 0, 0, 0, 0, 0, 0]),
      Some(Framing::AnnexB)
    );
    assert_eq!(detect(&[0x1a, 0x45, 0xdf, 0xa3]), None);
    assert_eq!(detect(b"\0\0\0 ftypisom"), None);
  }

  #[test]
  fn reads_low_overhead_temporal_units() {
    let stream = vec![0x12, 0x00, 0x32, 0x01, 0xaa, 0x12, 0x00, 0x32, 0x02, 0xbb, 0xcc];
    let mut video = VideoReader::open(reader(stream), Some((25, 1))).unwrap();
    assert_eq!(video.framing, Framing::LowOverhead);
    let first = video.next_packet().unwrap().unwrap();
    assert_eq!(first.data, vec![0x12, 0x00, 0x32, 0x01, 0xaa]);
    let second = video.next_packet().unwrap().unwrap();
    assert_eq!((second.pts, second.data.len()), (1, 6));
    assert!(video.next_packet().unwrap().is_none());
    assert_eq!(video.timestamp(1, 10_000_000), 400_000);
  }

  #[test]
  fn annexb_round_trips() {
    let delimiter = [0x10];
    let metadata = [0x28, 0x01, 0x02];
    let mut frame_one = Vec::new();
    for obu in [&delimiter[..], &metadata[..]] {
      write_leb128(&mut frame_one, obu.len() as u64);
      frame_one.extend_from_slice(obu);
    }
    let mut frame_two = Vec::new();
    write_leb128(&mut frame_two, metadata.len() as u64);
    frame_two.extend_from_slice(&metadata);
    let mut temporal = Vec::new();
    for frame in [&frame_one, &frame_two] {
      write_leb128(&mut temporal, frame.len() as u64);
      temporal.extend_from_slice(frame);
    }
    let mut stream = Vec::new();
    write_leb128(&mut stream, temporal.len() as u64);
    stream.extend_from_slice(&temporal);

    let mut video = VideoReader::open(reader(stream.clone()), Some((24, 1))).unwrap();
    assert_eq!(video.framing, Framing::AnnexB);
    let packet = video.next_packet().unwrap().unwrap();
    assert_eq!(packet.units, Some(vec![2, 1]));
    assert_eq!(packet.data, vec![0x12, 0x00, 0x2a, 0x02, 0x01, 0x02, 0x2a, 0x02, 0x01, 0x02]);
    let mut written = Vec::new();
    write_annexb(&mut written, &packet.data, packet.units.as_ref().unwrap()).unwrap();
    let again = VideoReader::open(reader(written), Some((24, 1)))
      .unwrap()
      .next_packet()
      .unwrap()
      .unwrap();
    assert_eq!(again.data, packet.data);
    assert_eq!(again.units, packet.units);
  }

  #[test]
  fn close_stops_reading() {
    let stream = vec![0x12, 0x00, 0x12, 0x00];
    let mut video = VideoReader::open(reader(stream), None).unwrap();
    assert_eq!(video.ivf_header()[16..24], [0xc0, 0x5d, 0, 0, 0xe9, 0x03, 0, 0]);
    video.close();
    assert!(video.next_packet().unwrap().is_none());
  }

  #[test]
  fn rejects_other_data() {
    let error = VideoReader::open(reader(b"\x1a\x45\xdf\xa3 matroska".to_vec()), None)
      .err()
      .unwrap();
    assert_eq!(error.kind(), ErrorKind::UnsupportedInput);
  }
}
