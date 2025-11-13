//! UI overlay for Wayland using iced
//!
//! This module provides a visual overlay showing transcription state and audio spectrum.

mod animation;
mod app;
mod buffer;
mod colors;
mod socket;
mod widgets;

use anyhow::Result;
use iced_layershell::build_pattern::daemon;

pub use app::TranscriptionConfig;

/// Run the OSD overlay with transcription
pub fn run_osd(socket_path: &str, config: TranscriptionConfig) -> Result<()> {
    eprintln!("OSD: Starting iced layershell overlay in daemon mode");
    eprintln!("OSD: Connecting to socket: {}", socket_path);
    eprintln!(
        "OSD: Transcription config: max_duration={}, silence_duration={}, insert={}, copy={}",
        config.max_duration, config.silence_duration, config.insert, config.copy
    );

    let socket_path_owned = socket_path.to_string();

    daemon(
        app::OsdApp::namespace,
        app::OsdApp::update,
        app::OsdApp::view,
        app::OsdApp::remove_id,
    )
    .style(app::OsdApp::style)
    .subscription(app::OsdApp::subscription)
    .settings(app::OsdApp::settings())
    .run_with(move || app::OsdApp::new(&socket_path_owned, config))?;

    Ok(())
}
