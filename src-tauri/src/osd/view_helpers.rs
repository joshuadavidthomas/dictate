//! View State Matching Helpers
//!
//! Provides cleaner view construction by matching on visual state,
//! inspired by the pattern from the architecture exploration.
//!
//! Benefits:
//! - Exhaustive state handling (compiler ensures all states are covered)
//! - Clear mapping from state to view
//! - Easy to add new states/views
//! - Self-documenting view logic

use crate::osd::state::OsdVisualState;
use crate::osd::theme::colors;
use crate::recording::RecordingSnapshot;
use iced::Color;

/// Visual properties derived from OSD state
#[derive(Debug, Clone, Copy)]
pub struct StateVisuals {
    /// Primary color for the current state
    pub color: Color,
    /// Whether to show the waveform
    pub show_waveform: bool,
    /// Whether to show the timer
    pub show_timer: bool,
    /// Whether the dot should pulse
    pub should_pulse: bool,
    /// Label for the state (for accessibility/debugging)
    pub label: &'static str,
}

impl StateVisuals {
    /// Derive visuals from recording state
    pub fn from_recording_state(state: RecordingSnapshot, idle_hot: bool) -> Self {
        match (state, idle_hot) {
            (RecordingSnapshot::Idle, false) => Self {
                color: colors::IDLE,
                show_waveform: false,
                show_timer: false,
                should_pulse: false,
                label: "Idle",
            },
            (RecordingSnapshot::Idle, true) => Self {
                color: colors::IDLE_HOT,
                show_waveform: false,
                show_timer: false,
                should_pulse: false,
                label: "Ready",
            },
            (RecordingSnapshot::Recording, _) => Self {
                color: colors::RECORDING,
                show_waveform: true,
                show_timer: true,
                should_pulse: true,
                label: "Recording",
            },
            (RecordingSnapshot::Transcribing, _) => Self {
                color: colors::TRANSCRIBING,
                show_waveform: true,
                show_timer: false,
                should_pulse: true,
                label: "Transcribing",
            },
            (RecordingSnapshot::Error, _) => Self {
                color: colors::ERROR,
                show_waveform: false,
                show_timer: false,
                should_pulse: false,
                label: "Error",
            },
        }
    }

    /// Derive visuals from visual state machine
    pub fn from_visual_state(state: &OsdVisualState) -> Self {
        let (recording_state, idle_hot) = state.current_state_info();
        Self::from_recording_state(recording_state, idle_hot)
    }
}

/// Content type to render based on state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentType {
    /// Just show status dot
    StatusOnly,
    /// Show status dot + waveform
    StatusAndWaveform,
    /// Show status dot + waveform + timer
    Full,
    /// Show nothing (hidden state)
    None,
}

impl ContentType {
    /// Determine content type from visual state
    pub fn from_visual_state(state: &OsdVisualState) -> Self {
        match state {
            OsdVisualState::Hidden => ContentType::None,

            OsdVisualState::Appearing { target, .. }
            | OsdVisualState::Visible { state: target, .. }
            | OsdVisualState::Hovering { state: target, .. }
            | OsdVisualState::Lingering { state: target, .. } => match target {
                RecordingSnapshot::Recording => ContentType::Full,
                RecordingSnapshot::Transcribing => ContentType::StatusAndWaveform,
                _ => ContentType::StatusOnly,
            },

            OsdVisualState::Disappearing { previous_state, .. } => match previous_state {
                RecordingSnapshot::Recording => ContentType::Full,
                RecordingSnapshot::Transcribing => ContentType::StatusAndWaveform,
                _ => ContentType::StatusOnly,
            },
        }
    }

    /// Determine content type from recording state directly
    pub fn from_recording_state(state: RecordingSnapshot) -> Self {
        match state {
            RecordingSnapshot::Recording => ContentType::Full,
            RecordingSnapshot::Transcribing => ContentType::StatusAndWaveform,
            RecordingSnapshot::Idle | RecordingSnapshot::Error => ContentType::StatusOnly,
        }
    }
}

/// Animation phase for the OSD window
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnimationPhase {
    /// No animation, steady state
    Steady,
    /// Window is appearing
    Appearing,
    /// Window is disappearing
    Disappearing,
}

impl AnimationPhase {
    /// Determine animation phase from visual state
    pub fn from_visual_state(state: &OsdVisualState) -> Self {
        match state {
            OsdVisualState::Hidden => AnimationPhase::Steady,
            OsdVisualState::Appearing { .. } => AnimationPhase::Appearing,
            OsdVisualState::Visible { .. } => AnimationPhase::Steady,
            OsdVisualState::Hovering { .. } => AnimationPhase::Steady,
            OsdVisualState::Lingering { .. } => AnimationPhase::Steady,
            OsdVisualState::Disappearing { .. } => AnimationPhase::Disappearing,
        }
    }
}

/// Combined view context with all derived state
#[derive(Debug, Clone)]
pub struct ViewContext {
    pub visuals: StateVisuals,
    pub content_type: ContentType,
    pub animation_phase: AnimationPhase,
    pub is_hovering: bool,
    pub is_lingering: bool,
}

impl ViewContext {
    /// Create view context from visual state
    pub fn from_visual_state(state: &OsdVisualState) -> Self {
        Self {
            visuals: StateVisuals::from_visual_state(state),
            content_type: ContentType::from_visual_state(state),
            animation_phase: AnimationPhase::from_visual_state(state),
            is_hovering: state.is_hovering(),
            is_lingering: state.is_lingering(),
        }
    }
}

/// Example of view state matching in practice:
///
/// ```ignore
/// fn view(&self, id: window::Id) -> Element<'_, Message> {
///     let ctx = ViewContext::from_visual_state(&self.visual_state);
///
///     let content = match ctx.content_type {
///         ContentType::Full => {
///             row![
///                 status_dot(ctx.visuals.color),
///                 spectrum_waveform(self.spectrum_bands, ctx.visuals.color),
///                 timer_display(self.elapsed_secs),
///             ]
///         }
///         ContentType::StatusAndWaveform => {
///             row![
///                 status_dot(ctx.visuals.color),
///                 spectrum_waveform(self.spectrum_bands, ctx.visuals.color),
///             ]
///         }
///         ContentType::StatusOnly => {
///             row![status_dot(ctx.visuals.color)]
///         }
///         ContentType::None => {
///             row![]
///         }
///     };
///
///     // Apply animation transforms based on phase
///     let animated = match ctx.animation_phase {
///         AnimationPhase::Appearing => content.opacity(self.appear_progress),
///         AnimationPhase::Disappearing => content.opacity(self.disappear_progress),
///         AnimationPhase::Steady => content,
///     };
///
///     container(animated)
///         .style(|_| bar_style(ctx.visuals.color))
///         .into()
/// }
/// ```

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_state_visuals() {
        let recording = StateVisuals::from_recording_state(RecordingSnapshot::Recording, false);
        assert!(recording.show_waveform);
        assert!(recording.show_timer);
        assert!(recording.should_pulse);

        let idle = StateVisuals::from_recording_state(RecordingSnapshot::Idle, false);
        assert!(!idle.show_waveform);
        assert!(!idle.should_pulse);
    }

    #[test]
    fn test_content_type() {
        assert_eq!(
            ContentType::from_recording_state(RecordingSnapshot::Recording),
            ContentType::Full
        );
        assert_eq!(
            ContentType::from_recording_state(RecordingSnapshot::Transcribing),
            ContentType::StatusAndWaveform
        );
        assert_eq!(
            ContentType::from_recording_state(RecordingSnapshot::Idle),
            ContentType::StatusOnly
        );
    }

    #[test]
    fn test_animation_phase() {
        let hidden = OsdVisualState::Hidden;
        assert_eq!(
            AnimationPhase::from_visual_state(&hidden),
            AnimationPhase::Steady
        );
    }
}
