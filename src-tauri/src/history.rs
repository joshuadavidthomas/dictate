use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::{SqlitePool, Row};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptionHistory {
    pub id: i64,
    pub text: String,
    pub created_at: i64, // Unix timestamp in seconds
    pub duration_ms: Option<i64>,
    pub model_name: Option<String>,
    pub audio_path: Option<String>,
    pub output_mode: Option<String>,
    pub audio_size_bytes: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewTranscription {
    pub text: String,
    pub duration_ms: Option<i64>,
    pub model_name: Option<String>,
    pub audio_path: Option<String>,
    pub output_mode: Option<String>,
    pub audio_size_bytes: Option<i64>,
}

impl NewTranscription {
    pub fn new(text: String) -> Self {
        Self {
            text,
            duration_ms: None,
            model_name: None,
            audio_path: None,
            output_mode: None,
            audio_size_bytes: None,
        }
    }
    
    pub fn with_duration(mut self, duration_ms: i64) -> Self {
        self.duration_ms = Some(duration_ms);
        self
    }
    
    pub fn with_model(mut self, model_name: String) -> Self {
        self.model_name = Some(model_name);
        self
    }
    
    pub fn with_audio_path(mut self, audio_path: String) -> Self {
        self.audio_path = Some(audio_path);
        self
    }
    
    pub fn with_output_mode(mut self, output_mode: String) -> Self {
        self.output_mode = Some(output_mode);
        self
    }
    
    pub fn with_audio_size(mut self, audio_size_bytes: i64) -> Self {
        self.audio_size_bytes = Some(audio_size_bytes);
        self
    }
}

pub async fn save_transcription(pool: &SqlitePool, transcription: NewTranscription) -> Result<i64> {
    let created_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs() as i64;
    
    let result = sqlx::query(
        r#"
        INSERT INTO transcriptions (text, created_at, duration_ms, model_name, audio_path, output_mode, audio_size_bytes)
        VALUES (?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(&transcription.text)
    .bind(created_at)
    .bind(transcription.duration_ms)
    .bind(transcription.model_name)
    .bind(transcription.audio_path)
    .bind(transcription.output_mode)
    .bind(transcription.audio_size_bytes)
    .execute(pool)
    .await?;
    
    Ok(result.last_insert_rowid())
}

pub async fn get_transcription(pool: &SqlitePool, id: i64) -> Result<Option<TranscriptionHistory>> {
    let result = sqlx::query(
        r#"
        SELECT id, text, created_at, duration_ms, model_name, audio_path, output_mode, audio_size_bytes
        FROM transcriptions
        WHERE id = ?
        "#,
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    
    let transcription = result.map(|row| TranscriptionHistory {
        id: row.get(0),
        text: row.get(1),
        created_at: row.get(2),
        duration_ms: row.get(3),
        model_name: row.get(4),
        audio_path: row.get(5),
        output_mode: row.get(6),
        audio_size_bytes: row.get(7),
    });
    
    Ok(transcription)
}

pub async fn list_transcriptions(pool: &SqlitePool, limit: i64, offset: i64) -> Result<Vec<TranscriptionHistory>> {
    let rows = sqlx::query(
        r#"
        SELECT id, text, created_at, duration_ms, model_name, audio_path, output_mode, audio_size_bytes
        FROM transcriptions
        ORDER BY created_at DESC
        LIMIT ? OFFSET ?
        "#,
    )
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    
    let transcriptions = rows.into_iter().map(|row| TranscriptionHistory {
        id: row.get(0),
        text: row.get(1),
        created_at: row.get(2),
        duration_ms: row.get(3),
        model_name: row.get(4),
        audio_path: row.get(5),
        output_mode: row.get(6),
        audio_size_bytes: row.get(7),
    }).collect();
    
    Ok(transcriptions)
}

pub async fn search_transcriptions(pool: &SqlitePool, query: &str, limit: i64) -> Result<Vec<TranscriptionHistory>> {
    let search_pattern = format!("%{}%", query);
    
    let rows = sqlx::query(
        r#"
        SELECT id, text, created_at, duration_ms, model_name, audio_path, output_mode, audio_size_bytes
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
    
    let transcriptions = rows.into_iter().map(|row| TranscriptionHistory {
        id: row.get(0),
        text: row.get(1),
        created_at: row.get(2),
        duration_ms: row.get(3),
        model_name: row.get(4),
        audio_path: row.get(5),
        output_mode: row.get(6),
        audio_size_bytes: row.get(7),
    }).collect();
    
    Ok(transcriptions)
}

pub async fn delete_transcription(pool: &SqlitePool, id: i64) -> Result<bool> {
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

pub async fn count_transcriptions(pool: &SqlitePool) -> Result<i64> {
    let row = sqlx::query(
        r#"
        SELECT COUNT(*) as count
        FROM transcriptions
        "#
    )
    .fetch_one(pool)
    .await?;
    
    let count: i64 = row.get(0);
    Ok(count)
}
