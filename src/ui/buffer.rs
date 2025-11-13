//! Data buffers for UI rendering
//!
//! This module provides ring buffers and other data structures for storing and smoothing
//! variable-rate data streams from the server. These buffers enable smooth visualization
//! by decoupling server data production rate from client rendering rate (60 FPS).

use crate::audio::SPECTRUM_BANDS;

/// Ring buffer for spectrum data
///
/// Provides temporal smoothing for spectrum visualization by maintaining a circular
/// buffer of recent spectrum frames. The UI samples from this buffer at render time,
/// allowing it to handle variable-rate updates from the server while maintaining
/// smooth 60 FPS rendering.
#[derive(Debug)]
pub struct SpectrumRingBuffer {
    buffer: [[f32; SPECTRUM_BANDS]; Self::CAPACITY],
    index: usize,
}

impl SpectrumRingBuffer {
    /// Number of frames buffered for temporal smoothing
    ///
    /// At 60 FPS rendering, 30 frames provides 500ms of history.
    /// This smooths variable-rate updates from the server while
    /// maintaining responsive visualization.
    pub const CAPACITY: usize = 30;

    pub fn new() -> Self {
        Self {
            buffer: [[0.0; SPECTRUM_BANDS]; Self::CAPACITY],
            index: 0,
        }
    }

    /// Push a new spectrum frame into the buffer
    pub fn push(&mut self, bands: [f32; SPECTRUM_BANDS]) {
        self.buffer[self.index] = bands;
        self.index = (self.index + 1) % Self::CAPACITY;
    }

    /// Get the most recent spectrum frame for rendering
    pub fn last_frame(&self) -> [f32; SPECTRUM_BANDS] {
        let idx = (self.index + Self::CAPACITY - 1) % Self::CAPACITY;
        self.buffer[idx]
    }
}
