use crate::conf::{OutputMode, SettingsState};
use crate::db::Database;
use crate::models::ModelId;
use crate::recording::{DisplayServer, RecordedAudio};
use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use std::path::Path;
use std::process::Command;
use tauri::{AppHandle, Manager};
use tauri_plugin_clipboard_manager::ClipboardExt;
use tokio::sync::Mutex;
use transcribe_rs::{
    TranscriptionEngine as TranscribeTrait,
    engines::parakeet::{ParakeetEngine, ParakeetModelParams},
    engines::whisper::WhisperEngine,
};

pub struct TranscriptionContext<'a> {
    pub engine_state: &'a Mutex<Option<(ModelId, LoadedEngine)>>,
    pub settings: &'a SettingsState,
    pub database: Option<&'a Database>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transcription {
    pub id: Option<i64>,
    pub text: String,
    pub created_at: Option<i64>,
    pub duration_ms: Option<i64>,
    pub model_id: Option<ModelId>,
    pub audio_path: Option<String>,
    pub output_mode: Option<String>,
    pub audio_size_bytes: Option<i64>,
}

impl Transcription {
    /// Create a transcription from recorded audio
    ///
    /// This method:
    /// - Selects and loads appropriate ML model
    /// - Runs inference on the audio file
    /// - Constructs the transcription entity
    /// - Persists to database (if present and non-empty)
    pub async fn from_audio(
        audio: RecordedAudio,
        context: TranscriptionContext<'_>,
    ) -> Result<Self> {
        let mut engine_guard = context.engine_state.lock().await;
        let (model_id, engine) = Self::ensure_engine_loaded(&mut engine_guard, context.settings).await?;

        let text = engine.transcribe(&audio.path)?;

        let output_mode = context.settings.get().await.output_mode;

        let mut transcription = Self::from_recording(
            text,
            &audio.path,
            &audio.buffer,
            audio.sample_rate,
            output_mode,
            Some(*model_id),
        );

        if !transcription.text.trim().is_empty()
            && let Some(db) = context.database
        {
            transcription = save(db.pool(), transcription).await?;
            eprintln!(
                "[Transcription] Saved with ID: {}",
                transcription.id.unwrap()
            );
        }

        Ok(transcription)
    }

    async fn ensure_engine_loaded<'a>(
        cache: &'a mut Option<(ModelId, LoadedEngine)>,
        settings: &SettingsState,
    ) -> Result<(&'a ModelId, &'a mut LoadedEngine)> {
        let settings_data = settings.get().await;
        let descriptor = crate::models::preferred_or_default(settings_data.preferred_model);
        let model_id = descriptor.id;
        let path = crate::models::local_path(model_id)?;

        // Verify model is downloaded
        if !crate::models::is_downloaded(model_id)? {
            return Err(anyhow!(
                "Model '{:?}' not downloaded. Please download it first.",
                model_id
            ));
        }

        // Load engine if cache is empty or ID changed
        let needs_load = match cache {
            Some((cached_id, _)) if *cached_id == model_id => false,
            _ => true,
        };

        if needs_load {
            println!("Loading transcription model from: {}", path.display());
            
            let engine = if descriptor.is_directory {
                // Parakeet model (directory-based)
                let mut parakeet_engine = ParakeetEngine::new();
                parakeet_engine.load_model_with_params(&path, ParakeetModelParams::int8())
                    .map_err(|e| anyhow!("Failed to load Parakeet model: {}", e))?;
                LoadedEngine::Parakeet { engine: parakeet_engine }
            } else {
                // Whisper model (file-based)
                let mut whisper_engine = WhisperEngine::new();
                whisper_engine.load_model(&path)
                    .map_err(|e| {
                        let metadata = std::fs::metadata(&path).ok();
                        let file_size = metadata.map(|m| m.len()).unwrap_or(0);
                        
                        if file_size < 1_000_000 {
                            anyhow!(
                                "Failed to load Whisper model (file may be corrupt, size: {} bytes): {}",
                                file_size,
                                e
                            )
                        } else {
                            anyhow!("Failed to load Whisper model: {}", e)
                        }
                    })?;
                LoadedEngine::Whisper { engine: whisper_engine }
            };
            
            println!("Model loaded successfully");
            *cache = Some((model_id, engine));
        }

        // Return references to the cached model
        let (id, engine) = cache.as_mut().unwrap();
        Ok((id, engine))
    }

    fn from_recording(
        text: String,
        audio_path: &Path,
        audio_buffer: &[i16],
        sample_rate: u32,
        output_mode: OutputMode,
        model_id: Option<ModelId>,
    ) -> Self {
        let duration_ms = Some((audio_buffer.len() as i64 * 1000) / sample_rate as i64);
        let audio_size_bytes = std::fs::metadata(audio_path).ok().map(|m| m.len() as i64);
        let output_mode_str = match output_mode {
            OutputMode::Print => "print",
            OutputMode::Copy => "copy",
            OutputMode::Insert => "insert",
        };

        Self {
            id: None,
            created_at: None,
            text,
            duration_ms,
            model_id,
            audio_path: Some(audio_path.to_string_lossy().to_string()),
            output_mode: Some(output_mode_str.to_string()),
            audio_size_bytes,
        }
    }
}

pub enum LoadedEngine {
    Whisper { engine: WhisperEngine },
    Parakeet { engine: ParakeetEngine },
}

impl LoadedEngine {
    pub fn transcribe(&mut self, audio_path: &Path) -> Result<String> {
        println!("Transcribing audio file: {}", audio_path.display());

        match self {
            LoadedEngine::Whisper { engine } => {
                match engine.transcribe_file(audio_path, None) {
                    Ok(result) => {
                        let text = result.text;
                        println!("Transcription completed: {}", text);
                        Ok(text)
                    }
                    Err(e) => {
                        println!("Transcription failed: {}", e);
                        Err(anyhow!("Whisper transcription failed: {}", e))
                    }
                }
            }
            LoadedEngine::Parakeet { engine } => {
                match engine.transcribe_file(audio_path, None) {
                    Ok(result) => {
                        let text = result.text;
                        println!("Transcription completed: {}", text);
                        Ok(text)
                    }
                    Err(e) => {
                        println!("Transcription failed: {}", e);
                        Err(anyhow!("Parakeet transcription failed: {}", e))
                    }
                }
            }
        }
    }
}

pub async fn save(pool: &SqlitePool, mut transcription: Transcription) -> Result<Transcription> {
    let created_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs() as i64;

    let model_id_json = transcription
        .model_id
        .and_then(|id| serde_json::to_string(&id).ok());

    let result = sqlx::query(
        r#"
        INSERT INTO transcriptions (text, created_at, duration_ms, model_id, audio_path, output_mode, audio_size_bytes)
        VALUES (?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(&transcription.text)
    .bind(created_at)
    .bind(transcription.duration_ms)
    .bind(model_id_json)
    .bind(&transcription.audio_path)
    .bind(&transcription.output_mode)
    .bind(transcription.audio_size_bytes)
    .execute(pool)
    .await?;

    transcription.id = Some(result.last_insert_rowid());
    transcription.created_at = Some(created_at);

    Ok(transcription)
}

pub async fn get(pool: &SqlitePool, id: i64) -> Result<Option<Transcription>> {
    let result = sqlx::query(
        r#"
        SELECT id, text, created_at, duration_ms, model_id, audio_path, output_mode, audio_size_bytes
        FROM transcriptions
        WHERE id = ?
        "#,
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;

    let transcription = result.map(|row| {
        let model_id_json: Option<String> = row.get(4);
        let model_id = model_id_json.and_then(|json| serde_json::from_str(&json).ok());

        Transcription {
            id: Some(row.get(0)),
            text: row.get(1),
            created_at: Some(row.get(2)),
            duration_ms: row.get(3),
            model_id,
            audio_path: row.get(5),
            output_mode: row.get(6),
            audio_size_bytes: row.get(7),
        }
    });

    Ok(transcription)
}

pub async fn list(pool: &SqlitePool, limit: i64, offset: i64) -> Result<Vec<Transcription>> {
    let rows = sqlx::query(
        r#"
        SELECT id, text, created_at, duration_ms, model_id, audio_path, output_mode, audio_size_bytes
        FROM transcriptions
        ORDER BY created_at DESC
        LIMIT ? OFFSET ?
        "#,
    )
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;

    let transcriptions = rows
        .into_iter()
        .map(|row| {
            let model_id_json: Option<String> = row.get(4);
            let model_id = model_id_json.and_then(|json| serde_json::from_str(&json).ok());

            Transcription {
                id: Some(row.get(0)),
                text: row.get(1),
                created_at: Some(row.get(2)),
                duration_ms: row.get(3),
                model_id,
                audio_path: row.get(5),
                output_mode: row.get(6),
                audio_size_bytes: row.get(7),
            }
        })
        .collect();

    Ok(transcriptions)
}

pub async fn search(pool: &SqlitePool, query: &str, limit: i64) -> Result<Vec<Transcription>> {
    let search_pattern = format!("%{}%", query);

    let rows = sqlx::query(
        r#"
        SELECT id, text, created_at, duration_ms, model_id, audio_path, output_mode, audio_size_bytes
        FROM transcriptions
        WHERE text LIKE ?
        ORDER BY created_at DESC
        LIMIT ?
        "#,
    )
    .bind(search_pattern)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    let transcriptions = rows
        .into_iter()
        .map(|row| {
            let model_id_json: Option<String> = row.get(4);
            let model_id = model_id_json.and_then(|json| serde_json::from_str(&json).ok());

            Transcription {
                id: Some(row.get(0)),
                text: row.get(1),
                created_at: Some(row.get(2)),
                duration_ms: row.get(3),
                model_id,
                audio_path: row.get(5),
                output_mode: row.get(6),
                audio_size_bytes: row.get(7),
            }
        })
        .collect();

    Ok(transcriptions)
}

pub async fn delete(pool: &SqlitePool, id: i64) -> Result<bool> {
    let result = sqlx::query(
        r#"
        DELETE FROM transcriptions
        WHERE id = ?
        "#,
    )
    .bind(id)
    .execute(pool)
    .await?;

    Ok(result.rows_affected() > 0)
}

pub async fn count(pool: &SqlitePool) -> Result<i64> {
    let row = sqlx::query(
        r#"
        SELECT COUNT(*) as count
        FROM transcriptions
        "#,
    )
    .fetch_one(pool)
    .await?;

    let count: i64 = row.get(0);
    Ok(count)
}

/// Insert text at cursor position using appropriate tool for display server
fn insert_text(text: &str) -> Result<()> {
    let display_server = DisplayServer::detect();

    let mut cmd = match display_server {
        DisplayServer::X11 => {
            let mut cmd = Command::new("xdotool");
            cmd.args(["type", "--"]).arg(text);
            cmd
        }
        DisplayServer::Wayland => {
            let mut cmd = Command::new("wtype");
            cmd.arg(text);
            cmd
        }
        DisplayServer::Unknown => {
            return Err(anyhow!("Unknown display server, cannot insert text"));
        }
    };

    let tool = cmd.get_program().to_string_lossy().to_string();
    if !Command::new("which").arg(&tool).output()?.status.success() {
        return Err(anyhow!("{} not found", tool));
    }

    let output = cmd.output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("{} failed: {}", tool, stderr));
    }

    Ok(())
}

fn deliver_output(text: &str, output_mode: crate::conf::OutputMode, app: &AppHandle) -> Result<()> {
    match output_mode {
        crate::conf::OutputMode::Print => {
            println!("{}", text);
            Ok(())
        }
        crate::conf::OutputMode::Copy => app
            .clipboard()
            .write_text(text.to_string())
            .map_err(|e| anyhow!("Failed to write to clipboard: {}", e)),
        crate::conf::OutputMode::Insert => insert_text(text),
    }
}

/// Transcribe audio and deliver output - the main entry point
pub async fn transcribe_and_deliver(
    audio_path: &Path,
    audio_buffer: &[i16],
    sample_rate: u32,
    app: &AppHandle,
) -> Result<Transcription> {
    let transcription_state: tauri::State<Mutex<Option<(ModelId, LoadedEngine)>>> = app.state();
    let settings: tauri::State<crate::conf::SettingsState> = app.state();
    let db = app.try_state::<crate::db::Database>();

    let context = TranscriptionContext {
        engine_state: &transcription_state,
        settings: &settings,
        database: db.as_deref(),
    };

    let audio = RecordedAudio {
        buffer: audio_buffer.to_vec(),
        path: audio_path.to_path_buf(),
        sample_rate,
    };

    let transcription = Transcription::from_audio(audio, context).await?;

    // Deliver output
    let output_mode = settings.get().await.output_mode;
    deliver_output(&transcription.text, output_mode, app)?;

    Ok(transcription)
}
