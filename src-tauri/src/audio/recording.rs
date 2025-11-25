use anyhow::Result;
use crate::audio::{AudioRecorder, buffer_to_wav};
use crate::broadcast::BroadcastServer;
use crate::conf::SettingsState;
use crate::state::{RecordingSnapshot, RecordingState};
use cpal::traits::StreamTrait;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tauri::AppHandle;

pub struct RecordedAudio {
    pub buffer: Vec<i16>,
    pub path: PathBuf,
    pub sample_rate: u32,
}

pub async fn start(
    recording: &RecordingState,
    settings: &SettingsState,
    broadcast: &BroadcastServer,
    _app: &AppHandle,
) -> Result<(), String> {
    // Get audio settings from config
    let settings_data = settings.get().await;
    let device_name = settings_data.audio_device.clone();
    let sample_rate = settings_data.sample_rate;

    // Create recorder with configured device and sample rate
    let recorder = AudioRecorder::new_with_device(device_name.as_deref(), sample_rate)
        .map_err(|e| e.to_string())?;

    // Create recording buffers
    let audio_buffer = Arc::new(std::sync::Mutex::new(Vec::new()));
    let stop_signal = Arc::new(AtomicBool::new(false));

    // Create spectrum channel
    let (spectrum_tx, mut spectrum_rx) = tokio::sync::mpsc::unbounded_channel();

    // Start recording stream with spectrum analysis
    let stream = recorder
        .start_recording_background(
            audio_buffer.clone(),
            stop_signal.clone(),
            Some(spectrum_tx),
        )
        .map_err(|e| e.to_string())?;

    // Start the stream
    stream.play().map_err(|e| e.to_string())?;

    // Handle spectrum broadcasting
    // Spawn a background task that reads from spectrum_rx and broadcasts
    let broadcast = broadcast.clone();
    let start_time = std::time::Instant::now();
    tokio::spawn(async move {
        while let Some(spectrum) = spectrum_rx.recv().await {
            let ts = start_time.elapsed().as_millis() as u64;
            broadcast
                .recording_status(RecordingSnapshot::Recording, Some(spectrum), false, ts)
                .await;
        }
    });

    // Start recording with the new state API
    recording
        .start_recording(stream, audio_buffer, stop_signal)
        .await;

    eprintln!("[start_recording] Recording started successfully");
    Ok(())
}

pub async fn stop(recording: &RecordingState) -> Result<RecordedAudio> {
    // Stop the recording and get the audio buffer
    let audio_buffer = recording
        .stop_recording()
        .await
        .ok_or_else(|| anyhow::anyhow!("No active recording"))?;

    // Small delay to ensure last samples are written
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Get the recorded audio
    let buffer = audio_buffer.lock().unwrap().clone();

    if buffer.is_empty() {
        eprintln!("[stop] No audio recorded");
        recording.finish_transcription().await;
        return Err(anyhow::anyhow!("No audio recorded"));
    }

    eprintln!("[stop] Recorded {} samples", buffer.len());

    // Calculate duration from buffer length and sample rate
    let duration_ms = (buffer.len() as i64 * 1000) / 16000;
    eprintln!("[stop] Duration: {}ms", duration_ms);

    // Save to recordings directory with timestamp
    let recordings_dir = {
        use directories::ProjectDirs;
        let project_dirs = ProjectDirs::from("com", "dictate", "dictate")
            .ok_or_else(|| anyhow::anyhow!("Failed to get project directories"))?;
        let dir = project_dirs.data_dir().join("recordings");
        tokio::fs::create_dir_all(&dir)
            .await?;
        dir
    };

    let timestamp = jiff::Zoned::now().strftime("%Y-%m-%d_%H-%M-%S");
    let audio_path = recordings_dir.join(format!("{}.wav", timestamp));
    buffer_to_wav(&buffer, &audio_path, 16000)?;

    eprintln!("[stop] Audio saved to: {:?}", audio_path);

    Ok(RecordedAudio {
        buffer,
        path: audio_path,
        sample_rate: 16000,
    })
}
