use crate::osd::animation::pulsing_waveform;
use crate::osd::app::OsdState;
use crate::osd::colors;
use crate::osd::widgets::{spectrum_waveform, status_dot, timer_display};
use crate::recording::{RecordingSnapshot, SPECTRUM_BANDS};
use iced::alignment::Vertical::Center;
use iced::widget::{container, mouse_area, row};
use iced::{Color, Element, Length, Shadow, Vector};

/// Visual configuration for the OSD bar styling
pub struct OsdBarStyle {
    pub height: f32,
    pub window_scale: f32,
    pub window_opacity: f32,
}

/// Compute the color for a given state
fn state_color(
    state: RecordingSnapshot,
    idle_hot: bool,
    recording_elapsed_secs: Option<u32>,
) -> Color {
    // Override to orange when near recording limit
    if recording_elapsed_secs.unwrap_or(0) >= 25 {
        return colors::ORANGE;
    }

    match (state, idle_hot) {
        (RecordingSnapshot::Idle, false) => colors::GRAY,
        (RecordingSnapshot::Idle, true) => colors::DIM_GREEN,
        (RecordingSnapshot::Recording, _) => colors::RED,
        (RecordingSnapshot::Transcribing, _) => colors::BLUE,
        (RecordingSnapshot::Error, _) => colors::ORANGE,
    }
}

/// Create a complete OSD bar with status content, styling, and mouse interaction
pub fn osd_bar<'a, Message: 'a + Clone>(
    state: &OsdState,
    style: OsdBarStyle,
    on_mouse_entered: Message,
    on_mouse_exited: Message,
) -> Element<'a, Message> {
    const PADDING: f32 = 10.0;

    // Compute color from state
    let color = state_color(state.state, state.idle_hot, state.recording_elapsed_secs);

    // Build the status bar content
    let content = bar_content(
        state.state,
        color,
        state.pulse_alpha,
        state.content_alpha,
        state.recording_elapsed_secs,
        state.current_ts,
        state.spectrum_bands,
        state.timer_width,
    );

    let scaled_height = style.height * style.window_scale;

    // Apply window opacity to background (alpha is f32 0.0-1.0)
    let bg_alpha = 0.94 * style.window_opacity;
    let shadow_alpha = 0.35 * style.window_opacity;

    let styled_bar = container(content)
        .width(Length::Shrink)
        .height(Length::Fixed(scaled_height))
        .center_y(scaled_height)
        .style(move |_theme| container::Style {
            background: Some(colors::with_alpha(colors::DARK_GRAY, bg_alpha).into()),
            border: iced::Border {
                radius: (12.0 * style.window_scale).into(),
                ..Default::default()
            },
            shadow: Shadow {
                color: colors::with_alpha(colors::BLACK, shadow_alpha),
                offset: Vector::new(0.0, 2.0),
                blur_radius: 12.0,
            },
            ..Default::default()
        });

    // Wrap the styled bar with mouse_area FIRST, before outer container
    // This ensures mouse events track the actual visual bounds of the widget
    let interactive_bar = mouse_area(styled_bar)
        .on_enter(on_mouse_entered)
        .on_exit(on_mouse_exited);

    // Then wrap in outer container with padding for shadow space
    container(interactive_bar)
        .padding(PADDING) // Padding to give shadow room to render
        .center(Length::Fill)
        .into()
}

/// Build the content layout for the status bar
fn bar_content<'a, Message: 'a>(
    state: RecordingSnapshot,
    color: Color,
    pulse_alpha: f32,
    content_alpha: f32,
    recording_elapsed_secs: Option<u32>,
    current_timestamp_ms: u64,
    spectrum_bands: [f32; SPECTRUM_BANDS],
    timer_width: f32,
) -> Element<'a, Message> {
    const PADDING_VERTICAL: f32 = 4.0;
    const PADDING_HORIZONTAL: f32 = 8.0;
    const ELEMENT_SPACING: f32 = 8.0;

    let status = status_dot_display(color, pulse_alpha, content_alpha, recording_elapsed_secs);

    let content = match state {
        RecordingSnapshot::Recording | RecordingSnapshot::Transcribing => {
            if let Some(audio) = audio_display(
                state,
                color,
                content_alpha,
                recording_elapsed_secs,
                current_timestamp_ms,
                spectrum_bands,
                timer_width,
            ) {
                row![status, audio].spacing(ELEMENT_SPACING)
            } else {
                row![status]
            }
        }
        _ => row![status],
    };

    content
        .padding([PADDING_VERTICAL, PADDING_HORIZONTAL])
        .align_y(Center)
        .into()
}

/// Build the status dot display (no text, just the dot)
fn status_dot_display<'a, Message: 'a>(
    color: Color,
    pulse_alpha: f32,
    content_alpha: f32,
    recording_elapsed_secs: Option<u32>,
) -> Element<'a, Message> {
    const DOT_RADIUS: f32 = 6.0;
    const NEAR_LIMIT_THRESHOLD_SECS: u32 = 25;

    // Override to yellow/orange when near recording limit
    let status_dot_color = if recording_elapsed_secs.unwrap_or(0) >= NEAR_LIMIT_THRESHOLD_SECS {
        colors::ORANGE
    } else {
        color
    };

    // Dot color with alpha pulse (also respecting content visibility)
    let dot_alpha = pulse_alpha * content_alpha;
    status_dot(
        DOT_RADIUS,
        Color {
            r: status_dot_color.r,
            g: status_dot_color.g,
            b: status_dot_color.b,
            a: dot_alpha,
        },
    )
    .into()
}

/// Build the audio display (waveform + timer) - shown during recording and transcribing
fn audio_display<'a, Message: 'a>(
    state: RecordingSnapshot,
    color: Color,
    content_alpha: f32,
    recording_elapsed_secs: Option<u32>,
    current_timestamp_ms: u64,
    spectrum_bands: [f32; SPECTRUM_BANDS],
    timer_width: f32,
) -> Option<Element<'a, Message>> {
    if state != RecordingSnapshot::Recording && state != RecordingSnapshot::Transcribing {
        return None;
    }

    const SPACING: f32 = 8.0;
    const WAVEFORM_OPACITY: f32 = 1.0;

    // For transcribing state, use animated pulsing waveform
    // For recording, use actual spectrum data (or pulsing if no data yet)
    let display_bands = if state == RecordingSnapshot::Transcribing {
        pulsing_waveform(current_timestamp_ms)
    } else {
        // Check if we have actual spectrum data (not all zeros)
        let has_spectrum_data = spectrum_bands.iter().any(|&v| v > 0.0);
        if !has_spectrum_data {
            // Create a gentle pulsing pattern based on timestamp for "loading" effect
            let pulse = ((current_timestamp_ms as f32 / 300.0).sin() + 1.0) / 2.0;
            let base_level = 0.15 + (pulse * 0.1);
            [base_level; SPECTRUM_BANDS]
        } else {
            spectrum_bands
        }
    };

    let waveform = spectrum_waveform(
        display_bands,
        Color {
            r: color.r,
            g: color.g,
            b: color.b,
            a: WAVEFORM_OPACITY * content_alpha,
        },
    );

    // Timer with animated width (shrinks to 0 during transcribing transition)
    let elapsed = recording_elapsed_secs.unwrap_or(0);

    // Only include timer if it has width, otherwise spacing creates asymmetry
    let row_elem = if timer_width > 0.5 {
        let timer = timer_display(elapsed, current_timestamp_ms, timer_width);
        row![waveform, timer].spacing(SPACING)
    } else {
        row![waveform]
    };
    Some(row_elem.align_y(Center).into())
}
