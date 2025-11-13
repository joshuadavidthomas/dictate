//! UI overlay for Wayland using iced
//!
//! This module provides a visual overlay showing transcription state and audio spectrum.

mod animation;
mod app;
mod colors;
mod socket;
mod widgets;

use anyhow::Result;
use iced_layershell::build_pattern::{daemon, MainSettings};
use iced_layershell::reexport::{Anchor, Layer};
use iced_layershell::settings::{LayerShellSettings, StartMode};

pub use app::TranscriptionConfig;

/// Run the OSD overlay with transcription
pub fn run_osd(socket_path: &str, config: TranscriptionConfig) -> Result<()> {
    eprintln!("OSD: Starting iced layershell overlay in daemon mode");
    eprintln!("OSD: Connecting to socket: {}", socket_path);
    eprintln!("OSD: Transcription config: max_duration={}, silence_duration={}, insert={}, copy={}", 
        config.max_duration, config.silence_duration, config.insert, config.copy);

    let socket_path_owned = socket_path.to_string();

    daemon(app::namespace, app::update, app::view, app::remove_id)
        .style(app::style)
        .subscription(app::subscription)
        .settings(MainSettings {
            layer_settings: LayerShellSettings {
                size: None, // No initial window
                exclusive_zone: 0,
                anchor: Anchor::Top | Anchor::Left | Anchor::Right,
                layer: Layer::Overlay,
                margin: (10, 0, 0, 0),
                start_mode: StartMode::Background, // KEY: No focus stealing!
                ..Default::default()
            },
            ..Default::default()
        })
        .run_with(move || app::new_osd_app(&socket_path_owned, config))?;

    Ok(())
}
