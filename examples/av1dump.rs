//! Prints the sequence header, the metadata and a summary of the frames of an AV1 file.
//!
//! cargo run --example av1dump -- <video> [--frames]

use rabbit_av1::metadata::{T35_COUNTRY_US, T35_PROVIDER_DOLBY, T35_PROVIDER_SAMSUNG};
use rabbit_av1::{Input, Metadata, ObuKind, RefSlots, StreamParser};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;

fn describe(metadata: &Metadata) -> String {
  match metadata {
    Metadata::ContentLightLevel(cll) => format!("content light level: MaxCLL {} MaxFALL {}", cll.max_cll, cll.max_fall),
    Metadata::MasteringDisplay(mdcv) => {
      let [r, g, b] = mdcv.primaries_f64();
      let w = mdcv.white_point_f64();
      format!(
        "mastering display: R({:.4},{:.4}) G({:.4},{:.4}) B({:.4},{:.4}) WP({:.4},{:.4}) L({},{})",
        r[0],
        r[1],
        g[0],
        g[1],
        b[0],
        b[1],
        w[0],
        w[1],
        mdcv.max_nits(),
        mdcv.min_nits()
      )
    }
    Metadata::ItuTT35(t35) if t35.is_from(T35_COUNTRY_US, T35_PROVIDER_SAMSUNG) => "ITU-T T.35 from Samsung (HDR10+)".to_string(),
    Metadata::ItuTT35(t35) if t35.is_from(T35_COUNTRY_US, T35_PROVIDER_DOLBY) => "ITU-T T.35 from Dolby (Dolby Vision)".to_string(),
    Metadata::ItuTT35(t35) => format!(
      "ITU-T T.35 country {:#04x} provider {:?}, {} bytes",
      t35.country_code,
      t35.provider_code(),
      t35.payload.len()
    ),
    Metadata::Other { metadata_type, data } => format!("metadata type {metadata_type}, {} bytes", data.len()),
  }
}

fn run(path: PathBuf, frames: bool) -> rabbit_av1::Result<()> {
  let mut input = Input::open(&path, "ffmpeg", None)?;
  println!(
    "framing: {:?}, time base {}/{}",
    input.reader.framing, input.reader.scale, input.reader.rate
  );
  let mut parser = StreamParser::new();
  let mut slots = RefSlots::new(None::<usize>);
  let mut metadata: BTreeMap<String, usize> = BTreeMap::new();
  // With spatial layers a temporal unit shows one frame per layer, and a decoder outputs only the
  // highest one, so pictures are counted per temporal unit.
  let (mut packets, mut coded, mut shown, mut pictures) = (0usize, 0usize, 0usize, 0usize);
  let mut sequences = 0;
  let result = (|| -> rabbit_av1::Result<()> {
    while let Some(packet) = input.reader.next_packet()? {
      packets += 1;
      let obus = parser
        .parse_packet(&packet.data)
        .map_err(|error| error.context(format!("temporal unit {packets}")))?;
      let mut shows = false;
      for obu in obus {
        match obu.kind {
          ObuKind::Sequence(seq) => {
            if sequences == 0 {
              println!(
                "sequence: profile {} level {} {}x{} {} bit, primaries {} transfer {} matrix {} {} range, film grain {}",
                seq.profile,
                seq.operating_points[0].level,
                seq.max_frame_width,
                seq.max_frame_height,
                seq.bit_depth,
                seq.color_primaries,
                seq.transfer_characteristics,
                seq.matrix_coefficients,
                if seq.full_range { "full" } else { "limited" },
                seq.film_grain_params_present
              );
            }
            sequences += 1;
          }
          ObuKind::Metadata(found) => *metadata.entry(describe(&found)).or_default() += 1,
          ObuKind::Frame(frame) => {
            let header = &frame.header;
            let index = if header.show_existing_frame {
              slots.show_existing(header)
            } else {
              coded += 1;
              slots.store(header, Some(coded - 1));
              Some(coded - 1)
            };
            if header.show_frame {
              shown += 1;
              shows = true;
              if frames {
                println!(
                  "  pts {} shows coded frame {:?}, type {}{}",
                  packet.pts,
                  index,
                  header.frame_type,
                  if header.show_existing_frame { " again" } else { "" }
                );
              }
            }
          }
          _ => {}
        }
      }
      pictures += usize::from(shows);
    }
    Ok(())
  })();
  if let Err(error) = result {
    return Err(input.explain(error));
  }
  input.finish()?;
  println!(
    "{packets} temporal units, {coded} coded frames, {pictures} shown pictures ({shown} shown frames), {sequences} sequence headers"
  );
  for (description, count) in metadata {
    println!("{count:>6} x {description}");
  }
  Ok(())
}

fn main() -> ExitCode {
  let mut args = std::env::args_os().skip(1);
  let Some(path) = args.next() else {
    eprintln!("usage: av1dump <video> [--frames]");
    return ExitCode::from(2);
  };
  let frames = args.next().is_some_and(|arg| arg == "--frames");
  match run(PathBuf::from(path), frames) {
    Ok(()) => ExitCode::SUCCESS,
    Err(error) => {
      eprintln!("error ({}): {error}", error.code());
      ExitCode::FAILURE
    }
  }
}
