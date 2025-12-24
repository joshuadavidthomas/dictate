//! Visual State Machine for the OSD
//!
//! Consolidates scattered state flags into an explicit state machine,
//! inspired by waypomo's `State` enum pattern.

use std::time::Instant;

use crate::recording::RecordingSnapshot;

/// Visual state of the OSD window
///
/// This enum represents the complete visual state of the OSD,
/// consolidating multiple boolean flags into explicit states
/// with explicit transitions.
#[derive(Debug, Clone)]
pub enum OsdVisualState {
    /// Window is hidden, not rendered
    Hidden,

    /// Window is appearing with animation
    Appearing {
        started_at: Instant,
        target: RecordingSnapshot,
        idle_hot: bool,
    },

    /// Window is fully visible and showing state
    Visible {
        state: RecordingSnapshot,
        idle_hot: bool,
    },

    /// Mouse is hovering over the window (prevents disappearing)
    Hovering {
        state: RecordingSnapshot,
        idle_hot: bool,
    },

    /// Window is lingering after state change before disappearing
    Lingering {
        state: RecordingSnapshot,
        idle_hot: bool,
        until: Instant,
    },

    /// Window is disappearing with animation
    Disappearing {
        started_at: Instant,
        previous_state: RecordingSnapshot,
    },
}

/// Events that can trigger state transitions
#[derive(Debug, Clone)]
pub enum StateEvent {
    /// Show the OSD with a specific recording state
    Show {
        state: RecordingSnapshot,
        idle_hot: bool,
    },

    /// Recording state changed while visible
    StateChanged {
        state: RecordingSnapshot,
        idle_hot: bool,
    },

    /// Window appearance animation completed
    AppearComplete,

    /// Mouse entered the window
    MouseEnter,

    /// Mouse exited the window
    MouseExit,

    /// Start lingering period before disappearing
    StartLinger { until: Instant },

    /// Linger period expired
    LingerExpired,

    /// Start disappearing animation
    StartDisappear,

    /// Window disappear animation completed
    DisappearComplete,

    /// Force hide the window immediately
    ForceHide,
}

impl Default for OsdVisualState {
    fn default() -> Self {
        OsdVisualState::Hidden
    }
}

impl OsdVisualState {
    /// Create a new hidden state
    pub fn new() -> Self {
        Self::default()
    }

    /// Process an event and transition to the next state
    pub fn transition(&mut self, event: StateEvent) {
        use OsdVisualState::*;
        use StateEvent::*;

        *self = match (&*self, event) {
            // From Hidden
            (Hidden, Show { state, idle_hot }) => Appearing {
                started_at: Instant::now(),
                target: state,
                idle_hot,
            },

            // From Appearing
            (
                Appearing {
                    target, idle_hot, ..
                },
                AppearComplete,
            ) => Visible {
                state: target.clone(),
                idle_hot: *idle_hot,
            },
            (Appearing { idle_hot, .. }, StateChanged { state, .. }) => Appearing {
                started_at: Instant::now(),
                target: state,
                idle_hot: *idle_hot,
            },

            // From Visible
            (Visible { state, idle_hot }, MouseEnter) => Hovering {
                state: state.clone(),
                idle_hot: *idle_hot,
            },
            (Visible { .. }, StartLinger { until }) => {
                let (state, idle_hot) = self.current_state_info();
                Lingering {
                    state,
                    idle_hot,
                    until,
                }
            }
            (Visible { idle_hot, .. }, StateChanged { state, .. }) => Visible {
                state,
                idle_hot: *idle_hot,
            },
            (Visible { .. }, StartDisappear) => {
                let (state, _) = self.current_state_info();
                Disappearing {
                    started_at: Instant::now(),
                    previous_state: state,
                }
            }

            // From Hovering
            (Hovering { state, idle_hot }, MouseExit) => Visible {
                state: state.clone(),
                idle_hot: *idle_hot,
            },
            (Hovering { idle_hot, .. }, StateChanged { state, .. }) => Hovering {
                state,
                idle_hot: *idle_hot,
            },

            // From Lingering
            (
                Lingering {
                    state, idle_hot, ..
                },
                MouseEnter,
            ) => Hovering {
                state: state.clone(),
                idle_hot: *idle_hot,
            },
            (Lingering { .. }, LingerExpired) => {
                let (state, _) = self.current_state_info();
                Disappearing {
                    started_at: Instant::now(),
                    previous_state: state,
                }
            }
            (Lingering { idle_hot, .. }, StateChanged { state, .. }) => {
                // If we get a new active state while lingering, become visible again
                if matches!(
                    state,
                    RecordingSnapshot::Recording | RecordingSnapshot::Transcribing
                ) {
                    Visible {
                        state,
                        idle_hot: *idle_hot,
                    }
                } else {
                    // Otherwise keep lingering with updated state
                    let until = self.linger_until().unwrap_or_else(Instant::now);
                    Lingering {
                        state,
                        idle_hot: *idle_hot,
                        until,
                    }
                }
            }

            // From Disappearing
            (Disappearing { .. }, DisappearComplete) => Hidden,
            (Disappearing { .. }, Show { state, idle_hot }) => Appearing {
                started_at: Instant::now(),
                target: state,
                idle_hot,
            },

            // Force hide from any state
            (_, ForceHide) => Hidden,

            // Default: no change
            (current, _) => current.clone(),
        };
    }

    /// Returns true if the window should be visible (rendered)
    pub fn is_visible(&self) -> bool {
        !matches!(self, OsdVisualState::Hidden)
    }

    /// Returns true if currently in appearing animation
    pub fn is_appearing(&self) -> bool {
        matches!(self, OsdVisualState::Appearing { .. })
    }

    /// Returns true if currently in disappearing animation
    pub fn is_disappearing(&self) -> bool {
        matches!(self, OsdVisualState::Disappearing { .. })
    }

    /// Returns true if mouse is hovering
    pub fn is_hovering(&self) -> bool {
        matches!(self, OsdVisualState::Hovering { .. })
    }

    /// Returns true if currently lingering
    pub fn is_lingering(&self) -> bool {
        matches!(self, OsdVisualState::Lingering { .. })
    }

    /// Get animation start time if in animation state
    pub fn animation_start(&self) -> Option<Instant> {
        match self {
            OsdVisualState::Appearing { started_at, .. } => Some(*started_at),
            OsdVisualState::Disappearing { started_at, .. } => Some(*started_at),
            _ => None,
        }
    }

    /// Get linger until time if lingering
    pub fn linger_until(&self) -> Option<Instant> {
        match self {
            OsdVisualState::Lingering { until, .. } => Some(*until),
            _ => None,
        }
    }

    /// Get current recording state and idle_hot flag
    pub fn current_state_info(&self) -> (RecordingSnapshot, bool) {
        match self {
            OsdVisualState::Hidden => (RecordingSnapshot::Idle, false),
            OsdVisualState::Appearing {
                target, idle_hot, ..
            } => (target.clone(), *idle_hot),
            OsdVisualState::Visible { state, idle_hot } => (state.clone(), *idle_hot),
            OsdVisualState::Hovering { state, idle_hot } => (state.clone(), *idle_hot),
            OsdVisualState::Lingering {
                state, idle_hot, ..
            } => (state.clone(), *idle_hot),
            OsdVisualState::Disappearing { previous_state, .. } => (previous_state.clone(), false),
        }
    }

    /// Get the recording state (convenience method)
    pub fn recording_state(&self) -> RecordingSnapshot {
        self.current_state_info().0
    }

    /// Get idle_hot flag (convenience method)
    pub fn idle_hot(&self) -> bool {
        self.current_state_info().1
    }

    /// Check if the current state requires an active window
    pub fn needs_window(&self) -> bool {
        match self {
            OsdVisualState::Hidden => false,
            OsdVisualState::Appearing { .. } => true,
            OsdVisualState::Visible { state, .. } => matches!(
                state,
                RecordingSnapshot::Recording
                    | RecordingSnapshot::Transcribing
                    | RecordingSnapshot::Error
            ),
            OsdVisualState::Hovering { .. } => true,
            OsdVisualState::Lingering { .. } => true,
            OsdVisualState::Disappearing { .. } => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hidden_to_appearing() {
        let mut state = OsdVisualState::Hidden;
        state.transition(StateEvent::Show {
            state: RecordingSnapshot::Recording,
            idle_hot: false,
        });
        assert!(matches!(state, OsdVisualState::Appearing { .. }));
    }

    #[test]
    fn test_appearing_to_visible() {
        let mut state = OsdVisualState::Appearing {
            started_at: Instant::now(),
            target: RecordingSnapshot::Recording,
            idle_hot: false,
        };
        state.transition(StateEvent::AppearComplete);
        assert!(matches!(state, OsdVisualState::Visible { .. }));
    }

    #[test]
    fn test_visible_to_hovering() {
        let mut state = OsdVisualState::Visible {
            state: RecordingSnapshot::Recording,
            idle_hot: false,
        };
        state.transition(StateEvent::MouseEnter);
        assert!(matches!(state, OsdVisualState::Hovering { .. }));
    }

    #[test]
    fn test_disappearing_to_hidden() {
        let mut state = OsdVisualState::Disappearing {
            started_at: Instant::now(),
            previous_state: RecordingSnapshot::Idle,
        };
        state.transition(StateEvent::DisappearComplete);
        assert!(matches!(state, OsdVisualState::Hidden));
    }
}
