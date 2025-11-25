use crate::broadcast::BroadcastServer;
use crate::conf::SettingsState;
use crate::models::{ModelEngine, ModelId, ModelInfo, ModelManager};
use serde::Serialize;
use tauri::State;

#[derive(Debug, Serialize)]
pub struct UiModelInfo {
    pub id: ModelId,
    pub engine: ModelEngine,
    pub is_downloaded: bool,
    pub is_directory: bool,
    pub download_url: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct UiStorageInfo {
    pub models_dir: String,
    pub total_size_bytes: u64,
    pub downloaded_count: usize,
    pub available_count: usize,
}

#[derive(Debug, Serialize)]
pub struct UiModelSize {
    pub id: ModelId,
    pub size_bytes: u64,
}

fn map_model_info(info: &ModelInfo) -> UiModelInfo {
    UiModelInfo {
        id: info.id,
        engine: info.engine(),
        is_downloaded: info.is_downloaded(),
        is_directory: info.is_directory(),
        download_url: info.download_url().map(|s| s.to_string()),
    }
}

#[tauri::command]
pub async fn list_models() -> Result<Vec<UiModelInfo>, String> {
    let manager = ModelManager::new().map_err(|e| e.to_string())?;
    Ok(manager
        .list_available_models()
        .into_iter()
        .map(map_model_info)
        .collect())
}

#[tauri::command]
pub async fn get_model_storage_info() -> Result<UiStorageInfo, String> {
    let manager = ModelManager::new().map_err(|e| e.to_string())?;
    let info = manager.get_storage_info().map_err(|e| e.to_string())?;

    Ok(UiStorageInfo {
        models_dir: info.models_dir.to_string_lossy().to_string(),
        total_size_bytes: info.total_size,
        downloaded_count: info.downloaded_count,
        available_count: info.available_count,
    })
}

#[tauri::command]
pub async fn download_model(
    id: ModelId,
    broadcast: State<'_, BroadcastServer>,
) -> Result<(), String> {
    let manager = ModelManager::new().map_err(|e| e.to_string())?;
    manager
        .download_model(id, &broadcast)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn remove_model(id: ModelId) -> Result<(), String> {
    let manager = ModelManager::new().map_err(|e| e.to_string())?;
    manager.remove_model(id).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_preferred_model() -> Result<Option<ModelId>, String> {
    let settings = crate::conf::Settings::load();
    Ok(settings.preferred_model)
}

#[tauri::command]
pub async fn get_model_sizes() -> Result<Vec<UiModelSize>, String> {
    let mut manager = ModelManager::new().map_err(|e| e.to_string())?;
    let sizes = manager
        .get_all_model_sizes()
        .await
        .map_err(|e| e.to_string())?;

    Ok(sizes
        .into_iter()
        .map(|(id, size_bytes)| UiModelSize { id, size_bytes })
        .collect())
}

#[tauri::command]
pub async fn set_preferred_model(
    settings: State<'_, SettingsState>,
    model: Option<ModelId>,
) -> Result<(), String> {
    // Optional validation: ensure the model is one we know about
    if let Some(m) = model {
        let manager = ModelManager::new().map_err(|e| e.to_string())?;
        if manager.get_model_info(m).is_none() {
            return Err(format!("Unknown model: {:?}", m));
        }
    }

    settings.set_preferred_model(model).await
}
