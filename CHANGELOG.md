# Changelog

Error codes stay compatible within a major version. Messages meant for people may change in any release. Before 1.0, a minor version may change the API.

## 0.1.0 - 2026-10-22

First release: the AV1 parsing of regrain 0.2.3, as a library of its own.

### Features

- `StreamParser` reads sequence headers, frame headers up to and including film grain, and metadata OBUs, and keeps the reference state that frame headers depend on.
- `SequenceHeader` holds the profile, the level and tier of every operating point, the colour description and the bit positions of the colour description and the film grain flag.
- `Metadata` reads and writes `METADATA_TYPE_HDR_CLL`, `METADATA_TYPE_HDR_MDCV` and `METADATA_TYPE_ITUT_T35`. Other metadata types are kept byte for byte.
- `Packet` inserts and removes OBUs in a temporal unit and keeps Annex B frame units intact. `Packet::metadata_index` finds where metadata OBUs belong.
- `RefSlots` follows a value through the reference slots, for frames that `show_existing_frame` shows again.
- `GrainParams` reads, writes, normalizes and checks film grain parameters.
- `VideoReader` reads IVF, low overhead and Annex B streams and detects which one it has. `Input` and `Output` add Matroska, WebM and MP4 through ffmpeg.
- `Error` has a kind with a stable code.
- The `av1dump` example prints the sequence header, the metadata and a frame summary of an AV1 file.
