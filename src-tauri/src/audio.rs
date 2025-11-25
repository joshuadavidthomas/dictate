//! Audio recording and analysis

mod recorder;
pub mod recording;
mod spectrum;

pub use recorder::{AudioDeviceInfo, AudioRecorder, SampleRate, SampleRateOption, buffer_to_wav};
pub use spectrum::SPECTRUM_BANDS;
