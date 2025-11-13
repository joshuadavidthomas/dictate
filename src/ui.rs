//! UI overlay for Wayland using iced
//!
//! This module provides a visual overlay showing transcription state and audio spectrum.

mod animation;
mod app;
mod buffer;
mod colors;
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
    .run_with(move || app::OsdApp::new(&socket_path_owned, config, app::TranscriptionMode::Transcribe))?;

    Ok(())
}

/// Run the OSD overlay with Start command
pub fn run_osd_start(socket_path: &str, config: TranscriptionConfig, silence_duration: Option<u64>) -> Result<()> {
    eprintln!("OSD: Starting iced layershell overlay in daemon mode (Start command)");
    eprintln!("OSD: Connecting to socket: {}", socket_path);
    eprintln!(
        "OSD: Transcription config: max_duration={}, silence_duration={:?}, insert={}, copy={}",
        config.max_duration, silence_duration, config.insert, config.copy
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
    .run_with(move || app::OsdApp::new(&socket_path_owned, config, app::TranscriptionMode::Start(silence_duration)))?;

    Ok(())
}

/// Run the OSD overlay with Stop command
pub fn run_osd_stop(socket_path: &str, config: TranscriptionConfig) -> Result<()> {
    eprintln!("OSD: Starting iced layershell overlay in daemon mode (Stop command)");
    eprintln!("OSD: Connecting to socket: {}", socket_path);

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
    .run_with(move || app::OsdApp::new(&socket_path_owned, config, app::TranscriptionMode::Stop))?;

    Ok(())
}

/// Run the OSD overlay with Toggle command
pub fn run_osd_toggle(
    socket_path: &str,
    config: TranscriptionConfig,
    silence_duration: Option<u64>,
    current_state: crate::protocol::State,
) -> Result<()> {
    eprintln!("OSD: Starting iced layershell overlay in daemon mode (Toggle command)");
    eprintln!("OSD: Connecting to socket: {}", socket_path);
    eprintln!("OSD: Current state: {:?}", current_state);

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
    .run_with(move || {
        app::OsdApp::new(
            &socket_path_owned,
            config,
            app::TranscriptionMode::Toggle(silence_duration, current_state),
        )
    })?;

    Ok(())
}
