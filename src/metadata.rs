//! Metadata OBUs: HDR content light level, mastering display colour volume and ITU-T T.35.

use crate::bits::{BitReader, BitWriter, read_leb128, write_leb128};
use crate::error::{Error, ErrorKind, Result};
use crate::obu::{OBU_METADATA, ObuExtension, make_obu};

/// `METADATA_TYPE_HDR_CLL`
pub const METADATA_TYPE_HDR_CLL: u64 = 1;
/// `METADATA_TYPE_HDR_MDCV`
pub const METADATA_TYPE_HDR_MDCV: u64 = 2;
/// `METADATA_TYPE_SCALABILITY`
pub const METADATA_TYPE_SCALABILITY: u64 = 3;
/// `METADATA_TYPE_ITUT_T35`
pub const METADATA_TYPE_ITUT_T35: u64 = 4;
/// `METADATA_TYPE_TIMECODE`
pub const METADATA_TYPE_TIMECODE: u64 = 5;

/// ITU-T T.35 country code of the United States, used by HDR10+ and Dolby Vision.
pub const T35_COUNTRY_US: u8 = 0xb5;
/// ITU-T T.35 terminal provider code of Samsung, used by HDR10+ (SMPTE ST 2094-40).
pub const T35_PROVIDER_SAMSUNG: u16 = 0x003c;
/// ITU-T T.35 terminal provider code of Dolby, used by Dolby Vision.
pub const T35_PROVIDER_DOLBY: u16 = 0x003b;

/// `metadata_hdr_cll()`: the static content light level of HDR10.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ContentLightLevel {
  /// `max_cll`: the brightest pixel of the whole video in cd/m², MaxCLL.
  pub max_cll: u16,
  /// `max_fall`: the highest frame average brightness in cd/m², MaxFALL.
  pub max_fall: u16,
}

/// `metadata_hdr_mdcv()`: the colour volume of the display the video was mastered on.
///
/// Values are stored as the bitstream codes them. The `_f64` methods and
/// [`from_values`](Self::from_values) convert to and from chromaticity coordinates and cd/m².
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct MasteringDisplay {
  /// `primary_chromaticity_x` and `_y` for red, green and blue, as 0.16 fixed point CIE 1931 coordinates.
  pub primaries: [[u16; 2]; 3],
  /// `white_point_chromaticity_x` and `_y`, as 0.16 fixed point.
  pub white_point: [u16; 2],
  /// `luminance_max`, as 24.8 fixed point cd/m².
  pub max_luminance: u32,
  /// `luminance_min`, as 18.14 fixed point cd/m².
  pub min_luminance: u32,
}

/// Red, green and blue primaries of BT.2020 as CIE 1931 xy coordinates.
pub const PRIMARIES_BT_2020: [[f64; 2]; 3] = [[0.708, 0.292], [0.170, 0.797], [0.131, 0.046]];
/// Red, green and blue primaries of Display P3 as CIE 1931 xy coordinates.
pub const PRIMARIES_DISPLAY_P3: [[f64; 2]; 3] = [[0.680, 0.320], [0.265, 0.690], [0.150, 0.060]];
/// The D65 white point as CIE 1931 xy coordinates.
pub const WHITE_D65: [f64; 2] = [0.3127, 0.3290];

impl MasteringDisplay {
  /// Codes chromaticity coordinates and luminances in cd/m². Fails when a value does not fit.
  pub fn from_values(primaries: [[f64; 2]; 3], white_point: [f64; 2], max_nits: f64, min_nits: f64) -> Result<Self> {
    let fixed = |value: f64, scale: f64, limit: f64, name: &str| -> Result<f64> {
      let coded = (value * scale).round();
      if !value.is_finite() || coded < 0.0 || coded > limit {
        return Err(Error::new(ErrorKind::InvalidArgument, format!("{name} {value} is out of range")));
      }
      Ok(coded)
    };
    let xy = |pair: [f64; 2]| -> Result<[u16; 2]> {
      Ok([
        fixed(pair[0], 65536.0, u16::MAX as f64, "chromaticity")? as u16,
        fixed(pair[1], 65536.0, u16::MAX as f64, "chromaticity")? as u16,
      ])
    };
    Ok(MasteringDisplay {
      primaries: [xy(primaries[0])?, xy(primaries[1])?, xy(primaries[2])?],
      white_point: xy(white_point)?,
      max_luminance: fixed(max_nits, 256.0, u32::MAX as f64, "maximum luminance")? as u32,
      min_luminance: fixed(min_nits, 16384.0, u32::MAX as f64, "minimum luminance")? as u32,
    })
  }

  /// The primaries as CIE 1931 xy coordinates, red, green and blue.
  pub fn primaries_f64(&self) -> [[f64; 2]; 3] {
    self.primaries.map(|[x, y]| [x as f64 / 65536.0, y as f64 / 65536.0])
  }

  /// The white point as CIE 1931 xy coordinates.
  pub fn white_point_f64(&self) -> [f64; 2] {
    [self.white_point[0] as f64 / 65536.0, self.white_point[1] as f64 / 65536.0]
  }

  /// The maximum luminance in cd/m².
  pub fn max_nits(&self) -> f64 {
    self.max_luminance as f64 / 256.0
  }

  /// The minimum luminance in cd/m².
  pub fn min_nits(&self) -> f64 {
    self.min_luminance as f64 / 16384.0
  }
}

/// `metadata_itut_t35()`: data registered under an ITU-T T.35 country and provider code.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct ItuTT35 {
  /// `itu_t_t35_country_code`
  pub country_code: u8,
  /// `itu_t_t35_country_code_extension_byte`, present when the country code is 0xFF.
  pub country_code_extension: Option<u8>,
  /// `itu_t_t35_payload_bytes`, starting with the terminal provider code, without the trailing bits.
  pub payload: Vec<u8>,
}

impl ItuTT35 {
  /// The terminal provider code: the first two payload bytes, big endian.
  pub fn provider_code(&self) -> Option<u16> {
    Some(u16::from_be_bytes([*self.payload.first()?, *self.payload.get(1)?]))
  }

  /// True for the given country and terminal provider code.
  pub fn is_from(&self, country_code: u8, provider_code: u16) -> bool {
    self.country_code == country_code && self.provider_code() == Some(provider_code)
  }
}

/// The payload of a metadata OBU.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Metadata {
  /// `METADATA_TYPE_HDR_CLL`
  ContentLightLevel(ContentLightLevel),
  /// `METADATA_TYPE_HDR_MDCV`
  MasteringDisplay(MasteringDisplay),
  /// `METADATA_TYPE_ITUT_T35`
  ItuTT35(ItuTT35),
  /// Any other type, kept byte for byte so it can be written back unchanged.
  Other {
    /// `metadata_type`
    metadata_type: u64,
    /// Everything after `metadata_type`, trailing bits included.
    data: Vec<u8>,
  },
}

// The bytes before the trailing bits. A stream that leaves the trailing bits out keeps
// all of its bytes.
fn strip_trailing_bits(data: &[u8]) -> &[u8] {
  match data.iter().rposition(|&byte| byte != 0) {
    Some(last) if data[last] == 0x80 => &data[..last],
    Some(last) => &data[..=last],
    None => &[],
  }
}

impl Metadata {
  /// Parses the payload of a metadata OBU.
  pub fn parse(payload: &[u8]) -> Result<Metadata> {
    let (metadata_type, used) = read_leb128(payload)?;
    let data = &payload[used..];
    let mut r = BitReader::new(data);
    let metadata = match metadata_type {
      METADATA_TYPE_HDR_CLL => Metadata::ContentLightLevel(ContentLightLevel {
        max_cll: r.bits(16)? as u16,
        max_fall: r.bits(16)? as u16,
      }),
      METADATA_TYPE_HDR_MDCV => {
        let mut mdcv = MasteringDisplay::default();
        for primary in &mut mdcv.primaries {
          *primary = [r.bits(16)? as u16, r.bits(16)? as u16];
        }
        mdcv.white_point = [r.bits(16)? as u16, r.bits(16)? as u16];
        mdcv.max_luminance = r.bits(32)?;
        mdcv.min_luminance = r.bits(32)?;
        Metadata::MasteringDisplay(mdcv)
      }
      METADATA_TYPE_ITUT_T35 => {
        let Some((&country_code, mut rest)) = data.split_first() else {
          return Err(Error::bitstream("empty ITU-T T.35 metadata"));
        };
        let mut country_code_extension = None;
        if country_code == 0xff {
          let Some((&extension, after)) = rest.split_first() else {
            return Err(Error::bitstream("truncated ITU-T T.35 country code"));
          };
          country_code_extension = Some(extension);
          rest = after;
        }
        Metadata::ItuTT35(ItuTT35 {
          country_code,
          country_code_extension,
          payload: strip_trailing_bits(rest).to_vec(),
        })
      }
      _ => Metadata::Other {
        metadata_type,
        data: data.to_vec(),
      },
    };
    Ok(metadata)
  }

  /// The `metadata_type` value.
  pub fn metadata_type(&self) -> u64 {
    match self {
      Metadata::ContentLightLevel(_) => METADATA_TYPE_HDR_CLL,
      Metadata::MasteringDisplay(_) => METADATA_TYPE_HDR_MDCV,
      Metadata::ItuTT35(_) => METADATA_TYPE_ITUT_T35,
      Metadata::Other { metadata_type, .. } => *metadata_type,
    }
  }

  /// Writes the payload of a metadata OBU, trailing bits included.
  pub fn encode(&self) -> Vec<u8> {
    let mut out = Vec::new();
    write_leb128(&mut out, self.metadata_type());
    let mut w = BitWriter::new();
    match self {
      Metadata::ContentLightLevel(cll) => {
        w.bits(cll.max_cll as u32, 16);
        w.bits(cll.max_fall as u32, 16);
      }
      Metadata::MasteringDisplay(mdcv) => {
        for [x, y] in mdcv.primaries {
          w.bits(x as u32, 16);
          w.bits(y as u32, 16);
        }
        w.bits(mdcv.white_point[0] as u32, 16);
        w.bits(mdcv.white_point[1] as u32, 16);
        w.bits(mdcv.max_luminance, 32);
        w.bits(mdcv.min_luminance, 32);
      }
      Metadata::ItuTT35(t35) => {
        w.extend(&[t35.country_code]);
        if let Some(extension) = t35.country_code_extension {
          w.extend(&[extension]);
        }
        w.extend(&t35.payload);
      }
      Metadata::Other { data, .. } => {
        out.extend_from_slice(data);
        return out;
      }
    }
    w.trailing_bits();
    out.extend(w.finish());
    out
  }

  /// Writes a whole metadata OBU with a size field.
  pub fn to_obu(&self, extension: Option<ObuExtension>) -> Vec<u8> {
    make_obu(OBU_METADATA, extension, &self.encode())
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::obu::read_obu;

  fn round_trip(metadata: &Metadata) -> Vec<u8> {
    let payload = metadata.encode();
    assert_eq!(&Metadata::parse(&payload).unwrap(), metadata);
    payload
  }

  #[test]
  fn content_light_level() {
    let cll = Metadata::ContentLightLevel(ContentLightLevel {
      max_cll: 1000,
      max_fall: 400,
    });
    assert_eq!(round_trip(&cll), vec![0x01, 0x03, 0xe8, 0x01, 0x90, 0x80]);
  }

  #[test]
  fn mastering_display() {
    let mdcv = MasteringDisplay::from_values(PRIMARIES_DISPLAY_P3, WHITE_D65, 1000.0, 0.0001).unwrap();
    assert_eq!(mdcv.max_luminance, 256_000);
    assert_eq!(mdcv.min_luminance, 2);
    assert_eq!(mdcv.primaries[0], [44564, 20972]);
    assert!((mdcv.white_point_f64()[0] - 0.3127).abs() < 1e-4);
    assert_eq!(mdcv.max_nits(), 1000.0);
    let payload = round_trip(&Metadata::MasteringDisplay(mdcv));
    assert_eq!(payload.len(), 1 + 24 + 1);
    assert!(MasteringDisplay::from_values(PRIMARIES_BT_2020, WHITE_D65, -1.0, 0.0).is_err());
    assert!(MasteringDisplay::from_values([[1.5, 0.0]; 3], WHITE_D65, 1000.0, 0.0).is_err());
  }

  #[test]
  fn itu_t35() {
    let t35 = ItuTT35 {
      country_code: T35_COUNTRY_US,
      country_code_extension: None,
      payload: vec![0x00, 0x3c, 0x00, 0x01, 0x04, 0x01, 0x80],
    };
    assert_eq!(t35.provider_code(), Some(T35_PROVIDER_SAMSUNG));
    assert!(t35.is_from(T35_COUNTRY_US, T35_PROVIDER_SAMSUNG));
    let metadata = Metadata::ItuTT35(t35);
    let payload = round_trip(&metadata);
    assert_eq!(payload, vec![0x04, 0xb5, 0x00, 0x3c, 0x00, 0x01, 0x04, 0x01, 0x80, 0x80]);

    let obu = metadata.to_obu(None);
    let raw = read_obu(&obu, 0).unwrap();
    assert_eq!(raw.header.obu_type, OBU_METADATA);
    assert_eq!(Metadata::parse(&obu[raw.payload]).unwrap(), metadata);

    let extended = Metadata::ItuTT35(ItuTT35 {
      country_code: 0xff,
      country_code_extension: Some(0x12),
      payload: vec![0xab],
    });
    round_trip(&extended);
  }

  #[test]
  fn t35_without_trailing_bits_keeps_its_bytes() {
    let Metadata::ItuTT35(t35) = Metadata::parse(&[0x04, 0xb5, 0x00, 0x3b, 0x07]).unwrap() else {
      panic!("not T.35");
    };
    assert_eq!(t35.payload, vec![0x00, 0x3b, 0x07]);
    assert!(Metadata::parse(&[0x04]).is_err());
    assert!(Metadata::parse(&[0x04, 0xff]).is_err());
  }

  #[test]
  fn other_types_stay_unchanged() {
    let payload = vec![0x05, 0x12, 0x34, 0x80];
    let metadata = Metadata::parse(&payload).unwrap();
    assert_eq!(metadata.metadata_type(), METADATA_TYPE_TIMECODE);
    assert_eq!(metadata.encode(), payload);
  }

  #[test]
  fn short_payloads_fail() {
    assert!(Metadata::parse(&[0x01, 0x03]).is_err());
    assert!(Metadata::parse(&[0x02, 0, 0, 0]).is_err());
    assert!(Metadata::parse(&[]).is_err());
  }
}
