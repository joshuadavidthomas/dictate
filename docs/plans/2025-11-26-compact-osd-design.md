# Compact OSD Design

## Overview

Redesign the OSD to be more compact and visually minimal, inspired by Aqua Voice's aesthetic. Same information, less wasted space.

## Current State

- **Dimensions:** 420×36px pill
- **Layout:** `[dot] [text label] ...empty space... [timer] [waveform]`
- **Animation:** Scale + opacity animate together (size change hard to see)
- **States:** Idle (hidden), Recording, Transcribing, Error

## Proposed Design

### Layout

**Recording:**
```
[dot 16px] [waveform ~100px] [timer ~40px]
```
~180-200px total width

**Transcribing:**
```
[pulsing dot] [pulsing waveform]
```
~140-160px total width (no timer)

**Error:**
```
[orange dot] [static/empty waveform area]
```

### Key Changes

1. **Remove text labels** - No "Recording", "Transcribing", etc.
2. **Waveform dominates** - Centered, the main visual element
3. **Compact width** - ~200px instead of 420px
4. **Timer on right** - Recording only, after waveform

### Animation

Decouple opacity from scale so the bar stays **solid during size transitions**.

**Appearing:**
1. Tiny opaque pill appears
2. Expands to full size (bar stays opaque)
3. Content fades in at the end

**Disappearing:**
1. Content fades out
2. Bar shrinks while staying opaque
3. Tiny pill disappears

**Collapsed state:** ~60-80px × 36px (small dark pill)
**Expanded state:** ~180-200px × 36px

### Transcribing State

Both dot and waveform pulse to convey "working":
- Dot: existing pulse animation
- Waveform: synthetic bars that gently animate up/down in a wave pattern

### Not In Scope

- Always-visible idle state (OSD still only appears when active)
- Stop/cancel button (future enhancement)
- Tooltips (future enhancement)
- Text labels of any kind

## Implementation

### Files to Modify

1. **`src-tauri/src/osd/widgets/osd_bar.rs`**
   - Remove `status_display` text label
   - Reorder: dot → waveform → timer
   - Reduce padding/spacing
   - Show waveform in transcribing state (pulsing)

2. **`src-tauri/src/osd/app.rs`**
   - Change window size from 440×56 to ~220×48
   - Adjust `OsdBarStyle` dimensions

3. **`src-tauri/src/osd/animation.rs`**
   - Decouple `window_opacity` from `window_scale`
   - Bar stays at opacity 1.0 during scale animation
   - Only fade at the very start (appear) and very end (disappear)

### Widget Changes

**`osd_bar.rs` - New `bar_content` layout:**
```rust
// Recording: dot + waveform + timer
// Transcribing: dot + pulsing waveform
// Error: dot + empty space
row![dot, waveform, timer].spacing(8)
```

**`osd_bar.rs` - Show waveform during transcribing:**
```rust
// Currently audio_display returns None if not recording
// Change to return pulsing waveform during transcribing
```

### Animation Timing

**Appearing (total ~300ms):**
- 0-200ms: Scale 0.5 → 1.0, opacity 0.0 → 1.0 (quick fade in at start)
- 150-300ms: Content alpha 0.0 → 1.0

**Disappearing (total ~300ms):**
- 0-100ms: Content alpha 1.0 → 0.0
- 100-300ms: Scale 1.0 → 0.5, opacity stays 1.0 until final 50ms
- 250-300ms: Opacity 1.0 → 0.0

## Success Criteria

- OSD is roughly half the current width
- No text labels visible
- Waveform is the dominant visual element
- Size animation is clearly visible (bar doesn't fade while shrinking)
- Same information available: state color, waveform, timer
