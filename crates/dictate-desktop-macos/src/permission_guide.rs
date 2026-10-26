//! The floating guide beside System Settings' Privacy & Security pane.
//!
//! It sits at the bottom of the System Settings window, says which switch to
//! turn on, and offers the app as a tile to drag into the list when it is
//! missing there. It follows the window while it moves and closes once the
//! permission is granted or System Settings closes. Copied from Hex's guide.

use std::cell::RefCell;
use std::path::Path;
use std::path::PathBuf;
use std::ptr::NonNull;

use block2::RcBlock;
use dispatch2::DispatchQueue;
use objc2::AnyThread as _;
use objc2::DefinedClass as _;
use objc2::MainThreadMarker;
use objc2::MainThreadOnly as _;
use objc2::Message as _;
use objc2::define_class;
use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::runtime::ProtocolObject;
use objc2_app_kit::NSBackingStoreType;
use objc2_app_kit::NSBox;
use objc2_app_kit::NSBoxType;
use objc2_app_kit::NSColor;
use objc2_app_kit::NSCursor;
use objc2_app_kit::NSDragOperation;
use objc2_app_kit::NSDraggingContext;
use objc2_app_kit::NSDraggingItem;
use objc2_app_kit::NSDraggingSession;
use objc2_app_kit::NSDraggingSource;
use objc2_app_kit::NSEvent;
use objc2_app_kit::NSFloatingWindowLevel;
use objc2_app_kit::NSFont;
use objc2_app_kit::NSFontWeightMedium;
use objc2_app_kit::NSImage;
use objc2_app_kit::NSImageScaling;
use objc2_app_kit::NSImageView;
use objc2_app_kit::NSPanel;
use objc2_app_kit::NSRunningApplication;
use objc2_app_kit::NSScreen;
use objc2_app_kit::NSTextField;
use objc2_app_kit::NSTitlePosition;
use objc2_app_kit::NSView;
use objc2_app_kit::NSWindowCollectionBehavior;
use objc2_app_kit::NSWindowStyleMask;
use objc2_app_kit::NSWorkspace;
use objc2_core_graphics::CGWindowListCopyWindowInfo;
use objc2_core_graphics::CGWindowListOption;
use objc2_foundation::NSArray;
use objc2_foundation::NSDictionary;
use objc2_foundation::NSNumber;
use objc2_foundation::NSObjectProtocol;
use objc2_foundation::NSOperatingSystemVersion;
use objc2_foundation::NSPoint;
use objc2_foundation::NSProcessInfo;
use objc2_foundation::NSRect;
use objc2_foundation::NSSize;
use objc2_foundation::NSString;
use objc2_foundation::NSTimer;
use objc2_foundation::NSURL;

use crate::permissions::PermissionKind;
use crate::permissions::granted_in_settings;

const SETTINGS_BUNDLE_ID: &str = "com.apple.systempreferences";
const POLL_SECONDS: f64 = 0.12;
/// Matches Hex's guide, so the two read as the same piece of macOS furniture.
const WIDTH: f64 = 480.0;
const INSET: f64 = 18.0;
const TILE_HEIGHT: f64 = 44.0;
const LABEL_HEIGHT: f64 = 24.0;
/// Below the tile: 17 + 44 + 12 + 24 + 19.
const DRAGGABLE_HEIGHT: f64 = 116.0;
/// The microphone guide has no tile.
const SWITCH_ONLY_HEIGHT: f64 = 60.0;
/// How far the pointer moves before a press on the tile becomes a drag.
const DRAG_THRESHOLD: f64 = 3.0;
/// Polls System Settings is not frontmost for before the guide gets out of the
/// way, so opening the pane does not close it in the moment before activation.
const AWAY_TICKS: u32 = 4;

thread_local! {
    /// The open guide; only the main thread touches it.
    static GUIDE: RefCell<Option<Guide>> = const { RefCell::new(None) };
}

struct Guide {
    panel: Retained<NSPanel>,
    timer: Retained<NSTimer>,
    kind: PermissionKind,
    /// System Settings has shown a window since the guide opened, so its
    /// window going away means the user closed it.
    saw_settings: bool,
    /// Consecutive polls System Settings has not been the frontmost app.
    away: u32,
}

/// Shows the guide for `kind` beside System Settings, replacing any open
/// one. Safe to call from any thread. A binary outside an app bundle has
/// nothing to list, so it gets no guide.
pub(crate) fn show(kind: PermissionKind) {
    let Some(bundle) = app_bundle() else {
        eprintln!("not running from an app bundle; no permission guide");
        return;
    };
    DispatchQueue::main().exec_async(move || {
        if let Some(mtm) = MainThreadMarker::new() {
            open(kind, &bundle, mtm);
        }
    });
}

/// What System Settings calls the list `kind` lives in; macOS 27 renamed
/// Accessibility.
fn list_name(kind: PermissionKind) -> &'static str {
    match kind {
        PermissionKind::Microphone => "Microphone",
        PermissionKind::InputMonitoring => "Input Monitoring",
        PermissionKind::Accessibility => {
            let renamed = NSProcessInfo::processInfo().isOperatingSystemAtLeastVersion(
                NSOperatingSystemVersion {
                    majorVersion: 27,
                    minorVersion: 0,
                    patchVersion: 0,
                },
            );
            if renamed {
                "Device Control and Data Access"
            } else {
                "Accessibility"
            }
        }
    }
}

/// The `.app` this process runs from.
fn app_bundle() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    exe.ancestors()
        .find(|path| path.extension().is_some_and(|extension| extension == "app"))
        .map(Path::to_path_buf)
}

fn open(kind: PermissionKind, bundle: &Path, mtm: MainThreadMarker) {
    close();
    let app_name = bundle.file_stem().map_or_else(
        || "Dictate".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    let list = list_name(kind);
    // The Microphone list only holds apps that asked, so a refused
    // microphone is switched on rather than dragged in.
    let draggable = kind != PermissionKind::Microphone;
    let height = if draggable {
        DRAGGABLE_HEIGHT
    } else {
        SWITCH_ONLY_HEIGHT
    };

    let panel = NSPanel::initWithContentRect_styleMask_backing_defer(
        NSPanel::alloc(mtm),
        NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(WIDTH, height)),
        NSWindowStyleMask::Borderless
            | NSWindowStyleMask::NonactivatingPanel
            | NSWindowStyleMask::FullSizeContentView,
        NSBackingStoreType::Buffered,
        false,
    );
    panel.setFloatingPanel(true);
    panel.setLevel(NSFloatingWindowLevel);
    panel.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::FullScreenAuxiliary,
    );
    panel.setOpaque(false);
    panel.setBackgroundColor(Some(&NSColor::clearColor()));
    panel.setHasShadow(true);
    panel.setHidesOnDeactivate(false);
    // SAFETY: The guide keeps the panel retained until it orders it out.
    unsafe { panel.setReleasedWhenClosed(false) };

    let content = rounded_box(
        NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(WIDTH, height)),
        20.0,
        &NSColor::windowBackgroundColor(),
        mtm,
    );
    let text_width = WIDTH - INSET * 2.0;
    // One line, as Hex has it: the tile is the thing to drag, so the line
    // names the drag rather than carrying a second "not in the list" hint.
    let instruction = if draggable {
        format!("\u{2191}  Drag {app_name} into the {list} list above")
    } else {
        format!("\u{2191}  Switch on {app_name} in the {list} list above")
    };
    let headline = label(&instruction, 13.0, true, mtm);
    let headline_y = if draggable {
        17.0 + TILE_HEIGHT + 12.0
    } else {
        (SWITCH_ONLY_HEIGHT - LABEL_HEIGHT) / 2.0
    };
    headline.setFrame(NSRect::new(
        NSPoint::new(INSET, headline_y),
        NSSize::new(text_width, LABEL_HEIGHT),
    ));
    content.addSubview(&headline);
    if draggable {
        let tile = AppTile::new(
            NSRect::new(
                NSPoint::new(INSET, 17.0),
                NSSize::new(text_width, TILE_HEIGHT),
            ),
            bundle,
            &app_name,
            mtm,
        );
        content.addSubview(&tile);
    }
    panel.setContentView(Some(&content));

    let block = RcBlock::new(|_timer: NonNull<NSTimer>| {
        if let Some(mtm) = MainThreadMarker::new() {
            follow(mtm);
        }
    });
    // SAFETY: The timer is scheduled on the main run loop, which runs the
    // block on the main thread, and `close` invalidates it.
    let timer = unsafe {
        NSTimer::scheduledTimerWithTimeInterval_repeats_block(POLL_SECONDS, true, &block)
    };
    GUIDE.with_borrow_mut(|guide| {
        *guide = Some(Guide {
            panel,
            timer,
            kind,
            saw_settings: false,
            away: 0,
        });
    });
    follow(mtm);
}

fn close() {
    if let Some(guide) = GUIDE.with_borrow_mut(Option::take) {
        guide.timer.invalidate();
        guide.panel.orderOut(None);
    }
}

/// Keeps the guide at the bottom of the System Settings window, or closes
/// it once it has done its job.
fn follow(mtm: MainThreadMarker) {
    let Some((kind, saw_settings)) =
        GUIDE.with_borrow(|guide| guide.as_ref().map(|guide| (guide.kind, guide.saw_settings)))
    else {
        return;
    };
    if granted_in_settings(kind) {
        close();
        return;
    }
    let Some(settings) = settings_frame(mtm) else {
        if saw_settings {
            close();
        }
        return;
    };
    let strayed = GUIDE.with_borrow_mut(|guide| {
        let Some(guide) = guide.as_mut() else {
            return false;
        };
        guide.saw_settings = true;
        // The guide points at the list just above it. Once the user has gone
        // back to another app it is pointing at nothing, so it gets out of the
        // way instead of hovering over whatever they opened next. Our own app
        // does not count, because `open` hands off to System Settings and this
        // app is still frontmost for the moment before it activates.
        if in_place() {
            guide.away = 0;
        } else {
            guide.away += 1;
        }
        let size = guide.panel.frame().size;
        let visible = NSScreen::screens(mtm)
            .iter()
            .find(|screen| rects_intersect(screen.frame(), settings))
            .map_or(settings, |screen| screen.visibleFrame());
        let x = (settings.origin.x + settings.size.width - size.width - INSET).clamp(
            visible.origin.x + 8.0,
            (visible.origin.x + visible.size.width - size.width - 8.0).max(visible.origin.x),
        );
        let y = (settings.origin.y + INSET).clamp(
            visible.origin.y + 8.0,
            (visible.origin.y + visible.size.height - size.height - 8.0).max(visible.origin.y),
        );
        guide.panel.setFrameOrigin(NSPoint::new(x, y));
        guide.panel.orderFrontRegardless();
        guide.away >= AWAY_TICKS
    });
    if strayed {
        close();
    }
}

/// Whether the user is still where the guide points: System Settings, or this
/// app in the moment before System Settings finishes activating.
fn in_place() -> bool {
    let Some(frontmost) = NSWorkspace::sharedWorkspace().frontmostApplication() else {
        return true;
    };
    if i64::from(frontmost.processIdentifier()) == i64::from(std::process::id()) {
        return true;
    }
    frontmost
        .bundleIdentifier()
        .is_none_or(|id| id.to_string() == SETTINGS_BUNDLE_ID)
}

const fn rects_intersect(a: NSRect, b: NSRect) -> bool {
    a.origin.x < b.origin.x + b.size.width
        && b.origin.x < a.origin.x + a.size.width
        && a.origin.y < b.origin.y + b.size.height
        && b.origin.y < a.origin.y + a.size.height
}

/// The largest on-screen System Settings window, in `AppKit` coordinates
/// (origin at the bottom left of the primary screen). Window bounds and
/// owners need no Screen Recording permission; only titles would.
fn settings_frame(mtm: MainThreadMarker) -> Option<NSRect> {
    let info = CGWindowListCopyWindowInfo(
        CGWindowListOption::OptionOnScreenOnly | CGWindowListOption::ExcludeDesktopElements,
        0,
    )?;
    // SAFETY: CFArray is toll-free bridged to NSArray, and every element of
    // this one is a window's description dictionary.
    let windows: &NSArray<NSDictionary<NSString, AnyObject>> =
        unsafe { &*std::ptr::from_ref(&*info).cast() };
    let number = |window: &NSDictionary<NSString, AnyObject>, key: &str| {
        window
            .objectForKey(&NSString::from_str(key))
            .and_then(|value| value.downcast::<NSNumber>().ok())
    };
    let primary_height = NSScreen::screens(mtm).firstObject()?.frame().size.height;
    windows
        .iter()
        .filter(|window| {
            number(window, "kCGWindowLayer").is_some_and(|layer| layer.intValue() == 0)
        })
        .filter(|window| {
            number(window, "kCGWindowOwnerPID").is_some_and(|pid| {
                NSRunningApplication::runningApplicationWithProcessIdentifier(pid.intValue())
                    .and_then(|app| app.bundleIdentifier())
                    .is_some_and(|id| id.to_string() == SETTINGS_BUNDLE_ID)
            })
        })
        .filter_map(|window| {
            let bounds = window
                .objectForKey(&NSString::from_str("kCGWindowBounds"))?
                .downcast::<NSDictionary>()
                .ok()?;
            // SAFETY: The bounds dictionary maps the keys below to numbers.
            let bounds: &NSDictionary<NSString, AnyObject> =
                unsafe { &*Retained::as_ptr(&bounds).cast() };
            let value = |key| number(bounds, key).map(|number| number.doubleValue());
            let (x, y) = (value("X")?, value("Y")?);
            let (width, height) = (value("Width")?, value("Height")?);
            // Sheets and popovers are smaller than the window they belong to.
            (width >= 300.0 && height >= 200.0).then(|| {
                NSRect::new(
                    NSPoint::new(x, primary_height - y - height),
                    NSSize::new(width, height),
                )
            })
        })
        .max_by(|a, b| (a.size.width * a.size.height).total_cmp(&(b.size.width * b.size.height)))
}

fn rounded_box(
    frame: NSRect,
    radius: f64,
    fill: &NSColor,
    mtm: MainThreadMarker,
) -> Retained<NSBox> {
    let view = NSBox::initWithFrame(NSBox::alloc(mtm), frame);
    style_box(&view, radius, fill);
    view
}

fn style_box(view: &NSBox, radius: f64, fill: &NSColor) {
    view.setBoxType(NSBoxType::Custom);
    view.setTitlePosition(NSTitlePosition::NoTitle);
    view.setContentViewMargins(NSSize::new(0.0, 0.0));
    view.setCornerRadius(radius);
    view.setBorderWidth(1.0);
    view.setBorderColor(&NSColor::separatorColor());
    view.setFillColor(fill);
}

fn label(text: &str, size: f64, medium: bool, mtm: MainThreadMarker) -> Retained<NSTextField> {
    let label = NSTextField::labelWithString(&NSString::from_str(text), mtm);
    let font = if medium {
        // SAFETY: Reading an AppKit constant.
        NSFont::systemFontOfSize_weight(size, unsafe { NSFontWeightMedium })
    } else {
        NSFont::systemFontOfSize(size)
    };
    label.setFont(Some(&font));
    label
}

struct TileIvars {
    bundle: Retained<NSURL>,
    icon: Retained<NSImage>,
    /// The press that may turn into a drag.
    press: RefCell<Option<Retained<NSEvent>>>,
}

define_class!(
    /// The app's icon and name as System Settings lists them; dragging it
    /// drops the app bundle into the list.
    #[unsafe(super(NSBox, NSView, objc2_app_kit::NSResponder, objc2::runtime::NSObject))]
    #[thread_kind = objc2::MainThreadOnly]
    #[ivars = TileIvars]
    struct AppTile;

    impl AppTile {
        // The icon and name inside are part of the tile, not targets of their own.
        #[unsafe(method(hitTest:))]
        fn hit_test(&self, point: NSPoint) -> *mut NSView {
            if rects_intersect(self.frame(), NSRect::new(point, NSSize::new(1.0, 1.0))) {
                std::ptr::from_ref::<NSView>(self).cast_mut()
            } else {
                std::ptr::null_mut()
            }
        }

        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            true
        }

        #[unsafe(method(resetCursorRects))]
        fn reset_cursor_rects(&self) {
            self.addCursorRect_cursor(self.bounds(), &NSCursor::openHandCursor());
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            *self.ivars().press.borrow_mut() = Some(event.retain());
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, _event: &NSEvent) {
            self.ivars().press.borrow_mut().take();
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &NSEvent) {
            self.drag(event);
        }
    }

    unsafe impl NSObjectProtocol for AppTile {}

    unsafe impl NSDraggingSource for AppTile {
        #[unsafe(method(draggingSession:sourceOperationMaskForDraggingContext:))]
        fn source_operation_mask(
            &self,
            _session: &NSDraggingSession,
            _context: NSDraggingContext,
        ) -> NSDragOperation {
            NSDragOperation::Copy
        }
    }
);

impl AppTile {
    fn new(frame: NSRect, bundle: &Path, name: &str, mtm: MainThreadMarker) -> Retained<Self> {
        let path = NSString::from_str(&bundle.to_string_lossy());
        let icon = NSWorkspace::sharedWorkspace().iconForFile(&path);
        icon.setSize(NSSize::new(32.0, 32.0));
        let this = Self::alloc(mtm).set_ivars(TileIvars {
            bundle: NSURL::fileURLWithPath(&path),
            icon: icon.clone(),
            press: RefCell::new(None),
        });
        // SAFETY: NSBox's designated initializer, called once on a fresh allocation.
        let this: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame] };
        style_box(&this, 8.0, &NSColor::controlBackgroundColor());

        let image = NSImageView::imageViewWithImage(&icon, mtm);
        image.setImageScaling(NSImageScaling::ScaleProportionallyUpOrDown);
        image.setFrame(NSRect::new(
            NSPoint::new(10.0, (TILE_HEIGHT - 32.0) / 2.0),
            NSSize::new(32.0, 32.0),
        ));
        let title = label(name, 13.0, true, mtm);
        title.setFrame(NSRect::new(
            NSPoint::new(52.0, (TILE_HEIGHT - 18.0) / 2.0),
            NSSize::new(frame.size.width - 62.0, 18.0),
        ));
        if let Some(inside) = this.contentView() {
            inside.addSubview(&image);
            inside.addSubview(&title);
        }
        this
    }

    fn drag(&self, event: &NSEvent) {
        let Some(press) = self.ivars().press.borrow().clone() else {
            return;
        };
        let (from, to) = (press.locationInWindow(), event.locationInWindow());
        if (to.x - from.x).hypot(to.y - from.y) < DRAG_THRESHOLD {
            return;
        }
        self.ivars().press.borrow_mut().take();
        let item = NSDraggingItem::initWithPasteboardWriter(
            NSDraggingItem::alloc(),
            ProtocolObject::from_ref(&*self.ivars().bundle),
        );
        let start = self.convertPoint_fromView(from, None);
        let icon_frame = NSRect::new(
            NSPoint::new(start.x - 16.0, start.y - 16.0),
            NSSize::new(32.0, 32.0),
        );
        // SAFETY: An NSImage is valid dragging contents.
        unsafe { item.setDraggingFrame_contents(icon_frame, Some(&self.ivars().icon)) };
        let session = self.beginDraggingSessionWithItems_event_source(
            &NSArray::from_retained_slice(&[item]),
            &press,
            ProtocolObject::from_ref(self),
        );
        session.setAnimatesToStartingPositionsOnCancelOrFail(true);
    }
}
