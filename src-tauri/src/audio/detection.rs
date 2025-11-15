//! Audio silence detection for automatic recording termination

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Detects silence in audio stream for automatic recording stop
///
/// Monitors audio samples and tracks the last time sound was detected.
/// When silence duration exceeds the configured threshold, signals that
/// recording should stop.
#[derive(Clone)]
pub struct SilenceDetector {
    threshold: f32,
    duration: Duration,
    last_sound_time: Arc<Mutex<Instant>>,
}

impl SilenceDetector {
    /// Create a new silence detector
    ///
    /// # Arguments
    /// * `threshold` - Amplitude threshold below which audio is considered silent (0.0-1.0)
    /// * `duration` - How long silence must persist before triggering stop
    ///
    /// # Example
    /// ```
    /// use std::time::Duration;
    /// use dictate::audio::SilenceDetector;
    ///
    /// // Stop after 2 seconds of silence below 0.01 amplitude
    /// let detector = SilenceDetector::new(0.01, Duration::from_secs(2));
    /// ```
    pub fn new(threshold: f32, duration: Duration) -> Self {
        Self {
            threshold,
            duration,
            last_sound_time: Arc::new(Mutex::new(Instant::now())),
        }
    }

    /// Check if a sample is considered silent
    pub fn is_silent(&self, sample: f32) -> bool {
        sample.abs() < self.threshold
    }

    /// Check if silence duration threshold has been exceeded
    ///
    /// Returns `true` if it's been longer than the configured duration
    /// since the last sound was detected.
    pub fn should_stop(&self) -> bool {
        let last_sound = match self.last_sound_time.lock() {
            Ok(guard) => *guard,
            Err(_) => {
                // Mutex poisoned, use current time as fallback
                Instant::now()
            }
        };
        last_sound.elapsed() > self.duration
    }

    /// Update the last sound detection time to now
    ///
    /// Call this whenever non-silent audio is detected to reset
    /// the silence timer.
    pub fn update_sound_time(&self) {
        if let Ok(mut last_sound) = self.last_sound_time.lock() {
            *last_sound = Instant::now();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn test_is_silent() {
        let detector = SilenceDetector::new(0.01, Duration::from_secs(1));
        
        assert!(detector.is_silent(0.005));  // Below threshold
        assert!(!detector.is_silent(0.05));  // Above threshold
        assert!(detector.is_silent(-0.005)); // Below threshold (negative)
    }

    #[test]
    fn test_should_stop_after_duration() {
        let detector = SilenceDetector::new(0.01, Duration::from_millis(50));
        
        // Initially should not stop (just created)
        assert!(!detector.should_stop());
        
        // Wait for silence duration to elapse
        thread::sleep(Duration::from_millis(60));
        
        // Now should signal stop
        assert!(detector.should_stop());
    }

    #[test]
    fn test_update_sound_time_resets_timer() {
        let detector = SilenceDetector::new(0.01, Duration::from_millis(50));
        
        // Wait a bit
        thread::sleep(Duration::from_millis(30));
        
        // Update sound time (reset timer)
        detector.update_sound_time();
        
        // Should not stop yet
        assert!(!detector.should_stop());
    }
}
