# rabbit-av1

[![crates.io](https://img.shields.io/crates/v/rabbit-av1.svg)](https://crates.io/crates/rabbit-av1)
[![docs.rs](https://docs.rs/rabbit-av1/badge.svg)](https://docs.rs/rabbit-av1)

rabbit-av1 reads and rewrites AV1 bitstreams without decoding them. It parses AV1 down to the uncompressed frame header, so a tool can change what sits around the coded pictures and leave the pictures bit for bit as they were.

It is the shared base of [regrain](https://github.com/Rabbit-Company/regrain), which puts film grain synthesis into AV1 video, and [retone](https://github.com/Rabbit-Company/retone), which does the same for HDR metadata.

| Part              | What it does                                                                                                      |
| ----------------- | ----------------------------------------------------------------------------------------------------------------- |
| `StreamParser`    | Reads sequence headers, frame headers with their film grain, and metadata OBUs, one temporal unit at a time       |
| `SequenceHeader`  | Profile, levels, frame size, bit depth, chroma layout and colour description (primaries, transfer, matrix, range) |
| `Metadata`        | Reads and writes HDR content light level, mastering display colour volume and ITU-T T.35 metadata                 |
| `GrainParams`     | Reads, writes and checks film grain parameters                                                                    |
| `Packet`          | Inserts and removes OBUs in a temporal unit, keeping Annex B frame units intact                                   |
| `RefSlots`        | Follows values through the reference slots, for frames that `show_existing_frame` shows again                     |
| `VideoReader`     | Reads IVF, low overhead (`.obu`) and Annex B streams                                                              |
| `Input`, `Output` | Open and write video files, using `ffmpeg` for Matroska, WebM and MP4                                             |

The crate depends on nothing but the standard library.

## Installing

```sh
cargo add rabbit-av1
```

The minimum supported Rust version is 1.88.

## Reading HDR metadata

```rust,no_run
use rabbit_av1::{Input, Metadata, ObuKind, StreamParser};
use std::path::Path;

fn main() -> rabbit_av1::Result<()> {
  let mut input = Input::open(Path::new("video.mkv"), "ffmpeg", None)?;
  let mut parser = StreamParser::new();
  while let Some(packet) = input.reader.next_packet()? {
    for obu in parser.parse_packet(&packet.data)? {
      match obu.kind {
        ObuKind::Metadata(Metadata::ContentLightLevel(cll)) => {
          println!("MaxCLL {} MaxFALL {}", cll.max_cll, cll.max_fall)
        }
        ObuKind::Metadata(Metadata::MasteringDisplay(mdcv)) => {
          println!("mastered at {} to {} cd/m²", mdcv.min_nits(), mdcv.max_nits())
        }
        _ => {}
      }
    }
  }
  if let Some(seq) = parser.sequence() {
    println!("PQ: {}, {} bit", seq.is_pq(), seq.bit_depth);
  }
  input.finish()
}
```

## Writing metadata into a video

Metadata OBUs belong after the temporal delimiter and sequence header and before the first frame of a temporal unit. `Packet::metadata_index` finds that place.

```rust,no_run
use rabbit_av1::metadata::{MasteringDisplay, PRIMARIES_BT_2020, WHITE_D65};
use rabbit_av1::obu::OBU_METADATA;
use rabbit_av1::{ContentLightLevel, Input, Metadata, Output};
use std::path::Path;

fn main() -> rabbit_av1::Result<()> {
  let source = Path::new("encode.mkv");
  let mut input = Input::open(source, "ffmpeg", None)?;
  let mut output = Output::create(Path::new("hdr10.mkv"), source, &input.reader, "ffmpeg")?;

  let cll = Metadata::ContentLightLevel(ContentLightLevel { max_cll: 1000, max_fall: 400 }).to_obu(None);
  let mdcv = Metadata::MasteringDisplay(MasteringDisplay::from_values(PRIMARIES_BT_2020, WHITE_D65, 1000.0, 0.0001)?).to_obu(None);

  while let Some(mut packet) = input.reader.next_packet()? {
    // Drop the metadata the encoder wrote and put the new values in its place.
    packet.retain_obus(|obu, _| obu.header.obu_type != OBU_METADATA)?;
    let index = packet.metadata_index()?;
    packet.insert_obus(index, &[&cll, &mdcv])?;
    output.write(&packet)?;
  }
  input.finish()?;
  output.finish()?.commit()
}
```

Static HDR10 metadata is only needed with each sequence header, while HDR10+ goes with every shown frame. `RefSlots` tells which frame a `show_existing_frame` header shows again, so that frame can get the metadata it was given when it was coded.

## Files

| Framing                  | Extension                       | Read             | Written          |
| ------------------------ | ------------------------------- | ---------------- | ---------------- |
| IVF                      | `.ivf`                          | directly         | directly         |
| Low overhead (Section 5) | `.obu`                          | directly         | directly         |
| Annex B                  | `.obu`                          | directly         | directly         |
| Matroska, WebM, MP4      | `.mkv`, `.webm`, `.mp4`, `.m4v` | through `ffmpeg` | through `ffmpeg` |

The framing of a raw stream is detected from its first bytes. A `.obu` output keeps the framing of the input.

ffmpeg copies the video track without decoding it. When the output is a container and the input was one too, its audio, subtitles, attachments, chapters and metadata are copied as well. `Output::mux_args` adds more ffmpeg arguments to that step. `Output` writes under a temporary name, and `Written::commit` moves the file into place, so a failed run leaves no half written file behind.

`container::decode_messages` decodes a file with ffmpeg's libdav1d decoder and returns what the decoder complained about, to check an output before committing it.

## Errors

Every error has a kind with a stable code that stays the same across releases:

| Code                 | Meaning                                                  |
| -------------------- | -------------------------------------------------------- |
| `invalid_bitstream`  | The AV1 data breaks the bitstream syntax or is truncated |
| `unsupported_input`  | The input is not AV1, or ffmpeg found no AV1 track in it |
| `unsupported_output` | The output extension or framing cannot be written        |
| `input_not_found`    | The input file does not exist                            |
| `invalid_argument`   | A value passed in is out of range                        |
| `io_error`           | Reading or writing a file or pipe failed                 |
| `ffmpeg_missing`     | ffmpeg could not be started                              |
| `ffmpeg_failed`      | ffmpeg reported a failure                                |
| `decoder_missing`    | ffmpeg has no libdav1d decoder                           |

Damaged input gives an error and never a panic.

## Testing

```sh
cargo test --release
```

The tests that need real streams make them with `ffmpeg` and SVT-AV1 and are skipped when those are missing. They write metadata into every output format and check that dav1d decodes the same pictures as from the source.

CI also parses each of the 242 libaom test vectors, and checks that it finds as many shown pictures as dav1d decodes:

```sh
cargo build --release --example av1dump
scripts/conformance.sh <folder with the vectors>
```

Then it damages the vectors in many random ways and checks that parsing and editing them never panics:

```sh
RABBIT_AV1_FUZZ_DIR=<folder with the vectors> cargo test --release damaged_streams
```

The `av1dump` example prints the sequence header, the metadata and a frame summary of any AV1 file:

```sh
cargo run --release --example av1dump -- video.mkv --frames
```

## Licence

MIT
