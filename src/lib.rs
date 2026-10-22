//! Reading and rewriting AV1 bitstreams without decoding them.
//!
//! `rabbit-av1` parses AV1 down to the uncompressed frame header, so tools can change what
//! sits around the coded pictures without touching them: film grain parameters, HDR metadata
//! and other metadata OBUs, or the colour description of the sequence header.
//!
//! - [`StreamParser`] reads sequence headers, frame headers with their film grain, and
//!   metadata OBUs, keeping the reference state that frame headers depend on.
//! - [`Metadata`] reads and writes HDR content light level, mastering display colour volume and
//!   ITU-T T.35 metadata, the carrier of HDR10+ and Dolby Vision.
//! - [`Packet`] inserts and removes OBUs in a temporal unit, and [`RefSlots`] follows values
//!   through the reference slots for frames that `show_existing_frame` shows again.
//! - [`VideoReader`] reads IVF, low overhead (`.obu`) and Annex B streams without any other
//!   program. [`Input`] and [`Output`] also read and write Matroska, WebM and MP4 by running
//!   `ffmpeg`, which copies the video track without decoding it.
//!
//! The crate depends on nothing but the standard library.
//!
//! # Example
//!
//! Lists the HDR metadata of every shown frame:
//!
//! ```no_run
//! use rabbit_av1::{Input, Metadata, ObuKind, StreamParser};
//! use std::path::Path;
//!
//! # fn main() -> rabbit_av1::Result<()> {
//! let mut input = Input::open(Path::new("video.mkv"), "ffmpeg", None)?;
//! let mut parser = StreamParser::new();
//! while let Some(packet) = input.reader.next_packet()? {
//!   for obu in parser.parse_packet(&packet.data)? {
//!     match obu.kind {
//!       ObuKind::Metadata(Metadata::ContentLightLevel(cll)) => println!("MaxCLL {} MaxFALL {}", cll.max_cll, cll.max_fall),
//!       ObuKind::Metadata(Metadata::ItuTT35(t35)) => println!("T.35 metadata from provider {:?}", t35.provider_code()),
//!       ObuKind::Frame(frame) if frame.header.show_frame => println!("frame at {}", packet.pts),
//!       _ => {}
//!     }
//!   }
//! }
//! input.finish()?;
//! # Ok(())
//! # }
//! ```

#![warn(missing_docs)]

pub mod bits;
pub mod container;
pub mod error;
pub mod frame;
pub mod framing;
pub mod grain;
pub mod ivf;
pub mod metadata;
pub mod obu;
pub mod packet;
pub mod parser;
pub mod sequence;
pub mod slots;

pub use container::{Input, Output, Written};
pub use error::{Error, ErrorKind, Result};
pub use frame::{FrameHeader, FrameState};
pub use framing::{Framing, VideoReader};
pub use grain::GrainParams;
pub use metadata::{ContentLightLevel, ItuTT35, MasteringDisplay, Metadata};
pub use obu::{ObuExtension, ObuHeader, RawObu};
pub use packet::Packet;
pub use parser::{CodedFrame, CodedGrain, Obu, ObuKind, StreamParser};
pub use sequence::{ChromaFormat, SequenceHeader};
pub use slots::RefSlots;

#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeExamples;
