//! UI overlay for Wayland using iced

pub mod app;
pub mod backend;
mod buffer;
pub mod state;
pub mod theme;
pub mod timeline;
pub mod widgets;

use anyhow::Result;
use backend::detect_backend;
use iced_layershell::build_pattern::daemon;
use std::sync::Arc;
use tokio::sync::broadcast;

/// Run the OSD overlay in observer mode (Tauri-spawned)
/// The UI just displays events from the broadcast channel, doesn't send commands
pub fn run_osd_observer(
    broadcast_rx: broadcast::Receiver<crate::broadcast::Message>,
    osd_position: crate::conf::OsdPosition,
) -> Result<()> {
    let backend = detect_backend();
    log::info!("Starting iced layershell overlay in observer mode");
    log::info!("Using {} backend", backend.name());
    log::debug!("Backend available: {}", backend.is_available());
    log::debug!("Using tokio broadcast channel for events");
    log::debug!("OSD position: {:?}", osd_position);

    daemon(
        app::OsdApp::namespace,
        app::OsdApp::update,
        app::OsdApp::view,
        app::OsdApp::remove_id,
    )
    .style(app::OsdApp::style)
    .subscription(app::OsdApp::subscription)
    .settings(backend.app_settings(osd_position))
    .run_with(move || app::OsdApp::new(Arc::clone(&backend), broadcast_rx, osd_position))?;

    Ok(())
}
