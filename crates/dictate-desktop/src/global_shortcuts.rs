use ashpd::AppID;
use ashpd::desktop::CreateSessionOptions;
use ashpd::desktop::global_shortcuts::Activated;
use ashpd::desktop::global_shortcuts::BindShortcutsOptions;
use ashpd::desktop::global_shortcuts::Deactivated;
use ashpd::desktop::global_shortcuts::GlobalShortcuts;
use ashpd::desktop::global_shortcuts::NewShortcut;
use ashpd::zbus::names::InterfaceName;
use ashpd::zbus::names::MemberName;
use futures::StreamExt;
use thiserror::Error;

const PUSH_TO_TALK_ID: &str = "push-to-talk";

/// A portal-managed global shortcut for press-and-hold dictation.
#[derive(Clone, Debug)]
pub struct PushToTalkShortcut {
    app_id: AppID,
    preferred_trigger: Option<String>,
}

impl PushToTalkShortcut {
    /// Creates a shortcut associated with the installed desktop application ID.
    pub fn new(app_id: &str, preferred_trigger: Option<&str>) -> Result<Self, PushToTalkError> {
        Ok(Self {
            app_id: app_id.try_into()?,
            preferred_trigger: preferred_trigger.map(ToOwned::to_owned),
        })
    }
}

/// A change in the held state of the push-to-talk shortcut.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PushToTalkEvent {
    Activated,
    Deactivated,
}

/// A failure that ends a portal shortcut session.
#[derive(Debug, Error)]
pub enum PushToTalkError {
    #[error("global shortcuts portal operation failed")]
    Portal(#[from] ashpd::Error),
    #[error("the global shortcuts portal did not bind push-to-talk")]
    NotBound,
    #[error("the global shortcuts portal closed the push-to-talk session")]
    SessionClosed,
    #[error("the global shortcuts portal stopped sending shortcut events")]
    EventsEnded,
}

/// Binds push-to-talk through the XDG `GlobalShortcuts` portal and blocks while
/// dispatching shortcut events.
///
/// # Errors
///
/// Returns an error when registration or binding fails, the portal closes the
/// session, or the ordered D-Bus message stream ends.
pub fn listen_push_to_talk(
    shortcut: &PushToTalkShortcut,
    on_event: impl FnMut(PushToTalkEvent),
) -> Result<(), PushToTalkError> {
    futures::executor::block_on(listen_push_to_talk_async(shortcut, on_event))
}

async fn listen_push_to_talk_async(
    shortcut: &PushToTalkShortcut,
    mut on_event: impl FnMut(PushToTalkEvent),
) -> Result<(), PushToTalkError> {
    ashpd::register_host_app(shortcut.app_id.clone()).await?;

    let portal = GlobalShortcuts::new().await?;
    let messages = ashpd::zbus::MessageStream::from(portal.connection()).fuse();
    futures::pin_mut!(messages);

    let session = portal
        .create_session(CreateSessionOptions::default())
        .await?;

    let new_shortcut = NewShortcut::new(PUSH_TO_TALK_ID, "Push to talk")
        .preferred_trigger(shortcut.preferred_trigger.as_deref());
    let response = portal
        .bind_shortcuts(
            &session,
            &[new_shortcut],
            None,
            BindShortcutsOptions::default(),
        )
        .await?
        .response()?;
    if !response
        .shortcuts()
        .iter()
        .any(|bound| bound.id() == PUSH_TO_TALK_ID)
    {
        return Err(PushToTalkError::NotBound);
    }

    loop {
        let Some(message) = messages.next().await else {
            return Err(PushToTalkError::EventsEnded);
        };
        let message = message.map_err(ashpd::Error::from)?;
        let header = message.header();
        let interface = header.interface().map(InterfaceName::as_str);
        let member = header.member().map(MemberName::as_str);
        match (interface, member) {
            (Some("org.freedesktop.portal.GlobalShortcuts"), Some("Activated")) => {
                let event = message
                    .body()
                    .deserialize::<Activated>()
                    .map_err(ashpd::Error::from)?;
                if event.shortcut_id() == PUSH_TO_TALK_ID {
                    on_event(PushToTalkEvent::Activated);
                }
            }
            (Some("org.freedesktop.portal.GlobalShortcuts"), Some("Deactivated")) => {
                let event = message
                    .body()
                    .deserialize::<Deactivated>()
                    .map_err(ashpd::Error::from)?;
                if event.shortcut_id() == PUSH_TO_TALK_ID {
                    on_event(PushToTalkEvent::Deactivated);
                }
            }
            (Some("org.freedesktop.portal.Session"), Some("Closed")) => {
                return Err(PushToTalkError::SessionClosed);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portal_application_ids_follow_desktop_id_rules() {
        assert!(PushToTalkShortcut::new("dev.joshthomas.dictate", None).is_ok());
        assert!(PushToTalkShortcut::new("dev.joshthomas.dictate_dev", None).is_ok());
        assert!(PushToTalkShortcut::new("dev.joshthomas.dictate-dev.gpui", None).is_err());
    }
}
