//! The sequence header OBU.

use crate::bits::BitReader;
use crate::error::{Error, Result};

/// `SELECT_SCREEN_CONTENT_TOOLS`
pub const SELECT_SCREEN_CONTENT_TOOLS: u32 = 2;
/// `SELECT_INTEGER_MV`
pub const SELECT_INTEGER_MV: u32 = 2;

/// `color_primaries` value for BT.709.
pub const CP_BT_709: u8 = 1;
/// `color_primaries` value when unspecified.
pub const CP_UNSPECIFIED: u8 = 2;
/// `color_primaries` value for BT.2020 and BT.2100.
pub const CP_BT_2020: u8 = 9;
/// `color_primaries` value for SMPTE EG 432 (Display P3).
pub const CP_SMPTE_432: u8 = 12;
/// `transfer_characteristics` value for BT.709.
pub const TC_BT_709: u8 = 1;
/// `transfer_characteristics` value when unspecified.
pub const TC_UNSPECIFIED: u8 = 2;
/// `transfer_characteristics` value for sRGB.
pub const TC_SRGB: u8 = 13;
/// `transfer_characteristics` value for SMPTE ST 2084 (PQ), used by HDR10 and HDR10+.
pub const TC_SMPTE_2084: u8 = 16;
/// `transfer_characteristics` value for ARIB STD-B67 (HLG).
pub const TC_HLG: u8 = 18;
/// `matrix_coefficients` value for identity (RGB).
pub const MC_IDENTITY: u8 = 0;
/// `matrix_coefficients` value for BT.709.
pub const MC_BT_709: u8 = 1;
/// `matrix_coefficients` value when unspecified.
pub const MC_UNSPECIFIED: u8 = 2;
/// `matrix_coefficients` value for BT.2020 non-constant luminance.
pub const MC_BT_2020_NCL: u8 = 9;
/// `matrix_coefficients` value for ICtCp.
pub const MC_ICTCP: u8 = 14;

/// One entry of the operating point list.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OperatingPoint {
  /// `operating_point_idc`: which temporal (low 8 bits) and spatial layers (bits 8 to 11) it decodes. 0 means all.
  pub idc: u32,
  /// `seq_level_idx`. 31 means no level restrictions.
  pub level: u8,
  /// `seq_tier`, true for the high tier.
  pub tier: bool,
  /// `decoder_model_present_for_this_op`
  pub decoder_model_present: bool,
}

/// The chroma layout the film grain syntax depends on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ChromaFormat {
  /// `mono_chrome`
  pub mono_chrome: bool,
  /// `subsampling_x`
  pub subsampling_x: bool,
  /// `subsampling_y`
  pub subsampling_y: bool,
}

impl ChromaFormat {
  /// 4:2:0 with chroma planes.
  pub const YUV420: ChromaFormat = ChromaFormat {
    mono_chrome: false,
    subsampling_x: true,
    subsampling_y: true,
  };
}

/// A parsed `sequence_header_obu()`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SequenceHeader {
  /// `seq_profile`: 0 Main, 1 High, 2 Professional.
  pub profile: u8,
  /// `still_picture`
  pub still_picture: bool,
  /// `reduced_still_picture_header`
  pub reduced_still_picture_header: bool,
  /// `timing_info_present_flag`
  pub timing_info_present: bool,
  /// `decoder_model_info_present_flag`
  pub decoder_model_info_present: bool,
  /// `equal_picture_interval`
  pub equal_picture_interval: bool,
  /// `buffer_removal_time_length_minus_1 + 1`
  pub buffer_removal_time_length: u32,
  /// `frame_presentation_time_length_minus_1 + 1`
  pub frame_presentation_time_length: u32,
  /// Frames per second as numerator and denominator, when the timing info gives a constant rate.
  pub frame_rate: Option<(u32, u32)>,
  /// The operating points, at least one.
  pub operating_points: Vec<OperatingPoint>,
  /// `frame_width_bits_minus_1 + 1`
  pub frame_width_bits: u32,
  /// `frame_height_bits_minus_1 + 1`
  pub frame_height_bits: u32,
  /// `max_frame_width_minus_1 + 1`
  pub max_frame_width: u32,
  /// `max_frame_height_minus_1 + 1`
  pub max_frame_height: u32,
  /// `frame_id_numbers_present_flag`
  pub frame_id_numbers_present: bool,
  /// `delta_frame_id_length_minus_2 + 2`
  pub delta_frame_id_length: u32,
  /// `additional_frame_id_length_minus_1 + 1`
  pub additional_frame_id_length: u32,
  /// `use_128x128_superblock`
  pub use_128x128_superblock: bool,
  /// `enable_warped_motion`
  pub enable_warped_motion: bool,
  /// `enable_order_hint`
  pub enable_order_hint: bool,
  /// `enable_ref_frame_mvs`
  pub enable_ref_frame_mvs: bool,
  /// `seq_force_screen_content_tools`
  pub force_screen_content_tools: u32,
  /// `seq_force_integer_mv`
  pub force_integer_mv: u32,
  /// `OrderHintBits`
  pub order_hint_bits: u32,
  /// `enable_superres`
  pub enable_superres: bool,
  /// `enable_cdef`
  pub enable_cdef: bool,
  /// `enable_restoration`
  pub enable_restoration: bool,
  /// `BitDepth`: 8, 10 or 12.
  pub bit_depth: u8,
  /// `mono_chrome`
  pub mono_chrome: bool,
  /// `color_description_present_flag`
  pub color_description_present: bool,
  /// `color_primaries`, one of the `CP_` constants or another ITU-T H.273 value.
  pub color_primaries: u8,
  /// `transfer_characteristics`, one of the `TC_` constants or another ITU-T H.273 value.
  pub transfer_characteristics: u8,
  /// `matrix_coefficients`, one of the `MC_` constants or another ITU-T H.273 value.
  pub matrix_coefficients: u8,
  /// `color_range`, true for full range.
  pub full_range: bool,
  /// `chroma_sample_position`: 0 unknown, 1 vertical, 2 colocated.
  pub chroma_sample_position: u8,
  /// `subsampling_x`
  pub subsampling_x: bool,
  /// `subsampling_y`
  pub subsampling_y: bool,
  /// `separate_uv_delta_q`
  pub separate_uv_delta_q: bool,
  /// `film_grain_params_present`
  pub film_grain_params_present: bool,
  /// Bit position of `color_description_present_flag` in the payload.
  pub color_description_bit: usize,
  /// Bit position of `film_grain_params_present` in the payload, to switch film grain on or off in place.
  pub film_grain_bit: usize,
}

impl SequenceHeader {
  /// `NumPlanes`
  pub fn num_planes(&self) -> usize {
    if self.mono_chrome { 1 } else { 3 }
  }

  /// The chroma layout.
  pub fn chroma_format(&self) -> ChromaFormat {
    ChromaFormat {
      mono_chrome: self.mono_chrome,
      subsampling_x: self.subsampling_x,
      subsampling_y: self.subsampling_y,
    }
  }

  /// True when the transfer characteristics are PQ, as HDR10 and HDR10+ need.
  pub fn is_pq(&self) -> bool {
    self.transfer_characteristics == TC_SMPTE_2084
  }

  /// True when the transfer characteristics are HLG.
  pub fn is_hlg(&self) -> bool {
    self.transfer_characteristics == TC_HLG
  }

  /// Parses the payload of a sequence header OBU.
  pub fn parse(data: &[u8]) -> Result<Self> {
    let mut r = BitReader::new(data);
    let mut seq = SequenceHeader::default();
    let profile = r.bits(3)?;
    if profile > 2 {
      return Err(Error::bitstream(format!("unsupported AV1 profile {profile}")));
    }
    seq.profile = profile as u8;
    seq.still_picture = r.bit()?;
    seq.reduced_still_picture_header = r.bit()?;
    if seq.reduced_still_picture_header {
      seq.operating_points.push(OperatingPoint {
        level: r.bits(5)? as u8,
        ..OperatingPoint::default()
      });
    } else {
      let mut buffer_delay_length = 0;
      seq.timing_info_present = r.bit()?;
      if seq.timing_info_present {
        let units_per_tick = r.bits(32)?;
        let time_scale = r.bits(32)?;
        seq.equal_picture_interval = r.bit()?;
        let mut ticks_per_picture = 1u64;
        if seq.equal_picture_interval {
          ticks_per_picture = r.uvlc()? as u64 + 1;
        }
        let denominator = units_per_tick as u64 * ticks_per_picture;
        if time_scale > 0 && denominator > 0 && denominator <= u32::MAX as u64 {
          seq.frame_rate = Some((time_scale, denominator as u32));
        }
        seq.decoder_model_info_present = r.bit()?;
        if seq.decoder_model_info_present {
          buffer_delay_length = r.bits(5)? + 1;
          r.skip(32)?;
          seq.buffer_removal_time_length = r.bits(5)? + 1;
          seq.frame_presentation_time_length = r.bits(5)? + 1;
        }
      }
      let initial_display_delay_present = r.bit()?;
      let count = r.bits(5)? + 1;
      for _ in 0..count {
        let mut point = OperatingPoint {
          idc: r.bits(12)?,
          level: r.bits(5)? as u8,
          ..OperatingPoint::default()
        };
        if point.level > 7 {
          point.tier = r.bit()?;
        }
        if seq.decoder_model_info_present {
          point.decoder_model_present = r.bit()?;
          if point.decoder_model_present {
            r.skip(2 * buffer_delay_length as usize + 1)?;
          }
        }
        if initial_display_delay_present && r.bit()? {
          r.skip(4)?;
        }
        seq.operating_points.push(point);
      }
    }
    seq.frame_width_bits = r.bits(4)? + 1;
    seq.frame_height_bits = r.bits(4)? + 1;
    seq.max_frame_width = r.bits(seq.frame_width_bits)? + 1;
    seq.max_frame_height = r.bits(seq.frame_height_bits)? + 1;
    if !seq.reduced_still_picture_header {
      seq.frame_id_numbers_present = r.bit()?;
    }
    if seq.frame_id_numbers_present {
      seq.delta_frame_id_length = r.bits(4)? + 2;
      seq.additional_frame_id_length = r.bits(3)? + 1;
    }
    seq.use_128x128_superblock = r.bit()?;
    r.skip(2)?;
    seq.force_screen_content_tools = SELECT_SCREEN_CONTENT_TOOLS;
    seq.force_integer_mv = SELECT_INTEGER_MV;
    if !seq.reduced_still_picture_header {
      r.skip(2)?;
      seq.enable_warped_motion = r.bit()?;
      r.skip(1)?;
      seq.enable_order_hint = r.bit()?;
      if seq.enable_order_hint {
        r.skip(1)?;
        seq.enable_ref_frame_mvs = r.bit()?;
      }
      if !r.bit()? {
        seq.force_screen_content_tools = r.bits(1)?;
      }
      if seq.force_screen_content_tools > 0 && !r.bit()? {
        seq.force_integer_mv = r.bits(1)?;
      }
      if seq.enable_order_hint {
        seq.order_hint_bits = r.bits(3)? + 1;
      }
    }
    seq.enable_superres = r.bit()?;
    seq.enable_cdef = r.bit()?;
    seq.enable_restoration = r.bit()?;
    seq.color_config(&mut r)?;
    seq.film_grain_bit = r.position();
    seq.film_grain_params_present = r.bit()?;
    Ok(seq)
  }

  fn color_config(&mut self, r: &mut BitReader) -> Result<()> {
    let high_bitdepth = r.bit()?;
    self.bit_depth = if self.profile == 2 && high_bitdepth {
      if r.bit()? { 12 } else { 10 }
    } else if high_bitdepth {
      10
    } else {
      8
    };
    self.mono_chrome = if self.profile == 1 { false } else { r.bit()? };
    self.color_description_bit = r.position();
    self.color_description_present = r.bit()?;
    (self.color_primaries, self.transfer_characteristics, self.matrix_coefficients) = (CP_UNSPECIFIED, TC_UNSPECIFIED, MC_UNSPECIFIED);
    if self.color_description_present {
      self.color_primaries = r.bits(8)? as u8;
      self.transfer_characteristics = r.bits(8)? as u8;
      self.matrix_coefficients = r.bits(8)? as u8;
    }
    if self.mono_chrome {
      self.full_range = r.bit()?;
      self.subsampling_x = true;
      self.subsampling_y = true;
      return Ok(());
    }
    if self.color_primaries == CP_BT_709 && self.transfer_characteristics == TC_SRGB && self.matrix_coefficients == MC_IDENTITY {
      self.full_range = true;
    } else {
      self.full_range = r.bit()?;
      match self.profile {
        0 => {
          self.subsampling_x = true;
          self.subsampling_y = true;
        }
        1 => {}
        _ => {
          if self.bit_depth == 12 {
            self.subsampling_x = r.bit()?;
            if self.subsampling_x {
              self.subsampling_y = r.bit()?;
            }
          } else {
            self.subsampling_x = true;
          }
        }
      }
      if self.subsampling_x && self.subsampling_y {
        self.chroma_sample_position = r.bits(2)? as u8;
      }
    }
    self.separate_uv_delta_q = r.bit()?;
    Ok(())
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::bits::BitWriter;

  // A Main profile 10 bit 4:2:0 PQ header with timing info and one operating point.
  fn pq_header(grain: bool) -> Vec<u8> {
    let mut w = BitWriter::new();
    w.bits(0, 3); // seq_profile
    w.bit(false); // still_picture
    w.bit(false); // reduced_still_picture_header
    w.bit(true); // timing_info_present_flag
    w.bits(1001, 32);
    w.bits(24000, 32);
    w.bit(true); // equal_picture_interval
    w.bit(true); // uvlc 0
    w.bit(false); // decoder_model_info_present_flag
    w.bit(false); // initial_display_delay_present_flag
    w.bits(0, 5); // one operating point
    w.bits(0, 12);
    w.bits(13, 5); // level 5.1
    w.bit(true); // high tier
    w.bits(11, 4);
    w.bits(11, 4);
    w.bits(3839, 12);
    w.bits(2159, 12);
    w.bit(false); // frame_id_numbers_present_flag
    w.bit(false); // use_128x128_superblock
    w.bits(0b11, 2);
    w.bits(0b11, 2);
    w.bit(true); // enable_warped_motion
    w.bit(true);
    w.bit(true); // enable_order_hint
    w.bit(true);
    w.bit(true); // enable_ref_frame_mvs
    w.bit(true); // seq_choose_screen_content_tools
    w.bit(true); // seq_choose_integer_mv
    w.bits(6, 3); // order_hint_bits_minus_1
    w.bit(false); // enable_superres
    w.bit(true); // enable_cdef
    w.bit(true); // enable_restoration
    w.bit(true); // high_bitdepth
    w.bit(false); // mono_chrome
    w.bit(true); // color_description_present_flag
    w.bits(CP_BT_2020 as u32, 8);
    w.bits(TC_SMPTE_2084 as u32, 8);
    w.bits(MC_BT_2020_NCL as u32, 8);
    w.bit(false); // color_range
    w.bits(2, 2); // chroma_sample_position
    w.bit(false); // separate_uv_delta_q
    w.bit(grain);
    w.trailing_bits();
    w.finish()
  }

  #[test]
  fn reads_color_and_levels() {
    let seq = SequenceHeader::parse(&pq_header(true)).unwrap();
    assert_eq!(seq.profile, 0);
    assert_eq!(seq.frame_rate, Some((24000, 1001)));
    assert_eq!(seq.operating_points[0].level, 13);
    assert!(seq.operating_points[0].tier);
    assert_eq!((seq.max_frame_width, seq.max_frame_height), (3840, 2160));
    assert_eq!(seq.bit_depth, 10);
    assert!(seq.is_pq() && !seq.is_hlg());
    assert_eq!(seq.color_primaries, CP_BT_2020);
    assert_eq!(seq.matrix_coefficients, MC_BT_2020_NCL);
    assert!(!seq.full_range);
    assert_eq!(seq.chroma_sample_position, 2);
    assert_eq!(seq.chroma_format(), ChromaFormat::YUV420);
    assert_eq!(seq.order_hint_bits, 7);
    assert!(seq.film_grain_params_present);

    let data = pq_header(true);
    let mut r = BitReader::new(&data);
    r.skip(seq.color_description_bit).unwrap();
    assert!(r.bit().unwrap());
    assert_eq!(r.bits(8).unwrap(), CP_BT_2020 as u32);

    let mut cleared = pq_header(true);
    cleared[seq.film_grain_bit >> 3] &= !(1 << (7 - (seq.film_grain_bit & 7)));
    assert!(!SequenceHeader::parse(&cleared).unwrap().film_grain_params_present);
    assert_eq!(cleared, pq_header(false));
  }

  #[test]
  fn rejects_reserved_profiles() {
    assert!(SequenceHeader::parse(&[0xe0, 0, 0, 0]).is_err());
  }
}
