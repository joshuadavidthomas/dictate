//! What the window asks the daemon's plugins for, and what comes back.
//!
//! The daemon owns the plugins on their own thread; the window sends it
//! [`PluginRequest`]s through a [`PluginBridge`] and awaits the replies.

use std::fmt;
use std::sync::Arc;

use futures::channel::oneshot;
use gpui::SharedString;

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

pub enum PluginRequest {
    Pages(oneshot::Sender<Vec<PluginPage>>),
    Render {
        page: SharedString,
        query: PageQuery,
        reply: oneshot::Sender<Result<Feed, String>>,
    },
    Plugins(oneshot::Sender<Vec<PluginInfo>>),
}

/// Sends [`PluginRequest`]s to the daemon's plugins.
#[derive(Clone)]
pub struct PluginBridge(Arc<dyn Fn(PluginRequest) + Send + Sync>);

impl PluginBridge {
    pub fn new(send: impl Fn(PluginRequest) + Send + Sync + 'static) -> Self {
        Self(Arc::new(send))
    }

    pub(crate) fn pages(&self) -> oneshot::Receiver<Vec<PluginPage>> {
        let (reply, receiver) = oneshot::channel();
        (self.0)(PluginRequest::Pages(reply));
        receiver
    }

    pub(crate) fn render(
        &self,
        page: SharedString,
        query: PageQuery,
    ) -> oneshot::Receiver<Result<Feed, String>> {
        let (reply, receiver) = oneshot::channel();
        (self.0)(PluginRequest::Render { page, query, reply });
        receiver
    }

    pub(crate) fn plugins(&self) -> oneshot::Receiver<Vec<PluginInfo>> {
        let (reply, receiver) = oneshot::channel();
        (self.0)(PluginRequest::Plugins(reply));
        receiver
    }
}

impl fmt::Debug for PluginBridge {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PluginBridge")
    }
}
