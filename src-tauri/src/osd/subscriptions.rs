//! Subscription Batching Pattern
//!
//! Inspired by COSMIC applets, this module provides clean organization
//! of async subscriptions with explicit batching.
//!
//! Benefits:
//! - Clear separation of subscription sources
//! - Easy to add/remove subscriptions
//! - Self-documenting async data flow
//! - Centralized subscription configuration

use iced::time::{self, Duration};
use iced::Subscription;
use std::time::Duration as StdDuration;

/// Subscription configuration
pub struct SubscriptionConfig {
    /// Animation frame rate (fps)
    pub animation_fps: u32,

    /// Whether animation subscription is enabled
    pub animation_enabled: bool,

    /// Broadcast polling interval (if not using native subscription)
    pub broadcast_poll_interval: Option<StdDuration>,
}

impl Default for SubscriptionConfig {
    fn default() -> Self {
        Self {
            animation_fps: 60,
            animation_enabled: true,
            broadcast_poll_interval: None,
        }
    }
}

impl SubscriptionConfig {
    /// Create config for high-performance animation
    pub fn high_fps() -> Self {
        Self {
            animation_fps: 120,
            ..Default::default()
        }
    }

    /// Create config for power-saving mode
    pub fn power_saving() -> Self {
        Self {
            animation_fps: 30,
            ..Default::default()
        }
    }
}

/// Subscription builder for the OSD application
///
/// Usage:
/// ```ignore
/// fn subscription(&self) -> Subscription<Message> {
///     OsdSubscriptions::new()
///         .animation(Message::Tick)
///         .with_config(SubscriptionConfig::default())
///         .build()
/// }
/// ```
pub struct OsdSubscriptions<Message> {
    config: SubscriptionConfig,
    subscriptions: Vec<Subscription<Message>>,
}

impl<Message: Clone + Send + 'static> OsdSubscriptions<Message> {
    /// Create a new subscription builder
    pub fn new() -> Self {
        Self {
            config: SubscriptionConfig::default(),
            subscriptions: Vec::new(),
        }
    }

    /// Set custom configuration
    pub fn with_config(mut self, config: SubscriptionConfig) -> Self {
        self.config = config;
        self
    }

    /// Add animation tick subscription
    ///
    /// Provides regular frame updates for smooth animations.
    pub fn animation(mut self, on_tick: Message) -> Self
    where
        Message: Clone,
    {
        if self.config.animation_enabled {
            let interval = Duration::from_secs_f64(1.0 / self.config.animation_fps as f64);
            self.subscriptions
                .push(time::every(interval).map(move |_| on_tick.clone()));
        }
        self
    }

    /// Add a custom subscription
    ///
    /// For adding external data sources, IPC channels, etc.
    pub fn custom(mut self, subscription: Subscription<Message>) -> Self {
        self.subscriptions.push(subscription);
        self
    }

    /// Add keyboard event subscription (for future use)
    #[allow(dead_code)]
    pub fn keyboard<F>(mut self, handler: F) -> Self
    where
        F: Fn(iced::keyboard::Event) -> Option<Message> + Send + 'static,
    {
        self.subscriptions
            .push(iced::event::listen_with(move |event, _status, _id| {
                if let iced::Event::Keyboard(keyboard_event) = event {
                    handler(keyboard_event)
                } else {
                    None
                }
            }));
        self
    }

    /// Build the final batched subscription
    pub fn build(self) -> Subscription<Message> {
        Subscription::batch(self.subscriptions)
    }
}

impl<Message: Clone + Send + 'static> Default for OsdSubscriptions<Message> {
    fn default() -> Self {
        Self::new()
    }
}

/// Example of subscription batching in practice:
///
/// ```ignore
/// impl OsdApp {
///     fn subscription(&self) -> Subscription<Message> {
///         // Using the builder pattern
///         OsdSubscriptions::new()
///             .animation(Message::Tick)
///             .build()
///
///         // Or manually with Subscription::batch for more control:
///         Subscription::batch([
///             // Animation frames - 60fps for smooth animations
///             time::every(Duration::from_millis(16)).map(|_| Message::Tick),
///
///             // Window events (handled by iced-layershell)
///             // iced_layershell provides these automatically
///
///             // Future: Separate broadcast channel subscription
///             // broadcast::subscription(self.broadcast_rx.resubscribe())
///             //     .map(Message::BroadcastEvent),
///
///             // Future: Keyboard shortcuts
///             // keyboard::on_key_press(|key, modifiers| {
///             //     match (key, modifiers) {
///             //         (Key::Escape, _) => Some(Message::Dismiss),
///             //         _ => None,
///             //     }
///             // }),
///         ])
///     }
/// }
/// ```

/// Subscription recipe for broadcast channel (future enhancement)
///
/// This could be used to create a proper async subscription from the
/// broadcast channel instead of polling in the tick handler.
#[allow(dead_code)]
pub mod broadcast_recipe {
    use tokio::sync::broadcast;

    /// A recipe for subscribing to broadcast messages
    ///
    /// Usage (when implemented):
    /// ```ignore
    /// fn subscription(&self) -> Subscription<Message> {
    ///     Subscription::batch([
    ///         time::every(Duration::from_millis(16)).map(|_| Message::Tick),
    ///         broadcast_subscription(self.broadcast_rx.resubscribe())
    ///             .map(Message::BroadcastEvent),
    ///     ])
    /// }
    /// ```
    pub struct BroadcastRecipe<T: Clone> {
        #[allow(dead_code)]
        receiver: broadcast::Receiver<T>,
    }

    impl<T: Clone> BroadcastRecipe<T> {
        pub fn new(receiver: broadcast::Receiver<T>) -> Self {
            Self { receiver }
        }
    }

    // Note: Full implementation would require implementing iced::subscription::Recipe
    // This is a placeholder showing the pattern
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone)]
    enum TestMessage {
        Tick,
    }

    #[test]
    fn test_subscription_builder() {
        let _sub: Subscription<TestMessage> =
            OsdSubscriptions::new().animation(TestMessage::Tick).build();
    }

    #[test]
    fn test_config_presets() {
        let high_fps = SubscriptionConfig::high_fps();
        assert_eq!(high_fps.animation_fps, 120);

        let power_saving = SubscriptionConfig::power_saving();
        assert_eq!(power_saving.animation_fps, 30);
    }
}
