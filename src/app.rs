use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Duration;

use anyhow::Result;
use futures::StreamExt;
use futures::channel::mpsc;
use gpui::App as GpuiApp;
use gpui::Bounds;
use gpui::QuitMode;
use gpui::WindowBackgroundAppearance;
use gpui::WindowBounds;
use gpui::WindowHandle;
use gpui::WindowKind;
use gpui::WindowOptions;
use gpui::layer_shell::Anchor;
use gpui::layer_shell::KeyboardInteractivity;
use gpui::layer_shell::Layer;
use gpui::layer_shell::LayerShellOptions;
use gpui::point;
use gpui::prelude::*;
use gpui::px;
use gpui::size;
use gpui_platform::application;

use crate::overlay::OverlayState;
use crate::overlay::OverlayView;
use crate::spectrum::SPECTRUM_BANDS;
use crate::spectrum::SpectrumLevels;

pub(crate) const OVERLAY_WINDOW_WIDTH: f32 = 80.0;
pub(crate) const OVERLAY_WINDOW_HEIGHT: f32 = 48.0;
const BOTTOM_MARGIN: f32 = 40.0;

#[derive(Clone, Debug)]
pub struct Overlay {
    sender: mpsc::UnboundedSender<OverlayMessage>,
    revision: Arc<AtomicU64>,
    spectrum: SpectrumLevels,
}

impl Overlay {
    pub(crate) fn show(&self, state: OverlayState) {
        self.show_with_timeout(state, None);
    }

    pub(crate) fn show_briefly(&self, state: OverlayState, duration: Duration) {
        self.show_with_timeout(state, Some(duration));
    }

    fn show_with_timeout(&self, state: OverlayState, hide_after: Option<Duration>) {
        let revision = self.revision.fetch_add(1, Ordering::AcqRel) + 1;
        drop(self.sender.unbounded_send(OverlayMessage::Show {
            state,
            revision,
            hide_after,
        }));
    }

    pub fn hide(&self) {
        let revision = self.revision.fetch_add(1, Ordering::AcqRel) + 1;
        drop(
            self.sender
                .unbounded_send(OverlayMessage::Hide { revision }),
        );
    }

    pub fn send_spectrum(&self, bands: [f32; SPECTRUM_BANDS]) {
        self.spectrum.set(bands);
    }
}

#[derive(Clone, Copy, Debug)]
enum OverlayMessage {
    Show {
        state: OverlayState,
        revision: u64,
        hide_after: Option<Duration>,
    },
    Hide {
        revision: u64,
    },
}

pub fn run(start_daemon: impl FnOnce(Overlay) -> Result<()> + 'static) -> Result<()> {
    let (sender, mut receiver) = mpsc::unbounded();
    let revision = Arc::new(AtomicU64::new(0));
    let spectrum = SpectrumLevels::new();

    start_daemon(Overlay {
        sender: sender.clone(),
        revision: Arc::clone(&revision),
        spectrum: spectrum.clone(),
    })?;

    application()
        .with_quit_mode(QuitMode::Explicit)
        .run(move |cx: &mut GpuiApp| {
            cx.spawn(async move |cx| {
                let mut window: Option<WindowHandle<OverlayView>> = None;

                while let Some(mut message) = receiver.next().await {
                    loop {
                        match message {
                            OverlayMessage::Show {
                                state,
                                revision: message_revision,
                                hide_after,
                            } if message_revision == revision.load(Ordering::Acquire) => {
                                match &window {
                                    Some(handle) => {
                                        drop(handle.update(cx, |overlay, _, cx| {
                                            overlay.set_state(state, cx);
                                        }));
                                    }
                                    None => {
                                        match open_overlay_window(cx, spectrum.clone(), state) {
                                            Ok(handle) => window = Some(handle),
                                            Err(error) => {
                                                eprintln!("failed to show overlay: {error:#}");
                                            }
                                        }
                                    }
                                }

                                if let Some(duration) = hide_after {
                                    let sender = sender.clone();
                                    cx.spawn(async move |cx| {
                                        cx.background_executor().timer(duration).await;
                                        drop(sender.unbounded_send(OverlayMessage::Hide {
                                            revision: message_revision,
                                        }));
                                    })
                                    .detach();
                                }
                            }
                            OverlayMessage::Hide {
                                revision: message_revision,
                            } if message_revision == revision.load(Ordering::Acquire) => {
                                if let Some(handle) = window.take() {
                                    drop(handle.update(cx, |_, window, _| {
                                        window.remove_window();
                                    }));
                                }
                            }
                            OverlayMessage::Show { .. } | OverlayMessage::Hide { .. } => {}
                        }

                        match receiver.try_recv() {
                            Ok(next) => message = next,
                            Err(_) => break,
                        }
                    }
                }
            })
            .detach();
        });

    Ok(())
}

fn open_overlay_window(
    cx: &gpui::AsyncApp,
    spectrum: SpectrumLevels,
    state: OverlayState,
) -> gpui::Result<WindowHandle<OverlayView>> {
    cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                point(px(0.0), px(0.0)),
                size(px(OVERLAY_WINDOW_WIDTH), px(OVERLAY_WINDOW_HEIGHT)),
            ))),
            titlebar: None,
            focus: false,
            is_resizable: false,
            is_minimizable: false,
            app_id: Some(env!("DICTATE_OVERLAY_APP_ID").to_owned()),
            window_background: WindowBackgroundAppearance::Transparent,
            kind: WindowKind::LayerShell(LayerShellOptions {
                namespace: env!("DICTATE_OVERLAY_NAMESPACE").to_owned(),
                layer: Layer::Overlay,
                anchor: Anchor::BOTTOM,
                margin: Some((px(0.0), px(0.0), px(BOTTOM_MARGIN), px(0.0))),
                keyboard_interactivity: KeyboardInteractivity::None,
                ..Default::default()
            }),
            ..Default::default()
        },
        |_, cx| cx.new(|cx| OverlayView::new(spectrum, state, cx)),
    )
}
