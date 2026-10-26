//! A plain `dictate` launch: bring the running instance's window forward,
//! or start the app the way this platform supervises it.

#[cfg(unix)]
use std::process::Command;
#[cfg(unix)]
use std::process::Stdio;
#[cfg(unix)]
use std::thread;
#[cfg(unix)]
use std::time::Duration;
#[cfg(unix)]
use std::time::Instant;

use anyhow::Result;
use dictate_ui::UiIdentity;

use crate::daemon::InitialWindow;

/// How long a launch waits for a supervised start to answer on its socket.
#[cfg(unix)]
const START_TIMEOUT: Duration = Duration::from_secs(10);
#[cfg(unix)]
const POLL_INTERVAL: Duration = Duration::from_millis(20);

pub fn run(identity: UiIdentity) -> Result<()> {
    let activation_token = activation_token();
    if crate::daemon::open_window(activation_token.clone()).is_ok() {
        return Ok(());
    }
    #[cfg(unix)]
    if start_supervised()? {
        return wait_for_window(activation_token.as_deref());
    }
    crate::daemon::run(identity, None, InitialWindow::Open { activation_token })
}

#[cfg(unix)]
fn wait_for_window(activation_token: Option<&str>) -> Result<()> {
    let deadline = Instant::now() + START_TIMEOUT;
    loop {
        match crate::daemon::open_window(activation_token.map(ToOwned::to_owned)) {
            Ok(()) => return Ok(()),
            Err(error) if Instant::now() >= deadline => return Err(error),
            Err(_) => thread::sleep(POLL_INTERVAL),
        }
    }
}

/// Starts this channel's systemd user unit when it is installed, so the
/// daemon stays supervised; false when there is no unit to start.
#[cfg(target_os = "linux")]
fn start_supervised() -> Result<bool> {
    let unit = concat!(env!("DICTATE_CONFIG_DIRECTORY"), ".service");
    let Ok(load_state) = Command::new("systemctl")
        .args(["--user", "show", "--property=LoadState", "--value", unit])
        .stderr(Stdio::null())
        .output()
    else {
        return Ok(false);
    };
    if !load_state.status.success() || load_state.stdout.trim_ascii() != b"loaded" {
        return Ok(false);
    }
    let status = Command::new("systemctl")
        .args(["--user", "start", unit])
        .status()?;
    if !status.success() {
        anyhow::bail!(
            "systemctl --user start {unit} failed ({status}); see journalctl --user -u {unit}"
        );
    }
    Ok(true)
}

/// Opens the app bundle this binary lives in when it runs from a terminal,
/// so macOS attributes permissions to the app instead of the terminal.
/// `LaunchServices` starts apps as children of launchd; those become the
/// instance themselves.
#[cfg(target_os = "macos")]
fn start_supervised() -> Result<bool> {
    if std::os::unix::process::parent_id() == 1 {
        return Ok(false);
    }
    let exe = std::env::current_exe()?.canonicalize()?;
    let Some(bundle) = exe
        .ancestors()
        .find(|path| path.extension().is_some_and(|extension| extension == "app"))
    else {
        return Ok(false);
    };
    let status = Command::new("open")
        .arg("-a")
        .arg(bundle)
        .stdout(Stdio::null())
        .status()?;
    if !status.success() {
        anyhow::bail!("open -a {} failed ({status})", bundle.display());
    }
    Ok(true)
}

pub(crate) fn activation_token() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        std::env::var("XDG_ACTIVATION_TOKEN")
            .ok()
            .filter(|token| !token.is_empty())
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}
