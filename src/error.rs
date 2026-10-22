//! The error type.

use std::fmt::{self, Display};

/// What kind of failure an [`Error`] is.
///
/// Every kind has a stable [`code`](ErrorKind::code) that programs can match on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ErrorKind {
  /// The AV1 data breaks the bitstream syntax or is truncated.
  InvalidBitstream,
  /// The input is not AV1, or uses something this crate does not read.
  UnsupportedInput,
  /// The output file extension or framing cannot be written.
  UnsupportedOutput,
  /// The input file does not exist.
  InputNotFound,
  /// A value passed in by the caller is out of range.
  InvalidArgument,
  /// Reading or writing a file or pipe failed.
  Io,
  /// ffmpeg could not be started.
  FfmpegMissing,
  /// ffmpeg ran but reported a failure.
  FfmpegFailed,
  /// ffmpeg has no libdav1d decoder.
  DecoderMissing,
}

impl ErrorKind {
  /// A short snake_case name that stays the same across releases.
  pub fn code(self) -> &'static str {
    match self {
      ErrorKind::InvalidBitstream => "invalid_bitstream",
      ErrorKind::UnsupportedInput => "unsupported_input",
      ErrorKind::UnsupportedOutput => "unsupported_output",
      ErrorKind::InputNotFound => "input_not_found",
      ErrorKind::InvalidArgument => "invalid_argument",
      ErrorKind::Io => "io_error",
      ErrorKind::FfmpegMissing => "ffmpeg_missing",
      ErrorKind::FfmpegFailed => "ffmpeg_failed",
      ErrorKind::DecoderMissing => "decoder_missing",
    }
  }
}

/// The error type of every fallible function in this crate.
///
/// The message is complete on its own: for I/O failures it already includes what the
/// operating system reported, so [`source`](std::error::Error::source) is always `None` and
/// printers that walk the source chain do not repeat it. [`io_kind`](Error::io_kind) gives the
/// kind of the I/O failure.
#[derive(Debug)]
pub struct Error {
  kind: ErrorKind,
  message: String,
  io_kind: Option<std::io::ErrorKind>,
}

/// A `Result` with [`Error`] as its error type.
pub type Result<T> = std::result::Result<T, Error>;

impl Error {
  /// Creates an error of the given kind.
  pub fn new(kind: ErrorKind, message: impl Display) -> Self {
    Error {
      kind,
      message: message.to_string(),
      io_kind: None,
    }
  }

  pub(crate) fn bitstream(message: impl Display) -> Self {
    Error::new(ErrorKind::InvalidBitstream, message)
  }

  pub(crate) fn io(message: impl Display, source: std::io::Error) -> Self {
    Error {
      kind: ErrorKind::Io,
      message: format!("{message}: {source}"),
      io_kind: Some(source.kind()),
    }
  }

  /// The kind of failure.
  pub fn kind(&self) -> ErrorKind {
    self.kind
  }

  /// The kind of I/O failure, for errors of kind [`ErrorKind::Io`].
  pub fn io_kind(&self) -> Option<std::io::ErrorKind> {
    self.io_kind
  }

  /// The stable code of the kind, the same as `self.kind().code()`.
  pub fn code(&self) -> &'static str {
    self.kind.code()
  }

  /// Puts a description of what was being done in front of the message, keeping the kind.
  pub fn context(mut self, context: impl Display) -> Self {
    self.message = format!("{context}: {}", self.message);
    self
  }
}

impl Display for Error {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.write_str(&self.message)
  }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
  fn from(error: std::io::Error) -> Self {
    Error {
      kind: ErrorKind::Io,
      message: error.to_string(),
      io_kind: Some(error.kind()),
    }
  }
}

pub(crate) trait ResultExt<T> {
  fn context(self, context: impl Display) -> Result<T>;
}

impl<T> ResultExt<T> for Result<T> {
  fn context(self, context: impl Display) -> Result<T> {
    self.map_err(|error| error.context(context))
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn context_keeps_the_kind() {
    let error = Error::bitstream("unexpected end of OBU data").context("packet 3");
    assert_eq!(error.kind(), ErrorKind::InvalidBitstream);
    assert_eq!(error.code(), "invalid_bitstream");
    assert_eq!(error.to_string(), "packet 3: unexpected end of OBU data");
  }

  #[test]
  fn io_errors_name_their_cause_once() {
    let error = Error::io("failed to read", std::io::Error::other("disk gone"));
    assert_eq!(error.code(), "io_error");
    assert_eq!(error.io_kind(), Some(std::io::ErrorKind::Other));
    assert_eq!(error.to_string(), "failed to read: disk gone");
    assert!(std::error::Error::source(&error).is_none());
  }
}
