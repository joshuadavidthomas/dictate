//! The tray icon over `StatusNotifierItem` (SNI), for Waybar, KDE, and other SNI hosts.

use std::io::Cursor;
use std::sync::Arc;

use dictate_desktop::TrayCommand;
use dictate_desktop::TrayEvent;
use dictate_desktop::TrayStatus;
use ksni::blocking::TrayMethods as _;
use thiserror::Error;

const ICONS: [&[u8]; 3] = [
    include_bytes!("../../../packaging/icons/platform/tray-24.png"),
    include_bytes!("../../../packaging/icons/platform/tray-32.png"),
    include_bytes!("../../../packaging/icons/platform/tray-48.png"),
];

#[derive(Debug, Error)]
#[error("could not start the tray icon: {0}")]
pub struct TrayError(#[from] ksni::Error);

/// The running tray icon; dropping it removes the icon.
pub struct Tray {
    handle: ksni::blocking::Handle<SniTray>,
}

impl Tray {
    /// Shows the tray icon. `id` names this app to the host; `on_event`
    /// runs on the tray's own thread.
    pub fn spawn(
        id: &str,
        title: &str,
        on_event: impl Fn(TrayEvent) + Send + Sync + 'static,
    ) -> Result<Self, TrayError> {
        let tray = SniTray {
            id: id.to_owned(),
            title: title.to_owned(),
            status: TrayStatus::Idle,
            on_event: Arc::new(on_event),
        };
        // Waybar and other hosts may start after Dictate, or restart; the
        // icon appears whenever a host registers.
        let handle = tray.assume_sni_available(true).spawn()?;
        Ok(Self { handle })
    }

    pub fn set_status(&self, status: TrayStatus) {
        self.handle.update(|tray| tray.status = status);
    }
}

struct SniTray {
    id: String,
    title: String,
    status: TrayStatus,
    on_event: Arc<dyn Fn(TrayEvent) + Send + Sync>,
}

impl SniTray {
    fn send(&self, command: TrayCommand) {
        (self.on_event)(TrayEvent {
            command,
            activation_token: ksni::current_xdg_activation_token(),
        });
    }
}

impl ksni::Tray for SniTray {
    fn id(&self) -> String {
        self.id.clone()
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn category(&self) -> ksni::Category {
        ksni::Category::ApplicationStatus
    }

    // Pixmaps only: a themed icon name would win over them in most hosts
    // and hide the state.
    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        icons(self.status != TrayStatus::Idle)
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: self.title.clone(),
            description: self.status.tooltip().to_owned(),
            ..Default::default()
        }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        self.send(TrayCommand::OpenWindow);
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        let mut items = Vec::new();
        for item in self.status.menu() {
            if item.command == TrayCommand::Quit {
                items.push(ksni::MenuItem::Separator);
            }
            let command = item.command;
            items.push(
                ksni::menu::StandardItem {
                    label: item.label.to_owned(),
                    enabled: item.enabled,
                    activate: Box::new(move |tray: &mut Self| tray.send(command)),
                    ..Default::default()
                }
                .into(),
            );
        }
        items
    }
}

/// Dictate's bars in orange while it records or transcribes, and in grey
/// while it is idle.
fn icons(active: bool) -> Vec<ksni::Icon> {
    ICONS
        .iter()
        .filter_map(|png| match decode_argb(png) {
            Ok(mut icon) => {
                if !active {
                    desaturate(&mut icon);
                }
                Some(icon)
            }
            Err(error) => {
                eprintln!("could not decode a tray icon: {error}");
                None
            }
        })
        .collect()
}

fn decode_argb(png: &[u8]) -> Result<ksni::Icon, png::DecodingError> {
    let mut decoder = png::Decoder::new(Cursor::new(png));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info()?;
    let mut rgba = vec![0; reader.output_buffer_size().unwrap_or_default()];
    let frame = reader.next_frame(&mut rgba)?;
    rgba.truncate(frame.buffer_size());
    let channels = frame.color_type.samples();
    let data = rgba
        .chunks_exact(channels)
        .flat_map(|pixel| match *pixel {
            [r, g, b, a] => [a, r, g, b],
            [r, g, b] => [0xff, r, g, b],
            [l, a] => [a, l, l, l],
            [l] => [0xff, l, l, l],
            _ => [0; 4],
        })
        .collect();
    Ok(ksni::Icon {
        width: i32::try_from(frame.width).unwrap_or(i32::MAX),
        height: i32::try_from(frame.height).unwrap_or(i32::MAX),
        data,
    })
}

/// Replaces each pixel's color with its luminance, keeping its alpha.
fn desaturate(icon: &mut ksni::Icon) {
    for pixel in icon.data.chunks_exact_mut(4) {
        let [_, r, g, b] = [pixel[0], pixel[1], pixel[2], pixel[3]].map(u32::from);
        let luminance = u8::try_from((r * 299 + g * 587 + b * 114) / 1000).unwrap_or(u8::MAX);
        pixel[1..].fill(luminance);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_icons_decode_to_argb_pixmaps() {
        let icons = icons(true);
        assert_eq!(
            icons
                .iter()
                .map(|icon| (icon.width, icon.height, icon.data.len()))
                .collect::<Vec<_>>(),
            [
                (24, 24, 24 * 24 * 4),
                (32, 32, 32 * 32 * 4),
                (48, 48, 48 * 48 * 4)
            ]
        );
    }

    #[test]
    fn only_the_idle_icon_is_grey() {
        let is_grey = |icon: &ksni::Icon| {
            icon.data
                .chunks_exact(4)
                .all(|pixel| pixel[1] == pixel[2] && pixel[2] == pixel[3])
        };
        assert!(icons(false).iter().all(is_grey));
        assert!(!icons(true).iter().any(is_grey));
    }
}
