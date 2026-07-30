use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::thread;
use std::thread::JoinHandle;

use anyhow::Context;
use anyhow::Result;
use gpui::AnyElement;
use gpui::App;
use gpui::IntoElement;
use gpui::ParentElement;
use gpui::Window;
use gpui::div;
use gpui::prelude::*;
use gpui::px;
use gpui::rgb;

use crate::debug::chrome::StatBlockOptions;
use crate::debug::chrome::stat_block;
use crate::debug::chrome::stats_row;
use crate::debug::registry::DebugComponent;
use crate::eval::BenchResult;
use crate::eval::TranscriptionSession;

const CORPUS_IDS: &[&str] = &["spoken-commands", "cmu-arctic", "ljspeech"];

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::debug) struct FixtureCorpus {
    id: &'static str,
    files: Vec<BenchFixture>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::debug) struct BenchFixture {
    label: String,
    path: PathBuf,
}

#[derive(Clone)]
enum BenchEntry {
    Transcribing,
    Complete(Arc<BenchResult>),
    Failed(Arc<String>),
}

#[derive(Default)]
struct BenchWorker {
    session: Option<TranscriptionSession>,
}

fn lock_or_recover<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[derive(Default)]
struct BenchState {
    selected_files: Arc<Mutex<HashMap<&'static str, PathBuf>>>,
    entries: Arc<Mutex<HashMap<PathBuf, BenchEntry>>>,
    worker: Arc<Mutex<BenchWorker>>,
    handles: Mutex<Vec<JoinHandle<()>>>,
    shutdown: Arc<AtomicBool>,
}

pub(in crate::debug) struct BenchPreview {
    corpora: Result<Vec<FixtureCorpus>, Arc<String>>,
    state: BenchState,
}

impl BenchPreview {
    pub(in crate::debug) fn new() -> Self {
        let corpora =
            discover_fixture_corpora(&fixture_root()).map_err(|error| Arc::new(error.to_string()));

        Self {
            corpora,
            state: BenchState::default(),
        }
    }

    fn render_corpus(&self, corpus_id: &str) -> AnyElement {
        let Some(corpus) = self.corpus(corpus_id) else {
            return Self::error_view(format!("unknown bench corpus {corpus_id:?}"));
        };

        let Some(selected_file) = self.selected_file(corpus) else {
            return Self::error_view(format!(
                "no WAV fixtures found under tests/fixtures/{}",
                corpus.id
            ));
        };

        self.ensure_transcription_started(&selected_file);

        let file_buttons = corpus
            .files
            .iter()
            .map(|fixture| self.file_button(corpus.id, fixture, &selected_file))
            .collect::<Vec<_>>();
        let result = self.result_for(&selected_file);

        div()
            .id("debug-bench-preview")
            .size_full()
            .rounded_md()
            .border_1()
            .border_color(rgb(0x001f_2937))
            .bg(rgb(0x000b_1020))
            .p(px(16.0))
            .flex()
            .min_w_0()
            .gap_4()
            .child(
                div()
                    .w(px(220.0))
                    .h_full()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(format!("{} fixtures", corpus.id)),
                    )
                    .children(file_buttons),
            )
            .child(Self::result_view(result))
            .into_any_element()
    }

    fn corpus(&self, corpus_id: &str) -> Option<&FixtureCorpus> {
        self.corpora
            .as_ref()
            .ok()?
            .iter()
            .find(|corpus| corpus.id == corpus_id)
    }

    fn selected_file(&self, corpus: &FixtureCorpus) -> Option<PathBuf> {
        let selected = lock_or_recover(&self.state.selected_files)
            .get(corpus.id)
            .cloned();

        selected
            .filter(|selected| corpus.files.iter().any(|fixture| fixture.path == *selected))
            .or_else(|| corpus.files.first().map(|fixture| fixture.path.clone()))
    }

    fn file_button(
        &self,
        corpus_id: &'static str,
        fixture: &BenchFixture,
        selected_file: &Path,
    ) -> AnyElement {
        let selected = fixture.path == selected_file;
        let path = fixture.path.clone();
        let selected_files = Arc::clone(&self.state.selected_files);

        div()
            .id(format!("debug-bench-file-{}", fixture.label))
            .rounded_sm()
            .px(px(8.0))
            .py(px(5.0))
            .cursor_pointer()
            .bg(if selected {
                rgb(0x001d_4ed8)
            } else {
                rgb(0x001f_2937)
            })
            .text_sm()
            .child(fixture.label.clone())
            .on_click(move |_, _, cx| {
                lock_or_recover(&selected_files).insert(corpus_id, path.clone());
                cx.refresh_windows();
            })
            .into_any_element()
    }

    fn ensure_transcription_started(&self, path: &Path) {
        let mut entries = lock_or_recover(&self.state.entries);
        if entries.contains_key(path) {
            return;
        }
        entries.insert(path.to_path_buf(), BenchEntry::Transcribing);
        drop(entries);

        let path = path.to_path_buf();
        let entries = Arc::clone(&self.state.entries);
        let worker = Arc::clone(&self.state.worker);
        let shutdown = Arc::clone(&self.state.shutdown);
        let handle = thread::spawn(move || {
            let path_for_worker = path.clone();
            let outcome = (|| -> Result<BenchResult> {
                let mut worker = lock_or_recover(&worker);
                if shutdown.load(Ordering::Relaxed) {
                    anyhow::bail!("bench shutting down");
                }
                if worker.session.is_none() {
                    worker.session = Some(TranscriptionSession::new(None)?);
                }

                let Some(session) = worker.session.as_ref() else {
                    anyhow::bail!("bench transcription session was not initialized");
                };
                session.transcribe_file(&path_for_worker)
            })();

            if shutdown.load(Ordering::Relaxed) {
                return;
            }

            let entry = match outcome {
                Ok(result) => BenchEntry::Complete(Arc::new(result)),
                Err(error) => BenchEntry::Failed(Arc::new(format!("{error:#}"))),
            };
            lock_or_recover(&entries).insert(path, entry);
        });

        lock_or_recover(&self.state.handles).push(handle);
    }

    fn result_for(&self, path: &Path) -> BenchEntry {
        lock_or_recover(&self.state.entries)
            .get(path)
            .cloned()
            .unwrap_or(BenchEntry::Transcribing)
    }

    fn result_view(result: BenchEntry) -> AnyElement {
        match result {
            BenchEntry::Transcribing => div()
                .flex_1()
                .min_w_0()
                .h_full()
                .rounded_md()
                .border_1()
                .border_color(rgb(0x0037_4151))
                .bg(rgb(0x0011_1827))
                .p(px(16.0))
                .text_color(rgb(0x00d1_d5db))
                .child("transcribing…")
                .into_any_element(),
            BenchEntry::Failed(error) => div()
                .flex_1()
                .min_w_0()
                .h_full()
                .rounded_md()
                .border_1()
                .border_color(rgb(0x007f_1d1d))
                .bg(rgb(0x0011_1827))
                .p(px(16.0))
                .text_color(rgb(0x00fc_a5a5))
                .child(format!("transcription failed:\n{}", error.as_ref()))
                .into_any_element(),
            BenchEntry::Complete(result) => div()
                .flex_1()
                .min_w_0()
                .w_full()
                .h_full()
                .flex()
                .flex_col()
                .gap_3()
                .child(
                    stats_row()
                        .w_full()
                        .child(stat_block(
                            "model",
                            result.model_id,
                            StatBlockOptions::flexible().truncate(),
                        ))
                        .child(stat_block(
                            "total",
                            format!("{:.1}", result.timing.total_ms),
                            StatBlockOptions::fixed(128.0)
                                .unit("ms")
                                .tabular()
                                .border_color(0x0037_4151),
                        ))
                        .child(stat_block(
                            "load",
                            format!("{:.1}", result.timing.load_ms),
                            StatBlockOptions::fixed(120.0).unit("ms").tabular(),
                        ))
                        .child(stat_block(
                            "transcribe",
                            format!("{:.1}", result.timing.transcribe_ms),
                            StatBlockOptions::fixed(132.0).unit("ms").tabular(),
                        ))
                        .child(stat_block(
                            "format",
                            format!("{:.1}", result.timing.format_ms),
                            StatBlockOptions::fixed(120.0).unit("ms").tabular(),
                        )),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .gap_3()
                        .child(transcript_pane("raw", &result.raw))
                        .child(transcript_pane("formatted", &result.formatted)),
                )
                .into_any_element(),
        }
    }

    fn error_view(message: String) -> AnyElement {
        div()
            .id("debug-bench-error")
            .size_full()
            .rounded_md()
            .border_1()
            .border_color(rgb(0x007f_1d1d))
            .bg(rgb(0x000b_1020))
            .p(px(16.0))
            .text_color(rgb(0x00fc_a5a5))
            .child(message)
            .into_any_element()
    }
}

impl Drop for BenchPreview {
    fn drop(&mut self) {
        self.state.shutdown.store(true, Ordering::Relaxed);
        let handles = lock_or_recover(&self.state.handles)
            .drain(..)
            .collect::<Vec<_>>();

        for handle in handles {
            drop(handle.join());
        }
    }
}

impl DebugComponent for BenchPreview {
    fn name(&self) -> &'static str {
        "bench"
    }

    fn description(&self) -> &'static str {
        "Transcribe fixture WAVs through the shared CLI composition and compare raw vs formatted output."
    }

    fn scenarios(&self) -> &'static [&'static str] {
        CORPUS_IDS
    }

    fn preview(&self, scenario: &str, _window: &mut Window, _cx: &mut App) -> AnyElement {
        match &self.corpora {
            Ok(_) => self.render_corpus(scenario),
            Err(error) => Self::error_view(format!("failed to discover bench fixtures: {error}")),
        }
    }
}

fn transcript_pane(title: &'static str, text: &str) -> AnyElement {
    div()
        .flex_1()
        .min_w_0()
        .h_full()
        .rounded_md()
        .border_1()
        .border_color(rgb(0x0037_4151))
        .bg(rgb(0x0011_1827))
        .p(px(12.0))
        .flex()
        .flex_col()
        .gap_2()
        .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child(title))
        .child(
            div()
                .min_w_0()
                .text_sm()
                .text_color(rgb(0x00f9_fafb))
                .whitespace_normal()
                .child(text.to_string()),
        )
        .into_any_element()
}

fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

pub(in crate::debug) fn discover_fixture_corpora(root: &Path) -> Result<Vec<FixtureCorpus>> {
    CORPUS_IDS
        .iter()
        .map(|&id| discover_fixture_corpus(root, id))
        .collect()
}

fn discover_fixture_corpus(root: &Path, id: &'static str) -> Result<FixtureCorpus> {
    let directory = root.join(id);
    let mut files = Vec::new();

    for entry in fs::read_dir(&directory)
        .with_context(|| format!("failed to read fixture directory {}", directory.display()))?
    {
        let entry = entry.with_context(|| {
            format!(
                "failed to read fixture directory entry under {}",
                directory.display()
            )
        })?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .with_context(|| format!("failed to read file type for {}", path.display()))?;

        if file_type.is_file() && is_wav_path(&path) {
            files.push(BenchFixture {
                label: entry.file_name().to_string_lossy().to_string(),
                path,
            });
        }
    }

    files.sort_by(|left, right| left.label.cmp(&right.label));

    Ok(FixtureCorpus { id, files })
}

fn is_wav_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("wav"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bench_scenario_ids_are_fixture_corpora() {
        assert_eq!(CORPUS_IDS, &["spoken-commands", "cmu-arctic", "ljspeech"]);
    }

    #[test]
    fn discovers_fixture_corpora_and_wav_files() {
        let corpora = discover_fixture_corpora(&fixture_root())
            .expect("fixture corpora should be discoverable");

        assert_eq!(corpora.len(), CORPUS_IDS.len());
        for (corpus, expected_id) in corpora.iter().zip(CORPUS_IDS) {
            assert_eq!(corpus.id, *expected_id);
            assert!(
                corpus.files.iter().any(|fixture| Path::new(&fixture.label)
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("wav"))),
                "{} should contain wav fixtures",
                corpus.id
            );
            assert!(
                corpus
                    .files
                    .windows(2)
                    .all(|pair| pair[0].label <= pair[1].label)
            );
        }
    }
}
