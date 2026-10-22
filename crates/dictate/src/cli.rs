use std::path::Path;
use std::path::PathBuf;
#[cfg(feature = "dev-tools")]
use std::str::FromStr;
#[cfg(feature = "dev-tools")]
use std::time::Duration;

use anyhow::Result;
use anyhow::bail;
use clap::CommandFactory;
use clap::FromArgMatches;
use clap::Parser;
use clap::Subcommand;
use clap::ValueEnum;
use dictate_desktop::DeliveryTarget;
use dictate_speech::DictationCommand;
use dictate_ui::UiIdentity;

#[derive(Debug, Parser)]
#[command(author, version, about)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum BackendArg {
    Auto,
    Cpu,
    Gpu,
}

impl From<BackendArg> for dictate_speech::ComputeBackend {
    fn from(backend: BackendArg) -> Self {
        match backend {
            BackendArg::Auto => Self::Auto,
            BackendArg::Cpu => Self::Cpu,
            BackendArg::Gpu => Self::Gpu,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum DeliveryArg {
    Stdout,
    Clipboard,
    Insert,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum HotkeyPhaseArg {
    Down,
    Up,
}

impl From<HotkeyPhaseArg> for dictate_plugins::HotkeyPhase {
    fn from(phase: HotkeyPhaseArg) -> Self {
        match phase {
            HotkeyPhaseArg::Down => Self::Down,
            HotkeyPhaseArg::Up => Self::Up,
        }
    }
}

#[cfg(target_os = "macos")]
#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum PermissionArg {
    Microphone,
    InputMonitoring,
    Accessibility,
}

#[cfg(target_os = "macos")]
#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum LoginItemAction {
    Status,
    Enable,
    Disable,
    OpenSettings,
}

#[cfg(target_os = "macos")]
impl From<PermissionArg> for dictate_desktop_macos::PermissionKind {
    fn from(permission: PermissionArg) -> Self {
        match permission {
            PermissionArg::Microphone => Self::Microphone,
            PermissionArg::InputMonitoring => Self::InputMonitoring,
            PermissionArg::Accessibility => Self::Accessibility,
        }
    }
}

impl From<DeliveryArg> for DeliveryTarget {
    fn from(delivery: DeliveryArg) -> Self {
        match delivery {
            DeliveryArg::Stdout => Self::Stdout,
            DeliveryArg::Clipboard => Self::Clipboard,
            DeliveryArg::Insert => Self::Insert,
        }
    }
}

#[cfg(feature = "dev-tools")]
#[derive(Clone, Copy, Debug, ValueEnum)]
enum DebugStatsFormat {
    Json,
}

#[cfg(feature = "dev-tools")]
impl From<DebugStatsFormat> for dictate_dev::DebugStatsFormat {
    fn from(format: DebugStatsFormat) -> Self {
        match format {
            DebugStatsFormat::Json => Self::Json,
        }
    }
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run the resident Dictate daemon.
    Daemon {
        /// Override the delivery target in this build channel's config file.
        #[arg(long, value_enum, value_name = "TARGET")]
        delivery: Option<DeliveryArg>,
    },
    /// Send recording commands from compositor keybindings or scripts.
    Record {
        #[arg(value_name = "COMMAND", help = "start, stop, toggle, or cancel")]
        command: DictationCommand,
        /// Override delivery for the recording started or stopped by this command.
        #[arg(long, value_enum, value_name = "TARGET")]
        delivery: Option<DeliveryArg>,
    },
    /// Press or release a named hotkey, for compositor bindings with
    /// press and release events. `dictate` is push-to-talk; plugins see
    /// every name.
    Hotkey {
        #[arg(value_name = "NAME")]
        name: String,
        #[arg(value_enum, value_name = "PHASE")]
        phase: HotkeyPhaseArg,
    },
    /// Insert the last completed dictation at the current cursor.
    Paste,
    /// Hide the current Dictate status overlay.
    Dismiss,
    /// Open the Dictate desktop window.
    Window,
    /// List available microphone input devices.
    Devices,
    /// Load every plugin and report its status; exits non-zero if any failed.
    Plugins {
        /// Emit plugin statuses as JSON.
        #[arg(long)]
        json: bool,
        /// Instead of checking plugins, write lua-language-server type
        /// definitions for the plugin API into the plugins directory.
        #[arg(long, conflicts_with = "json")]
        types: bool,
    },
    /// Inspect or configure the macOS permissions required by Dictate.
    #[cfg(target_os = "macos")]
    Permissions {
        /// Emit the current permission states as JSON.
        #[arg(long)]
        json: bool,
        /// Ask macOS for microphone access when it has not been requested yet.
        #[arg(long, conflicts_with = "open")]
        request_microphone: bool,
        /// Open the matching Privacy & Security pane in System Settings.
        #[arg(long, value_enum, value_name = "PERMISSION")]
        open: Option<PermissionArg>,
    },
    /// Manage launching the bundled macOS application at login.
    #[cfg(target_os = "macos")]
    LoginItem {
        #[arg(value_enum, default_value = "status")]
        action: LoginItemAction,
        /// Emit status as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Transcribe a WAV file through the dictation pipeline without the daemon.
    Transcribe {
        /// Path to a 16 kHz mono WAV file.
        #[arg(value_name = "WAV")]
        wav: PathBuf,
        /// Print the raw recognizer hypothesis instead of formatted dictation.
        ///
        /// With --json, both raw and formatted text are emitted, so this flag has no effect.
        #[arg(long)]
        raw: bool,
        /// Emit raw, formatted, timing, and model metadata as one JSON object.
        #[arg(long)]
        json: bool,
        /// Override the model in this build channel's config file.
        #[arg(long, value_name = "MODEL_ID")]
        model: Option<String>,
        /// Override the compute backend; `gpu` is Metal on macOS and Vulkan elsewhere.
        #[arg(long)]
        backend: Option<BackendArg>,
        /// Override the decode thread count (default: one per CPU core but one).
        #[arg(long, value_name = "N")]
        threads: Option<std::num::NonZeroUsize>,
    },
    /// Open the interactive development harness.
    #[cfg(feature = "dev-tools")]
    Debug {
        /// Print registered screens and scenarios as JSON without opening a window.
        #[arg(long)]
        list: bool,
        /// Open the window with the named screen selected.
        #[arg(long, value_name = "SCREEN")]
        screen: Option<String>,
        /// Open the window with the named scenario selected.
        #[arg(long, value_name = "SCENARIO")]
        scenario: Option<String>,
        /// Stream one JSON object per frame plus a final aggregate line to stdout.
        #[arg(long, value_enum, value_name = "FORMAT")]
        stats: Option<DebugStatsFormat>,
        /// Stop after a duration such as 2s, 500ms, or plain seconds; implies --exit.
        #[arg(long, value_name = "DURATION", value_parser = parse_debug_duration)]
        duration: Option<Duration>,
        /// Stop after N rendered preview frames; implies --exit.
        #[arg(long, value_name = "N")]
        frames: Option<u64>,
        /// Close the debug window and quit when a duration or frame bound is reached.
        #[arg(long)]
        exit: bool,
    },
}

pub fn run(ui_identity: UiIdentity) -> Result<()> {
    let invoked_name = std::env::args_os()
        .next()
        .as_deref()
        .map(Path::new)
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .unwrap_or(env!("CARGO_PKG_NAME"))
        .to_owned();
    let matches = Cli::command().name(invoked_name).get_matches();
    let cli = Cli::from_arg_matches(&matches)?;

    match cli.command.unwrap_or(Command::Daemon { delivery: None }) {
        Command::Daemon { delivery } => crate::daemon::run(
            ui_identity,
            delivery.map(Into::into),
            crate::daemon::InitialWindow::Hidden,
        ),
        Command::Record { command, delivery } => {
            crate::daemon::send(command, delivery.map(Into::into))
        }
        Command::Hotkey { name, phase } => crate::daemon::hotkey(name, phase.into()),
        Command::Paste => crate::daemon::paste_last(),
        Command::Dismiss => crate::daemon::dismiss(),
        Command::Window => {
            let activation_token = launcher_activation_token();
            crate::daemon::open_window(activation_token.clone()).or_else(|_| {
                crate::daemon::run(
                    ui_identity,
                    None,
                    crate::daemon::InitialWindow::Open { activation_token },
                )
            })
        }
        Command::Devices => list_devices(),
        Command::Plugins { types: true, .. } => write_plugin_types(),
        Command::Plugins { json, .. } => check_plugins(json),
        #[cfg(target_os = "macos")]
        Command::Permissions {
            json,
            request_microphone,
            open,
        } => macos_permissions(json, request_microphone, open),
        #[cfg(target_os = "macos")]
        Command::LoginItem { action, json } => macos_login_item(action, json),
        Command::Transcribe {
            wav,
            raw,
            json,
            model,
            backend,
            threads,
        } => transcribe_wav(&wav, raw, json, model.as_deref(), backend, threads),
        #[cfg(feature = "dev-tools")]
        Command::Debug {
            list,
            screen,
            scenario,
            stats,
            duration,
            frames,
            exit,
        } => {
            let debug_config = dictate_dev::DebugConfig::new(
                env!("DICTATE_DEBUG_APP_ID"),
                env!("DICTATE_DISPLAY_NAME"),
                Path::new(env!("CARGO_MANIFEST_DIR")).join("../dictate-speech/tests/fixtures"),
            );
            dictate_dev::run_debug(
                debug_config,
                &dictate_dev::DebugArgs {
                    list,
                    screen,
                    scenario,
                    stats: stats.map(Into::into),
                    duration,
                    frames,
                    exit,
                },
                || {
                    let settings = crate::settings::load()?;
                    settings.transcription_plan(None)
                },
            )
        }
    }
}

fn launcher_activation_token() -> Option<String> {
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

#[cfg(target_os = "macos")]
fn macos_login_item(action: LoginItemAction, json: bool) -> Result<()> {
    let status = match action {
        LoginItemAction::Status => dictate_desktop_macos::login_item_status()?,
        LoginItemAction::Enable => dictate_desktop_macos::set_login_item_enabled(true)?,
        LoginItemAction::Disable => dictate_desktop_macos::set_login_item_enabled(false)?,
        LoginItemAction::OpenSettings => {
            dictate_desktop_macos::open_login_item_settings();
            return Ok(());
        }
    };
    if json {
        println!("{}", serde_json::to_string(&status)?);
    } else {
        println!("launch at login: {status}");
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn macos_permissions(
    json: bool,
    request_microphone: bool,
    open: Option<PermissionArg>,
) -> Result<()> {
    if let Some(permission) = open {
        dictate_desktop_macos::open_permission_settings(permission.into())?;
        return Ok(());
    }
    if request_microphone {
        let state = dictate_desktop_macos::request_microphone_permission()?;
        eprintln!("microphone permission request finished: {state}");
    }

    let status = dictate_desktop_macos::permission_status()?;
    if json {
        println!("{}", serde_json::to_string(&status)?);
    } else {
        println!("microphone: {}", status.microphone);
        println!("input monitoring: {}", status.input_monitoring);
        println!("accessibility: {}", status.accessibility);
    }
    if status.ready() {
        Ok(())
    } else {
        bail!(
            "macOS permissions are incomplete; use `dictate permissions --request-microphone` or `dictate permissions --open <permission>`"
        )
    }
}

#[cfg(feature = "dev-tools")]
fn parse_debug_duration(value: &str) -> Result<Duration, String> {
    if let Some(milliseconds) = value.strip_suffix("ms") {
        let milliseconds = u64::from_str(milliseconds).map_err(|parse_error| {
            format!("invalid millisecond duration {value:?}: {parse_error}")
        })?;
        return Ok(Duration::from_millis(milliseconds));
    }

    if let Some(seconds) = value.strip_suffix('s') {
        let seconds = f64::from_str(seconds)
            .map_err(|parse_error| format!("invalid second duration {value:?}: {parse_error}"))?;
        return duration_from_seconds(seconds, value);
    }

    let seconds = f64::from_str(value).map_err(|parse_error| {
        format!("invalid duration {value:?}; use 2s, 500ms, or plain seconds: {parse_error}")
    })?;
    duration_from_seconds(seconds, value)
}

#[cfg(feature = "dev-tools")]
fn duration_from_seconds(seconds: f64, original: &str) -> Result<Duration, String> {
    if seconds.is_sign_negative() || !seconds.is_finite() {
        return Err(format!(
            "duration must be a finite non-negative value: {original:?}"
        ));
    }

    Ok(Duration::from_secs_f64(seconds))
}

fn list_devices() -> Result<()> {
    let settings = crate::settings::load()?;
    let devices = dictate_speech::list_input_devices()?;
    if devices.is_empty() {
        bail!("no input devices found");
    }
    for device in devices {
        let default_marker = if device.is_default { " (default)" } else { "" };
        let selected_marker = if settings.input_device() == Some(device.id.as_str()) {
            " (selected)"
        } else {
            ""
        };
        println!(
            "{} [{id}]{default_marker}{selected_marker}",
            device.name,
            id = device.id
        );
    }
    Ok(())
}

/// Load the plugins for a command-line run.
fn load_plugins(setup: dictate_plugins::PluginSetup) -> Result<dictate_plugins::Plugins> {
    dictate_plugins::Plugins::load(setup, std::sync::Arc::new(|| {}))
        .map_err(|error| anyhow::anyhow!("plugins could not start: {error}"))
}

/// Write `types/dictate.lua` and, if absent, a `.luarc.json` that points
/// lua-language-server at it, into the plugins directory.
fn write_plugin_types() -> Result<()> {
    let plugins_dir = crate::settings::plugins_dir()?;
    let types_dir = plugins_dir.join("types");
    std::fs::create_dir_all(&types_dir)?;
    let types = types_dir.join("dictate.lua");
    std::fs::write(&types, dictate_plugins::lua_types())?;
    println!("wrote {}", types.display());

    let luarc = plugins_dir.join(".luarc.json");
    if luarc.exists() {
        println!(
            "kept {}; add \"types\" to its workspace.library",
            luarc.display()
        );
    } else {
        std::fs::write(
            &luarc,
            "{\n  \"runtime.version\": \"Lua 5.4\",\n  \"workspace.library\": [\"types\"]\n}\n",
        )?;
        println!("wrote {}", luarc.display());
    }
    Ok(())
}

fn check_plugins(json: bool) -> Result<()> {
    let settings = crate::settings::load()?;
    let host = load_plugins(settings.plugin_setup()?)?;
    let plugins = host.plugins();

    if json {
        println!("{}", serde_json::to_string(&plugins)?);
    } else {
        for plugin in &plugins {
            let state = match (plugin.active, plugin.errors.is_empty()) {
                (true, true) => "ok",
                (false, true) => "disabled in config.toml",
                (true, false) => "running an earlier version",
                (false, false) => "not running",
            };
            println!("{} ({}): {state}", plugin.id, plugin.source);
            for error in &plugin.errors {
                println!("  {error}");
            }
        }
    }

    let failed = plugins
        .iter()
        .filter(|plugin| !plugin.errors.is_empty())
        .count();
    if failed > 0 {
        anyhow::bail!("{failed} plugin(s) reported errors");
    }
    Ok(())
}

fn transcribe_wav(
    wav: &Path,
    raw: bool,
    json: bool,
    model: Option<&str>,
    backend: Option<BackendArg>,
    threads: Option<std::num::NonZeroUsize>,
) -> Result<()> {
    let settings = crate::settings::load()?;
    let mut plan = settings.transcription_plan(model)?;
    if let Some(backend) = backend {
        plan = plan.with_backend(backend.into());
    }
    if threads.is_some() {
        plan = plan.with_threads(threads);
    }
    let mut host = load_plugins(settings.plugin_setup()?)?;
    let result = dictate_speech::TranscriptionSession::new(plan)?
        .transcribe_file_with(wav, |raw| {
            host.process(raw, &dictate_plugins::TranscriptContext::default())
        })?;

    if json {
        println!("{}", serde_json::to_string(&result)?);
    } else if raw {
        println!("{}", result.raw);
    } else {
        println!("{}", result.formatted);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_delivery_override_parses_for_toggle_hotkeys() {
        let cli = Cli::try_parse_from(["dictate", "record", "toggle", "--delivery", "clipboard"])
            .expect("record delivery override should parse");

        assert!(matches!(
            cli.command,
            Some(Command::Record {
                command: DictationCommand::Toggle,
                delivery: Some(DeliveryArg::Clipboard),
            })
        ));
    }

    #[test]
    fn hotkey_subcommand_parses_a_name_and_phase() {
        let cli = Cli::try_parse_from(["dictate", "hotkey", "meeting", "down"])
            .expect("hotkey should parse");
        assert!(matches!(
            cli.command,
            Some(Command::Hotkey { ref name, phase: HotkeyPhaseArg::Down }) if name == "meeting"
        ));
        assert!(Cli::try_parse_from(["dictate", "hotkey", "meeting", "sideways"]).is_err());
    }

    #[test]
    fn devices_subcommand_parses() {
        let cli = Cli::try_parse_from(["dictate", "devices"]).expect("devices should parse");
        assert!(matches!(cli.command, Some(Command::Devices)));
    }

    #[test]
    fn delivery_arguments_parse_and_convert_exhaustively() {
        for (name, argument, target) in [
            ("stdout", DeliveryArg::Stdout, DeliveryTarget::Stdout),
            (
                "clipboard",
                DeliveryArg::Clipboard,
                DeliveryTarget::Clipboard,
            ),
            ("insert", DeliveryArg::Insert, DeliveryTarget::Insert),
        ] {
            assert_eq!(DeliveryArg::from_str(name, false).ok(), Some(argument));
            assert_eq!(DeliveryTarget::from(argument), target);
        }
    }
}
