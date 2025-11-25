use super::{ModelEngine, ModelId, catalog};
use crate::conf;
use anyhow::{Result, anyhow};
use flate2::read::GzDecoder;
use fs2::available_space;
use futures::StreamExt;
use indicatif::{ProgressBar, ProgressStyle};
use std::fs;
use std::path::{Path, PathBuf};
use tar::Archive;
use tokio::fs as async_fs;
use tokio::io::AsyncWriteExt;

/// Information about model storage
#[derive(Debug)]
pub struct StorageInfo {
    pub models_dir: PathBuf,
    pub total_size: u64,
    pub downloaded_count: usize,
    pub available_count: usize,
}

/// Returns the models directory path, creating it if it doesn't exist
pub fn models_dir() -> Result<PathBuf> {
    let dir = conf::get_project_dirs()?.data_dir().join("models");
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Builds the local filesystem path for a model (file or directory) regardless of download state
pub fn local_path(id: ModelId) -> Result<PathBuf> {
    let dir = models_dir()?;
    let desc = catalog::find(id)
        .ok_or_else(|| anyhow!("Model '{:?}' not found in catalog", id))?;
    
    let path = if desc.is_directory {
        // Directory-based model (e.g., Parakeet)
        dir.join(desc.storage_name)
    } else {
        // File-based model (e.g., Whisper)
        dir.join(format!("{}.bin", desc.storage_name))
    };
    
    Ok(path)
}

/// Checks if a model exists on disk
pub fn is_downloaded(id: ModelId) -> Result<bool> {
    let path = local_path(id)?;
    Ok(path.exists())
}

/// Downloads a model with progress reporting
pub async fn download(
    id: ModelId,
    broadcast: &crate::broadcast::BroadcastServer,
) -> Result<()> {
    let desc = catalog::find(id)
        .ok_or_else(|| anyhow!("Model '{:?}' not found in catalog", id))?;

    let output_path = local_path(id)?;
    let engine = id.engine();
    let name = desc.storage_name;

    if output_path.exists() {
        println!("Model '{}' is already downloaded", name);
        broadcast
            .model_download_progress(id, engine, 0, 0, "done")
            .await;
        return Ok(());
    }

    let url = desc.download_url;
    let client = reqwest::Client::new();

    if desc.is_directory {
        // Directory-based model (e.g., Parakeet) - download tar.gz and extract
        let dir = models_dir()?;
        let temp_archive = dir.join(format!("{}.tar.gz", name));

        println!("Downloading model '{}'...", name);
        broadcast
            .model_download_progress(id, engine, 0, 0, "downloading")
            .await;
        download_file(&client, url, &temp_archive, Some((id, engine, broadcast)))
            .await?;

        println!("Extracting archive...");
        broadcast
            .model_download_progress(id, engine, 0, 0, "extracting")
            .await;
        extract_tar_gz(&temp_archive, name).await?;

        // Clean up temporary archive
        async_fs::remove_file(&temp_archive).await?;

        println!("Model '{}' downloaded and extracted successfully", name);
    } else {
        // Single file download (e.g., Whisper models)
        if let Some(parent) = output_path.parent() {
            async_fs::create_dir_all(parent).await?;
        }
        broadcast
            .model_download_progress(id, engine, 0, 0, "downloading")
            .await;
        download_file(&client, url, &output_path, Some((id, engine, broadcast)))
            .await?;
        println!("Model '{}' downloaded successfully", name);
    }

    broadcast
        .model_download_progress(id, engine, 0, 0, "done")
        .await;

    Ok(())
}

/// Removes a model from disk
pub async fn remove(id: ModelId) -> Result<()> {
    let desc = catalog::find(id)
        .ok_or_else(|| anyhow!("Model '{:?}' not found in catalog", id))?;

    let model_path = local_path(id)?;
    let name = desc.storage_name;

    if !model_path.exists() {
        println!("Model '{}' is not downloaded", name);
        return Ok(());
    }

    if desc.is_directory {
        // Remove directory recursively
        async_fs::remove_dir_all(&model_path).await?;
    } else {
        // Remove single file
        async_fs::remove_file(&model_path).await?;
    }

    println!("Model '{}' removed successfully", name);
    Ok(())
}

/// Computes total size and counts of downloaded models
pub fn storage_info() -> Result<StorageInfo> {
    let dir = models_dir()?;
    let mut total_size = 0u64;
    let mut downloaded_count = 0;
    let available_count = catalog::all_models().len();

    for desc in catalog::all_models() {
        let path = local_path(desc.id)?;
        
        if !path.exists() {
            continue;
        }

        if desc.is_directory {
            // Calculate directory size recursively
            if let Ok(size) = calculate_dir_size(&path) {
                total_size += size;
                downloaded_count += 1;
            }
        } else if let Ok(metadata) = fs::metadata(&path) {
            total_size += metadata.len();
            downloaded_count += 1;
        }
    }

    Ok(StorageInfo {
        models_dir: dir,
        total_size,
        downloaded_count,
        available_count,
    })
}

// Internal helper functions

async fn download_file(
    client: &reqwest::Client,
    url: &str,
    output_path: &Path,
    progress: Option<(ModelId, ModelEngine, &crate::broadcast::BroadcastServer)>,
) -> Result<()> {
    println!("Downloading to {}", output_path.display());

    if let Some(parent) = output_path.parent() {
        async_fs::create_dir_all(parent).await?;
    }

    let response = client.get(url).send().await?;
    let total_size = response.content_length().unwrap_or(0);

    // Check disk space
    if total_size > 0
        && let Some(parent) = output_path.parent()
        && let Ok(available) = available_space(parent)
    {
        let required_space = (total_size as f64 * 1.1) as u64;
        if available < required_space {
            return Err(anyhow!(
                "Insufficient disk space. Need {} MB, available {} MB",
                required_space / 1_000_000,
                available / 1_000_000
            ));
        }
    }

    // Progress bar
    let pb = ProgressBar::new(total_size);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})")
            .unwrap_or_else(|_| ProgressStyle::default_spinner())
            .progress_chars("#>-"),
    );

    let mut stream = response.bytes_stream();
    let mut file = async_fs::File::create(output_path).await?;

    let mut downloaded = 0u64;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        file.write_all(&chunk).await?;
        downloaded += chunk.len() as u64;
        pb.set_position(downloaded);

        if let Some((id, engine, broadcast)) = progress {
            broadcast
                .model_download_progress(id, engine, downloaded, total_size, "downloading")
                .await;
        }
    }

    pb.finish_with_message("Download complete!");
    Ok(())
}

async fn extract_tar_gz(archive_path: &Path, model_name: &str) -> Result<()> {
    let dir = models_dir()?;
    let temp_extract_dir = dir.join(format!("{}.extracting", model_name));
    let final_model_dir = dir.join(model_name);

    // Clean up any previous incomplete extraction
    if temp_extract_dir.exists() {
        async_fs::remove_dir_all(&temp_extract_dir).await?;
    }

    // Create temporary extraction directory
    async_fs::create_dir_all(&temp_extract_dir).await?;

    // Extract in blocking task to avoid blocking async runtime
    let archive_path = archive_path.to_path_buf();
    let temp_dir = temp_extract_dir.clone();

    tokio::task::spawn_blocking(move || -> Result<()> {
        let tar_gz = std::fs::File::open(&archive_path)?;
        let tar = GzDecoder::new(tar_gz);
        let mut archive = Archive::new(tar);
        archive.unpack(&temp_dir)?;
        Ok(())
    })
    .await??;

    // Clean up Apple resource fork files (._* files)
    let cleaned = clean_apple_resource_forks(&temp_extract_dir)?;
    if cleaned > 0 {
        println!("Removed {} Apple resource fork file(s)", cleaned);
    }

    // Find extracted directory (archive might have nested structure)
    let extracted_dirs: Vec<_> = fs::read_dir(&temp_extract_dir)?
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false))
        .collect();

    if extracted_dirs.len() == 1 {
        // Single directory extracted - move it to final location
        let source_dir = extracted_dirs[0].path();
        if final_model_dir.exists() {
            async_fs::remove_dir_all(&final_model_dir).await?;
        }
        async_fs::rename(&source_dir, &final_model_dir).await?;
        // Clean up temp directory
        async_fs::remove_dir_all(&temp_extract_dir).await?;
    } else {
        // Multiple items or no directories - rename temp dir itself
        if final_model_dir.exists() {
            async_fs::remove_dir_all(&final_model_dir).await?;
        }
        async_fs::rename(&temp_extract_dir, &final_model_dir).await?;
    }

    Ok(())
}

/// Remove Apple resource fork files (._*) from extracted directory
fn clean_apple_resource_forks(dir: &Path) -> Result<usize> {
    let mut removed_count = 0;

    if dir.is_dir() {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            let file_name = entry.file_name();
            let name_str = file_name.to_string_lossy();

            if path.is_dir() {
                // Recurse into subdirectories
                removed_count += clean_apple_resource_forks(&path)?;
            } else if name_str.starts_with("._") {
                // Remove Apple resource fork file
                fs::remove_file(&path)?;
                removed_count += 1;
            }
        }
    }

    Ok(removed_count)
}

fn calculate_dir_size(path: &Path) -> Result<u64> {
    let mut total = 0u64;

    if path.is_dir() {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let metadata = entry.metadata()?;

            if metadata.is_dir() {
                total += calculate_dir_size(&entry.path())?;
            } else {
                total += metadata.len();
            }
        }
    }

    Ok(total)
}
