//! UI overlay for Wayland using iced

mod animation;
mod app;
mod buffer;
mod colors;
mod widgets;

use anyhow::Result;
use iced_layershell::build_pattern::daemon;
use tokio::sync::broadcast;

pub use app::{TranscriptionConfig, TranscriptionMode};

/// Run the OSD overlay in observer mode (Tauri-spawned)
/// The UI just displays events from the broadcast channel, doesn't send commands
pub fn run_osd_observer(broadcast_rx: broadcast::Receiver<String>, config: TranscriptionConfig) -> Result<()> {
    eprintln!("[ui] Starting iced layershell overlay in observer mode");
    eprintln!("[ui] Using tokio broadcast channel for events");

    daemon(
        app::OsdApp::namespace,
        app::OsdApp::update,
        app::OsdApp::view,
        app::OsdApp::remove_id,
    )
    .style(app::OsdApp::style)
    .subscription(app::OsdApp::subscription)
    .settings(app::OsdApp::settings())
    .run_with(move || app::OsdApp::new(broadcast_rx, config.clone(), TranscriptionMode::Observer))?;

    Ok(())
}
