// Tests on real AV1 streams. They are made with ffmpeg and SVT-AV1 and skipped when those are missing.

use rabbit_av1::framing::write_annexb;
use rabbit_av1::metadata::{METADATA_TYPE_ITUT_T35, T35_COUNTRY_US};
use rabbit_av1::obu::{OBU_METADATA, obu_type_of};
use rabbit_av1::{Framing, Input, ItuTT35, Metadata, ObuKind, Output, Packet, RefSlots, StreamParser, VideoReader};
use std::io::Cursor;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::process::Command;

struct Scratch(PathBuf);

impl Scratch {
  fn new(name: &str) -> Scratch {
    let dir = std::env::temp_dir().join(format!("rabbit-av1-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    Scratch(dir)
  }

  fn path(&self, name: &str) -> PathBuf {
    self.0.join(name)
  }
}

impl Drop for Scratch {
  fn drop(&mut self) {
    let _ = std::fs::remove_dir_all(&self.0);
  }
}

fn ffmpeg(args: &[&str]) -> bool {
  Command::new("ffmpeg")
    .args(["-v", "error", "-nostdin", "-y"])
    .args(args)
    .output()
    .is_ok_and(|output| output.status.success())
}

// A 10 bit PQ stream with HDR10 static metadata. None when ffmpeg or SVT-AV1 is missing.
fn hdr_stream(path: &Path, frames: u32) -> Option<()> {
  let params = "color-primaries=9:transfer-characteristics=16:matrix-coefficients=9:enable-hdr=1:\
                mastering-display=G(0.265,0.69)B(0.15,0.06)R(0.68,0.32)WP(0.3127,0.329)L(1000,0.0001):content-light=1000,400";
  let made = ffmpeg(&[
    "-f",
    "lavfi",
    "-i",
    "testsrc2=size=96x64:rate=24",
    "-frames:v",
    &frames.to_string(),
    "-pix_fmt",
    "yuv420p10le",
    "-c:v",
    "libsvtav1",
    "-preset",
    "12",
    "-svtav1-params",
    params,
    path.to_str().unwrap(),
  ]);
  if !made {
    eprintln!("skipped: ffmpeg with libsvtav1 is needed");
  }
  made.then_some(())
}

// The MD5 of the decoded pictures.
fn picture(path: &Path) -> String {
  let mut command = Command::new("ffmpeg");
  command.args(["-v", "error", "-nostdin", "-c:v", "libdav1d"]);
  // ffmpeg reads .obu as low overhead framing and needs to be told about Annex B.
  if std::fs::read(path).ok().and_then(|data| rabbit_av1::framing::detect(&data)) == Some(Framing::AnnexB) {
    command.args(["-f", "av1"]);
  }
  let output = command
    .arg("-i")
    .arg(path)
    .args(["-map", "0:V:0", "-f", "md5", "-"])
    .output()
    .unwrap();
  assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
  String::from_utf8(output.stdout).unwrap()
}

fn read_all(path: &Path) -> (Framing, Vec<Packet>) {
  let mut input = Input::open(path, "ffmpeg", None).unwrap();
  let framing = input.reader.framing;
  let mut packets = Vec::new();
  while let Some(packet) = input.reader.next_packet().unwrap() {
    packets.push(packet);
  }
  input.finish().unwrap();
  (framing, packets)
}

// ffmpeg checks T.35 payloads from Samsung and Dolby, so the tests use a provider code nobody has.
const TEST_PROVIDER: u16 = 0x7e57;

fn tagged(frame: usize) -> Metadata {
  Metadata::ItuTT35(ItuTT35 {
    country_code: T35_COUNTRY_US,
    country_code_extension: None,
    payload: vec![0x7e, 0x57, 0x00, 0x01, 0x04, 0x01, frame as u8, 0x5a],
  })
}

fn count_t35(packets: &[Packet]) -> Vec<Vec<u8>> {
  let mut parser = StreamParser::new();
  let mut found = Vec::new();
  for packet in packets {
    for obu in parser.parse_packet(&packet.data).unwrap() {
      if let ObuKind::Metadata(Metadata::ItuTT35(t35)) = obu.kind {
        assert!(t35.is_from(T35_COUNTRY_US, TEST_PROVIDER));
        found.push(t35.payload);
      }
    }
  }
  found
}

#[test]
fn reads_hdr10_metadata_and_color() {
  let scratch = Scratch::new("hdr10");
  let source = scratch.path("hdr.ivf");
  let Some(()) = hdr_stream(&source, 10) else { return };
  let (framing, packets) = read_all(&source);
  assert_eq!(framing, Framing::Ivf);
  let mut parser = StreamParser::new();
  let (mut cll, mut mdcv, mut shown) = (None, None, 0);
  for packet in &packets {
    for obu in parser.parse_packet(&packet.data).unwrap() {
      match obu.kind {
        ObuKind::Metadata(Metadata::ContentLightLevel(found)) => cll = Some(found),
        ObuKind::Metadata(Metadata::MasteringDisplay(found)) => mdcv = Some(found),
        ObuKind::Frame(frame) if frame.header.show_frame => shown += 1,
        _ => {}
      }
    }
  }
  let seq = parser.sequence().unwrap();
  assert!(seq.is_pq());
  assert_eq!((seq.color_primaries, seq.matrix_coefficients, seq.bit_depth), (9, 9, 10));
  let cll = cll.unwrap();
  assert_eq!((cll.max_cll, cll.max_fall), (1000, 400));
  let mdcv = mdcv.unwrap();
  assert_eq!(mdcv.max_nits(), 1000.0);
  assert!((mdcv.primaries_f64()[0][0] - 0.68).abs() < 1e-4);
  assert_eq!(shown, 10);
}

#[test]
fn inserted_metadata_survives_every_output_and_keeps_the_pictures() {
  let scratch = Scratch::new("insert");
  let source = scratch.path("source.ivf");
  let Some(()) = hdr_stream(&source, 30) else { return };
  let reference = picture(&source);
  for extension in ["ivf", "obu", "mkv", "webm", "mp4"] {
    let target = scratch.path(&format!("tagged.{extension}"));
    let mut input = Input::open(&source, "ffmpeg", None).unwrap();
    let mut output = Output::create(&target, &source, &input.reader, "ffmpeg").unwrap();
    let mut parser = StreamParser::new();
    let mut slots = RefSlots::new(0usize);
    let mut shown = 0;
    while let Some(mut packet) = input.reader.next_packet().unwrap() {
      // One T.35 OBU per temporal unit, numbered by the frame it shows.
      let mut frame = None;
      for obu in parser.parse_packet(&packet.data).unwrap() {
        if let ObuKind::Frame(coded) = obu.kind {
          if coded.header.show_existing_frame {
            slots.show_existing(&coded.header);
          } else {
            slots.store(&coded.header, shown);
          }
          if coded.header.show_frame {
            frame = Some(shown);
            shown += 1;
          }
        }
      }
      if let Some(frame) = frame {
        let index = packet.metadata_index().unwrap();
        packet.insert_obu(index, &tagged(frame).to_obu(None)).unwrap();
      }
      output.write(&packet).unwrap();
    }
    input.finish().unwrap();
    output.finish().unwrap().commit().unwrap();

    let (_, packets) = read_all(&target);
    let found = count_t35(&packets);
    assert_eq!(found.len(), 30, "{extension}");
    assert!(found.iter().enumerate().all(|(i, payload)| payload[6] == i as u8), "{extension}");
    assert_eq!(picture(&target), reference, "{extension}");
    let (decoded, messages) = rabbit_av1::container::decode_messages(&target, "ffmpeg").unwrap();
    assert!(decoded && messages.is_empty(), "{extension}: {messages:?}");
  }
}

#[test]
fn removed_metadata_is_gone() {
  let scratch = Scratch::new("remove");
  let source = scratch.path("source.ivf");
  let Some(()) = hdr_stream(&source, 12) else { return };
  let target = scratch.path("clean.ivf");
  let mut input = Input::open(&source, "ffmpeg", None).unwrap();
  let mut output = Output::create(&target, &source, &input.reader, "ffmpeg").unwrap();
  let mut removed = 0;
  while let Some(mut packet) = input.reader.next_packet().unwrap() {
    removed += packet.retain_obus(|obu, _| obu.header.obu_type != OBU_METADATA).unwrap();
    output.write(&packet).unwrap();
  }
  input.finish().unwrap();
  output.finish().unwrap().commit().unwrap();
  assert_eq!(removed, 2);
  let (_, packets) = read_all(&target);
  assert!(
    packets
      .iter()
      .all(|packet| packet.obus().unwrap().iter().all(|obu| obu.header.obu_type != OBU_METADATA))
  );
  assert_eq!(picture(&target), picture(&source));
}

#[test]
fn annexb_keeps_frame_units_when_editing() {
  let scratch = Scratch::new("annexb");
  let source = scratch.path("source.ivf");
  let Some(()) = hdr_stream(&source, 8) else { return };
  let (_, packets) = read_all(&source);
  // Annex B with one frame unit per frame OBU, the other OBUs joining the next frame.
  let mut stream = Vec::new();
  for packet in &packets {
    let mut units = Vec::new();
    let mut count = 0;
    for obu in packet.obus().unwrap() {
      count += 1;
      if matches!(obu_type_of(packet.data[obu.prefix.start]), 3 | 6) {
        units.push(count);
        count = 0;
      }
    }
    if count > 0 {
      units.push(count);
    }
    write_annexb(&mut stream, &packet.data, &units).unwrap();
  }
  let annexb = scratch.path("source.obu");
  std::fs::write(&annexb, &stream).unwrap();
  let reference = picture(&source);
  assert_eq!(picture(&annexb), reference);

  let mut reader = VideoReader::open(Box::new(Cursor::new(stream)), None).unwrap();
  assert_eq!(reader.framing, Framing::AnnexB);
  assert!(reader.sequence().unwrap().is_pq());
  let target = scratch.path("tagged.obu");
  let mut output = Output::create(&target, &annexb, &reader, "ffmpeg").unwrap();
  let mut frame = 0;
  while let Some(mut packet) = reader.next_packet().unwrap() {
    let index = packet.metadata_index().unwrap();
    packet.insert_obu(index, &tagged(frame).to_obu(None)).unwrap();
    frame += 1;
    output.write(&packet).unwrap();
  }
  output.finish().unwrap().commit().unwrap();
  let (framing, packets) = read_all(&target);
  assert_eq!(framing, Framing::AnnexB);
  assert_eq!(count_t35(&packets).len(), 8);
  assert_eq!(picture(&target), reference);
}

#[test]
fn container_without_av1_is_unsupported_input() {
  let scratch = Scratch::new("no-video");
  let audio = scratch.path("audio.mp4");
  if !ffmpeg(&["-f", "lavfi", "-i", "sine", "-t", "1", "-c:a", "aac", audio.to_str().unwrap()]) {
    eprintln!("skipped: ffmpeg is needed");
    return;
  }
  let error = Input::open(&audio, "ffmpeg", None).err().unwrap();
  assert_eq!(error.kind(), rabbit_av1::ErrorKind::UnsupportedInput, "{error}");
  assert!(error.to_string().ends_with("the input is not an AV1 stream"), "{error}");
}

#[test]
fn metadata_type_is_reported() {
  assert_eq!(tagged(0).metadata_type(), METADATA_TYPE_ITUT_T35);
}

struct Rng(u64);

impl Rng {
  fn below(&mut self, limit: usize) -> usize {
    self.0 ^= self.0 << 13;
    self.0 ^= self.0 >> 7;
    self.0 ^= self.0 << 17;
    (self.0 % limit.max(1) as u64) as usize
  }
}

fn mutate(data: &[u8], rng: &mut Rng) -> Vec<u8> {
  let mut out = data.to_vec();
  // The 32 byte IVF header stays mostly intact so that mutations reach the AV1 data.
  let body = 32.min(out.len());
  for _ in 0..1 + rng.below(4) {
    if out.len() <= body {
      break;
    }
    let at = body + rng.below(out.len() - body);
    match rng.below(5) {
      0 => out[at] ^= 1 << rng.below(8),
      1 => out[at] = rng.below(256) as u8,
      2 => out.truncate(at),
      3 => {
        let end = (at + 1 + rng.below(64)).min(out.len());
        let copy = out[at..end].to_vec();
        let to = body + rng.below(out.len() - body);
        out.splice(to..to, copy);
      }
      _ => {
        let end = (at + 1 + rng.below(16)).min(out.len());
        out.drain(at..end);
      }
    }
  }
  out
}

// Runs a stream through parsing and editing. Errors are fine, panics are not.
fn exercise(data: &[u8]) {
  let Ok(mut reader) = VideoReader::open(Box::new(Cursor::new(data.to_vec())), None) else {
    return;
  };
  let mut parser = StreamParser::new();
  let mut slots = RefSlots::new(0u32);
  while let Ok(Some(mut packet)) = reader.next_packet() {
    let Ok(obus) = parser.parse_packet(&packet.data) else { break };
    for obu in obus {
      if let ObuKind::Frame(frame) = obu.kind {
        if frame.header.show_existing_frame {
          slots.show_existing(&frame.header);
        } else {
          slots.store(&frame.header, 1);
        }
      }
    }
    if let Ok(index) = packet.metadata_index() {
      let _ = packet.insert_obu(index, &tagged(0).to_obu(None));
    }
    let _ = packet.retain_obus(|obu, _| obu.header.obu_type != OBU_METADATA);
  }
}

fn sample_streams() -> Vec<(String, Vec<u8>)> {
  if let Some(folder) = std::env::var_os("RABBIT_AV1_FUZZ_DIR") {
    let mut streams: Vec<(String, Vec<u8>)> = std::fs::read_dir(folder)
      .unwrap()
      .filter_map(|entry| entry.ok().map(|entry| entry.path()))
      .filter(|path| path.extension().is_some_and(|ext| ext == "ivf" || ext == "obu"))
      .map(|path| (path.display().to_string(), std::fs::read(&path).unwrap()))
      .collect();
    streams.sort();
    return streams;
  }
  let scratch = Scratch::new("fuzz");
  let path = scratch.path("sample.ivf");
  match hdr_stream(&path, 20) {
    Some(()) => vec![("generated".to_string(), std::fs::read(&path).unwrap())],
    None => Vec::new(),
  }
}

#[test]
fn damaged_streams_never_panic() {
  let streams = sample_streams();
  if streams.is_empty() {
    return;
  }
  let rounds: usize = std::env::var("RABBIT_AV1_FUZZ_ROUNDS")
    .ok()
    .and_then(|value| value.parse().ok())
    .unwrap_or(3000 / streams.len())
    .max(1);
  let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
  for (name, data) in &streams {
    exercise(data);
    for round in 0..rounds {
      let damaged = mutate(data, &mut rng);
      if catch_unwind(AssertUnwindSafe(|| exercise(&damaged))).is_err() {
        let path = std::env::temp_dir().join("rabbit-av1-fuzz-crash.ivf");
        std::fs::write(&path, &damaged).unwrap();
        panic!("{name} round {round} panicked, the damaged stream is in {}", path.display());
      }
    }
  }
}
