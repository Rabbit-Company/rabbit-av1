//! Frame times as an encoder counts them.

/// Frame rates that constant frame rate video is checked against when its time stamps are rounded.
const COMMON_FRAME_RATES: [(u64, u64); 16] = [
  (24000, 1001),
  (24, 1),
  (25, 1),
  (30000, 1001),
  (30, 1),
  (48000, 1001),
  (48, 1),
  (50, 1),
  (60000, 1001),
  (60, 1),
  (100, 1),
  (120000, 1001),
  (120, 1),
  (144, 1),
  (240, 1),
  (15, 1),
];

/// Turns packet time stamps into the times an encoder such as libaom gives frames: counted from the
/// first frame and rounded down.
///
/// Containers can round time stamps. Matroska keeps milliseconds, so at 24000/1001 the frame at
/// 150.40025 s is stored as 150.400 s. When every frame sits within one time base unit of a frame
/// slot of a constant frame rate, the timeline uses the exact time of that slot, which undoes the
/// rounding. Otherwise it uses the time stamps as they are.
///
/// ```
/// use rabbit_av1::Timeline;
///
/// // A minute of 24000/1001 video in Matroska, which starts at 10 s and keeps milliseconds.
/// let pts: Vec<u64> = (0..1440).map(|n| 10_000 + (n * 1_001_000 + 12_000) / 24_000).collect();
/// let timeline = Timeline::new(&pts, 1000, 1, None);
/// assert_eq!(timeline.frame_rate(), Some((24000, 1001)));
/// assert_eq!(timeline.time(pts[0], 10_000_000), 0);
/// assert_eq!(timeline.time(pts[1], 10_000_000), 417_083);
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Timeline {
  first: u64,
  rate: u64,
  scale: u64,
  frame_rate: Option<(u64, u64)>,
}

impl Timeline {
  /// Builds the timeline from the time stamps of the shown frames, in any order, and the time base
  /// `scale / rate` seconds of [`VideoReader`](crate::VideoReader). `frame_rate` is checked along with
  /// common frame rates, and the lowest that fits wins.
  pub fn new(pts: &[u64], rate: u32, scale: u32, frame_rate: Option<(u32, u32)>) -> Timeline {
    let mut sorted = pts.to_vec();
    sorted.sort_unstable();
    let first = sorted.first().copied().unwrap_or(0);
    let (rate, scale) = (rate.max(1) as u64, scale.max(1) as u64);
    Timeline {
      first,
      rate,
      scale,
      frame_rate: constant_rate(&sorted, rate, scale, frame_rate),
    }
  }

  /// The frame rate as `(numerator, denominator)` when the frames follow a constant frame rate.
  pub fn frame_rate(&self) -> Option<(u64, u64)> {
    self.frame_rate
  }

  /// The time of the frame with time stamp `pts`, in units of `1 / units_per_second` seconds from the
  /// first frame, rounded down.
  pub fn time(&self, pts: u64, units_per_second: u64) -> u64 {
    let offset = pts.saturating_sub(self.first) as u128;
    let units = units_per_second as u128;
    let time = match self.frame_rate {
      Some((num, den)) => {
        let frame = (den * self.rate) as u128;
        let position = offset * (self.scale * num) as u128;
        let index = (position + frame / 2) / frame;
        index * den as u128 * units / num as u128
      }
      None => offset * self.scale as u128 * units / self.rate as u128,
    };
    time as u64
  }

  /// How far rounding in the container can have moved a time from [`time`](Self::time), in the same
  /// units: nothing at a constant frame rate, otherwise half a time base unit, rounded up.
  pub fn rounding(&self, units_per_second: u64) -> u64 {
    match self.frame_rate {
      Some(_) => 0,
      None => ((self.scale * units_per_second) as u128).div_ceil(2 * self.rate as u128) as u64,
    }
  }
}

// The lowest frame rate whose slots every frame fits, since multiples of it, such as 48 for 24 or the
// time base itself, fit too. Equal steps give the frame rate exactly.
fn constant_rate(sorted: &[u64], rate: u64, scale: u64, hint: Option<(u32, u32)>) -> Option<(u64, u64)> {
  if sorted.len() < 2 {
    return None;
  }
  let step = sorted[1] - sorted[0];
  let mut candidates = Vec::new();
  if step > 0 && sorted.windows(2).all(|pair| pair[1] - pair[0] == step) {
    candidates.push(reduce(rate, scale * step));
  }
  if let Some((num, den)) = hint.filter(|&(num, den)| num > 0 && den > 0) {
    candidates.push(reduce(num as u64, den as u64));
  }
  candidates.extend(COMMON_FRAME_RATES);
  let lower = |(a, b): (u64, u64), (c, d): (u64, u64)| (a as u128 * d as u128) < (c as u128 * b as u128);
  candidates
    .into_iter()
    .filter(|&candidate| fits(sorted, rate, scale, candidate))
    .reduce(|best, candidate| if lower(candidate, best) { candidate } else { best })
}

// Whether every frame is within one time base unit of a slot of its own. Positions are in units of
// 1 / (rate * num) seconds.
fn fits(sorted: &[u64], rate: u64, scale: u64, (num, den): (u64, u64)) -> bool {
  let unit = (scale * num) as u128;
  let frame = (den * rate) as u128;
  let tolerance = if 2 * unit < frame { unit } else { 0 };
  let mut last = None;
  for &pts in sorted {
    let position = (pts - sorted[0]) as u128 * unit;
    let index = (position + frame / 2) / frame;
    if position.abs_diff(index * frame) > tolerance || last == Some(index) {
      return false;
    }
    last = Some(index);
  }
  true
}

fn reduce(num: u64, den: u64) -> (u64, u64) {
  let (mut a, mut b) = (num, den);
  while b != 0 {
    (a, b) = (b, a % b);
  }
  (num / a, den / a)
}

#[cfg(test)]
mod tests {
  use super::*;

  const TICKS: u64 = 10_000_000;

  fn milliseconds(frames: u64, num: u64, den: u64, start: u64) -> Vec<u64> {
    (0..frames).map(|n| start + (n * den * 1000 * 2 + num) / (2 * num)).collect()
  }

  #[test]
  fn matroska_milliseconds_give_exact_frame_times() {
    let pts = milliseconds(3608, 24000, 1001, 0);
    let timeline = Timeline::new(&pts, 1000, 1, None);
    assert_eq!(timeline.frame_rate(), Some((24000, 1001)));
    assert_eq!(pts[3606], 150_400);
    assert_eq!(timeline.time(pts[3606], TICKS), 1_504_002_500);
    assert_eq!(timeline.time(pts[3607], TICKS), 1_504_419_583);
    assert_eq!(timeline.rounding(TICKS), 0);
  }

  #[test]
  fn exact_time_bases_round_down() {
    let pts: Vec<u64> = (0..12).collect();
    let timeline = Timeline::new(&pts, 24000, 1001, None);
    assert_eq!(timeline.frame_rate(), Some((24000, 1001)));
    assert_eq!(timeline.time(7, TICKS), 2_919_583);
    let pts: Vec<u64> = (0..12).map(|n| n * 1001).collect();
    assert_eq!(Timeline::new(&pts, 24000, 1, None).time(8008, TICKS), 3_336_666);
  }

  #[test]
  fn times_count_from_the_first_frame() {
    let pts: Vec<u64> = (3..15).collect();
    let timeline = Timeline::new(&pts, 24000, 1001, None);
    assert_eq!(timeline.time(3, TICKS), 0);
    assert_eq!(timeline.time(9, TICKS), 2_502_500);
    let pts = milliseconds(100, 24000, 1001, 10_000);
    assert_eq!(Timeline::new(&pts, 1000, 1, None).time(pts[6], TICKS), 2_502_500);
  }

  #[test]
  fn a_skipped_frame_keeps_the_frame_rate() {
    let mut pts = milliseconds(100, 24000, 1001, 0);
    pts.remove(3);
    let timeline = Timeline::new(&pts, 1000, 1, None);
    assert_eq!(timeline.frame_rate(), Some((24000, 1001)));
    assert_eq!(timeline.time(pts[3], TICKS), 4 * 1001 * TICKS / 24000);
  }

  #[test]
  fn variable_frame_rate_keeps_the_time_stamps() {
    let pts = [0, 40, 83, 100, 150, 190, 240];
    let timeline = Timeline::new(&pts, 1000, 1, None);
    assert_eq!(timeline.frame_rate(), None);
    assert_eq!(timeline.time(83, TICKS), 830_000);
    assert_eq!(timeline.rounding(TICKS), 5_000);
    let repeated = [0, 1, 2, 2, 4, 5];
    assert_eq!(Timeline::new(&repeated, 24000, 1001, None).frame_rate(), None);
  }

  #[test]
  fn the_lowest_fitting_frame_rate_wins() {
    let pts = milliseconds(200, 24, 1, 0);
    assert_eq!(Timeline::new(&pts, 1000, 1, None).frame_rate(), Some((24, 1)));
    let pts = milliseconds(200, 12000, 1001, 0);
    assert_eq!(Timeline::new(&pts, 1000, 1, Some((12000, 1001))).frame_rate(), Some((12000, 1001)));
    // The time base itself fits every time stamp, and so does every multiple of the frame rate.
    let pts = milliseconds(200, 24000, 1001, 0);
    assert_eq!(Timeline::new(&pts, 1000, 1, Some((1000, 1))).frame_rate(), Some((24000, 1001)));
    // A few frames fit 24 as well as 24000/1001.
    assert_eq!(Timeline::new(&pts[..5], 1000, 1, None).frame_rate(), Some((24000, 1001)));
  }

  #[test]
  fn too_few_frames_have_no_frame_rate() {
    let timeline = Timeline::new(&[500], 1000, 1, None);
    assert_eq!(timeline.frame_rate(), None);
    assert_eq!(timeline.time(500, TICKS), 0);
    assert_eq!(Timeline::new(&[], 1000, 1, None).time(0, TICKS), 0);
  }
}
