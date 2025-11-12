//! OSD (On-Screen Display) overlay for Wayland
//!
//! This module provides a visual overlay showing transcription state and audio levels.
//! Only available when compiled with the 'osd' feature flag.

mod render;
mod socket;
mod state;
mod wayland;

use anyhow::Result;
use std::time::Duration;
use wayland_client::{globals::registry_queue_init, Connection};

use self::wayland::OsdApp;

/// Run the OSD overlay
pub fn run_osd(socket_path: &str, _width: u32, _height: u32) -> Result<()> {
    eprintln!("OSD: Starting Wayland overlay");
    eprintln!("OSD: Connecting to socket: {}", socket_path);

    // Connect to Wayland
    let conn = Connection::connect_to_env()?;
    let (globals, mut event_queue) = registry_queue_init(&conn)?;
    let qh = event_queue.handle();

    // Create OSD app
    let mut app = OsdApp::new(globals, &qh, socket_path.to_string())?;

    // Create layer surface
    app.create_layer_surface(&qh)?;

    eprintln!("OSD: Layer surface created, waiting for configure event...");
    
    // Do a blocking roundtrip to wait for the configure event
    event_queue.blocking_dispatch(&mut app)?;
    event_queue.flush()?;
    
    eprintln!("OSD: Initial dispatch complete, entering event loop");

    // Main event loop
    eprintln!("OSD: Entering main loop");
    let mut frame_count = 0;
    let mut loop_count = 0;
    loop {
        loop_count += 1;
        eprintln!("OSD: Main loop iteration #{}", loop_count);
        
        // Handle socket messages
        app.handle_socket_messages();

        // Draw if needed
        let should_draw = app.should_draw();
        if loop_count % 60 == 0 || should_draw {
            eprintln!("OSD: Loop #{}, should_draw={}", loop_count, should_draw);
        }
        
        if should_draw {
            frame_count += 1;
            eprintln!("OSD: Drawing frame #{}", frame_count);
            if let Err(e) = app.draw(&qh) {
                eprintln!("OSD: Draw error: {}", e);
            } else {
                eprintln!("OSD: Frame #{} drawn successfully", frame_count);
            }
        }

        // Dispatch Wayland events (non-blocking)
        event_queue.dispatch_pending(&mut app)?;

        // Flush pending requests
        event_queue.flush()?;

        // Check for exit
        if app.exit {
            eprintln!("OSD: Exit requested");
            break;
        }

        // Small sleep to prevent busy loop
        std::thread::sleep(Duration::from_millis(16)); // ~60 FPS
    }

    eprintln!("OSD: Shutting down");
    Ok(())
}
