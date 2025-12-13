# OSD Architecture Exploration: iced vs relm4/GTK4 vs COSMIC

This document captures findings from exploring alternative approaches to the dictate OSD implementation, comparing the current iced-layershell approach against relm4/GTK4 (waypomo) and COSMIC/libcosmic patterns.

## Context

The dictate OSD is a Wayland layer-shell overlay that displays recording state, transcription status, and audio visualization. The current implementation uses:

- **iced** (upstream) for the UI framework
- **iced-layershell** (waycrate) for Wayland layer-shell integration
- Custom animation/tween code for window transitions
- Manual widget implementations for spectrum visualization and status indicators

## Reference Repositories Explored

| Repository | Purpose | Key Patterns |
|------------|---------|--------------|
| cosmic-time-reference/ | Animation library for iced | Timeline + chain! macro, keyframe-based animations |
| iced-reference/ | Official iced examples | Custom widgets, canvas, loading spinners |
| bar-rs-reference/ | Status bar with iced (pop-os fork) | daemon() pattern, popup handling, multi-window |
| iced-launcher-reference/ | Launcher with iced-layershell | Simple Application trait, LayerShellSettings |
| wob-reference/ | Minimal C OSD | Raw layer-shell protocol, buffer management |
| waypomo-reference/ | relm4/GTK4 layer-shell OSD | view! macro, CSS animations, Component trait |
| libcosmic-reference/ | COSMIC widget library | cosmic-time usage, theming patterns |
| cosmic-applets-reference/ | COSMIC applet implementations | Timeline + animations, state management |

## Alternatives Explored

### 1. relm4 + GTK4 (waypomo reference)

[waypomo](https://tangled.org/wrl.sh/waypomo/) is a Pomodoro timer using relm4 with gtk4-layer-shell.

**Strengths:**
- Declarative `view!` macro with `#[watch]` for automatic re-rendering
- CSS-based styling (transitions, animations, gradients, variables)
- Built-in `Component` and `Worker` traits for composition
- Mature gtk4-layer-shell integration

**Weaknesses:**
- No WASM/web target (GTK4 is Linux-focused)
- Heavier runtime (GLib, Cairo, Pango, etc.)
- Longer compile times due to GTK-rs bindings
- Tied to GTK ecosystem evolution

**Code comparison:**

```rust
// relm4 view declaration
view! {
    window = gtk::Window {
        set_layer: Layer::Overlay,
        gtk::Box {
            #[watch]  // Auto-updates when model changes
            set_css_classes: match model.state {
                State::Recording => &["recording"],
                State::Idle => &["idle"],
            },
        }
    }
}

// vs iced (current)
fn view(&self) -> Element<'_, Message> {
    container(content)
        .style(move |_| container::Style {
            background: Some(self.compute_background_color().into()),
            // ... manual style computation
        })
}
```

### 2. COSMIC/libcosmic (Pop!_OS)

[libcosmic](https://github.com/pop-os/libcosmic) is System76's widget library built on their iced fork.

**Strengths:**
- Rich widget library (toasts, notifications, themed containers)
- [cosmic-time](https://github.com/pop-os/cosmic-time) animation library
- Battle-tested across COSMIC desktop
- Consistent theming system

**Weaknesses:**
- Tied to pop-os/iced fork (not upstream)
- Heavy dependency (pulls in cosmic-theme, cosmic-config, cosmic-icons)
- Desktop-oriented, overkill for minimal OSD
- No WASM target (Wayland-specific)

### 3. Current Approach: iced + iced-layershell (waycrate)

**Strengths:**
- Tracks upstream iced (0.13, 0.14, etc.)
- WASM/web target possible
- Minimal dependencies
- Pure Rust, no FFI concerns
- Active maintainer (waycrate org)

**Weaknesses:**
- Manual animation code required
- Programmatic styling (no CSS)
- No built-in component composition model

## Decision: Stay with iced

Given the interest in potential WASM/web support, sticking with **iced + iced-layershell** is the right choice. The ecosystem benefits outweigh the convenience of relm4's CSS or COSMIC's widgets.

## Key Findings from Reference Code

### cosmic-time (Most Valuable)

The library provides a clean animation abstraction:

```rust
// Define animation IDs
static WINDOW_ANIM: Lazy<id::Container> = Lazy::new(id::Container::unique);

// Create keyframe chains
let animation = chain![
    WINDOW_ANIM,
    container(Duration::ZERO).opacity(0.0),
    container(Duration::from_millis(300)).opacity(1.0)
];
self.timeline.set_chain(animation).start();

// In view - uses anim! macro
anim!(WINDOW_ANIM, &self.timeline, content)
```

Built-in easing functions: `Cubic::Out`, `Elastic::InOut`, `Bounce::Out`, etc.

### iced Examples

The `loading_spinners` example shows how to manage animation state in custom widgets:
- Store animation state in `widget::tree::State`
- Update via `Event::Window(window::Event::RedrawRequested(now))`
- Use `canvas::Cache` for efficient redraws

### bar-rs (iced-layershell with pop-os fork)

Uses pop-os iced fork features directly (not iced-layershell crate):
- `daemon()` for multi-window apps
- `get_layer_surface()` / `destroy_layer_surface()` for surface management
- Registry pattern for modular components

### iced_launcher (iced-layershell with waycrate)

Simpler approach closer to your current code:
- Standard Application trait
- LayerShellSettings for layer shell config
- Event subscription for keyboard handling

## Adoptable Patterns

The following patterns from waypomo and COSMIC can improve the current iced implementation without switching frameworks.

### 1. Timeline-based Animations (cosmic-time inspired)

**Highest value change.** Provides declarative keyframe-based animations.

**Before:** ~236 lines of manual animation code in `animation.rs`
```rust
pub struct WindowTween {
    pub started_at: Instant,
    pub duration: Duration,
    pub direction: WindowDirection,
}

fn compute_window_animation(tween: &WindowTween, now: Instant) -> (f32, f32, f32) {
    let elapsed = now.duration_since(tween.started_at);
    let t = (elapsed.as_secs_f32() / tween.duration.as_secs_f32()).clamp(0.0, 1.0);
    let eased = ease_out_cubic(t);
    // ... manual interpolation
}
```

**After:** ~20 lines + timeline module
```rust
static WINDOW: LazyLock<AnimationId> = LazyLock::new(AnimationId::unique);

// Start animation
self.timeline.set(*WINDOW, WindowAnimation::appear(timing::APPEAR));

// Get current value
let progress = self.timeline.get(*WINDOW, 1.0);
```

**Benefits:**
- Eliminates manual interpolation code
- Keyframe-based (declarative, like CSS)
- Synchronized animations via chainable API
- No heap allocations during render loop

### 2. Theme Constants Module

Centralize colors, timing, and dimensions (inspired by COSMIC's `theme::active().cosmic().spacing`):

```rust
// src/osd/theme.rs

pub mod colors {
    pub const IDLE: Color = rgb8(122, 122, 122);
    pub const IDLE_HOT: Color = rgb8(118, 211, 155);
    pub const RECORDING: Color = rgb8(231, 76, 60);
    pub const TRANSCRIBING: Color = rgb8(52, 152, 219);
    pub const ERROR: Color = rgb8(243, 156, 18);
}

pub mod timing {
    pub const APPEAR: Duration = Duration::from_millis(300);
    pub const DISAPPEAR: Duration = Duration::from_millis(250);
    pub const LINGER: Duration = Duration::from_millis(1500);
    pub const PULSE_HZ: f32 = 0.5;
}

pub mod dimensions {
    pub const BAR_HEIGHT: f32 = 32.0;
    pub const BAR_RADIUS: f32 = 12.0;
    pub const DOT_RADIUS: f32 = 6.0;
}

pub mod spacing {
    pub const XXSMALL: f32 = 2.0;
    pub const XSMALL: f32 = 4.0;
    pub const SMALL: f32 = 8.0;
    pub const MEDIUM: f32 = 12.0;
    pub const LARGE: f32 = 16.0;
}
```

### 3. Visual State Machine

Consolidate scattered state flags into an explicit state machine (inspired by waypomo's `State` enum):

**Before:** 5+ separate fields scattered across the app
```rust
struct OsdApp {
    state: RecordingSnapshot,
    window_tween: Option<WindowTween>,
    is_window_disappearing: bool,
    is_mouse_hovering: bool,
    linger_until: Option<Instant>,
    idle_hot: bool,
}
```

**After:** 1 enum with explicit transitions
```rust
pub enum OsdVisualState {
    Hidden,
    Appearing { started_at: Instant, target: RecordingSnapshot, idle_hot: bool },
    Visible { state: RecordingSnapshot, idle_hot: bool },
    Hovering { state: RecordingSnapshot, idle_hot: bool },
    Lingering { state: RecordingSnapshot, idle_hot: bool, until: Instant },
    Disappearing { started_at: Instant, previous_state: RecordingSnapshot },
}

impl OsdVisualState {
    fn transition(&mut self, event: StateEvent) { ... }
}
```

**Benefits:**
- Impossible to represent invalid state combinations
- Explicit transitions prevent edge cases
- Self-documenting state flow
- Easy to add new states

### 4. Subscription Batching

Clean subscription organization (from COSMIC applets):

```rust
fn subscription(&self) -> Subscription<Message> {
    Subscription::batch([
        // Animation frames
        time::every(Duration::from_millis(16)).map(|_| Message::Tick),
        
        // Future: separate broadcast subscription
        // broadcast::subscription().map(Message::Broadcast),
    ])
}
```

### 5. View State Matching

Cleaner view construction by matching on visual state:

```rust
fn bar_content(state: &OsdVisualState) -> Element<'_, Message> {
    match state {
        OsdVisualState::Visible { state: RecordingSnapshot::Idle, idle_hot: true } =>
            idle_ready_view(),
        OsdVisualState::Visible { state: RecordingSnapshot::Recording, .. } =>
            recording_view(),
        OsdVisualState::Visible { state: RecordingSnapshot::Transcribing, .. } =>
            transcribing_view(),
        // ...
    }
}
```

## Priority of Changes

| Change | Effort | Impact | Status |
|--------|--------|--------|--------|
| Theme constants module | Low | Medium | ✅ Implemented (`src/osd/theme.rs`) |
| Visual state machine | Medium | Medium | ✅ Implemented (`src/osd/state.rs`) |
| Timeline animations | Medium | High | ✅ Implemented (`src/osd/timeline.rs`) |
| Subscription batching | Low | Low | Partial (already similar) |
| View state matching | Low | Low | Minor readability improvement |

## Implementation Results

### What Was Built

The prototype implementation adopts the key patterns without external dependencies:

1. **`src/osd/theme.rs`** - Theme constants module
   - Centralized colors, timing, dimensions, spacing
   - Single source of truth for visual styling

2. **`src/osd/state.rs`** - Visual state machine
   - `OsdVisualState` enum with explicit states
   - `StateEvent` for type-safe transitions
   - Unit tests for state transitions

3. **`src/osd/timeline.rs`** - Animation timeline (cosmic-time inspired)
   - `Timeline` for managing active animations
   - `Chain` for declarative keyframe sequences
   - `AnimationId` for type-safe animation identifiers
   - Built-in easing functions
   - No external dependencies, works with iced 0.13

4. **`src/osd/app_v2.rs`** - Prototype integration
   - Demonstrates all patterns working together
   - Uses state machine instead of boolean flags
   - Uses timeline for all animations

### Why Self-Contained (vs cosmic-time dependency)

cosmic-time only supports iced 0.9.x while the project uses iced 0.13. Rather than:
- Downgrade iced (breaking existing code)
- Wait for cosmic-time to update (uncertain timeline)

We implemented the patterns directly:
- Same declarative API style
- Same performance characteristics
- No external dependency
- Full compatibility with current iced version

### Results

- **~400-500 fewer lines** of animation boilerplate possible
- **Fewer edge cases** (state machine prevents invalid states)
- **Easier theming** (all constants in one place)
- **Still WASM-compatible** (no GTK or Wayland-specific dependencies in core logic)

## Resources

- [cosmic-time GitHub](https://github.com/pop-os/cosmic-time)
- [cosmic-time docs](https://docs.rs/cosmic-time/latest/cosmic_time/)
- [libcosmic docs](https://pop-os.github.io/libcosmic/cosmic/)
- [libcosmic book](https://pop-os.github.io/libcosmic-book/introduction.html)
- [iced-layershell](https://crates.io/crates/iced-layershell)
- [waycrate/exwlshelleventloop](https://github.com/waycrate/exwlshelleventloop)
- [iced examples](https://github.com/iced-rs/iced/tree/master/examples)
- [bar-rs](https://github.com/Faervan/bar-rs)
- [wob](https://github.com/francma/wob)
