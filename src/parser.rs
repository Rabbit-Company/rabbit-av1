//! Parsing a stream temporal unit by temporal unit.

use crate::bits::{BitReader, read_leb128};
use crate::error::{Error, Result, ResultExt};
use crate::frame::{FrameHeader, FrameState};
use crate::grain::{ChromaFormat, GrainParams};
use crate::metadata::Metadata;
use crate::obu::{
  OBU_FRAME, OBU_FRAME_HEADER, OBU_METADATA, OBU_REDUNDANT_FRAME_HEADER, OBU_SEQUENCE_HEADER, OBU_TEMPORAL_DELIMITER, OBU_TILE_GROUP,
  ObuExtension, read_obu,
};
use crate::sequence::SequenceHeader;
use std::ops::Range;

/// What a frame header says about film grain.
#[derive(Clone, Debug, PartialEq)]
pub enum CodedGrain {
  /// The sequence header disables film grain, or the frame cannot carry it.
  Absent,
  /// `apply_grain` is 0.
  NotApplied,
  /// The frame reuses the parameters of a reference frame with a new seed.
  Reference {
    /// `grain_seed`
    seed: u16,
    /// `film_grain_params_ref_idx`
    slot: usize,
  },
  /// The frame carries its own parameters.
  Full {
    /// `grain_seed`
    seed: u16,
    /// The parameters.
    params: GrainParams,
  },
}

/// A parsed frame header with its film grain.
#[derive(Clone, Debug)]
pub struct CodedFrame {
  /// The header up to `film_grain_params()`.
  pub header: FrameHeader,
  /// The film grain the header carries.
  pub grain: CodedGrain,
  /// Bit position in the payload where the header ends, after `film_grain_params()`.
  pub header_end_bit: usize,
}

/// What an OBU holds.
#[derive(Clone, Debug)]
pub enum ObuKind {
  /// A sequence header.
  Sequence(SequenceHeader),
  /// A frame header or frame OBU that starts a new frame.
  Frame(Box<CodedFrame>),
  /// A frame header or redundant frame header that repeats the header of the current frame.
  FrameCopy,
  /// A metadata OBU. Known types that do not parse are kept as [`Metadata::Other`].
  Metadata(Metadata),
  /// A temporal delimiter.
  TemporalDelimiter,
  /// Any other OBU: tile groups, tile lists, padding and reserved types.
  Other,
}

/// One OBU of a temporal unit.
#[derive(Clone, Debug)]
pub struct Obu {
  /// `obu_type`
  pub obu_type: u8,
  /// The extension header, when present.
  pub extension: Option<ObuExtension>,
  /// `obu_has_size_field`
  pub has_size: bool,
  /// The header bytes, without the size field.
  pub prefix: Range<usize>,
  /// The payload bytes.
  pub payload: Range<usize>,
  /// What the payload holds.
  pub kind: ObuKind,
}

impl Obu {
  /// The whole OBU: header, size field and payload.
  pub fn range(&self) -> Range<usize> {
    self.prefix.start..self.payload.end
  }
}

/// Parses the OBUs of a stream one temporal unit at a time, keeping the state that frame
/// headers depend on.
///
/// Feed it every temporal unit of a stream in order. It reads headers only, not tile data.
#[derive(Default)]
pub struct StreamParser {
  sequence: Option<SequenceHeader>,
  state: FrameState,
  current: Option<FrameHeader>,
}

impl StreamParser {
  /// Creates a parser for the start of a stream.
  pub fn new() -> Self {
    Self::default()
  }

  /// The latest sequence header.
  pub fn sequence(&self) -> Option<&SequenceHeader> {
    self.sequence.as_ref()
  }

  /// The chroma layout from the latest sequence header.
  pub fn format(&self) -> Option<ChromaFormat> {
    self.sequence.as_ref().map(SequenceHeader::chroma_format)
  }

  /// Parses all OBUs of a temporal unit.
  pub fn parse_packet(&mut self, data: &[u8]) -> Result<Vec<Obu>> {
    let mut obus = Vec::new();
    let mut pos = 0;
    while pos < data.len() {
      let raw = read_obu(data, pos)?;
      let (temporal_id, spatial_id) = raw
        .header
        .extension
        .map_or((0, 0), |extension| (extension.temporal_id as u32, extension.spatial_id as u32));
      let obu_type = raw.header.obu_type;
      let bytes = &data[raw.payload.clone()];
      let kind = match obu_type {
        OBU_SEQUENCE_HEADER => {
          let sequence = SequenceHeader::parse(bytes).context("failed to parse the sequence header")?;
          self.sequence = Some(sequence.clone());
          ObuKind::Sequence(sequence)
        }
        OBU_TEMPORAL_DELIMITER => {
          self.current = None;
          ObuKind::TemporalDelimiter
        }
        OBU_FRAME_HEADER | OBU_REDUNDANT_FRAME_HEADER | OBU_FRAME => {
          if self.current.is_some() && obu_type != OBU_FRAME {
            ObuKind::FrameCopy
          } else {
            let frame = self
              .parse_frame(bytes, temporal_id, spatial_id)
              .context("failed to parse a frame header")?;
            if obu_type == OBU_FRAME && !frame.header.show_existing_frame {
              let mut r = BitReader::new(bytes);
              r.skip(frame.header_end_bit.div_ceil(8) * 8)?;
              self.tile_group(&mut r)?;
            }
            ObuKind::Frame(Box::new(frame))
          }
        }
        OBU_TILE_GROUP => {
          self.tile_group(&mut BitReader::new(bytes))?;
          ObuKind::Other
        }
        OBU_METADATA => {
          let metadata = Metadata::parse(bytes).or_else(|_| {
            let (metadata_type, used) = read_leb128(bytes).context("failed to parse a metadata OBU")?;
            Ok::<_, Error>(Metadata::Other {
              metadata_type,
              data: bytes[used..].to_vec(),
            })
          })?;
          ObuKind::Metadata(metadata)
        }
        _ => ObuKind::Other,
      };
      obus.push(Obu {
        obu_type,
        extension: raw.header.extension,
        has_size: raw.header.has_size,
        prefix: raw.prefix,
        payload: raw.payload.clone(),
        kind,
      });
      pos = raw.payload.end;
    }
    Ok(obus)
  }

  fn parse_frame(&mut self, bytes: &[u8], temporal_id: u32, spatial_id: u32) -> Result<CodedFrame> {
    let Some(seq) = self.sequence.as_ref() else {
      return Err(Error::bitstream("frame header found before any sequence header"));
    };
    let mut r = BitReader::new(bytes);
    let header = self.state.parse(&mut r, seq, temporal_id, spatial_id)?;
    let mut grain = CodedGrain::Absent;
    if !header.show_existing_frame && seq.film_grain_params_present && header.grain_allowed() {
      grain = CodedGrain::NotApplied;
      if r.bit()? {
        let seed = r.bits(16)? as u16;
        let update = if header.is_inter() { r.bit()? } else { true };
        grain = if update {
          CodedGrain::Full {
            seed,
            params: GrainParams::read(&mut r, seq.chroma_format())?,
          }
        } else {
          CodedGrain::Reference {
            seed,
            slot: r.bits(3)? as usize,
          }
        };
      }
    }
    let header_end_bit = r.position();
    self.state.update(&header);
    self.current = if header.show_existing_frame { None } else { Some(header.clone()) };
    Ok(CodedFrame {
      header,
      grain,
      header_end_bit,
    })
  }

  fn tile_group(&mut self, r: &mut BitReader) -> Result<()> {
    let Some(header) = self.current.as_ref() else {
      return Ok(());
    };
    let tiles = header.num_tiles();
    let mut last = tiles - 1;
    if tiles > 1 && r.bit()? {
      let bits = header.tile_cols_log2 + header.tile_rows_log2;
      r.skip(bits as usize)?;
      last = r.bits(bits)?;
    }
    if last == tiles - 1 {
      self.current = None;
    }
    Ok(())
  }
}
