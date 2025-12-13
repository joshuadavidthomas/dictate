//! Message Domain Grouping Pattern
//!
//! Inspired by COSMIC applets, this module organizes messages into domains
//! for cleaner update() logic and better code organization.
//!
//! Benefits:
//! - Clear separation of concerns in update()
//! - Easy to add new message types without cluttering main enum
//! - Self-documenting message flow
//! - Easier testing of individual domains

use crate::recording::RecordingSnapshot;

/// Top-level message enum with domain grouping
///
/// Each variant represents a domain of functionality, making the
/// update() function more organized and readable.
#[derive(Debug, Clone)]
pub enum DomainMessage {
    /// Animation-related messages (ticks, frame requests)
    Animation(AnimationMessage),

    /// User interaction messages (mouse, keyboard)
    Interaction(InteractionMessage),

    /// External data messages (broadcast channel, IPC)
    External(ExternalMessage),

    /// Window lifecycle messages (from iced-layershell)
    Window(WindowMessage),
}

/// Animation domain messages
#[derive(Debug, Clone)]
pub enum AnimationMessage {
    /// Regular animation tick (60fps)
    Tick,

    /// Animation completed for a specific target
    AnimationComplete { target: AnimationTarget },

    /// Request to start a specific animation
    StartAnimation { target: AnimationTarget },
}

/// Animation targets that can be animated
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnimationTarget {
    WindowAppear,
    WindowDisappear,
    TimerWidth,
    Pulse,
    ContentFade,
}

/// User interaction domain messages
#[derive(Debug, Clone)]
pub enum InteractionMessage {
    /// Mouse entered the OSD window
    MouseEntered,

    /// Mouse exited the OSD window
    MouseExited,

    /// Click on specific element (future use)
    Clicked { element: ClickTarget },
}

/// Clickable elements in the OSD
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClickTarget {
    StatusDot,
    Waveform,
    Timer,
}

/// External data domain messages
#[derive(Debug, Clone)]
pub enum ExternalMessage {
    /// Recording state changed
    StateChanged {
        state: RecordingSnapshot,
        idle_hot: bool,
        timestamp: u64,
    },

    /// New spectrum data received
    SpectrumData { bands: Vec<f32>, timestamp: u64 },

    /// Transcription result received
    TranscriptionResult { text: String },

    /// Error occurred
    Error { message: String },

    /// Configuration changed
    ConfigChanged { position: crate::conf::OsdPosition },
}

/// Window lifecycle domain messages
#[derive(Debug, Clone)]
pub enum WindowMessage {
    /// Window should be created
    Create,

    /// Window should be destroyed
    Destroy,

    /// Window position changed
    Repositioned,
}

/// Helper trait for domain-based update handling
///
/// Implement this trait to handle messages from specific domains,
/// keeping the main update() function clean.
pub trait HandleDomain<M> {
    type Output;

    fn handle(&mut self, message: M) -> Self::Output;
}

/// Example of how to use domain grouping in update():
///
/// ```ignore
/// fn update(&mut self, message: DomainMessage) -> Task<DomainMessage> {
///     match message {
///         DomainMessage::Animation(msg) => self.handle_animation(msg),
///         DomainMessage::Interaction(msg) => self.handle_interaction(msg),
///         DomainMessage::External(msg) => self.handle_external(msg),
///         DomainMessage::Window(msg) => self.handle_window(msg),
///     }
/// }
///
/// fn handle_animation(&mut self, msg: AnimationMessage) -> Task<DomainMessage> {
///     match msg {
///         AnimationMessage::Tick => {
///             self.timeline.tick();
///             self.render_state = self.compute_render_state(Instant::now());
///             Task::none()
///         }
///         AnimationMessage::AnimationComplete { target } => {
///             match target {
///                 AnimationTarget::WindowAppear => {
///                     self.visual_state.transition(StateEvent::AppearComplete);
///                 }
///                 // ... other targets
///             }
///             Task::none()
///         }
///         AnimationMessage::StartAnimation { target } => {
///             match target {
///                 AnimationTarget::WindowAppear => {
///                     self.timeline.set(*ids::WINDOW, WindowAnimation::appear(timing::APPEAR));
///                 }
///                 // ... other targets
///             }
///             Task::none()
///         }
///     }
/// }
/// ```

/// Conversion helpers for easier message construction
impl From<AnimationMessage> for DomainMessage {
    fn from(msg: AnimationMessage) -> Self {
        DomainMessage::Animation(msg)
    }
}

impl From<InteractionMessage> for DomainMessage {
    fn from(msg: InteractionMessage) -> Self {
        DomainMessage::Interaction(msg)
    }
}

impl From<ExternalMessage> for DomainMessage {
    fn from(msg: ExternalMessage) -> Self {
        DomainMessage::External(msg)
    }
}

impl From<WindowMessage> for DomainMessage {
    fn from(msg: WindowMessage) -> Self {
        DomainMessage::Window(msg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message_conversion() {
        let anim_msg = AnimationMessage::Tick;
        let domain_msg: DomainMessage = anim_msg.into();
        assert!(matches!(domain_msg, DomainMessage::Animation(_)));
    }

    #[test]
    fn test_animation_target_equality() {
        assert_eq!(AnimationTarget::WindowAppear, AnimationTarget::WindowAppear);
        assert_ne!(AnimationTarget::WindowAppear, AnimationTarget::Pulse);
    }
}
