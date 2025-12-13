//! UI overlay for Wayland using iced
//!
//! This module provides the OSD (On-Screen Display) overlay for dictate.
//!
//! ## Architecture Patterns (from exploration)
//!
//! - **State Machine**: `state.rs` - Explicit visual states with transitions
//! - **Timeline Animations**: `timeline.rs` - Declarative keyframe animations
//! - **Theme Constants**: `theme.rs` - Centralized styling values
//! - **Message Domains**: `messages.rs` - Grouped message types
//! - **Subscription Batching**: `subscriptions.rs` - Organized async sources
//! - **View Helpers**: `view_helpers.rs` - State-to-view mapping utilities

mod animation;
pub mod app;
pub mod app_v2;
mod buffer;
pub mod colors;
pub mod messages;
pub mod state;
pub mod subscriptions;
pub mod theme;
pub mod timeline;
pub mod view_helpers;
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
    log::info!("Starting iced layershell overlay in observer mode");
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
    .settings(app::OsdApp::settings(osd_position))
    .run_with(move || app::OsdApp::new(broadcast_rx, osd_position))?;

    Ok(())
}
