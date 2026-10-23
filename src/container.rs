//! Files on disk: raw AV1 read and written directly, Matroska, WebM and MP4 through ffmpeg.

use crate::error::{Error, ErrorKind, Result};
use crate::framing::{Framing, VideoReader, detect, write_annexb, write_low_overhead};
use crate::ivf::{HEADER_SIZE, IvfWriter};
use crate::packet::Packet;
use std::ffi::OsString;
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

fn ffmpeg_failed(status: std::process::ExitStatus, what: &str) -> Error {
  Error::new(ErrorKind::FfmpegFailed, format!("ffmpeg failed while {what} ({status})"))
}

/// An open input video.
///
/// IVF, low overhead and Annex B files are read directly. Anything else is handed to ffmpeg,
/// which copies the first video track out as IVF without decoding it.
pub struct Input {
  /// The temporal units of the video.
  pub reader: VideoReader,
  child: Option<Child>,
}

impl Input {
  /// Opens a video file. `ffmpeg` is the program to run for containers. `fps` is passed to
  /// [`VideoReader::open`].
  pub fn open(path: &Path, ffmpeg: &str, fps: Option<(u32, u32)>) -> Result<Input> {
    if !path.is_file() {
      return Err(Error::new(
        ErrorKind::InputNotFound,
        format!("input file not found: {}", path.display()),
      ));
    }
    let file = File::open(path).map_err(|error| Error::io(format!("failed to open {}", path.display()), error))?;
    let mut buffered = BufReader::with_capacity(1 << 20, file);
    let start = buffered
      .fill_buf()
      .map_err(|error| Error::io(format!("failed to read {}", path.display()), error))?;
    if detect(start).is_some() {
      return Ok(Input {
        reader: VideoReader::open(Box::new(buffered), fps)?,
        child: None,
      });
    }
    drop(buffered);
    let mut child = Command::new(ffmpeg)
      .args(["-v", "error", "-nostdin", "-i"])
      .arg(path)
      .args(["-map", "0:v:0", "-c:v", "copy", "-f", "ivf", "-"])
      .stdin(Stdio::null())
      .stdout(Stdio::piped())
      .stderr(Stdio::inherit())
      .spawn()
      .map_err(|error| {
        Error::new(
          ErrorKind::FfmpegMissing,
          format!("failed to run {ffmpeg}, which is needed to read {}: {error}", path.display()),
        )
      })?;
    let stdout: Box<dyn BufRead> = Box::new(BufReader::with_capacity(1 << 20, child.stdout.take().unwrap()));
    match VideoReader::open(stdout, fps) {
      Ok(reader) => Ok(Input {
        reader,
        child: Some(child),
      }),
      Err(error) => {
        // The pipe is closed by now, so ffmpeg stops. When it failed, its own message is
        // already on standard error and says more than its exit status.
        let _ = child.wait();
        Err(Error::new(
          ErrorKind::UnsupportedInput,
          format!("ffmpeg could not extract AV1 video from {}: {error}", path.display()),
        ))
      }
    }
  }

  /// When reading fails while ffmpeg feeds the input, ffmpeg usually stopped first. Returns
  /// ffmpeg's failure in that case and `error` otherwise.
  pub fn explain(&mut self, error: Error) -> Error {
    if let Some(child) = self.child.as_mut() {
      self.reader.close();
      if let Ok(status) = child.wait()
        && !status.success()
      {
        return ffmpeg_failed(status, "reading the input");
      }
    }
    error
  }

  /// Closes the input and checks that ffmpeg, when it was used, succeeded.
  pub fn finish(mut self) -> Result<()> {
    self.reader.close();
    if let Some(mut child) = self.child.take() {
      let status = child.wait().map_err(|error| Error::io("failed to wait for ffmpeg", error))?;
      if !status.success() {
        return Err(ffmpeg_failed(status, "reading the input"));
      }
    }
    Ok(())
  }
}

impl Drop for Input {
  fn drop(&mut self) {
    if let Some(mut child) = self.child.take() {
      self.reader.close();
      let _ = child.kill();
      let _ = child.wait();
    }
  }
}

fn part_path(path: &Path, tag: &str) -> Result<PathBuf> {
  let mut name = path
    .file_stem()
    .ok_or_else(|| Error::new(ErrorKind::InvalidArgument, "output path has no file name"))?
    .to_os_string();
  name.push(tag);
  if let Some(extension) = path.extension() {
    name.push(".");
    name.push(extension);
  }
  Ok(path.with_file_name(name))
}

fn create(target: &Path) -> Result<BufWriter<File>> {
  let file = File::create(target).map_err(|error| Error::io(format!("failed to create {}", target.display()), error))?;
  Ok(BufWriter::with_capacity(1 << 20, file))
}

enum Writer {
  Ivf(IvfWriter<BufWriter<File>>),
  Raw { file: BufWriter<File>, annexb: bool },
}

// Containers are muxed by ffmpeg from a finished video file, so ffmpeg can interleave
// the tracks by time instead of by how fast each one arrives.
struct Container {
  video: PathBuf,
  format: &'static str,
  source: PathBuf,
  ffmpeg: String,
  extra_args: Vec<OsString>,
}

/// An output video, written under a temporary name until it is committed.
///
/// The extension of the path picks the format: `.ivf`, `.obu` (the framing of the input, low
/// overhead or Annex B), or `.mkv`, `.webm`, `.mp4` and `.m4v`, which ffmpeg muxes together
/// with the audio, subtitles, attachments, chapters and metadata of the source when it is a container.
pub struct Output {
  writer: Writer,
  container: Option<Container>,
  temp: PathBuf,
  path: PathBuf,
}

impl Output {
  /// Creates the output. `source` is the input path, `reader` its reader.
  pub fn create(path: &Path, source: &Path, reader: &VideoReader, ffmpeg: &str) -> Result<Output> {
    let extension = path.extension().and_then(|ext| ext.to_str()).unwrap_or("").to_ascii_lowercase();
    let temp = part_path(path, ".part")?;
    let ivf = |target: &Path, header: &[u8; HEADER_SIZE]| -> Result<Writer> { Ok(Writer::Ivf(IvfWriter::new(create(target)?, header)?)) };
    let (writer, container) = match extension.as_str() {
      "ivf" => (ivf(&temp, reader.ivf_header())?, None),
      "obu" => (
        Writer::Raw {
          file: create(&temp)?,
          annexb: reader.framing == Framing::AnnexB,
        },
        None,
      ),
      "mkv" | "webm" | "mp4" | "m4v" => {
        let format = match extension.as_str() {
          "mkv" => "matroska",
          "webm" => "webm",
          _ => "mp4",
        };
        let mut video = part_path(path, ".part.video")?;
        video.set_extension("ivf");
        let writer = ivf(&video, reader.ivf_header())?;
        let container = Container {
          video,
          format,
          source: source.to_path_buf(),
          ffmpeg: ffmpeg.to_string(),
          extra_args: Vec::new(),
        };
        (writer, Some(container))
      }
      other => {
        return Err(Error::new(
          ErrorKind::UnsupportedOutput,
          format!("unsupported output extension: .{other}"),
        ));
      }
    };
    Ok(Output {
      writer,
      container,
      temp,
      path: path.to_path_buf(),
    })
  }

  /// True when ffmpeg muxes the output into a container.
  pub fn is_container(&self) -> bool {
    self.container.is_some()
  }

  /// Adds arguments to the ffmpeg command that muxes a container, placed just before the
  /// output file. They have no effect on raw outputs.
  pub fn mux_args<I, S>(&mut self, args: I)
  where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
  {
    if let Some(container) = &mut self.container {
      container.extra_args.extend(args.into_iter().map(Into::into));
    }
  }

  /// Writes one temporal unit. `units` is needed for Annex B output.
  pub fn write_packet(&mut self, pts: u64, data: &[u8], units: Option<&[usize]>) -> Result<()> {
    match &mut self.writer {
      Writer::Ivf(writer) => writer.write_packet(pts, data),
      Writer::Raw { file, annexb: true } => {
        let units = units.ok_or_else(|| Error::new(ErrorKind::UnsupportedOutput, "Annex B output needs Annex B input"))?;
        write_annexb(file, data, units)
      }
      Writer::Raw { file, annexb: false } => write_low_overhead(file, data),
    }
  }

  /// Writes one packet.
  pub fn write(&mut self, packet: &Packet) -> Result<()> {
    self.write_packet(packet.pts, &packet.data, packet.units.as_deref())
  }

  fn mux(container: &Container, temp: &Path) -> Result<()> {
    let mut command = Command::new(&container.ffmpeg);
    command.args(["-v", "error", "-nostdin", "-y", "-i"]).arg(&container.video);
    let source_is_container = File::open(&container.source)
      .ok()
      .and_then(|file| BufReader::new(file).fill_buf().ok().map(|start| detect(start).is_none()))
      .unwrap_or(false);
    if source_is_container {
      command.arg("-i").arg(&container.source);
      command.args([
        "-map",
        "0:v:0",
        "-map",
        "1:a?",
        "-map",
        "1:s?",
        "-map",
        "1:t?",
        "-map_metadata",
        "1",
        "-map_chapters",
        "1",
      ]);
    } else {
      command.args(["-map", "0:v:0"]);
    }
    command.args(["-c", "copy", "-max_interleave_delta", "0"]);
    command.args(&container.extra_args);
    command.args(["-f", container.format]).arg(temp);
    let output = command
      .stdin(Stdio::null())
      .stdout(Stdio::null())
      .stderr(Stdio::piped())
      .output()
      .map_err(|error| {
        Error::new(
          ErrorKind::FfmpegMissing,
          format!(
            "failed to run {}, which is needed to write {}: {error}",
            container.ffmpeg,
            temp.display()
          ),
        )
      })?;
    if !output.status.success() {
      let message = String::from_utf8_lossy(&output.stderr);
      return Err(Error::new(
        ErrorKind::FfmpegFailed,
        format!(
          "ffmpeg failed while writing the output ({}): {}",
          output.status,
          message.lines().last().unwrap_or("")
        ),
      ));
    }
    Ok(())
  }

  /// Flushes the video and muxes the container. The result still sits under its temporary
  /// name until [`Written::commit`].
  pub fn finish(self) -> Result<Written> {
    let result = (|| -> Result<()> {
      match self.writer {
        Writer::Ivf(writer) => drop(writer.finish()?),
        Writer::Raw { mut file, .. } => file.flush().map_err(|error| Error::io("failed to write the output", error))?,
      }
      if let Some(container) = &self.container {
        Self::mux(container, &self.temp)?;
      }
      Ok(())
    })();
    if let Some(container) = &self.container {
      let _ = std::fs::remove_file(&container.video);
    }
    let written = Written {
      temp: self.temp,
      path: self.path,
    };
    match result {
      Ok(()) => Ok(written),
      Err(error) => {
        written.discard();
        Err(error)
      }
    }
  }

  /// Stops writing and removes the temporary files.
  pub fn discard(self) {
    drop(self.writer);
    if let Some(container) = &self.container {
      let _ = std::fs::remove_file(&container.video);
    }
    let _ = std::fs::remove_file(&self.temp);
  }
}

/// A finished output that still sits under its temporary name until it is committed.
pub struct Written {
  /// The temporary path, which can be read to check the output before committing it.
  pub temp: PathBuf,
  path: PathBuf,
}

impl Written {
  /// Moves the output to its final path, replacing what is there.
  pub fn commit(self) -> Result<()> {
    let result = std::fs::rename(&self.temp, &self.path)
      .map_err(|error| Error::io(format!("failed to move the output to {}", self.path.display()), error));
    if result.is_err() {
      let _ = std::fs::remove_file(&self.temp);
    }
    result
  }

  /// Removes the output.
  pub fn discard(self) {
    let _ = std::fs::remove_file(&self.temp);
  }
}

/// Decodes the first video track of a file with ffmpeg's libdav1d decoder.
///
/// Returns whether decoding succeeded and the decoder's messages without the bracketed prefixes ffmpeg puts in front.
pub fn decode_messages(path: &Path, ffmpeg: &str) -> Result<(bool, Vec<String>)> {
  decode_messages_with_progress(path, ffmpeg, |_| {})
}

/// Like [`decode_messages`], and calls `progress` with the number of frames decoded so far, about twice a second
/// as ffmpeg reports it.
pub fn decode_messages_with_progress(path: &Path, ffmpeg: &str, mut progress: impl FnMut(u64)) -> Result<(bool, Vec<String>)> {
  let mut child = Command::new(ffmpeg)
    .args([
      "-v",
      "error",
      "-nostdin",
      "-nostats",
      "-progress",
      "pipe:1",
      "-c:v",
      "libdav1d",
      "-i",
    ])
    .arg(path)
    .args(["-map", "0:V:0", "-f", "null", "-"])
    .stdin(Stdio::null())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()
    .map_err(|error| {
      Error::new(
        ErrorKind::FfmpegMissing,
        format!("failed to run {ffmpeg} to decode the output: {error}"),
      )
    })?;
  // Read on a thread of its own, so a decoder with many messages cannot stall on a full pipe.
  let mut stderr = child.stderr.take().expect("stderr is piped");
  let messages = std::thread::spawn(move || {
    let mut text = Vec::new();
    let _ = std::io::Read::read_to_end(&mut stderr, &mut text);
    String::from_utf8_lossy(&text).into_owned()
  });
  let stdout = child.stdout.take().expect("stdout is piped");
  for line in BufReader::new(stdout).lines().map_while(std::result::Result::ok) {
    if let Some(frames) = line.strip_prefix("frame=").and_then(|value| value.trim().parse().ok()) {
      progress(frames);
    }
  }
  let status = child
    .wait()
    .map_err(|error| Error::new(ErrorKind::FfmpegFailed, format!("{ffmpeg} did not finish: {error}")))?;
  let messages = messages.join().unwrap_or_default();
  if messages.contains("libdav1d") && (messages.contains("Unknown decoder") || messages.contains("Decoder not found")) {
    return Err(Error::new(ErrorKind::DecoderMissing, format!("{ffmpeg} has no libdav1d decoder")));
  }
  let lines = messages
    .lines()
    .filter(|line| !line.trim().is_empty() && !line.starts_with("[null @") && !line.contains("Last message repeated"))
    .map(|line| {
      let mut rest = line.trim();
      while rest.starts_with('[') {
        match rest.find(']') {
          Some(end) => rest = rest[end + 1..].trim_start(),
          None => break,
        }
      }
      rest.to_string()
    })
    .collect();
  Ok((status.success(), lines))
}

#[cfg(test)]
mod tests {
  use super::*;

  fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rabbit-av1-test-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
  }

  #[test]
  fn missing_input_and_unknown_output() {
    let error = Input::open(Path::new("/does/not/exist.ivf"), "ffmpeg", None).err().unwrap();
    assert_eq!(error.kind(), ErrorKind::InputNotFound);

    let dir = scratch("output");
    let source = dir.join("in.obu");
    std::fs::write(&source, [0x12, 0x00]).unwrap();
    let input = Input::open(&source, "ffmpeg", None).unwrap();
    let error = Output::create(&dir.join("out.avi"), &source, &input.reader, "ffmpeg")
      .err()
      .unwrap();
    assert_eq!(error.kind(), ErrorKind::UnsupportedOutput);
    std::fs::remove_dir_all(dir).unwrap();
  }

  #[test]
  fn raw_output_commits_under_its_name() {
    let dir = scratch("commit");
    let source = dir.join("in.obu");
    std::fs::write(&source, [0x12, 0x00, 0x12, 0x00]).unwrap();
    let mut input = Input::open(&source, "ffmpeg", None).unwrap();
    let target = dir.join("out.ivf");
    let mut output = Output::create(&target, &source, &input.reader, "ffmpeg").unwrap();
    assert!(!output.is_container());
    while let Some(packet) = input.reader.next_packet().unwrap() {
      output.write(&packet).unwrap();
    }
    input.finish().unwrap();
    let written = output.finish().unwrap();
    assert!(written.temp.ends_with("out.part.ivf"));
    assert!(!target.exists());
    written.commit().unwrap();
    assert_eq!(std::fs::read(&target).unwrap().len(), 32 + 2 * (12 + 2));
    std::fs::remove_dir_all(dir).unwrap();
  }
}
