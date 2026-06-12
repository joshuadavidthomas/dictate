use std::fs;
use std::io::ErrorKind;
use std::path::Path;
use std::path::PathBuf;

use anyhow::Context;
use anyhow::Result;
use anyhow::anyhow;
use directories::ProjectDirs;
use serde::Deserialize;

use crate::delivery::DeliveryTarget;
use crate::models::DEFAULT_MODEL_ID;
use crate::models::ModelCatalogEntry;
use crate::models::default_model;
use crate::models::model_by_id;
use crate::text::CustomDictionary;
use crate::text::DictationContext;
use crate::text::DictationMode;
use crate::text::ReplacementRule;
use crate::text::SpokenFormatting;

/// Persistent Dictate settings loaded from `~/.config/dictate/config.toml`.
///
/// Example:
///
/// ```toml
/// model = "parakeet-tdt-0.6b-v2-int8"
/// mode = "technical"
/// delivery = "clipboard"
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
    mode: SettingsDictationMode,
    spoken_formatting: Option<SettingsSpokenFormatting>,
    dictionary: Vec<DictionaryEntry>,
    replacements: Vec<ReplacementEntry>,
    delivery: SettingsDeliveryTarget,
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

    pub fn delivery(&self) -> DeliveryTarget {
        self.delivery.into()
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            model: default_model().id().as_str().to_owned(),
            mode: SettingsDictationMode::Message,
            spoken_formatting: None,
            dictionary: Vec::new(),
            replacements: Vec::new(),
            delivery: SettingsDeliveryTarget::Stdout,
        }
    }
}

pub fn load() -> Result<Settings> {
    load_from_path(&config_path()?)
}

fn config_path() -> Result<PathBuf> {
    let dirs = ProjectDirs::from("", "", "dictate")
        .ok_or_else(|| anyhow!("could not determine dictate config directory"))?;
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
}

impl From<SettingsDeliveryTarget> for DeliveryTarget {
    fn from(delivery: SettingsDeliveryTarget) -> Self {
        match delivery {
            SettingsDeliveryTarget::Stdout => Self::Stdout,
            SettingsDeliveryTarget::Clipboard => Self::Clipboard,
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

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;

    use super::*;
    use crate::text::DictationFormatter;
    use crate::transcription::RawTranscript;

    static SETTINGS_TEST_ID: AtomicUsize = AtomicUsize::new(0);

    fn settings_test_path(name: &str) -> PathBuf {
        let id = SETTINGS_TEST_ID.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "dictate-settings-{name}-{}-{id}.toml",
            std::process::id()
        ))
    }

    #[test]
    fn full_toml_parses_to_settings() {
        let settings = parse_settings(
            r#"
model = "parakeet-tdt-0.6b-v2-int8"
mode = "technical"
delivery = "clipboard"

[[dictionary]]
spoken = "gee pee you eye"
written = "GPUI"

[[replacements]]
spoken = "my email"
written = "josh@joshthomas.dev"
"#,
        )
        .unwrap();

        assert_eq!(
            settings,
            Settings {
                model: "parakeet-tdt-0.6b-v2-int8".to_owned(),
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
            }
        );
    }

    #[test]
    fn missing_file_loads_defaults() {
        let path = settings_test_path("missing");

        let settings = load_from_path(&path).unwrap();

        assert_eq!(settings.model().unwrap().id(), DEFAULT_MODEL_ID);
        assert_eq!(settings.dictation_context().mode(), DictationMode::Message);
        assert_eq!(settings.delivery(), DeliveryTarget::Stdout);
    }

    #[test]
    fn unknown_key_is_an_error() {
        let error = parse_settings("bogus = true").unwrap_err();
        let message = format!("{error:#}");

        assert!(message.contains("bogus"), "{message}");
    }

    #[test]
    fn bad_model_id_error_lists_valid_ids() {
        let path = settings_test_path("bad-model");
        fs::write(&path, "model = \"bogus-model\"\n").unwrap();

        let error = load_from_path(&path).unwrap_err();
        let message = format!("{error:#}");
        fs::remove_file(path).ok();

        assert!(message.contains("bogus-model"), "{message}");
        assert!(message.contains(DEFAULT_MODEL_ID.as_str()), "{message}");
    }

    #[test]
    fn dictionary_and_replacements_build_dictation_context() {
        let settings = parse_settings(
            r#"
mode = "technical"

[[dictionary]]
spoken = "gee pee you eye"
written = "GPUI"

[[replacements]]
spoken = "my handle"
written = "josh-thomas"
"#,
        )
        .unwrap();
        let formatter = DictationFormatter;
        let formatted = formatter.format(
            RawTranscript::new("I use gee pee you eye and my handle"),
            &settings.dictation_context(),
        );

        assert_eq!(formatted.as_str(), "I use GPUI and josh-thomas");
    }

    #[test]
    fn partial_settings_inherit_defaults() {
        let settings = parse_settings("mode = \"email\"\n").unwrap();

        assert_eq!(settings.model().unwrap().id(), DEFAULT_MODEL_ID);
        assert_eq!(settings.dictation_context().mode(), DictationMode::Email);
        assert_eq!(settings.delivery(), DeliveryTarget::Stdout);
    }
}
