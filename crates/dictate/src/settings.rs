use std::fs;
use std::io::ErrorKind;
use std::path::Path;
use std::path::PathBuf;

use anyhow::Context;
use anyhow::Result;
use anyhow::anyhow;
use dictate_desktop::DeliveryTarget;
use dictate_speech::CustomDictionary;
use dictate_speech::DEFAULT_MODEL_ID;
use dictate_speech::DictationContext;
use dictate_speech::DictationMode;
use dictate_speech::ModelCatalogEntry;
use dictate_speech::ReplacementRule;
use dictate_speech::SpokenFormatting;
use dictate_speech::TranscriptionPlan;
use dictate_speech::default_model;
use dictate_speech::model_by_id;
use directories::ProjectDirs;
use serde::Deserialize;

/// Persistent Dictate settings loaded from `~/.config/dictate/config.toml`.
///
/// Example:
///
/// ```toml
/// model = "parakeet-tdt-0.6b-v2-int8"
/// partials_model = "fast-conformer-ctc-en-80ms-int8"
/// mode = "technical"
/// delivery = "clipboard"
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
#[derive(Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    model: String,
    partials_model: Option<String>,
    mode: SettingsDictationMode,
    spoken_formatting: Option<SettingsSpokenFormatting>,
    dictionary: Vec<DictionaryEntry>,
    replacements: Vec<ReplacementEntry>,
    delivery: SettingsDeliveryTarget,
    input_device: Option<String>,
    shortcuts: SettingsShortcuts,
}

impl Settings {
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

    pub fn partials_model(&self) -> Result<&'static ModelCatalogEntry> {
        let partials_model = self
            .partials_model
            .as_deref()
            .unwrap_or(dictate_speech::default_partials_model().id().as_str());
        model_by_id(partials_model).ok_or_else(|| {
            anyhow!(
                "unknown partials_model id {:?}; valid model ids: {}; example: partials_model = {:?}",
                partials_model,
                valid_model_ids(),
                dictate_speech::default_partials_model().id().as_str()
            )
        })
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

        Ok(TranscriptionPlan::new(model, self.dictation_context()))
    }

    #[must_use]
    pub fn dictation_context(&self) -> DictationContext {
        let mut context = DictationContext::new(self.mode.into());

        if let Some(spoken_formatting) = self.spoken_formatting {
            context = context.with_spoken_formatting(spoken_formatting.into());
        }

        if !self.dictionary.is_empty() {
            let dictionary = CustomDictionary::from_entries(
                self.dictionary
                    .iter()
                    .map(|entry| (entry.spoken.as_str(), entry.written.as_str())),
            );
            context = context.with_dictionary(dictionary);
        }

        if !self.replacements.is_empty() {
            let replacements = self
                .replacements
                .iter()
                .map(|entry| ReplacementRule::new(entry.spoken.as_str(), entry.written.as_str()))
                .collect();
            context = context.with_replacement_rules(replacements);
        }

        context
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
    pub fn push_to_talk(&self) -> Option<&str> {
        self.shortcuts.push_to_talk.as_deref()
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            model: default_model().id().as_str().to_owned(),
            partials_model: None,
            mode: SettingsDictationMode::Message,
            spoken_formatting: None,
            dictionary: Vec::new(),
            replacements: Vec::new(),
            delivery: SettingsDeliveryTarget::Stdout,
            input_device: None,
            shortcuts: SettingsShortcuts::default(),
        }
    }
}

pub fn load() -> Result<Settings> {
    load_from_path(&config_path()?)
}

fn config_path() -> Result<PathBuf> {
    let dirs = ProjectDirs::from("", "", env!("DICTATE_CONFIG_DIRECTORY")).ok_or_else(|| {
        anyhow!(
            "could not determine {} config directory",
            env!("DICTATE_DISPLAY_NAME")
        )
    })?;
    Ok(dirs.config_dir().join("config.toml"))
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

    let settings = parse_settings(&contents).with_context(|| {
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
        .partials_model()
        .with_context(|| format!("invalid settings file {}", path.display()))?;

    Ok(settings)
}

fn parse_settings(contents: &str) -> Result<Settings> {
    toml::from_str(contents).context("invalid TOML settings")
}

fn valid_model_ids() -> String {
    ModelCatalogEntry::all()
        .iter()
        .map(|model| model.id().as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

fn valid_setting_examples() -> &'static str {
    "model = \"whisper-base-en\", mode = \"message\", delivery = \"stdout\""
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
enum SettingsDictationMode {
    Raw,
    Literal,
    #[default]
    Message,
    Email,
    Note,
    Technical,
    Command,
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
enum SettingsDeliveryTarget {
    #[default]
    Stdout,
    Clipboard,
    Insert,
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

#[derive(Debug, Default, Deserialize, PartialEq)]
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
model = "parakeet-tdt-0.6b-v2-int8"
mode = "technical"
delivery = "clipboard"
input_device = "alsa:hw:CARD=Headset,DEV=0"

[[dictionary]]
spoken = "gee pee you eye"
written = "GPUI"

[[replacements]]
spoken = "my email"
written = "josh@joshthomas.dev"
"#,
        );

        assert_eq!(
            settings,
            Settings {
                model: "parakeet-tdt-0.6b-v2-int8".to_owned(),
                partials_model: None,
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
                input_device: Some("alsa:hw:CARD=Headset,DEV=0".to_owned()),
                shortcuts: SettingsShortcuts::default(),
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
    fn insert_delivery_target_parses() {
        let settings = parse_test_settings("delivery = \"insert\"\n");

        assert_eq!(settings.delivery(), DeliveryTarget::Insert);
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
    fn partials_model_defaults_to_the_streaming_partials_entry() {
        let settings = parse_test_settings("");

        assert_eq!(
            settings
                .partials_model()
                .expect("partials model should resolve")
                .id()
                .as_str(),
            dictate_speech::default_partials_model().id().as_str()
        );
    }

    #[test]
    fn partials_model_override_parses() {
        let settings = parse_test_settings(
            "partials_model = \"parakeet-unified-0.6b-int8-streaming-560ms\"\n",
        );

        assert_eq!(
            settings
                .partials_model()
                .expect("partials model should resolve")
                .id()
                .as_str(),
            "parakeet-unified-0.6b-int8-streaming-560ms"
        );
    }

    #[test]
    fn bad_partials_model_id_error_lists_valid_ids() {
        let path = settings_test_path("bad-partials-model");
        fs::write(&path, "partials_model = \"bogus\"\n")
            .expect("bad-partials-model fixture should be written");

        let error = settings_error(load_from_path(&path));
        let message = format!("{error:#}");
        drop(fs::remove_file(path));

        assert!(message.contains("bogus"), "{message}");
        assert!(message.contains("partials_model"), "{message}");
    }

    #[test]
    fn transcription_plan_applies_model_override_and_formatting_context() {
        let settings = parse_test_settings("mode = \"technical\"\n");
        let plan = settings
            .transcription_plan(Some("whisper-tiny-en"))
            .expect("plan should compose");

        assert_eq!(plan.model().id().as_str(), "whisper-tiny-en");
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
