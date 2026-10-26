//! The menu bar icon (`NSStatusItem`), which lives on the main thread.

use dictate_desktop::TrayCommand;
use dictate_desktop::TrayEvent;
use dictate_desktop::TrayStatus;
use objc2::AnyThread as _;
use objc2::DefinedClass as _;
use objc2::MainThreadMarker;
use objc2::MainThreadOnly;
use objc2::define_class;
use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::NSObject;
use objc2::sel;
use objc2_app_kit::NSImage;
use objc2_app_kit::NSMenu;
use objc2_app_kit::NSMenuItem;
use objc2_app_kit::NSStatusBar;
use objc2_app_kit::NSStatusItem;
use objc2_app_kit::NSVariableStatusItemLength;
use objc2_foundation::NSData;
use objc2_foundation::NSSize;
use objc2_foundation::NSString;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum TrayError {
    #[error("the menu bar icon must be created on the main thread")]
    NotMainThread,
    #[error("macOS gave the menu bar icon no button")]
    NoButton,
}

/// The menu bar icon; dropping it removes the icon.
pub struct Tray {
    item: Retained<NSStatusItem>,
    /// The bars as a template image, which macOS draws in the menu bar's color.
    idle_icon: Option<Retained<NSImage>>,
    /// The bars in Dictate's orange.
    active_icon: Option<Retained<NSImage>>,
    menu: Retained<NSMenu>,
    target: Retained<MenuTarget>,
    title: String,
    mtm: MainThreadMarker,
}

impl Tray {
    /// Shows the menu bar icon. `on_event` runs on the main thread.
    pub fn spawn(
        _id: &str,
        title: &str,
        on_event: impl Fn(TrayEvent) + Send + Sync + 'static,
    ) -> Result<Self, TrayError> {
        let mtm = MainThreadMarker::new().ok_or(TrayError::NotMainThread)?;
        let item = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
        if item.button(mtm).is_none() {
            return Err(TrayError::NoButton);
        }
        let menu = NSMenu::new(mtm);
        // Enabled follows TrayStatus::menu, not AppKit's target validation.
        menu.setAutoenablesItems(false);
        item.setMenu(Some(&menu));
        let target = MenuTarget::new(mtm, Box::new(on_event));
        let tray = Self {
            item,
            idle_icon: bars_icon(true),
            active_icon: bars_icon(false),
            menu,
            target,
            title: title.to_owned(),
            mtm,
        };
        tray.set_status(TrayStatus::Idle);
        Ok(tray)
    }

    pub fn set_status(&self, status: TrayStatus) {
        if let Some(button) = self.item.button(self.mtm) {
            let image = match status {
                TrayStatus::Idle => &self.idle_icon,
                TrayStatus::Recording | TrayStatus::Transcribing => &self.active_icon,
            };
            button.setImage(image.as_deref());
            button.setToolTip(Some(&NSString::from_str(&format!(
                "{}: {}",
                self.title,
                status.tooltip()
            ))));
        }

        self.menu.removeAllItems();
        for item in status.menu() {
            if item.command == TrayCommand::Quit {
                self.menu.addItem(&NSMenuItem::separatorItem(self.mtm));
            }
            // SAFETY: the selector names a method MenuTarget defines with the
            // signature AppKit calls actions with.
            let menu_item = unsafe {
                NSMenuItem::initWithTitle_action_keyEquivalent(
                    NSMenuItem::alloc(self.mtm),
                    &NSString::from_str(item.label),
                    Some(sel!(menuItemClicked:)),
                    &NSString::new(),
                )
            };
            // SAFETY: the menu keeps the target alive no longer than this
            // Tray, which owns it.
            unsafe { menu_item.setTarget(Some(&self.target)) };
            menu_item.setTag(command_tag(item.command));
            menu_item.setEnabled(item.enabled);
            self.menu.addItem(&menu_item);
        }
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        NSStatusBar::systemStatusBar().removeStatusItem(&self.item);
    }
}

/// Dictate's bars at menu bar size, from a 2x raster. A template image is
/// drawn in the menu bar's own color from its alpha alone.
fn bars_icon(template: bool) -> Option<Retained<NSImage>> {
    const BARS: &[u8] = include_bytes!("../../../packaging/icons/platform/tray-36.png");
    let data = NSData::with_bytes(BARS);
    let image = NSImage::initWithData(NSImage::alloc(), &data)?;
    image.setSize(NSSize::new(18.0, 18.0));
    image.setTemplate(template);
    Some(image)
}

const COMMANDS: [TrayCommand; 4] = [
    TrayCommand::OpenWindow,
    TrayCommand::OpenSettings,
    TrayCommand::ToggleRecording,
    TrayCommand::Quit,
];

fn command_tag(command: TrayCommand) -> isize {
    COMMANDS
        .iter()
        .position(|candidate| *candidate == command)
        .and_then(|index| isize::try_from(index).ok())
        .unwrap_or(-1)
}

struct MenuTargetIvars {
    on_event: Box<dyn Fn(TrayEvent) + Send + Sync>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and MenuTarget
    // does not implement Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "DictateTrayMenuTarget"]
    #[ivars = MenuTargetIvars]
    struct MenuTarget;

    impl MenuTarget {
        #[unsafe(method(menuItemClicked:))]
        fn menu_item_clicked(&self, sender: &NSMenuItem) {
            let command = usize::try_from(sender.tag())
                .ok()
                .and_then(|index| COMMANDS.get(index));
            if let Some(&command) = command {
                (self.ivars().on_event)(TrayEvent {
                    command,
                    activation_token: None,
                });
            }
        }
    }
);

impl MenuTarget {
    fn new(
        mtm: MainThreadMarker,
        on_event: Box<dyn Fn(TrayEvent) + Send + Sync>,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(MenuTargetIvars { on_event });
        // SAFETY: NSObject's init takes no arguments and returns the initialized object.
        unsafe { msg_send![super(this), init] }
    }
}
