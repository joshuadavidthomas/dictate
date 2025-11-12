//! OSD (On-Screen Display) overlay for Wayland using iced
//!
//! This module provides a visual overlay showing transcription state and audio levels.

mod app;
mod socket;
mod state;
mod widgets;

use anyhow::Result;
use iced_layershell::reexport::{Anchor, Layer};
use iced_layershell::settings::{LayerShellSettings, Settings};
use iced_layershell::Application;

use self::app::OsdApp;

/// Run the OSD overlay
pub fn run_osd(socket_path: &str, _width: u32, _height: u32) -> Result<()> {
    eprintln!("OSD: Starting iced layershell overlay");
    eprintln!("OSD: Connecting to socket: {}", socket_path);

    OsdApp::run(Settings {
        layer_settings: LayerShellSettings {
            size: Some((440, 56)), // 420x36 bar + 20px padding for shadow
            exclusive_zone: 0,     // Don't reserve space
            anchor: Anchor::Top | Anchor::Left | Anchor::Right, // Top center
            layer: Layer::Overlay, // Top-most layer
            margin: (10, 0, 0, 0), // 10px from top
            // NOTE: StartMode::Background would prevent focus stealing, but is
            // forbidden by iced_layershell's Application::run() assertion.
            // The daemon() build pattern allows it, but is for multi-window apps.

            ..Default::default()
        },
        flags: socket_path.to_string(),
        ..Default::default()
    })?;

    Ok(())
}
