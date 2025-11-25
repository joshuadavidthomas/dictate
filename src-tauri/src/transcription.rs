use anyhow::{anyhow, Result};
use crate::conf::{OutputMode, SettingsState};
use crate::db::Database;
use crate::models::{ModelId, ModelManager, ParakeetModel, WhisperModel};
use crate::recording::{DisplayServer, RecordedAudio};
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use std::path::Path;
use std::process::Command;
use tauri::{AppHandle, Manager};
use tauri_plugin_clipboard_manager::ClipboardExt;
use tokio::sync::Mutex;
use transcribe_rs::{
    engines::parakeet::{ParakeetEngine, ParakeetModelParams},
    engines::whisper::WhisperEngine,
    TranscriptionEngine as TranscribeTrait,
};

// ============================================================================
// Domain: Transcription entity and factory
// ============================================================================

pub struct TranscriptionContext<'a> {
    pub engine_state: &'a TranscriptionState,
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
        // 1. Get or load ML engine
        let mut engine_guard = context.engine_state.engine().await;
        
        if engine_guard.is_none() {
            *engine_guard = Some(Self::load_engine(context.settings).await?);
        }
        
        let engine = engine_guard.as_mut().unwrap();
        
        // 2. Run ML inference
        let text = engine.transcribe_file(&audio.path)?;
        
        // 3. Get model info
        let model_id = engine.get_model_id();
        let output_mode = context.settings.get().await.output_mode;
        
        // 4. Construct entity
        let mut transcription = Self::from_recording(
            text,
            &audio.path,
            &audio.buffer,
            audio.sample_rate,
            output_mode,
            model_id,
        );
        
        // 5. Persist if database available and text non-empty
        if !transcription.text.trim().is_empty()
            && let Some(db) = context.database
        {
            transcription = save(db.pool(), transcription).await?;
            eprintln!("[Transcription] Saved with ID: {}", transcription.id.unwrap());
        }
        
        Ok(transcription)
    }
    
    async fn load_engine(settings: &SettingsState) -> Result<TranscriptionEngine> {
        let mut engine = TranscriptionEngine::new();
        let manager = ModelManager::new()?;
        
        // Try preferred model
        let settings_data = settings.get().await;
        if let Some(pref) = settings_data.preferred_model
            && let Some(path) = manager.get_model_path(pref)
        {
            engine.load_model(pref, &path.to_string_lossy())?;
            return Ok(engine);
        }
        
        // Fallback chain
        for candidate in [
            ModelId::Parakeet(ParakeetModel::V3),
            ModelId::Whisper(WhisperModel::Base),
        ] {
            if let Some(path) = manager.get_model_path(candidate) {
                engine.load_model(candidate, &path.to_string_lossy())?;
                return Ok(engine);
            }
        }
        
        Err(anyhow!("No transcription model available"))
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

// ============================================================================
// Engine: ML transcription backend
// ============================================================================

enum TranscriptionBackend {
    Whisper(WhisperEngine),
    Parakeet(ParakeetEngine),
}

pub struct TranscriptionEngine {
    backend: Option<TranscriptionBackend>,
    model_loaded: bool,
    model_path: Option<String>,
    model_id: Option<ModelId>,
}

impl TranscriptionEngine {
    pub fn new() -> Self {
        Self {
            backend: None,
            model_loaded: false,
            model_path: None,
            model_id: None,
        }
    }

    pub fn load_model(&mut self, model_id: ModelId, model_path: &str) -> Result<()> {
        println!("Loading transcription model from: {}", model_path);

        let path = Path::new(model_path);

        if !path.exists() {
            return Err(anyhow!("Model path not found: {}", model_path));
        }

        let is_directory = path.is_dir();

        if is_directory {
            // Parakeet model (directory-based)
            let mut parakeet_engine = ParakeetEngine::new();
            match parakeet_engine
                .load_model_with_params(path, ParakeetModelParams::int8())
            {
                Ok(_) => {
                    self.backend = Some(TranscriptionBackend::Parakeet(parakeet_engine));
                    self.model_loaded = true;
                    self.model_path = Some(model_path.to_string());
                    self.model_id = Some(model_id);
                    println!("Parakeet model loaded successfully");
                    Ok(())
                }
                Err(e) => {
                    eprintln!("DEBUG: Raw Parakeet error: {:?}", e);
                    Err(anyhow!("Failed to load Parakeet model: {}", e))
                }
            }
        } else {
            // Whisper model (file-based)
            let mut whisper_engine = WhisperEngine::new();
            match whisper_engine.load_model(path) {
                Ok(_) => {
                    self.backend = Some(TranscriptionBackend::Whisper(whisper_engine));
                    self.model_loaded = true;
                    self.model_path = Some(model_path.to_string());
                    self.model_id = Some(model_id);
                    println!("Whisper model loaded successfully");
                    Ok(())
                }
                Err(e) => {
                    let metadata = std::fs::metadata(path).ok();
                    let file_size = metadata.map(|m| m.len()).unwrap_or(0);

                    if file_size < 1_000_000 {
                        Err(anyhow!(
                            "Failed to load Whisper model (file may be corrupt, size: {} bytes): {}",
                            file_size,
                            e
                        ))
                    } else {
                        Err(anyhow!("Failed to load Whisper model: {}", e))
                    }
                }
            }
        }
    }

    pub fn transcribe_file<P: AsRef<Path>>(&mut self, audio_path: P) -> Result<String> {
        if !self.model_loaded {
            return Err(anyhow!("No model loaded"));
        }

        println!("Transcribing audio file: {}", audio_path.as_ref().display());

        // Placeholder mode check
        if let Some(model_path) = &self.model_path
            && model_path.starts_with("placeholder:")
        {
            println!("Using placeholder transcription (no real model loaded)");
            std::thread::sleep(std::time::Duration::from_millis(1000));
            let text = "This is a placeholder transcription from the audio file. Real transcription will work when model files are available.".to_string();
            println!("Transcription completed: {}", text);
            return Ok(text);
        }

        match &mut self.backend {
            Some(TranscriptionBackend::Whisper(engine)) => {
                match engine.transcribe_file(audio_path.as_ref(), None) {
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
            Some(TranscriptionBackend::Parakeet(engine)) => {
                match engine.transcribe_file(audio_path.as_ref(), None) {
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
            None => Err(anyhow!("No transcription backend initialized")),
        }
    }

    pub fn get_model_id(&self) -> Option<ModelId> {
        self.model_id
    }
}

impl Default for TranscriptionEngine {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Repository: Database operations
// ============================================================================

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

// ============================================================================
// Output Delivery
// ============================================================================

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
        crate::conf::OutputMode::Copy => {
            app.clipboard()
                .write_text(text.to_string())
                .map_err(|e| anyhow!("Failed to write to clipboard: {}", e))
        }
        crate::conf::OutputMode::Insert => {
            insert_text(text)
        }
    }
}

/// Transcribe audio and deliver output - the main entry point
pub async fn transcribe_and_deliver(
    audio_path: &Path,
    audio_buffer: &[i16],
    sample_rate: u32,
    app: &AppHandle,
) -> Result<Transcription> {
    let transcription_state: tauri::State<TranscriptionState> = app.state();
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

// ============================================================================
// State Management
// ============================================================================

/// Manages transcription engine state
pub struct TranscriptionState {
    engine: Mutex<Option<TranscriptionEngine>>,
}

impl TranscriptionState {
    pub fn new() -> Self {
        Self {
            engine: Mutex::new(None),
        }
    }

    pub async fn engine(&self) -> tokio::sync::MutexGuard<'_, Option<TranscriptionEngine>> {
        self.engine.lock().await
    }
}
