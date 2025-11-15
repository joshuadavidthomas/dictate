//! Audio recording and analysis

mod detection;
mod recorder;
mod spectrum;
pub mod recording;

pub use detection::SilenceDetector;
pub use recorder::{AudioRecorder, AudioDeviceInfo, SampleRate, SampleRateOption, buffer_to_wav};
pub use spectrum::SPECTRUM_BANDS;
