//! The IVF container.

use crate::error::{Error, ErrorKind, Result};
use crate::packet::Packet;
use std::io::{Read, Write};

/// The size of the IVF file header this crate writes.
pub const HEADER_SIZE: usize = 32;
const MAX_PACKET: usize = 1 << 28;

pub(crate) fn read_error(message: &str, error: std::io::Error) -> Error {
  if error.kind() == std::io::ErrorKind::UnexpectedEof {
    Error::bitstream(message)
  } else {
    Error::io(message, error)
  }
}

/// Builds an IVF file header for AV1.
///
/// `rate` and `scale` form the time base: one time stamp unit lasts `scale / rate` seconds.
/// `frames` may be 0 when the count is not known.
pub fn ivf_header(width: u16, height: u16, rate: u32, scale: u32, frames: u32) -> [u8; HEADER_SIZE] {
  let mut header = [0u8; HEADER_SIZE];
  header[0..4].copy_from_slice(b"DKIF");
  header[6..8].copy_from_slice(&(HEADER_SIZE as u16).to_le_bytes());
  header[8..12].copy_from_slice(b"AV01");
  header[12..14].copy_from_slice(&width.to_le_bytes());
  header[14..16].copy_from_slice(&height.to_le_bytes());
  header[16..20].copy_from_slice(&rate.to_le_bytes());
  header[20..24].copy_from_slice(&scale.to_le_bytes());
  header[24..28].copy_from_slice(&frames.to_le_bytes());
  header
}

/// Reads AV1 packets from an IVF file.
pub struct IvfReader<R: Read> {
  input: R,
  /// The file header, with its size field set to 32 bytes.
  pub header: [u8; HEADER_SIZE],
  /// The time base denominator.
  pub rate: u32,
  /// The time base numerator.
  pub scale: u32,
}

impl<R: Read> IvfReader<R> {
  /// Reads the file header. Fails when the file is not IVF or does not hold AV1.
  pub fn new(mut input: R) -> Result<Self> {
    let mut header = [0u8; HEADER_SIZE];
    input
      .read_exact(&mut header)
      .map_err(|error| read_error("failed to read the IVF header", error))?;
    if &header[0..4] != b"DKIF" {
      return Err(Error::new(ErrorKind::UnsupportedInput, "not an IVF file"));
    }
    if &header[8..12] != b"AV01" {
      return Err(Error::new(ErrorKind::UnsupportedInput, "the IVF file does not contain AV1 video"));
    }
    let header_size = u16::from_le_bytes([header[6], header[7]]) as usize;
    if header_size < HEADER_SIZE {
      return Err(Error::bitstream("invalid IVF header size"));
    }
    let mut extra = vec![0u8; header_size - HEADER_SIZE];
    input
      .read_exact(&mut extra)
      .map_err(|error| read_error("failed to read the IVF header", error))?;
    header[6..8].copy_from_slice(&(HEADER_SIZE as u16).to_le_bytes());
    let rate = u32::from_le_bytes(header[16..20].try_into().unwrap());
    let scale = u32::from_le_bytes(header[20..24].try_into().unwrap());
    if rate == 0 || scale == 0 {
      return Err(Error::bitstream("the IVF file has no valid time base"));
    }
    Ok(Self {
      input,
      header,
      rate,
      scale,
    })
  }

  /// Reads the next packet. `None` at the end of the file.
  pub fn next_packet(&mut self) -> Result<Option<Packet>> {
    let mut head = [0u8; 12];
    let mut filled = 0;
    while filled < head.len() {
      let read = match self.input.read(&mut head[filled..]) {
        Ok(read) => read,
        Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
        Err(error) => return Err(Error::io("failed to read an IVF frame", error)),
      };
      if read == 0 {
        if filled == 0 {
          return Ok(None);
        }
        return Err(Error::bitstream("truncated IVF frame header"));
      }
      filled += read;
    }
    let size = u32::from_le_bytes(head[0..4].try_into().unwrap()) as usize;
    if size > MAX_PACKET {
      return Err(Error::bitstream(format!("an IVF frame claims {size} bytes, the file is damaged")));
    }
    let pts = u64::from_le_bytes(head[4..12].try_into().unwrap());
    let mut data = vec![0u8; size];
    self
      .input
      .read_exact(&mut data)
      .map_err(|error| read_error("truncated IVF frame", error))?;
    Ok(Some(Packet { pts, data, units: None }))
  }
}

/// Writes AV1 packets to an IVF file.
pub struct IvfWriter<W: Write> {
  output: W,
}

impl<W: Write> IvfWriter<W> {
  /// Writes the file header.
  pub fn new(mut output: W, header: &[u8; HEADER_SIZE]) -> Result<Self> {
    output
      .write_all(header)
      .map_err(|error| Error::io("failed to write the IVF header", error))?;
    Ok(Self { output })
  }

  /// Writes one packet.
  pub fn write_packet(&mut self, pts: u64, data: &[u8]) -> Result<()> {
    let size = u32::try_from(data.len()).map_err(|_| Error::new(ErrorKind::InvalidArgument, "an IVF frame cannot hold 4 GiB"))?;
    let mut head = [0u8; 12];
    head[0..4].copy_from_slice(&size.to_le_bytes());
    head[4..12].copy_from_slice(&pts.to_le_bytes());
    self
      .output
      .write_all(&head)
      .and_then(|()| self.output.write_all(data))
      .map_err(|error| Error::io("failed to write an IVF frame", error))
  }

  /// Flushes and returns the output.
  pub fn finish(mut self) -> Result<W> {
    self
      .output
      .flush()
      .map_err(|error| Error::io("failed to write the IVF file", error))?;
    Ok(self.output)
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::io::Cursor;

  #[test]
  fn writes_and_reads_packets() {
    let header = ivf_header(1920, 1080, 24000, 1001, 2);
    let mut writer = IvfWriter::new(Vec::new(), &header).unwrap();
    writer.write_packet(0, &[0x12, 0x00]).unwrap();
    writer.write_packet(1001, &[0x12, 0x00, 0x32, 0x00]).unwrap();
    let bytes = writer.finish().unwrap();
    let mut reader = IvfReader::new(Cursor::new(bytes)).unwrap();
    assert_eq!((reader.rate, reader.scale), (24000, 1001));
    assert_eq!(reader.header, header);
    assert_eq!(reader.next_packet().unwrap().unwrap().data, vec![0x12, 0x00]);
    assert_eq!(reader.next_packet().unwrap().unwrap().pts, 1001);
    assert!(reader.next_packet().unwrap().is_none());
  }

  #[test]
  fn rejects_other_files() {
    assert_eq!(
      IvfReader::new(Cursor::new(b"RIFF....".to_vec())).err().unwrap().code(),
      "invalid_bitstream"
    );
    let mut vp9 = ivf_header(16, 16, 25, 1, 0);
    vp9[8..12].copy_from_slice(b"VP90");
    assert_eq!(IvfReader::new(Cursor::new(vp9.to_vec())).err().unwrap().code(), "unsupported_input");
  }

  #[test]
  fn truncated_frames_are_bitstream_errors() {
    let mut bytes = ivf_header(16, 16, 25, 1, 0).to_vec();
    bytes.extend([10, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x12]);
    let mut reader = IvfReader::new(Cursor::new(bytes)).unwrap();
    assert_eq!(reader.next_packet().unwrap_err().code(), "invalid_bitstream");
  }
}
