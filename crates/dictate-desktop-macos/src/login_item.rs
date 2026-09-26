use objc2_service_management::SMAppService;
use objc2_service_management::SMAppServiceStatus;
use serde::Serialize;
use thiserror::Error;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LoginItemStatus {
    Disabled,
    Enabled,
    RequiresApproval,
}

impl std::fmt::Display for LoginItemStatus {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Disabled => formatter.write_str("disabled"),
            Self::Enabled => formatter.write_str("enabled"),
            Self::RequiresApproval => formatter.write_str("requires approval"),
        }
    }
}

#[derive(Debug, Error)]
pub enum LoginItemError {
    #[error("macOS returned unknown login item status {status}")]
    UnknownStatus { status: isize },
    #[error("could not {operation} the macOS login item: {message}")]
    Service {
        operation: &'static str,
        message: String,
    },
}

pub fn login_item_status() -> Result<LoginItemStatus, LoginItemError> {
    // SAFETY: `mainAppService` returns the process-lifetime service for this application.
    let service = unsafe { SMAppService::mainAppService() };
    // SAFETY: Querying the process-lifetime service's status does not retain caller-owned pointers.
    let status = unsafe { service.status() };
    match status {
        SMAppServiceStatus::NotRegistered | SMAppServiceStatus::NotFound => {
            Ok(LoginItemStatus::Disabled)
        }
        SMAppServiceStatus::Enabled => Ok(LoginItemStatus::Enabled),
        SMAppServiceStatus::RequiresApproval => Ok(LoginItemStatus::RequiresApproval),
        _ => Err(LoginItemError::UnknownStatus { status: status.0 }),
    }
}

pub fn set_login_item_enabled(enabled: bool) -> Result<LoginItemStatus, LoginItemError> {
    // SAFETY: `mainAppService` returns the process-lifetime service for this application.
    let service = unsafe { SMAppService::mainAppService() };
    let current = login_item_status()?;
    if enabled && current == LoginItemStatus::Disabled {
        // SAFETY: The synchronous API owns the NSError returned on failure.
        unsafe { service.registerAndReturnError() }.map_err(|error| LoginItemError::Service {
            operation: "register",
            message: error.to_string(),
        })?;
    } else if !enabled && current != LoginItemStatus::Disabled {
        // SAFETY: The synchronous API owns the NSError returned on failure.
        unsafe { service.unregisterAndReturnError() }.map_err(|error| LoginItemError::Service {
            operation: "unregister",
            message: error.to_string(),
        })?;
    }
    login_item_status()
}

pub fn open_login_item_settings() {
    // SAFETY: This asks ServiceManagement to open its system-owned Login Items settings pane.
    unsafe { SMAppService::openSystemSettingsLoginItems() };
}
