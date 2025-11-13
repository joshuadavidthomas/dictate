use iced::widget::{horizontal_space, row, text};
use iced::{Center, Color, Element};

use crate::protocol::State;
use crate::ui::colors;
use crate::ui::widgets::{spectrum_waveform, status_dot, timer_display};

/// Create a status bar content widget
pub fn status_bar_content(
    state: State,
    color: Color,
    alpha: f32,
    recording_elapsed_secs: Option<u32>,
    current_timestamp_ms: u64,
    spectrum_bands: [f32; 8],
) -> StatusBarContent {
    StatusBarContent {
        state,
        color,
        alpha,
        recording_elapsed_secs,
        current_timestamp_ms,
        spectrum_bands,
    }
}

/// Status bar content widget showing state, audio info, and recording details
pub struct StatusBarContent {
    state: State,
    color: Color,
    alpha: f32,
    recording_elapsed_secs: Option<u32>,
    current_timestamp_ms: u64,
    spectrum_bands: [f32; 8],
}

impl StatusBarContent {
    /// Render the status bar content as an Element
    fn view<'a, Message: 'a>(&self) -> Element<'a, Message> {
        const PADDING_VERTICAL: f32 = 6.0;
        const PADDING_HORIZONTAL: f32 = 12.0;

        let status = self.status_display();

        let content = if let Some(audio) = self.audio_display() {
            row![status, horizontal_space(), audio]
        } else {
            row![status]
        };

        content
            .padding([PADDING_VERTICAL, PADDING_HORIZONTAL])
            .align_y(Center)
            .into()
    }

    fn status_display<'a, Message: 'a>(&self) -> Element<'a, Message> {
        const DOT_RADIUS: f32 = 8.0;
        const NEAR_LIMIT_THRESHOLD_SECS: u32 = 25;
        const SPACING: f32 = 8.0;
        const TEXT_SIZE: u16 = 14;

        // Override to yellow/orange when near recording limit
        let status_dot_color =
            if self.recording_elapsed_secs.unwrap_or(0) >= NEAR_LIMIT_THRESHOLD_SECS {
                colors::ORANGE
            } else {
                self.color
            };

        // Dot color with alpha pulse
        let status_dot = status_dot(
            DOT_RADIUS,
            Color {
                r: status_dot_color.r,
                g: status_dot_color.g,
                b: status_dot_color.b,
                a: self.alpha,
            },
        );

        row![
            status_dot,
            text(self.state.as_str())
                .size(TEXT_SIZE)
                .color(colors::LIGHT_GRAY)
        ]
        .spacing(SPACING)
        .align_y(Center)
        .into()
    }

    fn audio_display<'a, Message: 'a>(&self) -> Option<Element<'a, Message>> {
        if self.state != State::Recording {
            return None;
        }

        const SPACING: f32 = 8.0;
        const WAVEFORM_OPACITY: f32 = 1.0;

        let waveform = spectrum_waveform(
            self.spectrum_bands,
            Color {
                r: self.color.r,
                g: self.color.g,
                b: self.color.b,
                a: WAVEFORM_OPACITY,
            },
        );
        let elapsed = self.recording_elapsed_secs.unwrap_or(0); // Default to 0:00
        let timer = timer_display(elapsed, self.current_timestamp_ms);

        Some(
            row![timer, waveform]
                .spacing(SPACING)
                .align_y(Center)
                .into(),
        )
    }
}

impl<'a, Message: 'a> From<StatusBarContent> for Element<'a, Message> {
    fn from(status_bar: StatusBarContent) -> Self {
        status_bar.view()
    }
}
