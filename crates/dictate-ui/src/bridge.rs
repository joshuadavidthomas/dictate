//! What the window asks the daemon for, and what comes back.
//!
//! The daemon owns the plugins and the current settings on one thread; the
//! window sends it [`WindowRequest`]s through a [`DaemonBridge`] and awaits
//! the replies.

use std::fmt;
use std::sync::Arc;

use futures::channel::oneshot;
use gpui::SharedString;

use crate::main_window::MainWindowSettings;

/// A page a plugin added to the window.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PluginPage {
    pub id: SharedString,
    pub title: SharedString,
    /// The plugin that added it.
    pub plugin: SharedString,
}

/// What a page is asked to show.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PageQuery {
    pub search: Option<String>,
    pub limit: Option<u32>,
}

/// A page's content: rows under headings.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Feed {
    pub groups: Vec<FeedGroup>,
    /// Shown instead of the groups when there are none.
    pub empty: Option<SharedString>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FeedGroup {
    pub title: SharedString,
    pub rows: Vec<FeedRow>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FeedRow {
    pub text: SharedString,
    pub detail: Option<SharedString>,
    pub meta: Option<SharedString>,
}

/// A loaded plugin and what went wrong loading it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PluginInfo {
    pub id: SharedString,
    /// `bundled`, or the plugin file's path.
    pub source: SharedString,
    pub active: bool,
    pub errors: Vec<SharedString>,
}

/// One change the window asks the daemon to save to `config.toml`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SettingsEdit {
    AddDictionary {
        spoken: String,
        written: String,
    },
    RemoveDictionary {
        spoken: String,
    },
    AddReplacement {
        spoken: String,
        written: String,
    },
    RemoveReplacement {
        spoken: String,
    },
    /// A [`crate::Choice::key`] from [`MainWindowSettings::modes`].
    SetMode(SharedString),
    /// A [`crate::Choice::key`] from [`MainWindowSettings::deliveries`].
    SetDelivery(SharedString),
}

pub enum WindowRequest {
    Pages(oneshot::Sender<Vec<PluginPage>>),
    Render {
        page: SharedString,
        query: PageQuery,
        reply: oneshot::Sender<Result<Feed, String>>,
    },
    Plugins(oneshot::Sender<Vec<PluginInfo>>),
    /// The settings as `config.toml` says now.
    Settings(oneshot::Sender<MainWindowSettings>),
    /// Save `edit` to `config.toml`; the reply is the settings after it, or
    /// why it was not saved.
    Edit {
        edit: SettingsEdit,
        reply: oneshot::Sender<Result<MainWindowSettings, String>>,
    },
}

/// Sends [`WindowRequest`]s to the daemon.
#[derive(Clone)]
pub struct DaemonBridge(Arc<dyn Fn(WindowRequest) + Send + Sync>);

impl DaemonBridge {
    pub fn new(send: impl Fn(WindowRequest) + Send + Sync + 'static) -> Self {
        Self(Arc::new(send))
    }

    pub(crate) fn pages(&self) -> oneshot::Receiver<Vec<PluginPage>> {
        let (reply, receiver) = oneshot::channel();
        (self.0)(WindowRequest::Pages(reply));
        receiver
    }

    pub(crate) fn render(
        &self,
        page: SharedString,
        query: PageQuery,
    ) -> oneshot::Receiver<Result<Feed, String>> {
        let (reply, receiver) = oneshot::channel();
        (self.0)(WindowRequest::Render { page, query, reply });
        receiver
    }

    pub(crate) fn settings(&self) -> oneshot::Receiver<MainWindowSettings> {
        let (reply, receiver) = oneshot::channel();
        (self.0)(WindowRequest::Settings(reply));
        receiver
    }

    pub(crate) fn edit(
        &self,
        edit: SettingsEdit,
    ) -> oneshot::Receiver<Result<MainWindowSettings, String>> {
        let (reply, receiver) = oneshot::channel();
        (self.0)(WindowRequest::Edit { edit, reply });
        receiver
    }

    pub(crate) fn plugins(&self) -> oneshot::Receiver<Vec<PluginInfo>> {
        let (reply, receiver) = oneshot::channel();
        (self.0)(WindowRequest::Plugins(reply));
        receiver
    }
}

impl fmt::Debug for DaemonBridge {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("DaemonBridge")
    }
}
