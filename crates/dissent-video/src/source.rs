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
}
