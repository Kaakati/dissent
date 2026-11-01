//! The capture side: anything that produces raw frames on a clock.

use crate::error::Result;
use crate::frame::Frame;

/// Output size and rate a capture source aims for. Sources scale to this size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaptureFormat {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
}

/// A source of frames. `next_frame` blocks until the next frame is ready.
///
/// Implementations are `Send` so a capture thread can own them, but they are not shared.
pub trait VideoSource: Send {
    /// The format this source delivers, after any scaling.
    fn format(&self) -> CaptureFormat;

    /// Waits for and returns the next frame, stamped from the start of the source.
    fn next_frame(&mut self) -> Result<Frame>;

    /// The size of what the source shows before it scales to its format, for a source whose picture
    /// has a size of its own, such as a screen or a window. It follows the picture when that resizes.
    fn native_size(&self) -> Option<(u32, u32)> {
        None
    }

    /// Moves to a new format without reopening. Returns false if the source must be reopened.
    fn set_format(&mut self, _format: CaptureFormat) -> bool {
        false
    }
}
