use std::io::BufRead;
use std::io::BufReader;
use std::io::Write;
use std::process::Child;
use std::process::Command;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::mpsc;
use std::thread;
use std::thread::JoinHandle;

use anyhow::Context;
use anyhow::Result;
use gpui::App as GpuiApp;
use gpui::Bounds;
use gpui::WindowBackgroundAppearance;
use gpui::WindowBounds;
use gpui::WindowKind;
use gpui::WindowOptions;
use gpui::layer_shell::*;
use gpui::point;
use gpui::prelude::*;
use gpui::px;
use gpui::size;
use gpui_platform::application;

use crate::overlay::Overlay;
use crate::spectrum::SPECTRUM_BANDS;
use crate::state::SpectrumLevels;

const WINDOW_WIDTH: f32 = 72.0;
const WINDOW_HEIGHT: f32 = 40.0;
const BOTTOM_MARGIN: f32 = 40.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpectrumFrame {
    bands: [f32; SPECTRUM_BANDS],
}

impl SpectrumFrame {
    pub const fn new(bands: [f32; SPECTRUM_BANDS]) -> Self {
        Self { bands }
    }

    pub const fn bands(self) -> [f32; SPECTRUM_BANDS] {
        self.bands
    }

    fn encode(self) -> String {
        self.bands
            .into_iter()
            .map(|level| level.to_string())
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn parse(line: &str) -> Option<Self> {
        let values = line
            .split_whitespace()
            .map(str::parse::<f32>)
            .collect::<Result<Vec<_>, _>>()
            .ok()?;

        Some(Self::new(values.try_into().ok()?))
    }
}

#[derive(Clone, Debug)]
pub struct App {
    inner: Arc<AppInner>,
}

#[derive(Debug)]
struct AppInner {
    child: Mutex<Option<OverlayChild>>,
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(AppInner {
                child: Mutex::new(None),
            }),
        }
    }

    pub fn show(&self) {
        let mut child = self.inner.child.lock().unwrap();
        if let Some(overlay) = child.as_mut() {
            if overlay.is_running() {
                return;
            }

            if let Some(mut overlay) = child.take() {
                overlay.stop();
            }
        }

        match OverlayChild::spawn() {
            Ok(overlay) => *child = Some(overlay),
            Err(error) => eprintln!("failed to start overlay app: {error:#}"),
        }
    }

    pub fn hide(&self) {
        let mut child = self.inner.child.lock().unwrap();
        if let Some(mut overlay) = child.take() {
            overlay.stop();
        }
    }

    pub fn send_frame(&self, frame: SpectrumFrame) {
        let child = self.inner.child.lock().unwrap();
        if let Some(overlay) = child.as_ref()
            && let Some(sender) = overlay.sender.as_ref()
        {
            let _ = sender.send(frame);
        }
    }
}

impl Drop for AppInner {
    fn drop(&mut self) {
        if let Some(mut overlay) = self.child.lock().unwrap().take() {
            overlay.stop();
        }
    }
}

#[derive(Debug)]
struct OverlayChild {
    child: Child,
    sender: Option<mpsc::Sender<SpectrumFrame>>,
    writer: Option<JoinHandle<()>>,
}

impl OverlayChild {
    fn spawn() -> Result<Self> {
        let executable = std::env::current_exe().context("locating Dictate executable")?;
        let mut child = Command::new(executable)
            .arg("app")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .context("spawning overlay app")?;
        let child_input = child
            .stdin
            .take()
            .context("opening overlay app input pipe")?;
        let (sender, receiver) = mpsc::channel::<SpectrumFrame>();
        let writer = thread::spawn(move || {
            let mut child_input = child_input;
            for frame in receiver {
                if writeln!(child_input, "{}", frame.encode()).is_err() {
                    break;
                }
            }
        });

        Ok(Self {
            child,
            sender: Some(sender),
            writer: Some(writer),
        })
    }

    fn is_running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    fn stop(&mut self) {
        self.sender.take();
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
    }
}

pub fn run() {
    let spectrum = SpectrumLevels::new();
    let input_spectrum = spectrum.clone();
    thread::spawn(move || {
        let input = std::io::stdin();
        for line in BufReader::new(input).lines() {
            let Ok(line) = line else {
                break;
            };

            if let Some(frame) = SpectrumFrame::parse(&line) {
                input_spectrum.set(frame.bands());
            }
        }
        std::process::exit(0);
    });

    application().run(move |cx: &mut GpuiApp| {
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
            |_, cx| {
                let spectrum = spectrum.clone();
                cx.new(|cx| Overlay::new(cx, spectrum))
            },
        )
        .unwrap();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spectrum_frame_round_trips() {
        let frame = SpectrumFrame::new([0.0, 0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 1.0]);

        assert_eq!(SpectrumFrame::parse(&frame.encode()), Some(frame));
    }

    #[test]
    fn malformed_spectrum_frame_is_ignored() {
        assert_eq!(SpectrumFrame::parse("1 2 3"), None);
        assert_eq!(SpectrumFrame::parse("not a number"), None);
    }
}
