//! Captures plugins open with `dictate.capture.open`: a microphone or
//! system-audio stream each, recorded apart from dictation until closed.
//!
//! Each capture is its own device stream, so capturing the microphone
//! during dictation opens a second stream on the same device.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::MutexGuard;

use dictate_plugins::Audio;
use dictate_plugins::CaptureId;
use dictate_plugins::CaptureSource;
use dictate_speech::CaptureHandler;
use dictate_speech::DICTATION_SAMPLE_RATE;
use dictate_speech::Mic;
use dictate_speech::MicrophoneStreamError;
use dictate_speech::SpectrumUpdate;

/// Unread audio a capture keeps, 10 minutes; older samples are dropped so
/// a plugin that stops reading cannot grow the daemon without bound.
const MAX_UNREAD: usize = 10 * 60 * DICTATION_SAMPLE_RATE.as_hz() as usize;

/// Open captures, generic over the call id so they can be tested without
/// plugins or devices.
pub(super) struct Captures<C> {
    next_id: u64,
    open: HashMap<CaptureId, Capture<C>>,
}

impl<C> Default for Captures<C> {
    fn default() -> Self {
        Self {
            next_id: 0,
            open: HashMap::new(),
        }
    }
}

struct Capture<C> {
    /// Dropping it stops the device stream.
    _stream: Option<Mic>,
    buffer: Arc<Mutex<Buffer>>,
    /// A `Capture:read` waiting for this many samples.
    read: Option<(C, usize)>,
}

/// What the stream wrote and the reader has not taken.
#[derive(Default)]
pub(super) struct Buffer {
    samples: Vec<f32>,
    /// Why the stream stopped, once it has.
    ended: Option<String>,
    /// Whether unread audio was dropped since the last read.
    overflowed: bool,
}

impl Buffer {
    fn push(&mut self, samples: &[f32]) {
        self.samples.extend_from_slice(samples);
        if self.samples.len() > MAX_UNREAD {
            let excess = self.samples.len() - MAX_UNREAD;
            self.samples.drain(..excess);
            if !self.overflowed {
                eprintln!("a capture was not read for 10 minutes; dropping its oldest audio");
            }
            self.overflowed = true;
        }
    }

    /// The answer to a read waiting for `wanted` samples: everything
    /// buffered once there is that much or the stream ended, and why it
    /// ended once nothing is left.
    fn take(&mut self, wanted: usize) -> Option<Result<Audio, String>> {
        if self.samples.len() >= wanted || (self.ended.is_some() && !self.samples.is_empty()) {
            self.overflowed = false;
            return Some(Ok(Audio(std::mem::take(&mut self.samples))));
        }
        self.ended.clone().map(Err)
    }
}

/// Feeds a device stream into a capture's buffer.
struct Feed(Arc<Mutex<Buffer>>);

impl CaptureHandler for Feed {
    fn samples(&self, samples: &[f32]) -> SpectrumUpdate {
        lock(&self.0).push(samples);
        SpectrumUpdate::Skip
    }

    fn spectrum(&self, _bands: [f32; dictate_signal::SPECTRUM_BANDS]) {}

    fn stream_error(&self, error: &MicrophoneStreamError) {
        eprintln!("a capture's audio stream failed: {error}");
        lock(&self.0).ended = Some(format!("the audio stream failed: {error}"));
    }
}

fn lock(buffer: &Mutex<Buffer>) -> MutexGuard<'_, Buffer> {
    buffer
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl<C: Copy> Captures<C> {
    /// Start capturing `source`; `input_device` is the configured
    /// microphone.
    pub(super) fn open(
        &mut self,
        source: CaptureSource,
        input_device: Option<&str>,
    ) -> Result<CaptureId, String> {
        let buffer = Arc::new(Mutex::new(Buffer::default()));
        let feed = Feed(Arc::clone(&buffer));
        let rate = DICTATION_SAMPLE_RATE.as_hz();
        let stream = match source {
            CaptureSource::Mic => dictate_speech::capture(rate, input_device, feed),
            CaptureSource::System => dictate_speech::capture_system(rate, feed),
        }
        .map_err(|error| format!("the {source:?} capture could not start: {error:#}"))?;
        Ok(self.insert(Some(stream), buffer))
    }

    fn insert(&mut self, stream: Option<Mic>, buffer: Arc<Mutex<Buffer>>) -> CaptureId {
        self.next_id += 1;
        let id = CaptureId(self.next_id);
        self.open.insert(
            id,
            Capture {
                _stream: stream,
                buffer,
                read: None,
            },
        );
        id
    }

    /// A read of `samples` samples: the answer now, or `None` to answer
    /// from [`Self::answer`] once there is one.
    pub(super) fn read(
        &mut self,
        id: CaptureId,
        call: C,
        samples: usize,
    ) -> Option<Result<Audio, String>> {
        let Some(capture) = self.open.get_mut(&id) else {
            return Some(Err("the capture is closed".to_owned()));
        };
        if capture.read.is_some() {
            return Some(Err("the capture is already being read".to_owned()));
        }
        let ready = lock(&capture.buffer).take(samples);
        if ready.is_none() {
            capture.read = Some((call, samples));
        }
        ready
    }

    /// Reads that can be answered now.
    pub(super) fn answer(&mut self) -> Vec<(C, Result<Audio, String>)> {
        let mut answers = Vec::new();
        for capture in self.open.values_mut() {
            let Some((call, wanted)) = capture.read else {
                continue;
            };
            if let Some(answer) = lock(&capture.buffer).take(wanted) {
                capture.read = None;
                answers.push((call, answer));
            }
        }
        answers
    }

    /// Whether a read is waiting for audio.
    pub(super) fn waiting(&self) -> bool {
        self.open.values().any(|capture| capture.read.is_some())
    }

    /// Stop capturing; a read still waiting is answered with the reason.
    pub(super) fn close(&mut self, id: CaptureId) -> Option<(C, Result<Audio, String>)> {
        let capture = self.open.remove(&id)?;
        capture
            .read
            .map(|(call, _)| (call, Err("the capture was closed".to_owned())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opened(captures: &mut Captures<u32>) -> (CaptureId, Arc<Mutex<Buffer>>) {
        let buffer = Arc::new(Mutex::new(Buffer::default()));
        (captures.insert(None, Arc::clone(&buffer)), buffer)
    }

    fn samples(answer: Option<Result<Audio, String>>) -> usize {
        answer
            .expect("an answer")
            .expect("audio, not an ending")
            .0
            .len()
    }

    #[test]
    fn a_read_waits_for_its_samples_and_takes_everything_buffered() {
        let mut captures = Captures::default();
        let (id, buffer) = opened(&mut captures);
        lock(&buffer).push(&[0.1; 3]);
        assert!(captures.read(id, 1, 5).is_none(), "3 of 5 samples");
        assert!(captures.waiting());
        assert!(captures.answer().is_empty());

        lock(&buffer).push(&[0.2; 4]);
        let answers = captures.answer();
        assert_eq!(answers.len(), 1);
        let (call, audio) = &answers[0];
        assert_eq!(*call, 1);
        assert_eq!(audio.as_ref().map(|audio| audio.0.len()), Ok(7));
        assert!(!captures.waiting());

        // Nothing left: a zero-length read answers at once, empty.
        assert_eq!(samples(captures.read(id, 2, 0)), 0);
    }

    #[test]
    fn an_ended_stream_hands_over_what_is_left_then_says_why() {
        let mut captures = Captures::default();
        let (id, buffer) = opened(&mut captures);
        assert!(captures.read(id, 1, 100).is_none());
        {
            let mut buffer = lock(&buffer);
            buffer.push(&[0.5; 10]);
            buffer.ended = Some("unplugged".to_owned());
        }
        let answers = captures.answer();
        assert_eq!(answers[0].1.as_ref().map(|audio| audio.0.len()), Ok(10));
        assert_eq!(
            captures.read(id, 2, 100).expect("ended"),
            Err("unplugged".to_owned())
        );
    }

    #[test]
    fn closing_answers_the_waiting_read_and_later_reads_fail() {
        let mut captures = Captures::default();
        let (id, _buffer) = opened(&mut captures);
        assert!(captures.read(id, 1, 100).is_none());
        let (call, answer) = captures.close(id).expect("the waiting read");
        assert_eq!(
            (call, answer),
            (1, Err("the capture was closed".to_owned()))
        );
        assert_eq!(
            captures.read(id, 2, 0).expect("closed"),
            Err("the capture is closed".to_owned())
        );
    }

    #[test]
    fn unread_audio_is_capped_by_dropping_the_oldest() {
        let mut buffer = Buffer::default();
        buffer.push(&vec![0.0; MAX_UNREAD]);
        buffer.push(&[1.0; 3]);
        assert_eq!(buffer.samples.len(), MAX_UNREAD);
        assert_eq!(&buffer.samples[MAX_UNREAD - 3..], &[1.0; 3]);
        assert!(buffer.overflowed);
    }
}
