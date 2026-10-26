//! Changes to `config.toml` made one edit at a time.
//!
//! Every write re-reads the file, applies one [`ConfigEdit`] with
//! `toml_edit` so comments, ordering, and everything else in the file
//! survive, checks the result loads as settings, and replaces the file
//! atomically. Nothing is written from a copy loaded earlier, so an edit
//! made meanwhile by an editor or an agent is kept; two edits of the same
//! key are last-write-wins.

use std::fs;
use std::io::ErrorKind;
use std::path::Path;
use std::path::PathBuf;

use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use toml_edit::ArrayOfTables;
use toml_edit::DocumentMut;
use toml_edit::Item;
use toml_edit::Table;
use toml_edit::value;

use crate::settings::SettingsDeliveryTarget;
use crate::settings::SettingsDictationMode;

/// One change to `config.toml`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConfigEdit {
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
    SetMode(SettingsDictationMode),
    SetDelivery(SettingsDeliveryTarget),
    /// A model id from the catalog.
    SetModel(String),
    /// A microphone id from `dictate devices`; `None` is the system default.
    SetInputDevice(Option<String>),
    /// The push-to-talk shortcut, such as `<Command><Shift>d`.
    SetPushToTalk(String),
    /// macOS: whether Dictate shows a Dock icon.
    SetShowInDock(bool),
}

/// Apply `edit` to the settings file at `path`, creating it if missing.
pub fn apply(path: &Path, edit: &ConfigEdit) -> Result<()> {
    // Write through a symlink, as dotfile managers set up, not over it.
    let path = match fs::canonicalize(path) {
        Ok(target) => target,
        Err(error) if error.kind() == ErrorKind::NotFound => path.to_path_buf(),
        Err(error) => return Err(error).with_context(|| format!("{}", path.display())),
    };
    let contents = match fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == ErrorKind::NotFound => String::new(),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read {}", path.display()));
        }
    };
    let edited =
        edit_text(&contents, edit).with_context(|| format!("could not edit {}", path.display()))?;
    crate::settings::validate(&edited, &path)
        .context("the edit would leave the settings invalid, so nothing was written")?;
    write_atomically(&path, &edited)
}

/// `contents` with `edit` applied.
fn edit_text(contents: &str, edit: &ConfigEdit) -> Result<String> {
    let mut document: DocumentMut = contents.parse().context("invalid TOML")?;
    match edit {
        ConfigEdit::AddDictionary { spoken, written } => {
            add_entry(&mut document, "dictionary", spoken, written)?;
        }
        ConfigEdit::AddReplacement { spoken, written } => {
            add_entry(&mut document, "replacements", spoken, written)?;
        }
        ConfigEdit::RemoveDictionary { spoken } => {
            remove_entry(&mut document, "dictionary", spoken)?;
        }
        ConfigEdit::RemoveReplacement { spoken } => {
            remove_entry(&mut document, "replacements", spoken)?;
        }
        ConfigEdit::SetMode(mode) => set_value(&mut document, "mode", mode.key()),
        ConfigEdit::SetDelivery(delivery) => set_value(&mut document, "delivery", delivery.key()),
        ConfigEdit::SetModel(model) => set_value(&mut document, "model", model),
        ConfigEdit::SetInputDevice(Some(device)) => {
            set_value(&mut document, "input_device", device);
        }
        ConfigEdit::SetInputDevice(None) => {
            document.remove("input_device");
        }
        ConfigEdit::SetPushToTalk(shortcut) => {
            crate::daemon::check_shortcut(shortcut)?;
            let shortcuts = document
                .entry("shortcuts")
                .or_insert_with(|| Item::Table(Table::new()))
                .as_table_mut()
                .context("`shortcuts` is not written as a [shortcuts] table")?;
            set_value(shortcuts, "push_to_talk", shortcut);
        }
        ConfigEdit::SetShowInDock(show) => set_value(&mut document, "show_in_dock", *show),
    }
    Ok(document.to_string())
}

/// Set `key` in `table` to `new`, keeping the comment and spacing around its old value.
fn set_value(table: &mut Table, key: &str, new: impl Into<toml_edit::Value>) {
    match table.get_mut(key).and_then(Item::as_value_mut) {
        Some(old) => {
            let decor = old.decor().clone();
            *old = new.into();
            *old.decor_mut() = decor;
        }
        None => table[key] = value(new),
    }
}

fn entries<'a>(document: &'a mut DocumentMut, list: &str) -> Result<&'a mut ArrayOfTables> {
    document
        .entry(list)
        .or_insert_with(|| Item::ArrayOfTables(ArrayOfTables::new()))
        .as_array_of_tables_mut()
        .with_context(|| format!("`{list}` is not written as [[{list}]] tables"))
}

fn spoken_of(entry: &Table) -> Option<&str> {
    entry.get("spoken").and_then(Item::as_str)
}

/// Add an entry, or change the written form of the one with this `spoken`.
fn add_entry(document: &mut DocumentMut, list: &str, spoken: &str, written: &str) -> Result<()> {
    if spoken.trim().is_empty() || written.trim().is_empty() {
        bail!("spoken and written must not be empty");
    }
    let tables = entries(document, list)?;
    if let Some(existing) = tables
        .iter_mut()
        .find(|entry| spoken_of(entry) == Some(spoken))
    {
        existing["written"] = value(written);
        return Ok(());
    }
    let mut entry = Table::new();
    entry["spoken"] = value(spoken);
    entry["written"] = value(written);
    tables.push(entry);
    Ok(())
}

fn remove_entry(document: &mut DocumentMut, list: &str, spoken: &str) -> Result<()> {
    let tables = entries(document, list)?;
    let before = tables.len();
    tables.retain(|entry| spoken_of(entry) != Some(spoken));
    if tables.len() == before {
        bail!("no [[{list}]] entry says {spoken:?}");
    }
    if tables.is_empty() {
        document.remove(list);
    }
    Ok(())
}

/// Write a sibling temporary file and rename it over `path`, so readers see
/// the old file or the new one, never half of either.
fn write_atomically(path: &Path, contents: &str) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).with_context(|| format!("failed to create {}", dir.display()))?;
    }
    let temporary = PathBuf::from(format!("{}.{}.tmp", path.display(), std::process::id()));
    fs::write(&temporary, contents)
        .with_context(|| format!("failed to write {}", temporary.display()))?;
    fs::rename(&temporary, path).with_context(|| {
        drop(fs::remove_file(&temporary));
        format!("failed to replace {}", path.display())
    })
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;

    use super::*;

    static ID: AtomicUsize = AtomicUsize::new(0);

    struct TempConfig(PathBuf);

    impl TempConfig {
        fn new(contents: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "dictate-config-edit-{}-{}",
                std::process::id(),
                ID.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&dir).expect("dir");
            let path = dir.join("config.toml");
            fs::write(&path, contents).expect("config");
            Self(path)
        }

        fn read(&self) -> String {
            fs::read_to_string(&self.0).expect("config")
        }
    }

    impl Drop for TempConfig {
        fn drop(&mut self) {
            drop(fs::remove_dir_all(self.0.parent().expect("dir")));
        }
    }

    #[test]
    fn an_edit_keeps_comments_and_what_another_writer_added_since() {
        let config = TempConfig::new(
            "# my settings\nmode = \"message\" # chatty\n\n[[dictionary]]\nspoken = \"gee pee you eye\"\nwritten = \"GPUI\"\n",
        );
        // Someone else edits the file after the window loaded it.
        let mut changed = config.read();
        changed
            .push_str("\n[[replacements]]\nspoken = \"my email\"\nwritten = \"me@example.com\"\n");
        fs::write(&config.0, changed).expect("outside edit");

        apply(
            &config.0,
            &ConfigEdit::AddDictionary {
                spoken: "bark".to_owned(),
                written: "Bark".to_owned(),
            },
        )
        .expect("edit applies");
        apply(
            &config.0,
            &ConfigEdit::SetMode(SettingsDictationMode::Email),
        )
        .expect("mode");

        assert_eq!(
            config.read(),
            "# my settings\nmode = \"email\" # chatty\n\n[[dictionary]]\nspoken = \"gee pee you eye\"\nwritten = \"GPUI\"\n\n[[dictionary]]\nspoken = \"bark\"\nwritten = \"Bark\"\n\n[[replacements]]\nspoken = \"my email\"\nwritten = \"me@example.com\"\n"
        );
    }

    #[test]
    fn adding_a_spoken_phrase_again_changes_its_written_form() {
        let config = TempConfig::new("[[dictionary]]\nspoken = \"bark\"\nwritten = \"bark\"\n");
        apply(
            &config.0,
            &ConfigEdit::AddDictionary {
                spoken: "bark".to_owned(),
                written: "Bark".to_owned(),
            },
        )
        .expect("edit applies");
        assert_eq!(
            config.read(),
            "[[dictionary]]\nspoken = \"bark\"\nwritten = \"Bark\"\n"
        );

        apply(
            &config.0,
            &ConfigEdit::RemoveDictionary {
                spoken: "bark".to_owned(),
            },
        )
        .expect("remove");
        assert_eq!(config.read(), "");
        assert!(
            apply(
                &config.0,
                &ConfigEdit::RemoveDictionary {
                    spoken: "bark".to_owned()
                }
            )
            .is_err()
        );
    }

    #[test]
    fn the_microphone_returns_to_the_default_and_the_shortcut_lands_in_its_table() {
        let config = TempConfig::new(
            "input_device = \"studio-mic\" # the good one\n\n[shortcuts]\n# hold to talk\npush_to_talk = \"<Super>d\"\n",
        );
        apply(&config.0, &ConfigEdit::SetInputDevice(None)).expect("default microphone");
        apply(
            &config.0,
            &ConfigEdit::SetPushToTalk("<Control><Option>space".to_owned()),
        )
        .expect("shortcut");
        assert_eq!(
            config.read(),
            "\n[shortcuts]\n# hold to talk\npush_to_talk = \"<Control><Option>space\"\n"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_shortcut_macos_cannot_listen_for_is_not_written() {
        let config = TempConfig::new("");
        let error = apply(&config.0, &ConfigEdit::SetPushToTalk("d".to_owned()))
            .expect_err("a shortcut needs a modifier");
        assert!(format!("{error:#}").contains("modifier"), "{error:#}");
        assert_eq!(config.read(), "");
    }

    #[test]
    fn an_edit_that_leaves_the_settings_invalid_writes_nothing() {
        // Already invalid: an unknown model. Any edit must refuse to save it.
        let original = "model = \"no-such-model\"\n";
        let config = TempConfig::new(original);
        let error = apply(
            &config.0,
            &ConfigEdit::SetDelivery(SettingsDeliveryTarget::Insert),
        )
        .expect_err("invalid settings are not written");
        assert!(format!("{error:#}").contains("no-such-model"), "{error:#}");
        assert_eq!(config.read(), original);
    }
}
