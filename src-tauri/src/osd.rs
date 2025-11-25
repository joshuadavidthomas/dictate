//! UI overlay for Wayland using iced

mod animation;
pub mod app;
mod buffer;
pub mod colors;
pub mod widgets;

use anyhow::Result;
use iced_layershell::build_pattern::daemon;
use tokio::sync::broadcast;

/// Run the OSD overlay in observer mode (Tauri-spawned)
/// The UI just displays events from the broadcast channel, doesn't send commands
pub fn run_osd_observer(
    broadcast_rx: broadcast::Receiver<crate::broadcast::Message>,
    osd_position: crate::conf::OsdPosition,
) -> Result<()> {
    eprintln!("[ui] Starting iced layershell overlay in observer mode");
    eprintln!("[ui] Using tokio broadcast channel for events");
    eprintln!("[ui] OSD position: {:?}", osd_position);

    daemon(
        app::OsdApp::namespace,
        app::OsdApp::update,
        app::OsdApp::view,
        app::OsdApp::remove_id,
    )
    .style(app::OsdApp::style)
    .subscription(app::OsdApp::subscription)
    .settings(app::OsdApp::settings(osd_position))
    .run_with(move || app::OsdApp::new(broadcast_rx, osd_position))?;

    Ok(())
}
