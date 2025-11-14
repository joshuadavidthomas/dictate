use iced::widget::{container, horizontal_space, mouse_area, row, text};
use iced::{Color, Element, Length, Shadow, Vector};
use iced::alignment::Vertical::Center;

use crate::audio::SPECTRUM_BANDS;
use crate::protocol::State;
use crate::ui::app::OsdState;
use crate::ui::colors;
use crate::ui::widgets::{spectrum_waveform, status_dot, timer_display};

/// Visual configuration for the OSD bar styling
pub struct OsdBarStyle {
    pub width: f32,
    pub height: f32,
    pub window_scale: f32,
    pub window_opacity: f32,
}

/// Compute the color for a given state
fn state_color(state: State, idle_hot: bool, recording_elapsed_secs: Option<u32>) -> Color {
    // Override to orange when near recording limit
    if recording_elapsed_secs.unwrap_or(0) >= 25 {
        return colors::ORANGE;
    }

    match (state, idle_hot) {
        (State::Idle, false) => colors::GRAY,
        (State::Idle, true) => colors::DIM_GREEN,
        (State::Recording, _) => colors::RED,
        (State::Transcribing, _) => colors::BLUE,
        (State::Error, _) => colors::ORANGE,
    }
}

/// Create a complete OSD bar with status content, styling, and mouse interaction
pub fn osd_bar<'a, Message: 'a>(
    state: &OsdState,
    style: OsdBarStyle,
    on_mouse_entered: Message,
    on_mouse_exited: Message,
) -> Element<'a, Message>
where
    Message: Clone,
{
    const PADDING: f32 = 10.0;

    // Compute color from state
    let color = state_color(
        state.state,
        state.idle_hot,
        state.recording_elapsed_secs,
    );

    // Build the status bar content
    let content = bar_content(
        state.state,
        color,
        state.alpha,
        state.recording_elapsed_secs,
        state.current_ts,
        state.spectrum_bands,
    );

    let scaled_width = style.width * style.window_scale;
    let scaled_height = style.height * style.window_scale;

    // Apply window opacity to background (alpha is f32 0.0-1.0)
    let bg_alpha = 0.94 * style.window_opacity;
    let shadow_alpha = 0.35 * style.window_opacity;

    let styled_bar = container(content)
        .width(Length::Fixed(scaled_width))
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
    state: State,
    color: Color,
    alpha: f32,
    recording_elapsed_secs: Option<u32>,
    current_timestamp_ms: u64,
    spectrum_bands: [f32; SPECTRUM_BANDS],
) -> Element<'a, Message> {
    const PADDING_VERTICAL: f32 = 6.0;
    const PADDING_HORIZONTAL: f32 = 12.0;

    let status = status_display(state, color, alpha, recording_elapsed_secs);

    let content = if let Some(audio) = audio_display(
        state,
        color,
        recording_elapsed_secs,
        current_timestamp_ms,
        spectrum_bands,
    ) {
        row![status, horizontal_space(), audio]
    } else {
        row![status]
    };

    content
        .padding([PADDING_VERTICAL, PADDING_HORIZONTAL])
        .align_y(Center)
        .into()
}

/// Build the status display (dot + text)
fn status_display<'a, Message: 'a>(
    state: State,
    color: Color,
    alpha: f32,
    recording_elapsed_secs: Option<u32>,
) -> Element<'a, Message> {
    const DOT_RADIUS: f32 = 8.0;
    const NEAR_LIMIT_THRESHOLD_SECS: u32 = 25;
    const SPACING: f32 = 8.0;
    const TEXT_SIZE: u16 = 14;

    // Override to yellow/orange when near recording limit
    let status_dot_color = if recording_elapsed_secs.unwrap_or(0) >= NEAR_LIMIT_THRESHOLD_SECS {
        colors::ORANGE
    } else {
        color
    };

    // Dot color with alpha pulse
    let dot = status_dot(
        DOT_RADIUS,
        Color {
            r: status_dot_color.r,
            g: status_dot_color.g,
            b: status_dot_color.b,
            a: alpha,
        },
    );

    row![
        dot,
        text(state.as_str())
            .size(TEXT_SIZE)
            .color(colors::LIGHT_GRAY)
    ]
    .spacing(SPACING)
    .align_y(Center)
    .into()
}

/// Build the audio display (timer + waveform) - only shown when recording
fn audio_display<'a, Message: 'a>(
    state: State,
    color: Color,
    recording_elapsed_secs: Option<u32>,
    current_timestamp_ms: u64,
    spectrum_bands: [f32; SPECTRUM_BANDS],
) -> Option<Element<'a, Message>> {
    if state != State::Recording {
        return None;
    }

    const SPACING: f32 = 8.0;
    const WAVEFORM_OPACITY: f32 = 1.0;

    let waveform = spectrum_waveform(
        spectrum_bands,
        Color {
            r: color.r,
            g: color.g,
            b: color.b,
            a: WAVEFORM_OPACITY,
        },
    );
    let elapsed = recording_elapsed_secs.unwrap_or(0); // Default to 0:00
    let timer = timer_display(elapsed, current_timestamp_ms);

    Some(
        row![timer, waveform]
            .spacing(SPACING)
            .align_y(Center)
            .into(),
    )
}
