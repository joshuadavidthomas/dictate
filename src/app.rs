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
use gpui::layer_shell::*;
use gpui::point;
use gpui::prelude::*;
use gpui::px;
use gpui::size;
use gpui_platform::application;

use crate::overlay::OverlayView;
use crate::spectrum::SPECTRUM_BANDS;
use crate::spectrum::SpectrumLevels;

const WINDOW_WIDTH: f32 = 72.0;
const WINDOW_HEIGHT: f32 = 40.0;
const BOTTOM_MARGIN: f32 = 40.0;

#[derive(Clone, Debug)]
pub struct Overlay {
    sender: mpsc::UnboundedSender<OverlayMessage>,
    spectrum: SpectrumLevels,
}

impl Overlay {
    pub fn show(&self) {
        let _ = self.sender.unbounded_send(OverlayMessage::Show);
    }

    pub fn hide(&self) {
        let _ = self.sender.unbounded_send(OverlayMessage::Hide);
    }

    pub fn send_spectrum(&self, bands: [f32; SPECTRUM_BANDS]) {
        self.spectrum.set(bands);
    }
}

#[derive(Clone, Copy, Debug)]
enum OverlayMessage {
    Show,
    Hide,
}

pub fn run(start_daemon: impl FnOnce(Overlay) -> Result<()> + 'static) -> Result<()> {
    let (sender, mut receiver) = mpsc::unbounded();
    let spectrum = SpectrumLevels::new();

    start_daemon(Overlay {
        sender,
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
                            OverlayMessage::Show => {
                                if window.is_none() {
                                    match open_overlay_window(cx, spectrum.clone()) {
                                        Ok(handle) => window = Some(handle),
                                        Err(error) => {
                                            eprintln!("failed to show overlay: {error:#}")
                                        }
                                    }
                                }
                            }
                            OverlayMessage::Hide => {
                                if let Some(handle) = window.take() {
                                    let _ = handle.update(cx, |_, window, _| {
                                        window.remove_window();
                                    });
                                }
                            }
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
) -> gpui::Result<WindowHandle<OverlayView>> {
    cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                point(px(0.0), px(0.0)),
                size(px(WINDOW_WIDTH), px(WINDOW_HEIGHT)),
            ))),
            titlebar: None,
            focus: false,
            is_resizable: false,
            is_minimizable: false,
            app_id: Some("dev.joshthomas.dictate.gpui".to_string()),
            window_background: WindowBackgroundAppearance::Transparent,
            kind: WindowKind::LayerShell(LayerShellOptions {
                namespace: "dictate-overlay".to_string(),
                layer: Layer::Overlay,
                anchor: Anchor::BOTTOM,
                margin: Some((px(0.0), px(0.0), px(BOTTOM_MARGIN), px(0.0))),
                keyboard_interactivity: KeyboardInteractivity::None,
                ..Default::default()
            }),
            ..Default::default()
        },
        |_, cx| cx.new(|cx| OverlayView::new(spectrum, cx)),
    )
}
