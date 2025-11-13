//! Audio recording and analysis
//!
//! This module provides audio recording, spectrum analysis, and silence detection
//! functionality optimized for speech/voice transcription.
//!
//! # Modules
//!
//! - `recorder` - Audio recording to file or buffer
//! - `spectrum` - FFT-based frequency spectrum analysis
//! - `detection` - Silence detection for automatic recording termination
//!
//! # Examples
//!
//! ## Basic recording to WAV file
//! ```no_run
//! use dictate::audio::AudioRecorder;
//! use std::time::Duration;
//!
//! let recorder = AudioRecorder::new().unwrap();
//! let duration = recorder.record_to_wav(
//!     "output.wav",
//!     Duration::from_secs(10),
//!     None
//! ).unwrap();
//! println!("Recorded for {:?}", duration);
//! ```
//!
//! ## Recording with silence detection
//! ```no_run
//! use dictate::audio::{AudioRecorder, SilenceDetector};
//! use std::time::Duration;
//!
//! let recorder = AudioRecorder::new().unwrap();
//! let detector = SilenceDetector::new(0.01, Duration::from_secs(2));
//!
//! let duration = recorder.record_to_wav(
//!     "output.wav",
//!     Duration::from_secs(30),
//!     Some(detector)
//! ).unwrap();
//! ```
//!
//! ## Real-time spectrum analysis
//! ```no_run
//! use dictate::audio::SpectrumAnalyzer;
//!
//! let mut analyzer = SpectrumAnalyzer::new(16000);
//!
//! // In your audio callback:
//! // if let Some(bands) = analyzer.push_sample(sample) {
//! //     // bands is Vec<f32> with 8 frequency band levels
//! //     println!("Frequency bands: {:?}", bands);
//! // }
//! ```

mod detection;
mod recorder;
mod spectrum;

pub use detection::SilenceDetector;
pub use recorder::{AudioRecorder, buffer_to_wav};
