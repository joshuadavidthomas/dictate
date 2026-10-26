//! What Dictate needs before it can dictate, as the window's Get started
//! page lists it: the local model everywhere, and on macOS the permissions
//! the system asks the user for.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::PoisonError;

use dictate_speech::TranscriptionPlan;
use dictate_ui::SetupItem;
use dictate_ui::SetupState;
use dictate_ui::SetupStep;

#[cfg(target_os = "macos")]
use super::platform;

/// The local model, as the recognizer thread reports it.
#[derive(Clone, Debug)]
pub(super) struct ModelStatus {
    name: &'static str,
    load: Arc<Mutex<ModelLoad>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum ModelLoad {
    Waiting,
    Downloading { received: u64, total: Option<u64> },
    Loading,
    Ready,
    Failed(String),
}

impl ModelStatus {
    pub(super) fn new(plan: &TranscriptionPlan) -> Self {
        Self {
            name: plan.model().display_name(),
            load: Arc::new(Mutex::new(ModelLoad::Waiting)),
        }
    }

    pub(super) fn set(&self, load: ModelLoad) {
        *self.load.lock().unwrap_or_else(PoisonError::into_inner) = load;
    }

    fn step(&self) -> SetupStep {
        let name = self.name;
        let load = self
            .load
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let (detail, state) = match load {
            ModelLoad::Waiting => (format!("Checking {name}."), SetupState::Working),
            ModelLoad::Downloading { received, total } => {
                let received = received / 1_000_000;
                let progress = match total {
                    Some(total) => format!("{received} of {} MB", total / 1_000_000),
                    None => format!("{received} MB"),
                };
                (
                    format!("Downloading {name}: {progress}."),
                    SetupState::Working,
                )
            }
            ModelLoad::Loading => (format!("Loading {name}."), SetupState::Working),
            ModelLoad::Ready => (format!("{name} runs on this computer."), SetupState::Ready),
            ModelLoad::Failed(error) => (
                error,
                SetupState::Needed {
                    action: "Restart".into(),
                },
            ),
        };
        SetupStep {
            item: SetupItem::Model,
            title: "Speech model".into(),
            detail: detail.into(),
            state,
        }
    }
}

/// The daemon's view of setup, kept by the plugin thread that answers the
/// window.
#[derive(Debug)]
pub(super) struct Setup {
    /// `None` when dictation does not use the local model.
    model: Option<ModelStatus>,
}

impl Setup {
    pub(super) const fn new(model: Option<ModelStatus>) -> Self {
        Self { model }
    }

    pub(super) fn steps(&self) -> Vec<SetupStep> {
        let model = self.model.iter().map(ModelStatus::step);
        #[cfg(target_os = "macos")]
        let model = model.chain(permission_steps());
        model.collect()
    }

    /// Whether only starting over finishes `item`: a model that failed to
    /// load, or a permission turned on after this process started.
    pub(super) fn needs_restart(&self, item: SetupItem) -> bool {
        match item {
            SetupItem::Model => self.model.as_ref().is_some_and(|model| {
                matches!(
                    *model.load.lock().unwrap_or_else(PoisonError::into_inner),
                    ModelLoad::Failed(_)
                )
            }),
            SetupItem::Microphone | SetupItem::Accessibility | SetupItem::InputMonitoring => {
                permission_needs_restart(item)
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn permission_needs_restart(item: SetupItem) -> bool {
    let Ok(status) = platform::permission_status() else {
        return false;
    };
    let state = match item {
        SetupItem::Model => return false,
        SetupItem::Microphone => status.microphone,
        SetupItem::Accessibility => status.accessibility,
        SetupItem::InputMonitoring => status.input_monitoring,
    };
    state == platform::PermissionState::NeedsRestart
}

#[cfg(not(target_os = "macos"))]
const fn permission_needs_restart(_item: SetupItem) -> bool {
    false
}

/// Whether a launch should open Get started: something it lists is missing
/// and does not arrive by itself in a moment, as loading a downloaded model does.
/// `local_model` is the plan when dictation uses the local model.
pub(super) fn needed_at_launch(local_model: Option<&TranscriptionPlan>) -> bool {
    if local_model.is_some_and(|plan| !plan.model().is_downloaded()) {
        return true;
    }
    #[cfg(target_os = "macos")]
    if !platform::permission_status().is_ok_and(platform::PermissionStatus::ready) {
        return true;
    }
    false
}

/// Starts the user's part of a permission step.
pub(super) fn request(item: SetupItem) -> anyhow::Result<()> {
    #[cfg(target_os = "macos")]
    {
        let kind = match item {
            SetupItem::Microphone => platform::PermissionKind::Microphone,
            SetupItem::Accessibility => platform::PermissionKind::Accessibility,
            SetupItem::InputMonitoring => platform::PermissionKind::InputMonitoring,
            SetupItem::Model => anyhow::bail!("the model has no permission to request"),
        };
        Ok(platform::request_permission(kind)?)
    }
    #[cfg(not(target_os = "macos"))]
    {
        anyhow::bail!("{item:?} needs nothing from you on this platform")
    }
}

#[cfg(target_os = "macos")]
fn permission_steps() -> Vec<SetupStep> {
    use platform::PermissionState;

    let Ok(status) = platform::permission_status() else {
        return Vec::new();
    };
    let app = env!("DICTATE_DISPLAY_NAME");
    let step = |item, title: &str, why: &str, state: PermissionState| {
        let (detail, state) = match state {
            PermissionState::Ready => (why.to_owned(), SetupState::Ready),
            PermissionState::NeedsRequest => (
                why.to_owned(),
                SetupState::Needed {
                    action: "Allow".into(),
                },
            ),
            PermissionState::NeedsSettings => (
                format!("{why} Turn on {app} in System Settings."),
                SetupState::Needed {
                    action: "Open Settings".into(),
                },
            ),
            PermissionState::NeedsRestart => (
                format!("{why} Turned on; restart {app} to use it."),
                SetupState::Needed {
                    action: "Restart".into(),
                },
            ),
        };
        SetupStep {
            item,
            title: title.to_owned().into(),
            detail: detail.into(),
            state,
        }
    };
    vec![
        step(
            SetupItem::Microphone,
            "Microphone",
            "So Dictate can hear you.",
            status.microphone,
        ),
        step(
            SetupItem::Accessibility,
            "Accessibility",
            "So Dictate can type into the app you are using.",
            status.accessibility,
        ),
        step(
            SetupItem::InputMonitoring,
            "Input Monitoring",
            "So the push-to-talk shortcut works in every app.",
            status.input_monitoring,
        ),
    ]
}
