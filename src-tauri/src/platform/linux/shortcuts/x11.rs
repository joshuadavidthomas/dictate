//! X11 global shortcuts backend using tauri-plugin-global-shortcut

use super::{BackendCapabilities, ShortcutBackend, ShortcutPlatform};
use crate::platform::display::detect_compositor;
use anyhow::Result;
use std::future::Future;
use std::pin::Pin;
use tauri::{AppHandle, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut};

pub struct X11Backend {
    app: AppHandle,
}

impl X11Backend {
    pub fn new(app: AppHandle) -> Self {
        Self { app }
    }

    async fn register_impl(&self, shortcut: &str) -> Result<()> {
        let parsed = shortcut
            .parse::<Shortcut>()
            .map_err(|e| anyhow::anyhow!("Invalid shortcut format: {}", e))?;

        let app_handle = self.app.clone();

        self.app
            .global_shortcut()
            .on_shortcut(parsed, move |_app, _shortcut, _event| {
                let app = app_handle.clone();
                tauri::async_runtime::spawn(async move {
                    let _ = crate::commands::toggle_recording(
                        app.state::<crate::state::RecordingState>(),
                        app.state::<crate::state::TranscriptionState>(),
                        app.state::<crate::conf::SettingsState>(),
                        app.state::<crate::broadcast::BroadcastServer>(),
                        app.clone(),
                    )
                    .await;
                });
            })
            .map_err(|e| anyhow::anyhow!("Failed to register shortcut: {}", e))?;

        Ok(())
    }
}

impl ShortcutBackend for X11Backend {
    fn register(&self, shortcut: &str) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        let shortcut = shortcut.to_string();
        Box::pin(async move { self.register_impl(&shortcut).await })
    }

    fn unregister(&self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        Box::pin(async move { Ok(()) })
    }

    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities {
            platform: ShortcutPlatform::X11,
            can_register: true,
            compositor: detect_compositor(),
        }
    }
}
