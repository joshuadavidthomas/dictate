// Every `gpui::` path, and the code GPUI's macros emit, resolves through the Kit.
extern crate gpui_kit as gpui;

mod app;
mod assets;
mod components;
mod main_window;
mod overlay;
mod plugins;

pub use app::OVERLAY_WINDOW_HEIGHT;
pub use app::OVERLAY_WINDOW_WIDTH;
pub use app::Overlay;
pub use app::UiIdentity;
pub use app::run;
pub use main_window::MainWindowSettings;
pub use main_window::StyleSample;
pub use main_window::WordEntry;
pub use overlay::OverlayState;
pub use overlay::OverlayView;
pub use plugins::Feed;
pub use plugins::FeedGroup;
pub use plugins::FeedRow;
pub use plugins::PageQuery;
pub use plugins::PluginBridge;
pub use plugins::PluginInfo;
pub use plugins::PluginPage;
pub use plugins::PluginRequest;
