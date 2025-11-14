//! Audio recording and analysis

mod detection;
mod recorder;
mod spectrum;

pub use detection::SilenceDetector;
pub use recorder::{AudioRecorder, buffer_to_wav};
pub use spectrum::SPECTRUM_BANDS;
