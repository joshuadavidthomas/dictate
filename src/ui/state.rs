//! OSD state machine and visual properties

use std::time::{Duration, Instant};
use crate::protocol::State;
use super::colors;

/// Action taken with transcription result
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionAction {
    Copied,
    Inserted,
    Printed,
}

/// Visual properties for a state
#[derive(Debug, Clone, Copy)]
pub struct Visual {
    pub color: iced::Color,
    pub ratio: f32, // Width ratio [0.0, 1.0]
}

/// Get the visual properties for a given state
pub fn state_visual(state: State, idle_hot: bool) -> Visual {
    match (state, idle_hot) {
        (State::Idle, false) => Visual {
            color: colors::GRAY,
            ratio: 1.00,  // Consistent width
        },
        (State::Idle, true) => Visual {
            color: colors::DIM_GREEN,
            ratio: 1.00,  // Consistent width
        },
        (State::Recording, _) => Visual {
            color: colors::RED,
            ratio: 1.00,
        },
        (State::Transcribing, _) => Visual {
            color: colors::BLUE,
            ratio: 1.00,  // Consistent width
        },
        (State::Error, _) => Visual {
            color: colors::ORANGE,
            ratio: 1.00,  // Consistent width
        },
    }
}

/// Width animation with ease-out
#[derive(Debug)]
pub struct WidthAnimation {
    start: Instant,
    duration: Duration,
    from: f32,
    to: f32,
}

impl WidthAnimation {
    pub fn new(from: f32, to: f32) -> Self {
        Self {
            start: Instant::now(),
            duration: Duration::from_millis(180),
            from,
            to,
        }
    }

    /// Get current animated value and whether animation is complete
    pub fn tick(&self, now: Instant) -> (f32, bool) {
        let elapsed = (now - self.start).as_secs_f32();
        let t = (elapsed / self.duration.as_secs_f32()).clamp(0.0, 1.0);
        let ratio = self.from + (self.to - self.from) * ease_out_quad(t);
        (ratio, t >= 1.0)
    }
}

fn ease_out_quad(t: f32) -> f32 {
    1.0 - (1.0 - t) * (1.0 - t)
}

fn ease_out_cubic(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

fn ease_in_cubic(t: f32) -> f32 {
    t.powi(3)
}

/// Window animation direction
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowAnimationState {
    Appearing,
    Disappearing,
}

/// Window fade/scale animation
#[derive(Debug)]
pub struct WindowAnimation {
    pub state: WindowAnimationState,
    started_at: Instant,
    duration: Duration,
    paused_at: Option<Instant>,
    accumulated_pause: Duration,
}

impl WindowAnimation {
    pub fn new_appearing() -> Self {
        Self {
            state: WindowAnimationState::Appearing,
            started_at: Instant::now(),
            duration: Duration::from_millis(200), // Fast, snappy
            paused_at: None,
            accumulated_pause: Duration::ZERO,
        }
    }

    pub fn new_disappearing() -> Self {
        Self {
            state: WindowAnimationState::Disappearing,
            started_at: Instant::now(),
            duration: Duration::from_millis(150), // Slightly faster out
            paused_at: None,
            accumulated_pause: Duration::ZERO,
        }
    }

    /// Pause the animation
    pub fn pause(&mut self) {
        if self.paused_at.is_none() {
            self.paused_at = Some(Instant::now());
            eprintln!("OSD: Animation paused");
        }
    }

    /// Resume the animation
    pub fn resume(&mut self) {
        if let Some(paused) = self.paused_at {
            self.accumulated_pause += paused.elapsed();
            self.paused_at = None;
            eprintln!("OSD: Animation resumed (accumulated pause: {:?})", self.accumulated_pause);
        }
    }

    /// Check if animation is paused
    pub fn is_paused(&self) -> bool {
        self.paused_at.is_some()
    }

    /// Returns (progress, is_complete) where progress is 0.0→1.0
    pub fn tick(&self, now: Instant) -> (f32, bool) {
        if self.is_paused() {
            // Don't advance if paused - return current progress
            let elapsed = self.paused_at.unwrap() - self.started_at - self.accumulated_pause;
            let t = (elapsed.as_secs_f32() / self.duration.as_secs_f32()).clamp(0.0, 1.0);
            return (t, false); // Not complete while paused
        }

        let elapsed = (now - self.started_at - self.accumulated_pause).as_secs_f32();
        let t = (elapsed / self.duration.as_secs_f32()).clamp(0.0, 1.0);
        (t, t >= 1.0)
    }
}

/// Transcribing animation state
#[derive(Debug)]
pub struct TranscribingState {
    entered_at: Instant,
    frozen_level: f32,
}

impl TranscribingState {
    pub fn new(frozen_level: f32) -> Self {
        Self {
            entered_at: Instant::now(),
            frozen_level,
        }
    }

    /// Animate level (freeze 300ms, ease to 0 over 300ms) and alpha (pulse)
    pub fn animate(&self, now: Instant) -> (f32, f32) {
        let elapsed_ms = (now - self.entered_at).as_millis() as f32;

        // 1. Level: freeze 300ms, ease to 0 over 300ms
        let level = if elapsed_ms < 300.0 {
            self.frozen_level
        } else if elapsed_ms < 600.0 {
            let t = (elapsed_ms - 300.0) / 300.0;
            self.frozen_level * (1.0 - ease_out_quad(t))
        } else {
            0.0
        };

        // 2. Pulse: blue dot alpha oscillates 0.4-1.0 @ 0.5Hz (slower, more dramatic)
        let pulse_t = (elapsed_ms / 1000.0) * 0.5; // 0.5 Hz (2 second cycle)
        let alpha = 0.7 + 0.3 * (pulse_t * 2.0 * std::f32::consts::PI).sin();

        (level, alpha)
    }
}

/// Recording animation state (for pulsing dot)
#[derive(Debug)]
pub struct RecordingState {
    entered_at: Instant,
}

impl RecordingState {
    pub fn new() -> Self {
        Self {
            entered_at: Instant::now(),
        }
    }

    /// Pulse: red dot alpha oscillates 0.4-1.0 @ 0.5Hz (slower, more dramatic)
    pub fn animate(&self, now: Instant) -> f32 {
        let elapsed_ms = (now - self.entered_at).as_millis() as f32;
        let pulse_t = (elapsed_ms / 1000.0) * 0.5; // 0.5 Hz (2 second cycle)
        // Use sin without abs() for smooth fade in/out, map from [-1, 1] to [0.4, 1.0]
        0.7 + 0.3 * (pulse_t * 2.0 * std::f32::consts::PI).sin()
    }
}

/// Ring buffer for level bars (last 10 samples from 30-sample buffer)
#[derive(Debug)]
pub struct LevelRingBuffer {
    buffer: [f32; 30],
    index: usize,
}

impl LevelRingBuffer {
    pub fn new() -> Self {
        Self {
            buffer: [0.0; 30],
            index: 0,
        }
    }

    pub fn push(&mut self, level: f32) {
        self.buffer[self.index] = level;
        self.index = (self.index + 1) % 30;
    }

    /// Get the last 10 samples for display
    pub fn last_10(&self) -> [f32; 10] {
        let mut result = [0.0; 10];
        for i in 0..10 {
            let idx = (self.index + 20 + i) % 30;
            result[i] = self.buffer[idx];
        }
        result
    }
}

/// Ring buffer for spectrum data (last 30 frames of 8 bands each)
#[derive(Debug)]
pub struct SpectrumRingBuffer {
    buffer: [[f32; 8]; 30],
    index: usize,
}

impl SpectrumRingBuffer {
    pub fn new() -> Self {
        Self {
            buffer: [[0.0; 8]; 30],
            index: 0,
        }
    }

    pub fn push(&mut self, bands: [f32; 8]) {
        self.buffer[self.index] = bands;
        self.index = (self.index + 1) % 30;
    }

    /// Get the most recent frame
    pub fn last_frame(&self) -> [f32; 8] {
        let idx = (self.index + 29) % 30;
        self.buffer[idx]
    }
}

/// Complete OSD state with animations
#[derive(Debug)]
pub struct OsdState {
    pub state: State,
    pub idle_hot: bool,
    pub current_ratio: f32,
    pub width_animation: Option<WidthAnimation>,
    pub recording_state: Option<RecordingState>,
    pub transcribing_state: Option<TranscribingState>,
    pub level_buffer: LevelRingBuffer,
    pub spectrum_buffer: SpectrumRingBuffer,
    pub last_message: Instant,
    pub linger_until: Option<Instant>, // When to hide window after showing result
    pub window_animation: Option<WindowAnimation>,
    pub is_window_disappearing: bool, // Track if we're in disappearing animation
    pub is_mouse_hovering: bool, // Track if mouse is over window
    pub last_mouse_event: Instant, // Track when we last saw a mouse event
    pub recording_start_ts: Option<u64>, // Timestamp (ms) when recording started
    pub current_ts: u64, // Latest timestamp (ms) from server
    pub transcription_result: Option<String>, // Transcribed text result
    pub should_auto_exit: bool, // Flag to trigger UI exit after linger completes
    pub completion_action: Option<CompletionAction>, // Action taken with result
    pub completion_started_at: Option<Instant>, // When completion flash started
}

impl OsdState {
    pub fn new() -> Self {
        Self {
            state: State::Idle,
            idle_hot: false,
            current_ratio: 1.00, // Consistent full width
            width_animation: None,
            recording_state: None,
            transcribing_state: None,
            level_buffer: LevelRingBuffer::new(),
            spectrum_buffer: SpectrumRingBuffer::new(),
            last_message: Instant::now(),
            linger_until: None,
            window_animation: None,
            is_window_disappearing: false,
            is_mouse_hovering: false,
            last_mouse_event: Instant::now(),
            recording_start_ts: None,
            current_ts: 0,
            transcription_result: None,
            should_auto_exit: false,
            completion_action: None,
            completion_started_at: None,
        }
    }

    /// Update state from server event
    pub fn update_state(&mut self, new_state: State, idle_hot: bool, ts: u64) {
        self.last_message = Instant::now();
        self.current_ts = ts;

        let visual = state_visual(new_state, idle_hot);
        let old_ratio = self.current_ratio;
        if (visual.ratio - old_ratio).abs() > 0.01 {
            self.width_animation = Some(WidthAnimation::new(old_ratio, visual.ratio));
            self.current_ratio = visual.ratio;
            self.width_animation = None;
        }

        // Handle recording state transition
        if new_state == State::Recording && self.state != State::Recording {
            // Entering recording - start pulsing animation and clear lingering
            self.recording_state = Some(RecordingState::new());
            self.recording_start_ts = Some(ts);
            self.linger_until = None;
        } else if new_state != State::Recording {
            self.recording_state = None;
            self.recording_start_ts = None;
        }

        // Handle transcribing state transition
        if new_state == State::Transcribing && self.state != State::Transcribing {
            // Entering transcribing - freeze current level
            let frozen_level = self.level_buffer.last_10()[9]; // Last sample
            self.transcribing_state = Some(TranscribingState::new(frozen_level));
            // Clear any lingering when starting a new transcription
            self.linger_until = None;
        } else if new_state != State::Transcribing {
            // If transitioning away from Transcribing, check minimum display time
            if self.state == State::Transcribing {
                if let Some(trans_state) = &self.transcribing_state {
                    let elapsed = Instant::now().duration_since(trans_state.entered_at);
                    if elapsed < std::time::Duration::from_millis(500) {
                        // Don't transition yet - keep Transcribing state for minimum visibility
                        return;
                    }
                }

                // No linger needed - completion flash will be shown instead
            }
            self.transcribing_state = None;
        }

        self.state = new_state;
        self.idle_hot = idle_hot;
    }

    /// Update audio level
    pub fn update_level(&mut self, level: f32, ts: u64) {
        self.last_message = Instant::now();
        self.current_ts = ts;
        self.level_buffer.push(level);
    }

    /// Update spectrum bands
    pub fn update_spectrum(&mut self, bands: Vec<f32>, ts: u64) {
        self.last_message = Instant::now();
        self.current_ts = ts;
        if bands.len() == 8 {
            let bands_array: [f32; 8] = bands.try_into().unwrap_or([0.0; 8]);
            self.spectrum_buffer.push(bands_array);
        }
    }

    /// Set error state
    pub fn set_error(&mut self) {
        self.update_state(State::Error, false, self.current_ts);
    }

    /// Store transcription result
    pub fn set_transcription_result(&mut self, text: String) {
        self.transcription_result = Some(text);
    }

    /// Set completion action and start exit timer
    pub fn set_completion_action(&mut self, action: CompletionAction) {
        self.completion_action = Some(action);
        self.completion_started_at = Some(Instant::now());
    }

    /// Check if completion flash has expired and we should exit
    pub fn check_completion_exit(&self) -> bool {
        if let Some(started_at) = self.completion_started_at {
            // Exit after 750ms completion flash
            started_at.elapsed() >= Duration::from_millis(750)
        } else {
            false
        }
    }

    /// Check if we should auto-exit - simple state-driven approach
    pub fn check_auto_exit(&mut self) -> bool {
        // Exit when we have a transcription result, don't need window, and mouse isn't hovering
        if self.transcription_result.is_some() 
            && !self.needs_window() 
            && !self.is_mouse_hovering 
        {
            self.should_auto_exit = true;
            return true;
        }
        false
    }

    /// Tick animations and return current visual state
    pub fn tick(&mut self, now: Instant) -> OsdVisual {
        // Tick width animation
        if let Some(anim) = &self.width_animation {
            let (ratio, complete) = anim.tick(now);
            self.current_ratio = ratio;
            if complete {
                self.width_animation = None;
            }
        }

        // Get current alpha (for dot pulsing)
        let (_level, alpha) = if let Some(transcribing) = &self.transcribing_state {
            transcribing.animate(now)
        } else if let Some(recording) = &self.recording_state {
            // Recording: pulse alpha, use live level
            (self.level_buffer.last_10()[9], recording.animate(now))
        } else {
            (self.level_buffer.last_10()[9], 1.0)
        };

        let visual = state_visual(self.state, self.idle_hot);

        // Calculate recording timer and blink state
        let (recording_elapsed_secs, is_near_limit, timer_visible) = if self.state == State::Recording {
            if let Some(start_ts) = self.recording_start_ts {
                let elapsed_ms = self.current_ts.saturating_sub(start_ts);
                let elapsed_secs = (elapsed_ms / 1000) as u32;
                let near_limit = elapsed_secs >= 25;
                
                // Blink every 500ms (toggle twice per second)
                // Use current_ts for synchronized blinking
                let blink = (self.current_ts / 500) % 2 == 0;
                
                (Some(elapsed_secs), near_limit, blink)
            } else {
                (None, false, true)
            }
        } else {
            (None, false, true) // Always visible when not recording
        };

        // Calculate window animation values
        let (window_opacity, window_scale) = if let Some(anim) = &self.window_animation {
            let (t, complete) = anim.tick(now);
            let anim_state = anim.state;

            let result = match anim_state {
                WindowAnimationState::Appearing => {
                    // Ease out for smooth deceleration
                    let eased = ease_out_cubic(t);
                    let opacity = eased;
                    let scale = 0.5 + (0.5 * eased);
                    eprintln!("OSD: Appearing animation - t={:.3}, opacity={:.3}, scale={:.3}", t, opacity, scale);
                    (opacity, scale) // opacity: 0→1, scale: 0.5→1.0
                }
                WindowAnimationState::Disappearing => {
                    // Ease in for smooth acceleration
                    let eased = ease_in_cubic(t);
                    let inv = 1.0 - eased;
                    let opacity = inv;
                    let scale = 0.5 + (0.5 * inv);
                    eprintln!("OSD: Disappearing animation - t={:.3}, opacity={:.3}, scale={:.3}", t, opacity, scale);
                    (opacity, scale) // opacity: 1→0, scale: 1.0→0.5
                }
            };

            if complete {
                eprintln!("OSD: Animation complete, clearing animation state");
                self.window_animation = None;
            }

            result
        } else {
            (1.0, 1.0) // Fully visible, full scale
        };

        OsdVisual {
            state: self.state,
            color: visual.color,
            alpha,
            spectrum_bands: self.spectrum_buffer.last_frame(),
            window_opacity,
            window_scale,
            recording_elapsed_secs,
            is_near_limit,
            timer_visible,
        }
    }

    /// Check for timeout (no messages for 15 seconds)
    pub fn has_timeout(&self) -> bool {
        self.last_message.elapsed() > Duration::from_secs(15)
    }

    /// Returns true if current state requires a visible window
    pub fn needs_window(&self) -> bool {
        // Show window for Recording, Transcribing, Error
        if matches!(self.state, State::Recording | State::Transcribing | State::Error) {
            return true;
        }
        
        // Also show window during completion flash
        if self.completion_action.is_some() && !self.check_completion_exit() {
            return true;
        }
        
        false
    }

    /// Returns true if we just transitioned to needing a window
    pub fn should_create_window(&self, had_window: bool) -> bool {
        self.needs_window() && !had_window
    }

    /// Start appearing animation
    pub fn start_appearing_animation(&mut self) {
        self.window_animation = Some(WindowAnimation::new_appearing());
        self.is_window_disappearing = false;
    }

    /// Returns true if we should start disappearing animation
    pub fn should_start_disappearing(&self, had_window: bool) -> bool {
        !self.needs_window() 
            && had_window 
            && !self.is_window_disappearing
            && !self.is_mouse_hovering  // Don't start disappearing if mouse is hovering
    }

    /// Start disappearing animation
    pub fn start_disappearing_animation(&mut self) {
        self.window_animation = Some(WindowAnimation::new_disappearing());
        self.is_window_disappearing = true;
    }

    /// Returns true if disappearing animation is complete and we should close window
    pub fn should_close_window(&self) -> bool {
        // Close window if we're marked as disappearing but animation is done (cleared)
        self.is_window_disappearing && self.window_animation.is_none()
    }
}

/// Current visual state for rendering
#[derive(Debug, Clone)]
pub struct OsdVisual {
    pub state: State,
    pub color: iced::Color,
    pub alpha: f32,
    pub spectrum_bands: [f32; 8],
    pub window_opacity: f32,  // 0.0 → 1.0 for fade animation
    pub window_scale: f32,     // 0.5 → 1.0 for expand/shrink animation
    pub recording_elapsed_secs: Option<u32>, // Elapsed seconds while recording
    pub is_near_limit: bool,   // True if approaching 30s limit (>= 25s)
    pub timer_visible: bool,   // Blink state for timer display
}
