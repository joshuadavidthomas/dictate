# OSD Architecture Exploration: iced vs relm4/GTK4 vs COSMIC

This document captures findings from exploring alternative approaches to the dictate OSD implementation, comparing the current iced-layershell approach against relm4/GTK4 (waypomo) and COSMIC/libcosmic patterns.

## Context

The dictate OSD is a Wayland layer-shell overlay that displays recording state, transcription status, and audio visualization. The current implementation uses:

- **iced** (upstream) for the UI framework
- **iced-layershell** (waycrate) for Wayland layer-shell integration
- Custom animation/tween code for window transitions
- Manual widget implementations for spectrum visualization and status indicators

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

## Adoptable Patterns

The following patterns from waypomo and COSMIC can improve the current iced implementation without switching frameworks.

### 1. cosmic-time Animation Library

**Highest value change.** [cosmic-time](https://github.com/pop-os/cosmic-time) is designed for iced and provides declarative keyframe-based animations.

Current approach (~236 lines in `animation.rs`):
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

cosmic-time approach:
```rust
use cosmic_time::{Timeline, chain, id, anim};

static WINDOW_ANIM: LazyLock<id::Container> = LazyLock::new(id::Container::unique);

struct OsdApp {
    timeline: Timeline,
}

// In subscription
fn subscription(&self) -> Subscription<Message> {
    self.timeline.as_subscription().map(|(_, now)| Message::Frame(now))
}

// Start animation
fn start_appearing(&mut self) {
    let chain = chain::Container::on(WINDOW_ANIM.clone(), 1.0)
        .opacity(0.0, 1.0);
    self.timeline.set_chain(chain).start();
}

// In view - uses anim! macro
anim!(WINDOW_ANIM, &self.timeline, content)
```

**Benefits:**
- Eliminates manual interpolation code
- Keyframe-based (declarative, like CSS)
- Synchronized animations via `.start()`
- No heap allocations during render loop

**Integration:**
```toml
[dependencies]
cosmic-time = "0.4"  # Check for latest version
```

### 2. Theme Constants Module

Centralize colors, timing, and dimensions (inspired by COSMIC's `theme::active().cosmic().spacing`):

```rust
// src/osd/theme.rs

pub mod colors {
    use iced::Color;

    pub const IDLE: Color = Color::from_rgb(0.5, 0.5, 0.5);
    pub const IDLE_HOT: Color = Color::from_rgb(0.2, 0.7, 0.3);
    pub const RECORDING: Color = Color::from_rgb(0.9, 0.2, 0.2);
    pub const TRANSCRIBING: Color = Color::from_rgb(0.2, 0.5, 0.9);
    pub const ERROR: Color = Color::from_rgb(0.9, 0.5, 0.1);

    pub const DARK_GRAY: Color = Color::from_rgb(0.15, 0.15, 0.15);
    pub const BLACK: Color = Color::from_rgb(0.0, 0.0, 0.0);
}

pub mod timing {
    use std::time::Duration;

    pub const APPEAR: Duration = Duration::from_millis(300);
    pub const DISAPPEAR: Duration = Duration::from_millis(250);
    pub const LINGER: Duration = Duration::from_millis(1500);
    pub const PULSE_HZ: f32 = 0.5;
}

pub mod dimensions {
    pub const BAR_HEIGHT: f32 = 32.0;
    pub const BAR_RADIUS: f32 = 12.0;
    pub const DOT_RADIUS: f32 = 6.0;
    pub const SHADOW_BLUR: f32 = 8.0;
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

Current approach:
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

Proposed approach:
```rust
pub enum OsdVisualState {
    Hidden,
    Appearing {
        started_at: Instant,
        target: RecordingSnapshot,
    },
    Visible {
        state: RecordingSnapshot,
        idle_hot: bool,
    },
    Hovering {
        state: RecordingSnapshot,
    },
    Lingering {
        state: RecordingSnapshot,
        until: Instant,
    },
    Disappearing {
        started_at: Instant,
    },
}

impl OsdVisualState {
    fn transition(&mut self, event: StateEvent) {
        *self = match (&self, event) {
            (Hidden, StateEvent::Show(state)) => Appearing { started_at: Instant::now(), target: state },
            (Appearing { .. }, StateEvent::AnimationComplete) => Visible { .. },
            (Visible { .. }, StateEvent::MouseEnter) => Hovering { .. },
            // ... explicit transitions
        };
    }
}
```

### 4. Subscription Batching

Clean subscription organization (from COSMIC applets):

```rust
fn subscription(&self) -> Subscription<Message> {
    Subscription::batch([
        // Animation frames
        self.timeline.as_subscription().map(|(_, now)| Message::Frame(now)),

        // Or if not using cosmic-time:
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
        OsdVisualState::Visible { state: RecordingSnapshot::Idle, idle_hot: false } =>
            idle_cold_view(),
        OsdVisualState::Visible { state: RecordingSnapshot::Recording, .. } =>
            recording_view(),
        OsdVisualState::Visible { state: RecordingSnapshot::Transcribing, .. } =>
            transcribing_view(),
        // ...
    }
}
```

## Priority of Changes

| Change | Effort | Impact | Notes |
|--------|--------|--------|-------|
| Adopt cosmic-time | Medium | High | Eliminates ~200 LOC, cleaner animations |
| Theme constants module | Low | Medium | Prep for future theming, cleaner code |
| Visual state machine | Medium | Medium | Cleaner state transitions, fewer edge cases |
| Subscription batching | Low | Low | Already similar |
| View state matching | Low | Low | Minor readability improvement |

## Reference Code

The following repositories have been cloned for reference:

- `waypomo-reference/` - relm4/GTK4 layer-shell OSD
  - Key file: `src/waypomo.rs` (view! macro, Component trait)
  - Key file: `config/style.css` (CSS animations)

- `libcosmic-reference/` - COSMIC widget library
  - Key file: `examples/cosmic/src/window/demo.rs` (cosmic-time usage)
  - Key file: `src/theme/mod.rs` (theming patterns)

- `cosmic-applets-reference/` - COSMIC applet implementations
  - Key file: `cosmic-applet-audio/src/lib.rs` (Timeline + animations)
  - Key file: `cosmic-applet-notifications/src/lib.rs` (state management)

## Resources

- [cosmic-time GitHub](https://github.com/pop-os/cosmic-time)
- [cosmic-time docs](https://docs.rs/cosmic-time/latest/cosmic_time/)
- [libcosmic docs](https://pop-os.github.io/libcosmic/cosmic/)
- [libcosmic book](https://pop-os.github.io/libcosmic-book/introduction.html)
- [iced-layershell](https://crates.io/crates/iced-layershell)
- [waycrate/exwlshelleventloop](https://github.com/waycrate/exwlshelleventloop)
