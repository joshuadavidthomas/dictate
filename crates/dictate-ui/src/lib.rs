// Every `gpui::` path, and the code GPUI's macros emit, resolves through the Kit.
extern crate gpui_kit as gpui;

mod app;
mod assets;
mod bridge;
mod components;
mod main_window;
mod overlay;

pub use app::OVERLAY_WINDOW_HEIGHT;
pub use app::OVERLAY_WINDOW_WIDTH;
pub use app::Overlay;
pub use app::StatusIndicator;
pub use app::UiIdentity;
pub use app::run;
pub use bridge::DaemonBridge;
pub use bridge::Feed;
pub use bridge::FeedGroup;
pub use bridge::FeedRow;
pub use bridge::PageQuery;
pub use bridge::PluginInfo;
pub use bridge::PluginPage;
pub use bridge::SettingsEdit;
pub use bridge::WindowRequest;
pub use main_window::Choice;
pub use main_window::CommandLine;
pub use main_window::MainWindowSettings;
pub use main_window::SetupItem;
pub use main_window::SetupState;
pub use main_window::SetupStep;
pub use main_window::StyleSample;
pub use main_window::WordEntry;
pub use overlay::OverlayState;
pub use overlay::OverlayView;
