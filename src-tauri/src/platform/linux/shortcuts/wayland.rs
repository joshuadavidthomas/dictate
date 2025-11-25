//! Wayland Portal-based global shortcuts backend

use super::{
    BackendCapabilities, ShortcutBackend, ShortcutPlatform, SHORTCUT_DESCRIPTION, SHORTCUT_ID,
};
use crate::platform::display::detect_compositor;
use anyhow::{Context, Result};
use ashpd::desktop::global_shortcuts::{GlobalShortcuts, NewShortcut};
use futures_util::StreamExt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tauri::{AppHandle, Manager};
use tokio::sync::Mutex;

pub struct WaylandPortalBackend {
    app: AppHandle,
    proxy: Arc<Mutex<Option<GlobalShortcuts<'static>>>>,
    session: Arc<Mutex<Option<ashpd::desktop::Session<'static, GlobalShortcuts<'static>>>>>,
    listener_started: Arc<Mutex<bool>>,
}

impl WaylandPortalBackend {
    pub fn new(app: AppHandle) -> Self {
        Self {
            app,
            proxy: Arc::new(Mutex::new(None)),
            session: Arc::new(Mutex::new(None)),
            listener_started: Arc::new(Mutex::new(false)),
        }
    }

    /// Convert shortcut format: "CommandOrControl+Shift+Space" -> "<Control><Shift>space"
    fn convert_shortcut_format(shortcut: &str) -> String {
        let mut result = String::new();
        let parts: Vec<&str> = shortcut.split('+').collect();

        for (i, part) in parts.iter().enumerate() {
            let normalized = match part.trim() {
                "CommandOrControl" | "Ctrl" | "Control" => "<Control>",
                "Command" | "Super" | "Meta" => "<Super>",
                "Alt" => "<Alt>",
                "Shift" => "<Shift>",
                key => {
                    if i == parts.len() - 1 {
                        &key.to_lowercase()
                    } else {
                        continue;
                    }
                }
            };
            result.push_str(normalized);
        }

        result
    }

    async fn register_impl(&self, shortcut: &str) -> Result<()> {
        let portal_shortcut = Self::convert_shortcut_format(shortcut);

        // Create or get proxy
        let mut proxy_guard = self.proxy.lock().await;
        if proxy_guard.is_none() {
            let proxy = GlobalShortcuts::new()
                .await
                .context("Failed to create GlobalShortcuts proxy")?;
            *proxy_guard = Some(proxy);
        }
        let proxy = proxy_guard.as_ref().unwrap();

        // Create or get session
        let mut session_guard = self.session.lock().await;
        if session_guard.is_none() {
            let session = proxy
                .create_session()
                .await
                .context("Failed to create session")?;
            *session_guard = Some(session);
        }
        let session = session_guard.as_ref().unwrap();

        // Bind the shortcut
        let new_shortcut = NewShortcut::new(SHORTCUT_ID, SHORTCUT_DESCRIPTION)
            .preferred_trigger(Some(portal_shortcut.as_str()));

        let request = proxy
            .bind_shortcuts(session, &[new_shortcut], None)
            .await
            .context("Failed to create bind request")?;

        request
            .response()
            .context("Failed to get portal response")?;

        drop(session_guard);
        drop(proxy_guard);

        self.start_listener_if_needed().await;

        Ok(())
    }

    async fn start_listener_if_needed(&self) {
        let mut listener_started = self.listener_started.lock().await;
        if *listener_started {
            return;
        }
        *listener_started = true;

        let app_handle = self.app.clone();
        tokio::spawn(async move {
            let Ok(listener_proxy) = GlobalShortcuts::new().await else {
                return;
            };

            let Ok(mut stream) = listener_proxy.receive_activated().await else {
                return;
            };

            while let Some(_activated) = stream.next().await {
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
            }
        });
    }
}

impl ShortcutBackend for WaylandPortalBackend {
    fn register(&self, shortcut: &str) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        let shortcut = shortcut.to_string();
        Box::pin(async move { self.register_impl(&shortcut).await })
    }

    fn unregister(&self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        Box::pin(async move {
            let mut proxy_guard = self.proxy.lock().await;
            *proxy_guard = None;

            let mut session_guard = self.session.lock().await;
            if let Some(session) = session_guard.take() {
                drop(session);
            }

            Ok(())
        })
    }

    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities {
            platform: ShortcutPlatform::WaylandPortal,
            can_register: true,
            compositor: detect_compositor(),
        }
    }
}
