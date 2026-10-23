use std::collections::BTreeMap;
use std::fs;
use std::io::ErrorKind;
use std::num::NonZeroUsize;
use std::path::Path;
use std::path::PathBuf;

use anyhow::Context;
use anyhow::Result;
use anyhow::anyhow;
use clap::ValueEnum as _;
use dictate_desktop::DeliveryTarget;
use dictate_plugins::Entry;
use dictate_plugins::PluginSetup;
use dictate_speech::ComputeBackend;
use dictate_speech::DEFAULT_MODEL_ID;
use dictate_speech::DictationContext;
use dictate_speech::DictationFormatter;
use dictate_speech::DictationMode;
use dictate_speech::ModelCatalogEntry;
use dictate_speech::RawTranscript;
use dictate_speech::SpokenFormatting;
use dictate_speech::TranscriptionPlan;
use dictate_speech::default_model;
use dictate_speech::model_by_id;
use dictate_ui::Choice;
use dictate_ui::MainWindowSettings;
use dictate_ui::StyleSample;
use dictate_ui::WordEntry;
use directories::ProjectDirs;
use serde::Deserialize;

/// Persistent Dictate settings loaded from `~/.config/dictate/config.toml`.
///
/// Example:
///
/// ```toml
/// model = "bark-3.0-clean-q8"
/// # recognizer = "local"  # the plugin-registered engine dictation uses
/// backend = "auto"
/// # threads = 4  # decode threads; default is one per CPU core but one
/// mode = "technical"
/// delivery = "clipboard"
/// duck_audio = 0.2
/// # input_device = "alsa:hw:CARD=Headset,DEV=0"
///
/// [[dictionary]]
/// spoken = "gee pee you eye"
/// written = "GPUI"
///
/// [[replacements]]
/// spoken = "my email"
/// written = "josh@joshthomas.dev"
/// ```
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    model: String,
    /// Which entry of the plugins' recognizers registry transcribes.
    recognizer: String,
    backend: SettingsBackend,
    threads: Option<NonZeroUsize>,
    mode: SettingsDictationMode,
    spoken_formatting: Option<SettingsSpokenFormatting>,
    dictionary: Vec<DictionaryEntry>,
    replacements: Vec<ReplacementEntry>,
    delivery: SettingsDeliveryTarget,
    duck_audio: f64,
    input_device: Option<String>,
    shortcuts: SettingsShortcuts,
    /// `[plugins.<id>]` tables.
    plugins: BTreeMap<String, PluginSettings>,
}

/// One `[plugins.<id>]` table: `enabled = false` turns the plugin off, and
/// every other key is handed to the plugin as its options.
#[derive(Clone, Debug, Deserialize, PartialEq)]
struct PluginSettings {
    #[serde(default = "enabled_by_default")]
    enabled: bool,
    #[serde(flatten)]
    options: serde_json::Map<String, serde_json::Value>,
}

const fn enabled_by_default() -> bool {
    true
}

impl Settings {
    /// Settings that differ from `before` but only take effect when the
    /// daemon restarts, since threads started with them hold the old
    /// values; named as the window names them.
    #[must_use]
    pub fn needs_restart(&self, before: &Self) -> Vec<&'static str> {
        [
            ("model", self.model != before.model),
            ("backend", self.backend != before.backend),
            ("threads", self.threads != before.threads),
            ("delivery", self.delivery != before.delivery),
            (
                "audio ducking",
                self.duck_audio.to_bits() != before.duck_audio.to_bits(),
            ),
            ("microphone", self.input_device != before.input_device),
            ("push-to-talk shortcut", self.shortcuts != before.shortcuts),
        ]
        .into_iter()
        .filter_map(|(name, changed)| changed.then_some(name))
        .collect()
    }

    pub fn model(&self) -> Result<&'static ModelCatalogEntry> {
        model_by_id(&self.model).ok_or_else(|| {
            anyhow!(
                "unknown model id {:?}; valid model ids: {}; example: model = {:?}",
                self.model,
                valid_model_ids(),
                DEFAULT_MODEL_ID.as_str()
            )
        })
    }

    #[must_use]
    pub fn main_window_settings(&self) -> MainWindowSettings {
        let words = |entries: &[DictionaryEntry]| {
            entries
                .iter()
                .map(|entry| WordEntry {
                    spoken: entry.spoken.clone().into(),
                    written: entry.written.clone().into(),
                })
                .collect()
        };
        let replacement_words = self
            .replacements
            .iter()
            .map(|entry| WordEntry {
                spoken: entry.spoken.clone().into(),
                written: entry.written.clone().into(),
            })
            .collect();
        // The sample names the first dictionary term, so the preview shows the
        // user's own spelling coming through the formatter.
        let subject = self
            .dictionary
            .first()
            .map_or("the new build", |entry| entry.spoken.as_str());
        let spoken = format!(
            "um so i think we should ship {subject} on friday comma does that work for you question mark"
        );
        let written = DictationFormatter
            .format(
                &RawTranscript::new(spoken.as_str()),
                &self.dictation_context(),
            )
            .as_str()
            .to_owned();
        let (microphone, microphones) = self.microphones();
        MainWindowSettings {
            plugins_dir: plugins_dir()
                .map_or_else(
                    |_| "~/.config/dictate/plugins".to_owned(),
                    |path| display_home_relative(&path),
                )
                .into(),
            dictionary: words(&self.dictionary),
            replacements: replacement_words,
            style_sample: StyleSample {
                spoken: spoken.into(),
                written: written.into(),
            },
            model: model_choice(&self.model),
            models: ModelCatalogEntry::all()
                .iter()
                .map(|model| model_choice(model.id().as_str()))
                .collect(),
            mode: self.mode.choice(),
            modes: SettingsDictationMode::value_variants()
                .iter()
                .map(|mode| mode.choice())
                .collect(),
            delivery: self.delivery.choice(),
            deliveries: SettingsDeliveryTarget::value_variants()
                .iter()
                .map(|delivery| delivery.choice())
                .collect(),
            microphone,
            microphones,
            restart_needed: Vec::new(),
            shortcut: self
                .shortcuts
                .push_to_talk
                .clone()
                .unwrap_or_else(|| "Not configured".to_owned())
                .into(),
        }
    }

    /// The configured microphone and the ones to choose from: the system
    /// default (an empty key), then each connected device, then the
    /// configured one if it is not connected.
    fn microphones(&self) -> (Choice, Vec<Choice>) {
        let mut choices = vec![Choice {
            key: "".into(),
            label: "System default".into(),
        }];
        choices.extend(
            dictate_speech::list_input_devices()
                .unwrap_or_default()
                .into_iter()
                .map(|device| Choice {
                    key: device.id.into(),
                    label: device.name.into(),
                }),
        );
        let key = self.input_device.clone().unwrap_or_default();
        let current = choices
            .iter()
            .find(|choice| choice.key == key.as_str())
            .cloned()
            .unwrap_or_else(|| {
                let missing = Choice {
                    label: format!("{key} (not connected)").into(),
                    key: key.into(),
                };
                choices.push(missing.clone());
                missing
            });
        (current, choices)
    }

    pub fn transcription_plan(&self, model_override: Option<&str>) -> Result<TranscriptionPlan> {
        let model = match model_override {
            Some(model_id) => model_by_id(model_id).ok_or_else(|| {
                anyhow!(
                    "unknown model id {:?}; valid model ids: {}; example: --model {}",
                    model_id,
                    valid_model_ids(),
                    DEFAULT_MODEL_ID.as_str()
                )
            })?,
            None => self.model()?,
        };

        Ok(TranscriptionPlan::new(model, self.dictation_context())
            .with_backend(self.backend.into())
            .with_threads(self.threads))
    }

    #[must_use]
    pub fn dictation_context(&self) -> DictationContext {
        self.plugin_setup_in(None).base_context()
    }

    /// What the plugin host starts from: these settings plus the plugins
    /// directory beside the config file.
    pub fn plugin_setup(&self) -> Result<PluginSetup> {
        Ok(PluginSetup {
            data_dir: Some(data_dir()?.join("plugins")),
            ..self.plugin_setup_in(Some(plugins_dir()?))
        })
    }

    fn plugin_setup_in(&self, plugins_dir: Option<PathBuf>) -> PluginSetup {
        PluginSetup {
            mode: self.mode.into(),
            spoken_formatting: self.spoken_formatting.map(Into::into),
            dictionary: self
                .dictionary
                .iter()
                .map(|entry| entry_pair(&entry.spoken, &entry.written))
                .collect(),
            replacements: self
                .replacements
                .iter()
                .map(|entry| entry_pair(&entry.spoken, &entry.written))
                .collect(),
            plugins_dir,
            data_dir: None,
            options: self
                .plugins
                .iter()
                .map(|(id, plugin)| (id.clone(), plugin.options.clone().into()))
                .collect(),
            disabled: self
                .plugins
                .iter()
                .filter(|(_, plugin)| !plugin.enabled)
                .map(|(id, _)| id.clone())
                .collect(),
            recognizer: self.recognizer.clone(),
        }
    }

    #[must_use]
    pub fn delivery(&self) -> DeliveryTarget {
        self.delivery.into()
    }

    #[must_use]
    pub fn input_device(&self) -> Option<&str> {
        self.input_device.as_deref()
    }

    #[must_use]
    pub fn duck_audio(&self) -> f64 {
        self.duck_audio
    }

    fn validate_duck_audio(&self) -> Result<()> {
        if (0.0..=1.0).contains(&self.duck_audio) {
            Ok(())
        } else {
            Err(anyhow!(
                "duck_audio {:?} is outside the valid range 0.0..=1.0; example: duck_audio = 0.2",
                self.duck_audio
            ))
        }
    }

    #[must_use]
    pub fn push_to_talk(&self) -> Option<&str> {
        self.shortcuts.push_to_talk.as_deref()
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            model: default_model().id().as_str().to_owned(),
            recognizer: "local".to_owned(),
            backend: SettingsBackend::Auto,
            threads: None,
            mode: SettingsDictationMode::Message,
            spoken_formatting: None,
            dictionary: Vec::new(),
            replacements: Vec::new(),
            delivery: SettingsDeliveryTarget::Stdout,
            duck_audio: 0.2,
            input_device: None,
            shortcuts: SettingsShortcuts::default(),
            plugins: BTreeMap::new(),
        }
    }
}

pub fn load() -> Result<Settings> {
    load_from_path(&config_path()?)
}

/// `path` with the home directory written as `~`, the way the docs name it.
fn display_home_relative(path: &Path) -> String {
    std::env::var_os("HOME")
        .and_then(|home| path.strip_prefix(home).ok())
        .map_or_else(
            || path.display().to_string(),
            |rest| format!("~/{}", rest.display()),
        )
}

pub(crate) fn config_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("config.toml"))
}

fn entry_pair(spoken: &str, written: &str) -> Entry {
    Entry {
        spoken: spoken.to_owned(),
        written: written.to_owned(),
    }
}

pub fn plugins_dir() -> Result<PathBuf> {
    Ok(config_dir()?.join("plugins"))
}

fn data_dir() -> Result<PathBuf> {
    let dirs = ProjectDirs::from("", "", env!("DICTATE_CONFIG_DIRECTORY")).ok_or_else(|| {
        anyhow!(
            "could not determine {} data directory",
            env!("DICTATE_DISPLAY_NAME")
        )
    })?;
    Ok(dirs.data_dir().to_path_buf())
}

/// `$XDG_CONFIG_HOME/dictate` or `~/.config/dictate` on macOS as on Linux, not
/// `~/Library/Application Support`, so dotfile managers handle it the same way.
#[cfg(target_os = "macos")]
fn config_dir() -> Result<PathBuf> {
    let config_home = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        // The XDG spec says to ignore relative values.
        .filter(|path| path.is_absolute())
        .or_else(|| directories::BaseDirs::new().map(|dirs| dirs.home_dir().join(".config")))
        .ok_or_else(|| {
            anyhow!(
                "could not determine {} config directory",
                env!("DICTATE_DISPLAY_NAME")
            )
        })?;
    Ok(config_home.join(env!("DICTATE_CONFIG_DIRECTORY")))
}

#[cfg(not(target_os = "macos"))]
fn config_dir() -> Result<PathBuf> {
    let dirs = ProjectDirs::from("", "", env!("DICTATE_CONFIG_DIRECTORY")).ok_or_else(|| {
        anyhow!(
            "could not determine {} config directory",
            env!("DICTATE_DISPLAY_NAME")
        )
    })?;
    Ok(dirs.config_dir().to_path_buf())
}

fn load_from_path(path: &Path) -> Result<Settings> {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Settings::default()),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to read settings file {}", path.display()));
        }
    };

    validate(&contents, path)
}

/// Parse `contents` as the settings file at `path` and check what parsing
/// alone does not: the model exists and `duck_audio` is in range.
pub(crate) fn validate(contents: &str, path: &Path) -> Result<Settings> {
    let settings = parse_settings(contents).with_context(|| {
        format!(
            "failed to parse settings file {}; valid examples: {}",
            path.display(),
            valid_setting_examples()
        )
    })?;
    settings
        .model()
        .with_context(|| format!("invalid settings file {}", path.display()))?;
    settings
        .validate_duck_audio()
        .with_context(|| format!("invalid settings file {}", path.display()))?;

    Ok(settings)
}

fn parse_settings(contents: &str) -> Result<Settings> {
    toml::from_str(contents).context("invalid TOML settings")
}

/// A model by id, named as the catalog names it; an id the catalog does not
/// know is shown as it is.
fn model_choice(id: &str) -> Choice {
    Choice {
        key: id.to_owned().into(),
        label: model_by_id(id)
            .map_or(id, |model| model.display_name())
            .to_owned()
            .into(),
    }
}

fn valid_model_ids() -> String {
    ModelCatalogEntry::all()
        .iter()
        .map(|model| model.id().as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

fn valid_setting_examples() -> &'static str {
    "model = \"whisper-base-en-q8\", mode = \"message\", delivery = \"stdout\""
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum SettingsDictationMode {
    Raw,
    Literal,
    #[default]
    Message,
    Email,
    Note,
    Technical,
    Command,
}

impl SettingsDictationMode {
    /// The value `mode` takes in `config.toml`.
    pub(crate) const fn key(self) -> &'static str {
        match self {
            Self::Raw => "raw",
            Self::Literal => "literal",
            Self::Message => "message",
            Self::Email => "email",
            Self::Note => "note",
            Self::Technical => "technical",
            Self::Command => "command",
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::Raw => "Raw",
            Self::Literal => "Literal",
            Self::Message => "Message",
            Self::Email => "Email",
            Self::Note => "Note",
            Self::Technical => "Technical",
            Self::Command => "Command",
        }
    }

    fn choice(self) -> Choice {
        Choice {
            key: self.key().into(),
            label: self.label().into(),
        }
    }
}

impl From<SettingsDictationMode> for DictationMode {
    fn from(mode: SettingsDictationMode) -> Self {
        match mode {
            SettingsDictationMode::Raw => Self::Raw,
            SettingsDictationMode::Literal => Self::Literal,
            SettingsDictationMode::Message => Self::Message,
            SettingsDictationMode::Email => Self::Email,
            SettingsDictationMode::Note => Self::Note,
            SettingsDictationMode::Technical => Self::Technical,
            SettingsDictationMode::Command => Self::Command,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
enum SettingsSpokenFormatting {
    Disabled,
    PunctuationOnly,
    PunctuationAndLines,
}

impl From<SettingsSpokenFormatting> for SpokenFormatting {
    fn from(spoken_formatting: SettingsSpokenFormatting) -> Self {
        match spoken_formatting {
            SettingsSpokenFormatting::Disabled => Self::Disabled,
            SettingsSpokenFormatting::PunctuationOnly => Self::PunctuationOnly,
            SettingsSpokenFormatting::PunctuationAndLines => Self::PunctuationAndLines,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
enum SettingsBackend {
    #[default]
    Auto,
    Cpu,
    Gpu,
}

impl From<SettingsBackend> for ComputeBackend {
    fn from(backend: SettingsBackend) -> Self {
        match backend {
            SettingsBackend::Auto => Self::Auto,
            SettingsBackend::Cpu => Self::Cpu,
            SettingsBackend::Gpu => Self::Gpu,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum SettingsDeliveryTarget {
    #[default]
    Stdout,
    Clipboard,
    Insert,
}

impl SettingsDeliveryTarget {
    /// The value `delivery` takes in `config.toml`.
    pub(crate) const fn key(self) -> &'static str {
        match self {
            Self::Stdout => "stdout",
            Self::Clipboard => "clipboard",
            Self::Insert => "insert",
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::Stdout => "Standard output",
            Self::Clipboard => "Clipboard",
            Self::Insert => "Insert at cursor",
        }
    }

    fn choice(self) -> Choice {
        Choice {
            key: self.key().into(),
            label: self.label().into(),
        }
    }
}

impl From<SettingsDeliveryTarget> for DeliveryTarget {
    fn from(delivery: SettingsDeliveryTarget) -> Self {
        match delivery {
            SettingsDeliveryTarget::Stdout => Self::Stdout,
            SettingsDeliveryTarget::Clipboard => Self::Clipboard,
            SettingsDeliveryTarget::Insert => Self::Insert,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
struct DictionaryEntry {
    spoken: String,
    written: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
struct ReplacementEntry {
    spoken: String,
    written: String,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
struct SettingsShortcuts {
    push_to_talk: Option<String>,
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;

    use dictate_speech::DictationFormatter;
    use dictate_speech::ModelId;
    use dictate_speech::RawTranscript;

    use super::*;

    static SETTINGS_TEST_ID: AtomicUsize = AtomicUsize::new(0);

    fn settings_test_path(name: &str) -> PathBuf {
        let id = SETTINGS_TEST_ID.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "dictate-settings-{name}-{}-{id}.toml",
            std::process::id()
        ))
    }

    fn parse_test_settings(toml: &str) -> Settings {
        parse_settings(toml).expect("settings should parse")
    }

    fn load_test_settings(path: &Path) -> Settings {
        load_from_path(path).expect("settings should load")
    }

    fn settings_error(result: Result<Settings>) -> anyhow::Error {
        result.expect_err("settings operation should fail")
    }

    fn model_id(settings: &Settings) -> ModelId {
        settings.model().expect("model should resolve").id()
    }

    #[test]
    fn full_toml_parses_to_settings() {
        let settings = parse_test_settings(
            r#"
model = "parakeet-tdt-0.6b-v2-q8"
backend = "gpu"
threads = 6
mode = "technical"
delivery = "clipboard"
input_device = "alsa:hw:CARD=Headset,DEV=0"

[[dictionary]]
spoken = "gee pee you eye"
written = "GPUI"

[[replacements]]
spoken = "my email"
written = "josh@joshthomas.dev"

[plugins.agent]
backend = "pi"
timeout_ms = 9000

[plugins.dictation]
enabled = false
"#,
        );

        assert_eq!(
            settings,
            Settings {
                model: "parakeet-tdt-0.6b-v2-q8".to_owned(),
                recognizer: "local".to_owned(),
                backend: SettingsBackend::Gpu,
                threads: NonZeroUsize::new(6),
                mode: SettingsDictationMode::Technical,
                spoken_formatting: None,
                dictionary: vec![DictionaryEntry {
                    spoken: "gee pee you eye".to_owned(),
                    written: "GPUI".to_owned(),
                }],
                replacements: vec![ReplacementEntry {
                    spoken: "my email".to_owned(),
                    written: "josh@joshthomas.dev".to_owned(),
                }],
                delivery: SettingsDeliveryTarget::Clipboard,
                duck_audio: 0.2,
                input_device: Some("alsa:hw:CARD=Headset,DEV=0".to_owned()),
                shortcuts: SettingsShortcuts::default(),
                plugins: BTreeMap::from([
                    (
                        "agent".to_owned(),
                        PluginSettings {
                            enabled: true,
                            options: serde_json::json!({ "backend": "pi", "timeout_ms": 9000 })
                                .as_object()
                                .cloned()
                                .unwrap_or_default(),
                        },
                    ),
                    (
                        "dictation".to_owned(),
                        PluginSettings {
                            enabled: false,
                            options: serde_json::Map::new(),
                        },
                    ),
                ]),
            }
        );
    }

    #[test]
    fn missing_file_loads_defaults() {
        let path = settings_test_path("missing");

        let settings = load_test_settings(&path);

        assert_eq!(model_id(&settings), DEFAULT_MODEL_ID);
        assert_eq!(settings.dictation_context().mode(), DictationMode::Message);
        assert_eq!(settings.delivery(), DeliveryTarget::Stdout);
    }

    #[test]
    fn unknown_key_is_an_error() {
        let error = settings_error(parse_settings("bogus = true"));
        let message = format!("{error:#}");

        assert!(message.contains("bogus"), "{message}");
    }

    #[test]
    fn bad_model_id_error_lists_valid_ids() {
        let path = settings_test_path("bad-model");
        fs::write(&path, "model = \"bogus-model\"\n").expect("bad-model fixture should be written");

        let error = settings_error(load_from_path(&path));
        let message = format!("{error:#}");
        drop(fs::remove_file(path));

        assert!(message.contains("bogus-model"), "{message}");
        assert!(message.contains(DEFAULT_MODEL_ID.as_str()), "{message}");
    }

    #[test]
    fn dictionary_and_replacements_build_dictation_context() {
        let settings = parse_test_settings(
            r#"
mode = "technical"

[[dictionary]]
spoken = "gee pee you eye"
written = "GPUI"

[[replacements]]
spoken = "my handle"
written = "josh-thomas"
"#,
        );
        let formatter = DictationFormatter;
        let rendered_text = formatter.format(
            &RawTranscript::new("I use gee pee you eye and my handle"),
            &settings.dictation_context(),
        );

        assert_eq!(rendered_text.as_str(), "I use GPUI and josh-thomas");
    }

    #[test]
    fn partial_settings_inherit_defaults() {
        let settings = parse_test_settings("mode = \"email\"\n");

        assert_eq!(model_id(&settings), DEFAULT_MODEL_ID);
        assert_eq!(settings.dictation_context().mode(), DictationMode::Email);
        assert_eq!(settings.delivery(), DeliveryTarget::Stdout);
    }

    #[test]
    fn main_window_settings_reflect_loaded_configuration() {
        let settings = parse_test_settings(
            "model = \"whisper-base-en-q8\"\nmode = \"technical\"\ndelivery = \"clipboard\"\ninput_device = \"studio-mic\"\n[[dictionary]]\nspoken = \"gee pee you eye\"\nwritten = \"GPUI\"\n[[replacements]]\nspoken = \"my email\"\nwritten = \"josh@joshthomas.dev\"\n[shortcuts]\npush_to_talk = \"<Super>d\"\n",
        );

        let window = settings.main_window_settings();
        assert_eq!(window.model.key, "whisper-base-en-q8");
        assert_eq!(window.mode.label, "Technical");
        assert_eq!(window.delivery.label, "Clipboard");
        assert_eq!(window.microphone.key, "studio-mic");
        assert_eq!(window.microphone.label, "studio-mic (not connected)");
        assert!(window.microphones.contains(&window.microphone));
        assert_eq!(window.shortcut, "<Super>d");
        assert_eq!(
            window.dictionary,
            [WordEntry {
                spoken: "gee pee you eye".into(),
                written: "GPUI".into(),
            }]
        );
        assert_eq!(window.replacements.len(), 1);
        // The style preview runs the real formatter with the user's dictionary.
        assert_eq!(
            window.style_sample.written,
            "So I think we should ship GPUI on friday, does that work for you?"
        );
    }

    #[test]
    fn insert_delivery_target_parses() {
        let settings = parse_test_settings("delivery = \"insert\"\n");

        assert_eq!(settings.delivery(), DeliveryTarget::Insert);
    }

    #[test]
    fn duck_audio_defaults_to_a_slight_dip() {
        assert!((parse_test_settings("").duck_audio() - 0.2).abs() < f64::EPSILON);
    }

    #[test]
    fn duck_audio_accepts_enabled_and_disabled_values_at_load() {
        for (name, configured, expected) in [
            ("duck-enabled", "duck_audio = 0.35\n", 0.35),
            ("duck-disabled", "duck_audio = 0\n", 0.0),
        ] {
            let path = settings_test_path(name);
            fs::write(&path, configured).expect("duck audio fixture should be written");
            let settings = load_test_settings(&path);
            fs::remove_file(path).expect("duck audio fixture should be removed");
            assert!((settings.duck_audio() - expected).abs() < f64::EPSILON);
        }
    }

    #[test]
    fn duck_audio_rejects_values_outside_the_fraction_range_at_load() {
        for (name, configured) in [
            ("duck-negative", "duck_audio = -0.1\n"),
            ("duck-too-large", "duck_audio = 1.1\n"),
        ] {
            let path = settings_test_path(name);
            fs::write(&path, configured).expect("invalid duck audio fixture should be written");
            let error = settings_error(load_from_path(&path));
            fs::remove_file(path).expect("invalid duck audio fixture should be removed");
            let message = format!("{error:#}");
            assert!(message.contains("0.0..=1.0"), "{message}");
            assert!(message.contains("duck_audio = 0.2"), "{message}");
        }
    }

    #[test]
    fn input_device_is_absent_by_default() {
        assert_eq!(parse_test_settings("").input_device(), None);
    }

    #[test]
    fn input_device_id_is_preserved() {
        let settings = parse_test_settings("input_device = \"alsa:hw:CARD=Headset,DEV=0\"\n");
        assert_eq!(settings.input_device(), Some("alsa:hw:CARD=Headset,DEV=0"));
    }

    #[test]
    fn push_to_talk_is_disabled_when_its_shortcut_is_absent() {
        let settings = parse_test_settings("");

        assert_eq!(settings.push_to_talk(), None);
    }

    #[test]
    fn push_to_talk_shortcut_is_preserved() {
        let settings = parse_test_settings("[shortcuts]\npush_to_talk = \"<Control>space\"\n");

        assert_eq!(settings.push_to_talk(), Some("<Control>space"));
    }

    #[test]
    fn push_to_talk_rejects_non_string_shortcuts() {
        let error = settings_error(parse_settings("[shortcuts]\npush_to_talk = true\n"));

        assert!(format!("{error:#}").contains("string"));
    }

    #[test]
    fn unknown_shortcut_is_an_error() {
        let error = settings_error(parse_settings("[shortcuts]\nbogus = \"<Super>b\"\n"));

        assert!(format!("{error:#}").contains("bogus"));
    }

    #[test]
    fn transcription_plan_applies_model_override_and_formatting_context() {
        let settings = parse_test_settings("mode = \"technical\"\nbackend = \"cpu\"\n");
        let plan = settings
            .transcription_plan(Some("whisper-tiny-en-q8"))
            .expect("plan should compose");

        assert_eq!(plan.model().id().as_str(), "whisper-tiny-en-q8");
        assert_eq!(plan.backend(), ComputeBackend::Cpu);
        assert_eq!(plan.context().mode(), DictationMode::Technical);
    }

    #[test]
    fn invalid_plan_model_override_reports_valid_ids() {
        let error = Settings::default()
            .transcription_plan(Some("not-a-model"))
            .expect_err("invalid model id should fail")
            .to_string();

        assert!(error.contains("not-a-model"));
        assert!(error.contains(DEFAULT_MODEL_ID.as_str()));
    }
}
