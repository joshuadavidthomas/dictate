use iced::widget::text;
use iced::Element;

use crate::ui::colors;

/// Create a timer display widget with blinking colon
pub fn timer_display<'a, Message: 'a>(
    elapsed_seconds: u32,
    current_timestamp_ms: u64,
) -> Element<'a, Message> {
    // Blink every 500ms (toggle twice per second)
    let show_colon = (current_timestamp_ms / 500) % 2 == 0;
    let timer_str = format_duration(elapsed_seconds, show_colon);
    text(timer_str).size(14).color(colors::LIGHT_GRAY).into()
}

/// Format duration as M:SS or M SS (blink colon only)
/// When show_colon=true: "0:05", when false: "0 05"
fn format_duration(seconds: u32, show_colon: bool) -> String {
    let mins = seconds / 60;
    let secs = seconds % 60;
    let separator = if show_colon { ":" } else { " " };
    format!("{}{}{:02}", mins, separator, secs)
}
