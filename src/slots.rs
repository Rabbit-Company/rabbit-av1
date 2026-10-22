//! Following values through the reference frame slots.

use crate::frame::{FrameHeader, NUM_REF_FRAMES};

/// Keeps one value per reference slot, moving it the way a decoder moves frames between slots.
///
/// Use it to know what belongs to a frame that `show_existing_frame` shows again, such as
/// its display time or the metadata it was given:
///
/// ```
/// use rabbit_av1::{FrameHeader, RefSlots};
///
/// fn on_frame(slots: &mut RefSlots<Option<u64>>, header: &FrameHeader, index: u64) -> Option<u64> {
///   if header.show_existing_frame {
///     slots.show_existing(header)
///   } else {
///     slots.store(header, Some(index));
///     Some(index)
///   }
/// }
/// ```
#[derive(Clone, Debug)]
pub struct RefSlots<T> {
  slots: [T; NUM_REF_FRAMES],
}

impl<T: Clone> RefSlots<T> {
  /// Fills every slot with `value`.
  pub fn new(value: T) -> Self {
    RefSlots {
      slots: std::array::from_fn(|_| value.clone()),
    }
  }

  /// The value in a slot. Panics when `slot` is 8 or more.
  pub fn get(&self, slot: usize) -> &T {
    &self.slots[slot]
  }

  /// All eight slots.
  pub fn slots(&self) -> &[T; NUM_REF_FRAMES] {
    &self.slots
  }

  /// Stores `value` in every slot the frame refreshes. Call it for every header that is not
  /// `show_existing_frame`, in stream order.
  pub fn store(&mut self, header: &FrameHeader, value: T) {
    for (slot, target) in self.slots.iter_mut().enumerate() {
      if header.refresh_frame_flags >> slot & 1 == 1 {
        *target = value.clone();
      }
    }
  }

  /// Returns the value of the frame a `show_existing_frame` header shows. Showing a key frame
  /// again reloads every slot with it, as a decoder does.
  pub fn show_existing(&mut self, header: &FrameHeader) -> T {
    let value = self.slots[header.frame_to_show].clone();
    if header.is_key() {
      self.slots = std::array::from_fn(|_| value.clone());
    }
    value
  }
}

impl<T: Clone + Default> Default for RefSlots<T> {
  fn default() -> Self {
    RefSlots::new(T::default())
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::frame::{INTER_FRAME, KEY_FRAME};

  #[test]
  fn follows_refreshes_and_shown_key_frames() {
    let mut slots = RefSlots::new(0);
    let stored = FrameHeader {
      frame_type: INTER_FRAME,
      refresh_frame_flags: 0b0000_0101,
      ..FrameHeader::default()
    };
    slots.store(&stored, 7);
    assert_eq!(slots.slots(), &[7, 0, 7, 0, 0, 0, 0, 0]);
    let shown = FrameHeader {
      show_existing_frame: true,
      frame_to_show: 2,
      frame_type: INTER_FRAME,
      ..FrameHeader::default()
    };
    assert_eq!(slots.show_existing(&shown), 7);
    assert_eq!(*slots.get(1), 0);
    let shown_key = FrameHeader {
      frame_type: KEY_FRAME,
      ..shown
    };
    assert_eq!(slots.show_existing(&shown_key), 7);
    assert_eq!(slots.slots(), &[7; 8]);
  }
}
