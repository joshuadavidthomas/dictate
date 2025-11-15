use crate::db::Database;
use crate::history::TranscriptionHistory;
use tauri::State;

#[tauri::command]
pub async fn get_transcription_history(
    db: State<'_, Database>,
    limit: Option<i64>,
    offset: Option<i64>,
) -> Result<Vec<TranscriptionHistory>, String> {
    let limit = limit.unwrap_or(50);
    let offset = offset.unwrap_or(0);

    crate::db::transcriptions::list(db.pool(), limit, offset)
        .await
        .map_err(|e| format!("Failed to get transcription history: {}", e))
}

#[tauri::command]
pub async fn get_transcription_by_id(
    db: State<'_, Database>,
    id: i64,
) -> Result<Option<TranscriptionHistory>, String> {
    crate::db::transcriptions::get(db.pool(), id)
        .await
        .map_err(|e| format!("Failed to get transcription: {}", e))
}

#[tauri::command]
pub async fn delete_transcription_by_id(db: State<'_, Database>, id: i64) -> Result<bool, String> {
    crate::db::transcriptions::delete(db.pool(), id)
        .await
        .map_err(|e| format!("Failed to delete transcription: {}", e))
}

#[tauri::command]
pub async fn search_transcription_history(
    db: State<'_, Database>,
    query: String,
    limit: Option<i64>,
) -> Result<Vec<TranscriptionHistory>, String> {
    let limit = limit.unwrap_or(50);

    crate::db::transcriptions::search(db.pool(), &query, limit)
        .await
        .map_err(|e| format!("Failed to search transcriptions: {}", e))
}

#[tauri::command]
pub async fn get_transcription_count(db: State<'_, Database>) -> Result<i64, String> {
    crate::db::transcriptions::count(db.pool())
        .await
        .map_err(|e| format!("Failed to count transcriptions: {}", e))
}
