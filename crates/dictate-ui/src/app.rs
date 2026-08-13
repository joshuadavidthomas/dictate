use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Duration;

use anyhow::Result;
use dictate_signal::SPECTRUM_BANDS;
use dictate_signal::SpectrumLevels;
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
use crate::partial::PartialTextStyle;
use crate::partial::PartialView;

pub const OVERLAY_WINDOW_WIDTH: f32 = 80.0;
pub const OVERLAY_WINDOW_HEIGHT: f32 = 48.0;
const BOTTOM_MARGIN: f32 = 40.0;
const PARTIAL_WINDOW_WIDTH: f32 = 420.0;
const PARTIAL_WINDOW_HEIGHT: f32 = 160.0;
const PARTIAL_BOTTOM_MARGIN: f32 = 100.0;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UiIdentity {
    app_id: &'static str,
    wayland_namespace: &'static str,
}

impl UiIdentity {
    #[must_use]
    pub const fn new(app_id: &'static str, wayland_namespace: &'static str) -> Self {
        Self {
            app_id,
            wayland_namespace,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Overlay {
    sender: mpsc::UnboundedSender<OverlayMessage>,
    revision: Arc<AtomicU64>,
    spectrum: SpectrumLevels,
}

impl Overlay {
    pub fn show(&self, state: OverlayState) {
        self.show_with_timeout(state, None);
    }

    pub fn show_briefly(&self, state: OverlayState, duration: Duration) {
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

    pub fn send_partial(&self, text: &str) {
        let revision = self.revision.load(Ordering::Acquire);
        drop(self.sender.unbounded_send(OverlayMessage::Partial {
            text: text.to_owned(),
            revision,
        }));
    }

    pub fn send_spectrum(&self, bands: [f32; SPECTRUM_BANDS]) {
        self.spectrum.set(bands);
    }
}

#[derive(Clone, Debug)]
enum OverlayMessage {
    Show {
        state: OverlayState,
        revision: u64,
        hide_after: Option<Duration>,
    },
    Hide {
        revision: u64,
    },
    Partial {
        text: String,
        revision: u64,
    },
}

#[derive(Debug, Default, Eq, PartialEq)]
struct OverlaySession {
    pill: Option<OverlayState>,
    partial: Option<String>,
}

#[derive(Debug, Eq, PartialEq)]
enum OverlayCommand {
    Show(OverlayState),
    Hide,
    Partial(String),
}

fn apply_overlay_command(session: &mut OverlaySession, command: OverlayCommand) {
    match command {
        OverlayCommand::Show(state) => {
            session.pill = Some(state);
            session.partial = None;
        }
        OverlayCommand::Hide => {
            session.pill = None;
            session.partial = None;
        }
        OverlayCommand::Partial(text)
            if session.pill == Some(OverlayState::Recording) && !text.trim().is_empty() =>
        {
            session.partial = Some(text);
        }
        OverlayCommand::Partial(_) => {}
    }
}

pub fn run(
    identity: UiIdentity,
    partial_text_style: PartialTextStyle,
    start_daemon: impl FnOnce(Overlay) -> Result<()> + 'static,
) -> Result<()> {
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
                let mut pill_window: Option<WindowHandle<OverlayView>> = None;
                let mut partial_window: Option<WindowHandle<PartialView>> = None;
                let mut session = OverlaySession::default();

                while let Some(mut message) = receiver.next().await {
                    loop {
                        let mut hide_after = None;
                        let applied = match message {
                            OverlayMessage::Show {
                                state,
                                revision: message_revision,
                                hide_after: message_hide_after,
                            } if message_revision == revision.load(Ordering::Acquire) => {
                                apply_overlay_command(&mut session, OverlayCommand::Show(state));
                                hide_after =
                                    message_hide_after.map(|duration| (duration, message_revision));
                                true
                            }
                            OverlayMessage::Hide {
                                revision: message_revision,
                            } if message_revision == revision.load(Ordering::Acquire) => {
                                apply_overlay_command(&mut session, OverlayCommand::Hide);
                                true
                            }
                            OverlayMessage::Partial {
                                text,
                                revision: message_revision,
                            } if message_revision == revision.load(Ordering::Acquire) => {
                                apply_overlay_command(&mut session, OverlayCommand::Partial(text));
                                true
                            }
                            OverlayMessage::Show { .. }
                            | OverlayMessage::Hide { .. }
                            | OverlayMessage::Partial { .. } => false,
                        };

                        if applied {
                            sync_pill_window(
                                cx,
                                &mut pill_window,
                                spectrum.clone(),
                                session.pill,
                                identity,
                            );
                            sync_partial_window(
                                cx,
                                &mut partial_window,
                                session.partial.as_ref(),
                                &partial_text_style,
                                identity,
                            );

                            if let Some((duration, message_revision)) = hide_after {
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

fn sync_pill_window(
    cx: &mut gpui::AsyncApp,
    window: &mut Option<WindowHandle<OverlayView>>,
    spectrum: SpectrumLevels,
    state: Option<OverlayState>,
    identity: UiIdentity,
) {
    match (window.as_ref(), state) {
        (Some(handle), Some(state)) => {
            drop(handle.update(cx, |overlay, _, cx| {
                overlay.set_state(state, cx);
            }));
        }
        (None, Some(state)) => match open_overlay_window(cx, spectrum, state, identity) {
            Ok(handle) => *window = Some(handle),
            Err(error) => eprintln!("failed to show overlay: {error:#}"),
        },
        (Some(_), None) => {
            if let Some(handle) = window.take() {
                drop(handle.update(cx, |_, window, _| {
                    window.remove_window();
                }));
            }
        }
        (None, None) => {}
    }
}

fn sync_partial_window(
    cx: &mut gpui::AsyncApp,
    window: &mut Option<WindowHandle<PartialView>>,
    text: Option<&String>,
    style: &PartialTextStyle,
    identity: UiIdentity,
) {
    match (window.as_ref(), text) {
        (Some(handle), Some(text)) => {
            let text = text.clone();
            drop(handle.update(cx, move |partial, _, cx| {
                partial.set_text(&text, cx);
            }));
        }
        (None, Some(text)) => match open_partial_window(cx, text, style, identity) {
            Ok(handle) => *window = Some(handle),
            Err(error) => eprintln!("failed to show partials: {error:#}"),
        },
        (Some(_), None) => {
            if let Some(handle) = window.take() {
                drop(handle.update(cx, |_, window, _| {
                    window.remove_window();
                }));
            }
        }
        (None, None) => {}
    }
}

fn open_overlay_window(
    cx: &gpui::AsyncApp,
    spectrum: SpectrumLevels,
    state: OverlayState,
    identity: UiIdentity,
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
            app_id: Some(identity.app_id.to_owned()),
            window_background: WindowBackgroundAppearance::Transparent,
            kind: WindowKind::LayerShell(LayerShellOptions {
                namespace: identity.wayland_namespace.to_owned(),
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

fn open_partial_window(
    cx: &gpui::AsyncApp,
    text: &str,
    style: &PartialTextStyle,
    identity: UiIdentity,
) -> gpui::Result<WindowHandle<PartialView>> {
    let text = text.to_owned();
    let style = style.clone();
    cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                point(px(0.0), px(0.0)),
                size(px(PARTIAL_WINDOW_WIDTH), px(PARTIAL_WINDOW_HEIGHT)),
            ))),
            titlebar: None,
            focus: false,
            is_resizable: false,
            is_minimizable: false,
            app_id: Some(identity.app_id.to_owned()),
            window_background: WindowBackgroundAppearance::Transparent,
            kind: WindowKind::LayerShell(LayerShellOptions {
                namespace: format!("{}-partials", identity.wayland_namespace),
                layer: Layer::Overlay,
                anchor: Anchor::BOTTOM,
                margin: Some((px(0.0), px(0.0), px(PARTIAL_BOTTOM_MARGIN), px(0.0))),
                keyboard_interactivity: KeyboardInteractivity::None,
                ..Default::default()
            }),
            ..Default::default()
        },
        |_, cx| cx.new(|cx| PartialView::new(&text, style, cx)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recording_session() -> OverlaySession {
        OverlaySession {
            pill: Some(OverlayState::Recording),
            partial: None,
        }
    }

    #[test]
    fn partial_during_recording_is_stored() {
        let mut session = recording_session();

        apply_overlay_command(&mut session, OverlayCommand::Partial("hello".to_owned()));

        assert_eq!(session.partial.as_deref(), Some("hello"));
    }

    #[test]
    fn empty_partial_during_recording_is_ignored() {
        let mut session = recording_session();

        apply_overlay_command(&mut session, OverlayCommand::Partial(String::new()));

        assert_eq!(session.partial, None);
    }

    #[test]
    fn partial_without_a_pill_is_ignored() {
        let mut session = OverlaySession::default();

        apply_overlay_command(&mut session, OverlayCommand::Partial("late".to_owned()));

        assert_eq!(session, OverlaySession::default());
    }

    #[test]
    fn partial_after_hide_is_ignored() {
        let mut session = recording_session();
        apply_overlay_command(&mut session, OverlayCommand::Partial("visible".to_owned()));
        apply_overlay_command(&mut session, OverlayCommand::Hide);

        apply_overlay_command(&mut session, OverlayCommand::Partial("late".to_owned()));

        assert_eq!(session, OverlaySession::default());
    }

    #[test]
    fn partial_after_transcribing_show_is_ignored() {
        let mut session = recording_session();
        apply_overlay_command(
            &mut session,
            OverlayCommand::Show(OverlayState::Transcribing),
        );

        apply_overlay_command(&mut session, OverlayCommand::Partial("late".to_owned()));

        assert_eq!(session.pill, Some(OverlayState::Transcribing));
        assert_eq!(session.partial, None);
    }

    #[test]
    fn recording_show_clears_previous_partial() {
        let mut session = recording_session();
        session.partial = Some("old".to_owned());

        apply_overlay_command(&mut session, OverlayCommand::Show(OverlayState::Recording));

        assert_eq!(session.pill, Some(OverlayState::Recording));
        assert_eq!(session.partial, None);
    }

    #[test]
    fn transcribing_show_clears_previous_partial() {
        let mut session = recording_session();
        session.partial = Some("old".to_owned());

        apply_overlay_command(
            &mut session,
            OverlayCommand::Show(OverlayState::Transcribing),
        );

        assert_eq!(session.pill, Some(OverlayState::Transcribing));
        assert_eq!(session.partial, None);
    }

    #[test]
    fn hide_clears_pill_and_partial() {
        let mut session = recording_session();
        session.partial = Some("visible".to_owned());

        apply_overlay_command(&mut session, OverlayCommand::Hide);

        assert_eq!(session, OverlaySession::default());
    }
}
