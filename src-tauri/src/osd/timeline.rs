//! Timeline-based animation system
//!
//! Inspired by cosmic-time, this module provides declarative keyframe-based animations
//! without external dependencies. It maintains compatibility with iced 0.13.
//!
//! Key concepts:
//! - **Timeline**: Manages all active animations
//! - **AnimationId**: Type-safe identifier for animations
//! - **Keyframe**: Defines a target value at a specific duration
//! - **Chain**: Sequence of keyframes with easing functions
//! - **Generation**: Prevents stale completion events from old animations

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

// =============================================================================
// Animation IDs
// =============================================================================

/// Counter for generating unique animation IDs
static ANIMATION_ID_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Counter for generating animation generations (prevents stale events)
static GENERATION_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Type-safe animation identifier
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AnimationId(u64);

impl AnimationId {
    /// Create a new unique animation ID
    pub fn unique() -> Self {
        Self(ANIMATION_ID_COUNTER.fetch_add(1, Ordering::Relaxed))
    }
}

/// Well-known animation IDs for the OSD
pub mod ids {
    use super::AnimationId;
    use std::sync::LazyLock;

    /// Window appear/disappear animation
    pub static WINDOW: LazyLock<AnimationId> = LazyLock::new(AnimationId::unique);

    /// Timer width animation
    pub static TIMER_WIDTH: LazyLock<AnimationId> = LazyLock::new(AnimationId::unique);

    /// Status dot pulse animation
    pub static PULSE: LazyLock<AnimationId> = LazyLock::new(AnimationId::unique);

    /// Content fade animation
    pub static CONTENT: LazyLock<AnimationId> = LazyLock::new(AnimationId::unique);
}

// =============================================================================
// Easing Functions
// =============================================================================

/// Easing function type
pub type EasingFn = fn(f32) -> f32;

/// Linear easing (no acceleration)
pub fn linear(t: f32) -> f32 {
    t
}

/// Ease out cubic - decelerating to zero velocity
pub fn ease_out_cubic(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

/// Ease in cubic - accelerating from zero velocity
pub fn ease_in_cubic(t: f32) -> f32 {
    t.powi(3)
}

/// Ease in-out cubic - acceleration until halfway, then deceleration
pub fn ease_in_out_cubic(t: f32) -> f32 {
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
    }
}

/// Ease out elastic - overshoots slightly
pub fn ease_out_elastic(t: f32) -> f32 {
    if t == 0.0 || t == 1.0 {
        return t;
    }
    let c4 = (2.0 * std::f32::consts::PI) / 3.0;
    2.0_f32.powf(-10.0 * t) * ((t * 10.0 - 0.75) * c4).sin() + 1.0
}

// =============================================================================
// Keyframes
// =============================================================================

/// A single keyframe in an animation
#[derive(Debug, Clone)]
pub struct Keyframe {
    /// Target value at this keyframe
    pub value: f32,
    /// Duration to reach this keyframe from the previous one
    pub duration: Duration,
    /// Easing function to apply
    pub easing: EasingFn,
}

impl Keyframe {
    /// Create a new keyframe
    pub fn new(value: f32, duration: Duration) -> Self {
        Self {
            value,
            duration,
            easing: ease_out_cubic,
        }
    }

    /// Set the easing function
    pub fn with_easing(mut self, easing: EasingFn) -> Self {
        self.easing = easing;
        self
    }
}

// =============================================================================
// Animation Chain
// =============================================================================

/// Direction of animation playback
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayDirection {
    Forward,
    Reverse,
}

/// A chain of keyframes forming a complete animation
#[derive(Debug, Clone)]
pub struct Chain {
    /// Keyframes in order
    keyframes: Vec<Keyframe>,
    /// Whether the animation loops
    pub looping: bool,
    /// Starting value (before first keyframe)
    start_value: f32,
}

impl Chain {
    /// Create a new chain starting from a value
    pub fn new(start_value: f32) -> Self {
        Self {
            keyframes: Vec::new(),
            looping: false,
            start_value,
        }
    }

    /// Add a keyframe to animate to a value over duration
    pub fn then(mut self, value: f32, duration: Duration) -> Self {
        self.keyframes.push(Keyframe::new(value, duration));
        self
    }

    /// Add a keyframe with custom easing
    pub fn then_eased(mut self, value: f32, duration: Duration, easing: EasingFn) -> Self {
        self.keyframes
            .push(Keyframe::new(value, duration).with_easing(easing));
        self
    }

    /// Make the animation loop
    pub fn looping(mut self) -> Self {
        self.looping = true;
        self
    }

    /// Get total duration of the chain
    pub fn total_duration(&self) -> Duration {
        self.keyframes.iter().map(|k| k.duration).sum()
    }

    /// Compute value at a given progress (0.0 to 1.0 for non-looping)
    pub fn value_at(&self, elapsed: Duration) -> f32 {
        if self.keyframes.is_empty() {
            return self.start_value;
        }

        let total = self.total_duration();
        if total.is_zero() {
            return self
                .keyframes
                .last()
                .map(|k| k.value)
                .unwrap_or(self.start_value);
        }

        // Handle looping
        let effective_elapsed = if self.looping {
            Duration::from_secs_f64(elapsed.as_secs_f64() % total.as_secs_f64())
        } else {
            elapsed.min(total)
        };

        // Find which keyframe segment we're in
        let mut accumulated = Duration::ZERO;
        let mut prev_value = self.start_value;

        for keyframe in &self.keyframes {
            let segment_end = accumulated + keyframe.duration;

            if effective_elapsed <= segment_end {
                // We're in this segment
                let segment_elapsed = effective_elapsed - accumulated;
                let t = if keyframe.duration.is_zero() {
                    1.0
                } else {
                    (segment_elapsed.as_secs_f64() / keyframe.duration.as_secs_f64()) as f32
                };
                let eased_t = (keyframe.easing)(t.clamp(0.0, 1.0));
                return prev_value + (keyframe.value - prev_value) * eased_t;
            }

            accumulated = segment_end;
            prev_value = keyframe.value;
        }

        // Past the end
        self.keyframes
            .last()
            .map(|k| k.value)
            .unwrap_or(self.start_value)
    }

    /// Check if animation is complete (for non-looping)
    pub fn is_complete(&self, elapsed: Duration) -> bool {
        !self.looping && elapsed >= self.total_duration()
    }
}

// =============================================================================
// Active Animation
// =============================================================================

/// An animation that is currently running
#[derive(Debug, Clone)]
struct ActiveAnimation {
    chain: Chain,
    started_at: Instant,
    direction: PlayDirection,
    generation: u64,
}

impl ActiveAnimation {
    /// Get current value of the animation
    fn current_value(&self, now: Instant) -> f32 {
        let elapsed = now.saturating_duration_since(self.started_at);

        match self.direction {
            PlayDirection::Forward => self.chain.value_at(elapsed),
            PlayDirection::Reverse => {
                let total = self.chain.total_duration();
                let reverse_elapsed = total.saturating_sub(elapsed);
                self.chain.value_at(reverse_elapsed)
            }
        }
    }

    /// Check if animation is complete
    fn is_complete(&self, now: Instant) -> bool {
        let elapsed = now.saturating_duration_since(self.started_at);
        self.chain.is_complete(elapsed)
    }
}

// =============================================================================
// Timeline
// =============================================================================

/// Timeline manages all active animations
///
/// Use `timeline.set()` to start an animation, `timeline.get()` to read current value,
/// and `timeline.tick()` to update and remove completed animations.
#[derive(Debug, Default)]
pub struct Timeline {
    animations: HashMap<AnimationId, ActiveAnimation>,
}

impl Timeline {
    /// Create a new empty timeline
    pub fn new() -> Self {
        Self::default()
    }

    /// Start an animation with the given chain
    pub fn set(&mut self, id: AnimationId, chain: Chain) -> &mut Self {
        let generation = GENERATION_COUNTER.fetch_add(1, Ordering::Relaxed);
        self.animations.insert(
            id,
            ActiveAnimation {
                chain,
                started_at: Instant::now(),
                direction: PlayDirection::Forward,
                generation,
            },
        );
        self
    }

    /// Start an animation in reverse
    pub fn set_reverse(&mut self, id: AnimationId, chain: Chain) -> &mut Self {
        let generation = GENERATION_COUNTER.fetch_add(1, Ordering::Relaxed);
        self.animations.insert(
            id,
            ActiveAnimation {
                chain,
                started_at: Instant::now(),
                direction: PlayDirection::Reverse,
                generation,
            },
        );
        self
    }

    /// Get current value of an animation, returning default if not running
    pub fn get(&self, id: AnimationId, default: f32) -> f32 {
        self.get_at(id, Instant::now(), default)
    }

    /// Get animation value at a specific time
    pub fn get_at(&self, id: AnimationId, now: Instant, default: f32) -> f32 {
        self.animations
            .get(&id)
            .map(|anim| anim.current_value(now))
            .unwrap_or(default)
    }

    /// Check if an animation is currently running
    pub fn is_running(&self, id: AnimationId) -> bool {
        self.animations.contains_key(&id)
    }

    /// Check if an animation is complete (can be removed)
    pub fn is_complete(&self, id: AnimationId) -> bool {
        self.animations
            .get(&id)
            .map(|anim| anim.is_complete(Instant::now()))
            .unwrap_or(true)
    }

    /// Get generation of current animation (None if not running)
    pub fn generation(&self, id: AnimationId) -> Option<u64> {
        self.animations.get(&id).map(|a| a.generation)
    }

    /// Check if specific generation is complete
    ///
    /// Returns true if:
    /// - The animation with that generation has completed
    /// - A different generation is now running (old one is obsolete)
    /// - No animation is running for this ID
    pub fn is_generation_complete(&self, id: AnimationId, generation: u64) -> bool {
        match self.animations.get(&id) {
            Some(anim) if anim.generation == generation => anim.is_complete(Instant::now()),
            Some(_) => true, // Different generation = old one is "complete"
            None => true,    // Not running = complete
        }
    }

    /// Remove a specific animation
    pub fn remove(&mut self, id: AnimationId) {
        self.animations.remove(&id);
    }

    /// Tick the timeline - removes completed non-looping animations
    /// Returns true if any animations are still running
    pub fn tick(&mut self) -> bool {
        let now = Instant::now();
        self.animations
            .retain(|_, anim| !anim.is_complete(now) || anim.chain.looping);
        !self.animations.is_empty()
    }

    /// Clear all animations
    pub fn clear(&mut self) {
        self.animations.clear();
    }
}

// =============================================================================
// Animation Values (multiple values in one animation)
// =============================================================================

/// Animation output for window transitions
#[derive(Debug, Clone, Copy, Default)]
pub struct WindowAnimationValues {
    pub opacity: f32,
    pub scale: f32,
    pub content_alpha: f32,
}

/// Builder for window appear/disappear animations
pub struct WindowAnimation;

impl WindowAnimation {
    /// Create a chain for window appearing
    pub fn appear(duration: Duration) -> Chain {
        Chain::new(0.0).then_eased(1.0, duration, ease_out_cubic)
    }

    /// Create a chain for window disappearing
    pub fn disappear(duration: Duration) -> Chain {
        Chain::new(1.0).then_eased(0.0, duration, ease_in_cubic)
    }
}

/// Builder for pulse animations
pub struct PulseAnimation;

impl PulseAnimation {
    /// Create a looping pulse chain
    pub fn pulse(min: f32, max: f32, period: Duration) -> Chain {
        let half = Duration::from_secs_f64(period.as_secs_f64() / 2.0);
        Chain::new(min)
            .then_eased(max, half, ease_in_out_cubic)
            .then_eased(min, half, ease_in_out_cubic)
            .looping()
    }
}

/// Builder for width animations
pub struct WidthAnimation;

impl WidthAnimation {
    /// Create a chain for width transition
    pub fn transition(from: f32, to: f32, duration: Duration) -> Chain {
        Chain::new(from).then_eased(to, duration, ease_out_cubic)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chain_value() {
        let chain = Chain::new(0.0).then(1.0, Duration::from_secs(1));
        assert!((chain.value_at(Duration::ZERO) - 0.0).abs() < 0.01);
        assert!((chain.value_at(Duration::from_secs(1)) - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_looping_chain() {
        let chain = Chain::new(0.0).then(1.0, Duration::from_secs(1)).looping();
        assert!(!chain.is_complete(Duration::from_secs(2)));
    }

    #[test]
    fn test_timeline_set_get() {
        let mut timeline = Timeline::new();
        let id = AnimationId::unique();
        timeline.set(id, Chain::new(0.0).then(1.0, Duration::from_secs(1)));
        assert!(timeline.is_running(id));
    }

    #[test]
    fn test_easing_functions() {
        // Test boundary conditions
        assert_eq!(linear(0.0), 0.0);
        assert_eq!(linear(1.0), 1.0);
        assert_eq!(ease_out_cubic(0.0), 0.0);
        assert!((ease_out_cubic(1.0) - 1.0).abs() < 0.001);
        assert_eq!(ease_in_cubic(0.0), 0.0);
        assert!((ease_in_cubic(1.0) - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_generation_increments() {
        let mut timeline = Timeline::new();
        let id = AnimationId::unique();

        timeline.set(id, Chain::new(0.0).then(1.0, Duration::from_secs(1)));
        let gen1 = timeline.generation(id).unwrap();

        timeline.set(id, Chain::new(0.0).then(1.0, Duration::from_secs(1)));
        let gen2 = timeline.generation(id).unwrap();

        assert!(gen2 > gen1);
    }

    #[test]
    fn test_old_generation_is_complete() {
        let mut timeline = Timeline::new();
        let id = AnimationId::unique();

        timeline.set(id, Chain::new(0.0).then(1.0, Duration::from_secs(10)));
        let gen1 = timeline.generation(id).unwrap();

        // Start new animation (interrupts old one)
        timeline.set(id, Chain::new(0.0).then(1.0, Duration::from_secs(10)));

        // Old generation should be considered "complete" even though time hasn't elapsed
        assert!(timeline.is_generation_complete(id, gen1));
    }

    #[test]
    fn test_generation_none_when_not_running() {
        let timeline = Timeline::new();
        let id = AnimationId::unique();

        assert!(timeline.generation(id).is_none());
        assert!(timeline.is_generation_complete(id, 999)); // Any gen is "complete" if not running
    }
}
