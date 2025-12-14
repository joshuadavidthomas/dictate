//! OSD state machine with domain/presentation separation
//!
//! This module implements a pure state machine that:
//! - Separates domain state (what the app is doing) from visual state (how the OSD appears)
//! - Returns explicit actions as side effects (no mutations)
//! - Avoids embedding `Instant` in state (timeline owns timing)
//! - Uses generation tags to prevent stale completion events

use crate::osd::theme::timing;
use crate::recording::{RecordingSnapshot, SPECTRUM_BANDS};
use std::time::Duration;

// =============================================================================
// Type Aliases
// =============================================================================

/// Alias for recording phase - reuse existing broadcast type
pub type RecordingPhase = RecordingSnapshot;

// =============================================================================
// Domain State
// =============================================================================

/// Domain state - single source of truth for what the application is doing
#[derive(Debug, Clone)]
pub struct DomainState {
    /// Current recording phase
    pub phase: RecordingPhase,
    /// Whether the shortcut is held (ready to record)
    pub idle_hot: bool,
    /// Frequency spectrum bands for visualization
    pub spectrum: [f32; SPECTRUM_BANDS],
    /// Recording start timestamp (milliseconds)
    pub recording_start_ts: Option<u64>,
    /// Current timestamp (milliseconds)
    pub current_ts: u64,
}

impl Default for DomainState {
    fn default() -> Self {
        Self {
            phase: RecordingPhase::Idle,
            idle_hot: false,
            spectrum: [0.0; SPECTRUM_BANDS],
            recording_start_ts: None,
            current_ts: 0,
        }
    }
}

// =============================================================================
// Visual State
// =============================================================================

/// Visual state - how the overlay appears (presentation only)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisualState {
    /// Not visible on screen
    Hidden,
    /// Window appearing (animation in progress)
    Appearing,
    /// Fully visible, normal display
    Visible,
    /// Mouse hovering over OSD
    Hovering,
    /// Lingering before disappearing (post-activity delay)
    Lingering,
    /// Window disappearing (animation in progress)
    Disappearing,
}

impl Default for VisualState {
    fn default() -> Self {
        Self::Hidden
    }
}

// =============================================================================
// Events
// =============================================================================

/// Events that can trigger state transitions
#[derive(Debug, Clone)]
pub enum OsdEvent {
    /// Domain state changed (from broadcast)
    PhaseChanged { phase: RecordingPhase, idle_hot: bool },

    /// User requested preview (e.g., from settings page)
    PreviewRequested,

    /// User interaction
    MouseEnter,
    MouseExit,

    /// Internal events (from timeline/timers)
    AppearComplete { generation: u64 },
    DisappearComplete { generation: u64 },
    LingerExpired,

    /// Control
    ForceHide,
}

// =============================================================================
// Actions
// =============================================================================

/// Side effects to perform after transition
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OsdAction {
    // Animations
    StartAppearAnimation,
    StartDisappearAnimation,
    StartPulseAnimation,
    StopPulseAnimation,

    // Timers
    StartLingerTimer { duration: Duration },
    CancelLingerTimer,

    // Window
    CreateWindow,
    DestroyWindow,
}

// =============================================================================
// Transition Logic
// =============================================================================

impl RecordingPhase {
    /// Check if this phase represents active recording/processing
    pub fn is_active(&self) -> bool {
        matches!(
            self,
            RecordingPhase::Recording | RecordingPhase::Transcribing | RecordingPhase::Error
        )
    }
}

/// Process event and return new state + actions to perform
///
/// This is a pure function with no side effects. All state mutations and
/// side effects are returned as actions for the caller to execute.
///
/// # Arguments
/// * `visual` - Current visual state
/// * `domain` - Current domain state (for context)
/// * `event` - Event to process
/// * `current_animation_gen` - Generation of the current animation (if any)
///
/// # Returns
/// Tuple of (new_visual_state, actions_to_perform)
pub fn transition(
    visual: VisualState,
    _domain: &DomainState,
    event: OsdEvent,
    current_animation_gen: Option<u64>,
) -> (VisualState, Vec<OsdAction>) {
    use OsdAction::*;
    use OsdEvent::*;
    use VisualState::*;

    match (visual, event) {
        // Hidden -> Appearing when active phase starts
        (Hidden, PhaseChanged { phase, .. }) if phase.is_active() => (
            Appearing,
            vec![CreateWindow, StartAppearAnimation, StartPulseAnimation],
        ),

        // Hidden -> Appearing when preview requested (no pulse for preview)
        (Hidden, PreviewRequested) => (
            Appearing,
            vec![CreateWindow, StartAppearAnimation],
        ),

        // Appearing -> Visible when animation completes (verify generation)
        (Appearing, AppearComplete { generation }) if Some(generation) == current_animation_gen => {
            // Check if this was a preview (no active phase) - if so, go to Lingering
            if _domain.phase.is_active() {
                (Visible, vec![])
            } else {
                // Preview flow: go directly to Lingering after appearing
                (
                    Lingering,
                    vec![StartLingerTimer {
                        duration: timing::LINGER,
                    }],
                )
            }
        }

        // Visible -> Hovering on mouse enter
        (Visible, MouseEnter) => (Hovering, vec![]),

        // Hovering -> Visible on mouse exit
        (Hovering, MouseExit) => (Visible, vec![]),

        // Visible -> Lingering when phase becomes idle
        (Visible, PhaseChanged { phase, .. }) if !phase.is_active() => (
            Lingering,
            vec![
                StartLingerTimer {
                    duration: timing::LINGER,
                },
                StopPulseAnimation,
            ],
        ),

        // Lingering -> Disappearing when timer expires
        (Lingering, LingerExpired) => (Disappearing, vec![StartDisappearAnimation]),

        // Lingering -> Visible if active phase starts again
        (Lingering, PhaseChanged { phase, .. }) if phase.is_active() => {
            (Visible, vec![CancelLingerTimer, StartPulseAnimation])
        }

        // Disappearing -> Hidden when animation completes (verify generation)
        (Disappearing, DisappearComplete { generation })
            if Some(generation) == current_animation_gen =>
        {
            (Hidden, vec![DestroyWindow])
        }

        // Disappearing -> Appearing if active phase requested while disappearing
        (Disappearing, PhaseChanged { phase, .. }) if phase.is_active() => {
            (Appearing, vec![StartAppearAnimation, StartPulseAnimation])
        }

        // Force hide from any state
        (_, ForceHide) => (Hidden, vec![DestroyWindow, CancelLingerTimer]),

        // Default: no change (includes stale generation events)
        (state, _) => (state, vec![]),
    }
}

// =============================================================================
// Combined State
// =============================================================================

/// Complete OSD state combining domain and visual
pub struct OsdState {
    /// Domain state (what the app is doing)
    pub domain: DomainState,
    /// Visual state (how the OSD appears)
    pub visual: VisualState,
}

impl OsdState {
    /// Create new OSD state with defaults
    pub fn new() -> Self {
        Self {
            domain: DomainState::default(),
            visual: VisualState::default(),
        }
    }

    /// Update domain state from broadcast message
    pub fn update_domain(&mut self, phase: RecordingPhase, idle_hot: bool) {
        self.domain.phase = phase;
        self.domain.idle_hot = idle_hot;
    }

    /// Process visual event and return actions
    pub fn transition(&mut self, event: OsdEvent, anim_gen: Option<u64>) -> Vec<OsdAction> {
        let (new_visual, actions) = transition(self.visual, &self.domain, event, anim_gen);
        self.visual = new_visual;
        actions
    }
}

impl Default for OsdState {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn idle_domain() -> DomainState {
        DomainState {
            phase: RecordingPhase::Idle,
            idle_hot: false,
            ..Default::default()
        }
    }

    fn recording_domain() -> DomainState {
        DomainState {
            phase: RecordingPhase::Recording,
            idle_hot: false,
            ..Default::default()
        }
    }

    #[test]
    fn test_recording_phase_is_active() {
        assert!(!RecordingPhase::Idle.is_active());
        assert!(RecordingPhase::Recording.is_active());
        assert!(RecordingPhase::Transcribing.is_active());
        assert!(RecordingPhase::Error.is_active());
    }

    #[test]
    fn test_hidden_to_appearing_on_active_phase() {
        let domain = recording_domain();
        let event = OsdEvent::PhaseChanged {
            phase: RecordingPhase::Recording,
            idle_hot: false,
        };

        let (state, actions) = transition(VisualState::Hidden, &domain, event, None);

        assert_eq!(state, VisualState::Appearing);
        assert_eq!(
            actions,
            vec![
                OsdAction::CreateWindow,
                OsdAction::StartAppearAnimation,
                OsdAction::StartPulseAnimation,
            ]
        );
    }

    #[test]
    fn test_hidden_to_appearing_on_preview_requested() {
        let domain = idle_domain();
        let event = OsdEvent::PreviewRequested;

        let (state, actions) = transition(VisualState::Hidden, &domain, event, None);

        assert_eq!(state, VisualState::Appearing);
        assert_eq!(
            actions,
            vec![OsdAction::CreateWindow, OsdAction::StartAppearAnimation,]
        );
        // Verify NO pulse animation or linger timer for preview (linger starts after appear completes)
        assert!(!actions.contains(&OsdAction::StartPulseAnimation));
        assert!(!actions.iter().any(|a| matches!(
            a,
            OsdAction::StartLingerTimer { .. }
        )));
    }

    #[test]
    fn test_appearing_to_visible_on_completion() {
        let domain = recording_domain();
        let event = OsdEvent::AppearComplete { generation: 42 };

        let (state, actions) = transition(VisualState::Appearing, &domain, event, Some(42));

        assert_eq!(state, VisualState::Visible);
        assert!(actions.is_empty());
    }

    #[test]
    fn test_appearing_ignores_stale_completion() {
        let domain = recording_domain();
        let event = OsdEvent::AppearComplete { generation: 42 };

        // Wrong generation - should be ignored
        let (state, actions) = transition(VisualState::Appearing, &domain, event, Some(99));

        assert_eq!(state, VisualState::Appearing); // No change
        assert!(actions.is_empty());
    }

    #[test]
    fn test_visible_to_hovering() {
        let domain = recording_domain();
        let event = OsdEvent::MouseEnter;

        let (state, actions) = transition(VisualState::Visible, &domain, event, None);

        assert_eq!(state, VisualState::Hovering);
        assert!(actions.is_empty());
    }

    #[test]
    fn test_hovering_to_visible() {
        let domain = recording_domain();
        let event = OsdEvent::MouseExit;

        let (state, actions) = transition(VisualState::Hovering, &domain, event, None);

        assert_eq!(state, VisualState::Visible);
        assert!(actions.is_empty());
    }

    #[test]
    fn test_visible_to_lingering_on_idle() {
        let domain = idle_domain();
        let event = OsdEvent::PhaseChanged {
            phase: RecordingPhase::Idle,
            idle_hot: false,
        };

        let (state, actions) = transition(VisualState::Visible, &domain, event, None);

        assert_eq!(state, VisualState::Lingering);
        assert_eq!(
            actions,
            vec![
                OsdAction::StartLingerTimer {
                    duration: timing::LINGER
                },
                OsdAction::StopPulseAnimation,
            ]
        );
    }

    #[test]
    fn test_lingering_to_disappearing_on_timer_expiry() {
        let domain = idle_domain();
        let event = OsdEvent::LingerExpired;

        let (state, actions) = transition(VisualState::Lingering, &domain, event, None);

        assert_eq!(state, VisualState::Disappearing);
        assert_eq!(actions, vec![OsdAction::StartDisappearAnimation]);
    }

    #[test]
    fn test_lingering_to_visible_on_reactivation() {
        let domain = recording_domain();
        let event = OsdEvent::PhaseChanged {
            phase: RecordingPhase::Recording,
            idle_hot: false,
        };

        let (state, actions) = transition(VisualState::Lingering, &domain, event, None);

        assert_eq!(state, VisualState::Visible);
        assert_eq!(
            actions,
            vec![OsdAction::CancelLingerTimer, OsdAction::StartPulseAnimation]
        );
    }

    #[test]
    fn test_disappearing_to_hidden_on_completion() {
        let domain = idle_domain();
        let event = OsdEvent::DisappearComplete { generation: 42 };

        let (state, actions) = transition(VisualState::Disappearing, &domain, event, Some(42));

        assert_eq!(state, VisualState::Hidden);
        assert_eq!(actions, vec![OsdAction::DestroyWindow]);
    }

    #[test]
    fn test_disappearing_ignores_stale_completion() {
        let domain = idle_domain();
        let event = OsdEvent::DisappearComplete { generation: 42 };

        // Wrong generation - should be ignored
        let (state, actions) = transition(VisualState::Disappearing, &domain, event, Some(99));

        assert_eq!(state, VisualState::Disappearing); // No change
        assert!(actions.is_empty());
    }

    #[test]
    fn test_disappearing_to_appearing_on_reactivation() {
        let domain = recording_domain();
        let event = OsdEvent::PhaseChanged {
            phase: RecordingPhase::Recording,
            idle_hot: false,
        };

        let (state, actions) = transition(VisualState::Disappearing, &domain, event, None);

        assert_eq!(state, VisualState::Appearing);
        assert_eq!(
            actions,
            vec![OsdAction::StartAppearAnimation, OsdAction::StartPulseAnimation]
        );
    }

    #[test]
    fn test_force_hide_from_any_state() {
        let domain = recording_domain();
        let event = OsdEvent::ForceHide;

        for initial_state in [
            VisualState::Hidden,
            VisualState::Appearing,
            VisualState::Visible,
            VisualState::Hovering,
            VisualState::Lingering,
            VisualState::Disappearing,
        ] {
            let (state, actions) = transition(initial_state, &domain, event.clone(), None);

            assert_eq!(state, VisualState::Hidden);
            assert_eq!(
                actions,
                vec![OsdAction::DestroyWindow, OsdAction::CancelLingerTimer]
            );
        }
    }

    #[test]
    fn test_osd_state_new() {
        let state = OsdState::new();
        assert_eq!(state.visual, VisualState::Hidden);
        assert_eq!(state.domain.phase, RecordingPhase::Idle);
        assert!(!state.domain.idle_hot);
    }

    #[test]
    fn test_osd_state_update_domain() {
        let mut state = OsdState::new();

        state.update_domain(RecordingPhase::Recording, true);

        assert_eq!(state.domain.phase, RecordingPhase::Recording);
        assert!(state.domain.idle_hot);
    }

    #[test]
    fn test_osd_state_transition() {
        let mut state = OsdState::new();

        // Start recording
        let actions = state.transition(
            OsdEvent::PhaseChanged {
                phase: RecordingPhase::Recording,
                idle_hot: false,
            },
            None,
        );

        assert_eq!(state.visual, VisualState::Appearing);
        assert_eq!(
            actions,
            vec![
                OsdAction::CreateWindow,
                OsdAction::StartAppearAnimation,
                OsdAction::StartPulseAnimation,
            ]
        );
    }

    #[test]
    fn test_preview_flow_complete() {
        let mut state = OsdState::new();

        // 1. Preview requested (domain is idle)
        let actions = state.transition(OsdEvent::PreviewRequested, None);
        assert_eq!(state.visual, VisualState::Appearing);
        assert_eq!(
            actions,
            vec![OsdAction::CreateWindow, OsdAction::StartAppearAnimation,]
        );

        // 2. Appear animation completes - goes to Lingering (not Visible) for preview
        let actions = state.transition(OsdEvent::AppearComplete { generation: 1 }, Some(1));
        assert_eq!(state.visual, VisualState::Lingering);
        assert_eq!(
            actions,
            vec![OsdAction::StartLingerTimer {
                duration: timing::LINGER
            }]
        );

        // 3. Linger timer expires
        let actions = state.transition(OsdEvent::LingerExpired, None);
        assert_eq!(state.visual, VisualState::Disappearing);
        assert_eq!(actions, vec![OsdAction::StartDisappearAnimation]);

        // 4. Disappear animation completes
        let actions = state.transition(OsdEvent::DisappearComplete { generation: 2 }, Some(2));
        assert_eq!(state.visual, VisualState::Hidden);
        assert_eq!(actions, vec![OsdAction::DestroyWindow]);
    }

    #[test]
    fn test_full_recording_flow() {
        let mut state = OsdState::new();

        // 1. Start recording
        state.update_domain(RecordingPhase::Recording, false);
        let actions = state.transition(
            OsdEvent::PhaseChanged {
                phase: RecordingPhase::Recording,
                idle_hot: false,
            },
            None,
        );
        assert_eq!(state.visual, VisualState::Appearing);
        assert!(actions.contains(&OsdAction::StartPulseAnimation));

        // 2. Appear complete
        state.transition(OsdEvent::AppearComplete { generation: 1 }, Some(1));
        assert_eq!(state.visual, VisualState::Visible);

        // 3. Stop recording
        state.update_domain(RecordingPhase::Idle, false);
        let actions = state.transition(
            OsdEvent::PhaseChanged {
                phase: RecordingPhase::Idle,
                idle_hot: false,
            },
            None,
        );
        assert_eq!(state.visual, VisualState::Lingering);
        assert!(actions.contains(&OsdAction::StopPulseAnimation));

        // 4. Linger expires
        state.transition(OsdEvent::LingerExpired, None);
        assert_eq!(state.visual, VisualState::Disappearing);

        // 5. Disappear complete
        state.transition(OsdEvent::DisappearComplete { generation: 2 }, Some(2));
        assert_eq!(state.visual, VisualState::Hidden);
    }

    #[test]
    fn test_interruption_while_disappearing() {
        let mut state = OsdState::new();
        state.visual = VisualState::Disappearing;

        // Recording starts while disappearing
        state.update_domain(RecordingPhase::Recording, false);
        let actions = state.transition(
            OsdEvent::PhaseChanged {
                phase: RecordingPhase::Recording,
                idle_hot: false,
            },
            None,
        );

        assert_eq!(state.visual, VisualState::Appearing);
        assert_eq!(
            actions,
            vec![OsdAction::StartAppearAnimation, OsdAction::StartPulseAnimation]
        );
    }

    #[test]
    fn test_no_action_on_unhandled_events() {
        let domain = idle_domain();

        // Mouse events in wrong state
        let (state, actions) = transition(VisualState::Hidden, &domain, OsdEvent::MouseEnter, None);
        assert_eq!(state, VisualState::Hidden);
        assert!(actions.is_empty());

        // Linger expired in wrong state
        let (state, actions) =
            transition(VisualState::Visible, &domain, OsdEvent::LingerExpired, None);
        assert_eq!(state, VisualState::Visible);
        assert!(actions.is_empty());
    }

    // =========================================================================
    // Comprehensive State Machine Tests (dictate-d0g)
    // =========================================================================

    /// Test all PhaseChanged(active) transitions from all states
    #[test]
    fn test_phase_active_from_all_states() {
        let active = recording_domain();
        let event = OsdEvent::PhaseChanged {
            phase: RecordingPhase::Recording,
            idle_hot: false,
        };

        // Hidden -> Appearing with window creation
        let (next, actions) = transition(VisualState::Hidden, &active, event.clone(), None);
        assert_eq!(next, VisualState::Appearing);
        assert!(actions.contains(&OsdAction::CreateWindow));
        assert!(actions.contains(&OsdAction::StartAppearAnimation));
        assert!(actions.contains(&OsdAction::StartPulseAnimation));

        // Appearing -> no change
        let (next, actions) = transition(VisualState::Appearing, &active, event.clone(), None);
        assert_eq!(next, VisualState::Appearing);
        assert!(actions.is_empty());

        // Visible -> no change
        let (next, actions) = transition(VisualState::Visible, &active, event.clone(), None);
        assert_eq!(next, VisualState::Visible);
        assert!(actions.is_empty());

        // Hovering -> no change
        let (next, actions) = transition(VisualState::Hovering, &active, event.clone(), None);
        assert_eq!(next, VisualState::Hovering);
        assert!(actions.is_empty());

        // Lingering -> Visible
        let (next, actions) = transition(VisualState::Lingering, &active, event.clone(), None);
        assert_eq!(next, VisualState::Visible);
        assert!(actions.contains(&OsdAction::CancelLingerTimer));
        assert!(actions.contains(&OsdAction::StartPulseAnimation));

        // Disappearing -> Appearing
        let (next, actions) = transition(VisualState::Disappearing, &active, event, None);
        assert_eq!(next, VisualState::Appearing);
        assert!(actions.contains(&OsdAction::StartAppearAnimation));
        assert!(actions.contains(&OsdAction::StartPulseAnimation));
    }

    /// Test all PhaseChanged(idle) transitions from all states
    #[test]
    fn test_phase_idle_from_all_states() {
        let idle = idle_domain();
        let event = OsdEvent::PhaseChanged {
            phase: RecordingPhase::Idle,
            idle_hot: false,
        };

        // Hidden -> no change
        let (next, actions) = transition(VisualState::Hidden, &idle, event.clone(), None);
        assert_eq!(next, VisualState::Hidden);
        assert!(actions.is_empty());

        // Appearing -> no change
        let (next, actions) = transition(VisualState::Appearing, &idle, event.clone(), None);
        assert_eq!(next, VisualState::Appearing);
        assert!(actions.is_empty());

        // Visible -> Lingering
        let (next, actions) = transition(VisualState::Visible, &idle, event.clone(), None);
        assert_eq!(next, VisualState::Lingering);
        assert!(actions.iter().any(|a| matches!(a, OsdAction::StartLingerTimer { .. })));
        assert!(actions.contains(&OsdAction::StopPulseAnimation));

        // Hovering -> no change (KNOWN BUG: dictate-1lu)
        let (next, actions) = transition(VisualState::Hovering, &idle, event.clone(), None);
        assert_eq!(next, VisualState::Hovering);
        assert!(actions.is_empty());

        // Lingering -> no change
        let (next, actions) = transition(VisualState::Lingering, &idle, event.clone(), None);
        assert_eq!(next, VisualState::Lingering);
        assert!(actions.is_empty());

        // Disappearing -> no change
        let (next, actions) = transition(VisualState::Disappearing, &idle, event, None);
        assert_eq!(next, VisualState::Disappearing);
        assert!(actions.is_empty());
    }

    /// Test stale AppearComplete events are always no-op
    #[test]
    fn test_stale_appear_complete_comprehensive() {
        let active = recording_domain();
        
        // Test with None generation
        let event = OsdEvent::AppearComplete { generation: 42 };
        let (state, actions) = transition(VisualState::Appearing, &active, event.clone(), None);
        assert_eq!(state, VisualState::Appearing);
        assert!(actions.is_empty());

        // Test with mismatched generations
        let (state, actions) = transition(VisualState::Appearing, &active, event.clone(), Some(1));
        assert_eq!(state, VisualState::Appearing);
        assert!(actions.is_empty());

        let (state, actions) = transition(VisualState::Appearing, &active, event.clone(), Some(41));
        assert_eq!(state, VisualState::Appearing);
        assert!(actions.is_empty());

        let (state, actions) = transition(VisualState::Appearing, &active, event.clone(), Some(43));
        assert_eq!(state, VisualState::Appearing);
        assert!(actions.is_empty());

        let (state, actions) = transition(VisualState::Appearing, &active, event, Some(999));
        assert_eq!(state, VisualState::Appearing);
        assert!(actions.is_empty());
    }

    /// Test stale DisappearComplete events are always no-op
    #[test]
    fn test_stale_disappear_complete_comprehensive() {
        let idle = idle_domain();
        
        // Test with None generation
        let event = OsdEvent::DisappearComplete { generation: 42 };
        let (state, actions) = transition(VisualState::Disappearing, &idle, event.clone(), None);
        assert_eq!(state, VisualState::Disappearing);
        assert!(actions.is_empty());

        // Test with mismatched generations
        let (state, actions) = transition(VisualState::Disappearing, &idle, event.clone(), Some(1));
        assert_eq!(state, VisualState::Disappearing);
        assert!(actions.is_empty());

        let (state, actions) = transition(VisualState::Disappearing, &idle, event.clone(), Some(41));
        assert_eq!(state, VisualState::Disappearing);
        assert!(actions.is_empty());

        let (state, actions) = transition(VisualState::Disappearing, &idle, event.clone(), Some(43));
        assert_eq!(state, VisualState::Disappearing);
        assert!(actions.is_empty());

        let (state, actions) = transition(VisualState::Disappearing, &idle, event, Some(999));
        assert_eq!(state, VisualState::Disappearing);
        assert!(actions.is_empty());
    }

    /// Test event sequences: full recording cycle
    #[test]
    fn test_sequence_recording_cycle_complete() {
        let mut state = OsdState::new();

        // 1. Start recording
        state.update_domain(RecordingPhase::Recording, false);
        state.transition(
            OsdEvent::PhaseChanged {
                phase: RecordingPhase::Recording,
                idle_hot: false,
            },
            None,
        );
        assert_eq!(state.visual, VisualState::Appearing);

        // 2. Appear completes
        state.transition(OsdEvent::AppearComplete { generation: 1 }, Some(1));
        assert_eq!(state.visual, VisualState::Visible);

        // 3. Stop recording
        state.update_domain(RecordingPhase::Idle, false);
        state.transition(
            OsdEvent::PhaseChanged {
                phase: RecordingPhase::Idle,
                idle_hot: false,
            },
            None,
        );
        assert_eq!(state.visual, VisualState::Lingering);

        // 4. Linger expires
        state.transition(OsdEvent::LingerExpired, None);
        assert_eq!(state.visual, VisualState::Disappearing);

        // 5. Disappear completes
        state.transition(OsdEvent::DisappearComplete { generation: 2 }, Some(2));
        assert_eq!(state.visual, VisualState::Hidden);
    }

    /// Test event sequences: interrupted disappear
    #[test]
    fn test_sequence_interrupted_disappear() {
        let mut state = OsdState::new();

        state.update_domain(RecordingPhase::Recording, false);
        state.transition(
            OsdEvent::PhaseChanged {
                phase: RecordingPhase::Recording,
                idle_hot: false,
            },
            None,
        );
        state.transition(OsdEvent::AppearComplete { generation: 1 }, Some(1));
        state.update_domain(RecordingPhase::Idle, false);
        state.transition(
            OsdEvent::PhaseChanged {
                phase: RecordingPhase::Idle,
                idle_hot: false,
            },
            None,
        );
        state.transition(OsdEvent::LingerExpired, None);
        assert_eq!(state.visual, VisualState::Disappearing);

        // Interrupt with new recording
        state.update_domain(RecordingPhase::Recording, false);
        state.transition(
            OsdEvent::PhaseChanged {
                phase: RecordingPhase::Recording,
                idle_hot: false,
            },
            None,
        );
        assert_eq!(state.visual, VisualState::Appearing);
    }

    /// Test event sequences: multiple stale completions ignored
    #[test]
    fn test_sequence_stale_completions_ignored() {
        let mut state = OsdState::new();

        state.update_domain(RecordingPhase::Recording, false);
        state.transition(
            OsdEvent::PhaseChanged {
                phase: RecordingPhase::Recording,
                idle_hot: false,
            },
            None,
        );

        // Send stale completions
        state.transition(OsdEvent::AppearComplete { generation: 999 }, Some(1));
        assert_eq!(state.visual, VisualState::Appearing);

        state.transition(OsdEvent::AppearComplete { generation: 1000 }, Some(1));
        assert_eq!(state.visual, VisualState::Appearing);

        state.transition(OsdEvent::AppearComplete { generation: 1001 }, Some(1));
        assert_eq!(state.visual, VisualState::Appearing);

        // Correct generation works
        state.transition(OsdEvent::AppearComplete { generation: 1 }, Some(1));
        assert_eq!(state.visual, VisualState::Visible);
    }

    /// Test invariant: idle domain never has Visible/Hovering visual state
    #[test]
    fn test_invariant_idle_not_visible() {
        let mut state = OsdState::new();

        // Idle domain starts Hidden
        assert!(!state.domain.phase.is_active());
        assert_eq!(state.visual, VisualState::Hidden);

        // Preview flow: Appearing is transiently OK
        state.transition(OsdEvent::PreviewRequested, None);
        assert_eq!(state.visual, VisualState::Appearing);
        assert!(!state.domain.phase.is_active());

        // But completing appear goes to Lingering (not Visible) when idle
        state.transition(OsdEvent::AppearComplete { generation: 1 }, Some(1));
        assert_eq!(state.visual, VisualState::Lingering);
        assert!(!state.domain.phase.is_active());

        // Complete the cycle
        state.transition(OsdEvent::LingerExpired, None);
        assert_eq!(state.visual, VisualState::Disappearing);

        state.transition(OsdEvent::DisappearComplete { generation: 2 }, Some(2));
        assert_eq!(state.visual, VisualState::Hidden);
        assert!(!state.domain.phase.is_active());
    }
}
