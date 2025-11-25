use crate::transcription::TranscriptionEngine;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Instant;
use tokio::sync::Mutex;

/// Broadcastable snapshot of recording state (minimal, just the phase)
#[derive(Clone, Copy, Serialize, Deserialize, Debug, PartialEq, Eq)]
pub enum RecordingSnapshot {
    Idle,
    Recording,
    Transcribing,
    Error,
}

impl RecordingSnapshot {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Idle => "Ready",
            Self::Recording => "Recording",
            Self::Transcribing => "Transcribing",
            Self::Error => "Error",
        }
    }
}

/// Internal recording phase with associated data
enum RecordingPhase {
    Idle,
    Recording {
        /// Audio buffer shared with the audio callback thread
        audio_buffer: Arc<std::sync::Mutex<Vec<i16>>>,
        /// Signal to stop recording, shared with audio callback
        stop_signal: Arc<AtomicBool>,
        /// Audio stream - must be kept alive or recording stops
        stream: cpal::Stream,
        /// When recording started
        start_time: Instant,
    },
    Transcribing,
}

/// Manages the current recording state
///
/// This is a newtype wrapper around Mutex<RecordingPhase> that provides
/// a clean API for managing recording state transitions and data.
pub struct RecordingState(Mutex<RecordingPhase>);

impl RecordingState {
    pub fn new() -> Self {
        Self(Mutex::new(RecordingPhase::Idle))
    }

    /// Start a new recording with the given resources
    pub async fn start_recording(
        &self,
        stream: cpal::Stream,
        audio_buffer: Arc<std::sync::Mutex<Vec<i16>>>,
        stop_signal: Arc<AtomicBool>,
    ) {
        let mut phase = self.0.lock().await;
        *phase = RecordingPhase::Recording {
            audio_buffer,
            stop_signal,
            stream,
            start_time: Instant::now(),
        };
    }

    /// Stop the current recording and transition to transcribing
    ///
    /// Returns the audio buffer if recording was active, None otherwise
    pub async fn stop_recording(&self) -> Option<Arc<std::sync::Mutex<Vec<i16>>>> {
        let mut phase = self.0.lock().await;
        if let RecordingPhase::Recording {
            audio_buffer,
            stop_signal,
            stream,
            ..
        } = std::mem::replace(&mut *phase, RecordingPhase::Transcribing)
        {
            // Signal the audio callback to stop
            stop_signal.store(true, std::sync::atomic::Ordering::Release);
            // Drop the stream to stop recording
            drop(stream);
            Some(audio_buffer)
        } else {
            None
        }
    }

    /// Finish transcription and return to idle state
    pub async fn finish_transcription(&self) {
        let mut phase = self.0.lock().await;
        *phase = RecordingPhase::Idle;
    }

    /// Get a broadcastable snapshot of current state
    pub async fn snapshot(&self) -> RecordingSnapshot {
        let phase = self.0.lock().await;
        match &*phase {
            RecordingPhase::Idle => RecordingSnapshot::Idle,
            RecordingPhase::Recording { .. } => RecordingSnapshot::Recording,
            RecordingPhase::Transcribing => RecordingSnapshot::Transcribing,
        }
    }

    /// Get elapsed recording time in milliseconds
    pub async fn elapsed_ms(&self) -> u64 {
        let phase = self.0.lock().await;
        if let RecordingPhase::Recording { start_time, .. } = &*phase {
            start_time.elapsed().as_millis() as u64
        } else {
            0
        }
    }
}

/// Manages transcription engine state
pub struct TranscriptionState {
    engine: Mutex<Option<TranscriptionEngine>>,
}

impl TranscriptionState {
    pub fn new() -> Self {
        Self {
            engine: Mutex::new(None),
        }
    }

    pub async fn engine(&self) -> tokio::sync::MutexGuard<'_, Option<TranscriptionEngine>> {
        self.engine.lock().await
    }
}

/// State for managing keyboard shortcuts across different platforms
pub struct ShortcutState {
    backend: Arc<Mutex<Option<Box<dyn crate::platform::shortcuts::ShortcutBackend>>>>,
}

impl ShortcutState {
    pub fn new() -> Self {
        Self {
            backend: Arc::new(Mutex::new(None)),
        }
    }

    pub async fn set_backend(&self, backend: Box<dyn crate::platform::shortcuts::ShortcutBackend>) {
        *self.backend.lock().await = Some(backend);
    }

    pub async fn backend(&self) -> tokio::sync::MutexGuard<'_, Option<Box<dyn crate::platform::shortcuts::ShortcutBackend>>> {
        self.backend.lock().await
    }
}
