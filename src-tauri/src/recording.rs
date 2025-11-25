//! Recording pipeline: capture → audio file
//!
//! Handles audio capture, shortcuts, and the recording state machine.
//! Produces audio files that are consumed by transcription.rs.

use anyhow::{anyhow, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, StreamConfig};
use hound::{WavSpec, WavWriter};
use rustfft::{FftPlanner, num_complex::Complex};
use serde::{Deserialize, Serialize};
use std::env;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;
use tokio::sync::Mutex;

// ============================================================================
// State Machine
// ============================================================================

/// Broadcastable snapshot of recording state
#[derive(Clone, Copy, Serialize, Deserialize, Debug, PartialEq, Eq)]
pub enum RecordingSnapshot {
    Idle,
    Recording,
    Transcribing,
    Error,
}

impl RecordingSnapshot {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Idle => "Ready",
            Self::Recording => "Recording",
            Self::Transcribing => "Transcribing",
            Self::Error => "Error",
        }
    }
}

/// Internal recording phase with associated data
enum RecordingPhase {
    Idle,
    Recording {
        audio_buffer: Arc<std::sync::Mutex<Vec<i16>>>,
        stop_signal: Arc<AtomicBool>,
        stream: cpal::Stream,
        start_time: Instant,
    },
    Transcribing,
}

/// Manages the current recording state
pub struct RecordingState(Mutex<RecordingPhase>);

impl RecordingState {
    pub fn new() -> Self {
        Self(Mutex::new(RecordingPhase::Idle))
    }

    pub async fn start_recording(
        &self,
        stream: cpal::Stream,
        audio_buffer: Arc<std::sync::Mutex<Vec<i16>>>,
        stop_signal: Arc<AtomicBool>,
    ) {
        let mut phase = self.0.lock().await;
        *phase = RecordingPhase::Recording {
            audio_buffer,
            stop_signal,
            stream,
            start_time: Instant::now(),
        };
    }

    pub async fn stop_recording(&self) -> Option<Arc<std::sync::Mutex<Vec<i16>>>> {
        let mut phase = self.0.lock().await;
        if let RecordingPhase::Recording {
            audio_buffer,
            stop_signal,
            stream,
            ..
        } = std::mem::replace(&mut *phase, RecordingPhase::Transcribing)
        {
            stop_signal.store(true, std::sync::atomic::Ordering::Release);
            drop(stream);
            Some(audio_buffer)
        } else {
            None
        }
    }

    pub async fn finish_transcription(&self) {
        let mut phase = self.0.lock().await;
        *phase = RecordingPhase::Idle;
    }

    pub async fn snapshot(&self) -> RecordingSnapshot {
        let phase = self.0.lock().await;
        match &*phase {
            RecordingPhase::Idle => RecordingSnapshot::Idle,
            RecordingPhase::Recording { .. } => RecordingSnapshot::Recording,
            RecordingPhase::Transcribing => RecordingSnapshot::Transcribing,
        }
    }

    pub async fn elapsed_ms(&self) -> u64 {
        let phase = self.0.lock().await;
        if let RecordingPhase::Recording { start_time, .. } = &*phase {
            start_time.elapsed().as_millis() as u64
        } else {
            0
        }
    }
}

// ============================================================================
// Display Server Detection
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DisplayServer {
    Wayland,
    X11,
    Unknown,
}

impl DisplayServer {
    pub fn detect() -> Self {
        if env::var("WAYLAND_DISPLAY").is_ok()
            || env::var("XDG_SESSION_TYPE")
                .as_ref()
                .map(|s| s.as_str())
                == Ok("wayland")
        {
            return DisplayServer::Wayland;
        }

        if env::var("DISPLAY").is_ok() {
            return DisplayServer::X11;
        }

        if env::var("XDG_SESSION_TYPE")
            .as_ref()
            .map(|s| s.to_lowercase())
            == Ok("x11".to_string())
        {
            return DisplayServer::X11;
        }

        if let Ok(output) = Command::new("ps").args(["-e"]).output() {
            let output_str = String::from_utf8_lossy(&output.stdout);
            if output_str.contains("wayland") || output_str.contains("wlroots") {
                return DisplayServer::Wayland;
            }
            if output_str.contains("Xorg") || output_str.contains("Xwayland") {
                return DisplayServer::X11;
            }
        }

        DisplayServer::Unknown
    }
}

pub fn has_global_shortcuts_portal() -> bool {
    let output = Command::new("busctl")
        .args([
            "--user",
            "call",
            "org.freedesktop.portal.Desktop",
            "/org/freedesktop/portal/desktop",
            "org.freedesktop.DBus.Introspectable",
            "Introspect",
        ])
        .output();

    if let Ok(output) = output {
        let result = String::from_utf8_lossy(&output.stdout);
        return result.contains("org.freedesktop.portal.GlobalShortcuts");
    }

    false
}

pub fn detect_compositor() -> Option<String> {
    if env::var("HYPRLAND_INSTANCE_SIGNATURE").is_ok() {
        return Some("hyprland".to_string());
    }

    if let Ok(desktop) = env::var("XDG_CURRENT_DESKTOP") {
        let lower = desktop.to_lowercase();
        if lower.contains("hyprland") {
            return Some("hyprland".to_string());
        } else if lower.contains("sway") {
            return Some("sway".to_string());
        } else if lower.contains("gnome") {
            return Some("gnome".to_string());
        } else if lower.contains("kde") || lower.contains("plasma") {
            return Some("kde".to_string());
        }
        return Some(lower);
    }

    None
}

// ============================================================================
// Shortcut Backends
// ============================================================================

use std::future::Future;
use std::pin::Pin;
use tauri::AppHandle;

pub const SHORTCUT_ID: &str = "toggle-recording";
pub const SHORTCUT_DESCRIPTION: &str = "Toggle Recording";

#[derive(Debug, Clone, Serialize)]
pub enum ShortcutPlatform {
    X11,
    WaylandPortal,
    WaylandFallback,
    Unsupported,
}

#[derive(Debug, Clone, Serialize)]
pub struct BackendCapabilities {
    pub platform: ShortcutPlatform,
    pub can_register: bool,
    pub compositor: Option<String>,
}

pub trait ShortcutBackend: Send + Sync {
    fn register(&self, shortcut: &str) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>>;
    fn unregister(&self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>>;
    fn capabilities(&self) -> BackendCapabilities;
}

pub fn detect_platform() -> ShortcutPlatform {
    match DisplayServer::detect() {
        DisplayServer::Wayland => {
            if has_global_shortcuts_portal() {
                ShortcutPlatform::WaylandPortal
            } else {
                ShortcutPlatform::WaylandFallback
            }
        }
        DisplayServer::X11 => ShortcutPlatform::X11,
        DisplayServer::Unknown => ShortcutPlatform::Unsupported,
    }
}

pub fn create_backend(app: AppHandle) -> Box<dyn ShortcutBackend> {
    let platform = detect_platform();
    match platform {
        ShortcutPlatform::X11 => Box::new(X11Backend::new(app)),
        ShortcutPlatform::WaylandPortal => Box::new(WaylandPortalBackend::new(app)),
        ShortcutPlatform::WaylandFallback | ShortcutPlatform::Unsupported => {
            Box::new(FallbackBackend::new())
        }
    }
}

/// Manages keyboard shortcuts state
pub struct ShortcutState {
    backend: Arc<Mutex<Option<Box<dyn ShortcutBackend>>>>,
}

impl ShortcutState {
    pub fn new() -> Self {
        Self {
            backend: Arc::new(Mutex::new(None)),
        }
    }

    pub async fn set_backend(&self, backend: Box<dyn ShortcutBackend>) {
        *self.backend.lock().await = Some(backend);
    }

    pub async fn backend(&self) -> tokio::sync::MutexGuard<'_, Option<Box<dyn ShortcutBackend>>> {
        self.backend.lock().await
    }
}

// --- X11 Backend ---

use tauri::Manager;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut};

pub struct X11Backend {
    app: AppHandle,
}

impl X11Backend {
    pub fn new(app: AppHandle) -> Self {
        Self { app }
    }

    async fn register_impl(&self, shortcut: &str) -> Result<()> {
        let parsed = shortcut
            .parse::<Shortcut>()
            .map_err(|e| anyhow::anyhow!("Invalid shortcut format: {}", e))?;

        let app_handle = self.app.clone();

        self.app
            .global_shortcut()
            .on_shortcut(parsed, move |_app, _shortcut, _event| {
                let app = app_handle.clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(e) = crate::recording::toggle_recording(&app).await {
                        eprintln!("[shortcut] toggle_recording failed: {}", e);
                    }
                });
            })
            .map_err(|e| anyhow::anyhow!("Failed to register shortcut: {}", e))?;

        Ok(())
    }
}

impl ShortcutBackend for X11Backend {
    fn register(&self, shortcut: &str) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        let shortcut = shortcut.to_string();
        Box::pin(async move { self.register_impl(&shortcut).await })
    }

    fn unregister(&self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        Box::pin(async move { Ok(()) })
    }

    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities {
            platform: ShortcutPlatform::X11,
            can_register: true,
            compositor: detect_compositor(),
        }
    }
}

// --- Wayland Portal Backend ---

use ashpd::desktop::global_shortcuts::{GlobalShortcuts, NewShortcut};
use futures_util::StreamExt;

pub struct WaylandPortalBackend {
    app: AppHandle,
    proxy: Arc<Mutex<Option<GlobalShortcuts<'static>>>>,
    session: Arc<Mutex<Option<ashpd::desktop::Session<'static, GlobalShortcuts<'static>>>>>,
    listener_started: Arc<Mutex<bool>>,
}

impl WaylandPortalBackend {
    pub fn new(app: AppHandle) -> Self {
        Self {
            app,
            proxy: Arc::new(Mutex::new(None)),
            session: Arc::new(Mutex::new(None)),
            listener_started: Arc::new(Mutex::new(false)),
        }
    }

    fn convert_shortcut_format(shortcut: &str) -> String {
        let mut result = String::new();
        let parts: Vec<&str> = shortcut.split('+').collect();

        for (i, part) in parts.iter().enumerate() {
            let normalized = match part.trim() {
                "CommandOrControl" | "Ctrl" | "Control" => "<Control>",
                "Command" | "Super" | "Meta" => "<Super>",
                "Alt" => "<Alt>",
                "Shift" => "<Shift>",
                key => {
                    if i == parts.len() - 1 {
                        &key.to_lowercase()
                    } else {
                        continue;
                    }
                }
            };
            result.push_str(normalized);
        }

        result
    }

    async fn register_impl(&self, shortcut: &str) -> Result<()> {
        use anyhow::Context;
        
        let portal_shortcut = Self::convert_shortcut_format(shortcut);

        let mut proxy_guard = self.proxy.lock().await;
        if proxy_guard.is_none() {
            let proxy = GlobalShortcuts::new()
                .await
                .context("Failed to create GlobalShortcuts proxy")?;
            *proxy_guard = Some(proxy);
        }
        let proxy = proxy_guard.as_ref().unwrap();

        let mut session_guard = self.session.lock().await;
        if session_guard.is_none() {
            let session = proxy
                .create_session()
                .await
                .context("Failed to create session")?;
            *session_guard = Some(session);
        }
        let session = session_guard.as_ref().unwrap();

        let new_shortcut = NewShortcut::new(SHORTCUT_ID, SHORTCUT_DESCRIPTION)
            .preferred_trigger(Some(portal_shortcut.as_str()));

        let request = proxy
            .bind_shortcuts(session, &[new_shortcut], None)
            .await
            .context("Failed to create bind request")?;

        request
            .response()
            .context("Failed to get portal response")?;

        drop(session_guard);
        drop(proxy_guard);

        self.start_listener_if_needed().await;

        Ok(())
    }

    async fn start_listener_if_needed(&self) {
        let mut listener_started = self.listener_started.lock().await;
        if *listener_started {
            return;
        }
        *listener_started = true;

        let app_handle = self.app.clone();
        tokio::spawn(async move {
            let Ok(listener_proxy) = GlobalShortcuts::new().await else {
                return;
            };

            let Ok(mut stream) = listener_proxy.receive_activated().await else {
                return;
            };

            while let Some(_activated) = stream.next().await {
                let app = app_handle.clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(e) = crate::recording::toggle_recording(&app).await {
                        eprintln!("[shortcut] toggle_recording failed: {}", e);
                    }
                });
            }
        });
    }
}

impl ShortcutBackend for WaylandPortalBackend {
    fn register(&self, shortcut: &str) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        let shortcut = shortcut.to_string();
        Box::pin(async move { self.register_impl(&shortcut).await })
    }

    fn unregister(&self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        Box::pin(async move {
            let mut proxy_guard = self.proxy.lock().await;
            *proxy_guard = None;

            let mut session_guard = self.session.lock().await;
            if let Some(session) = session_guard.take() {
                drop(session);
            }

            Ok(())
        })
    }

    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities {
            platform: ShortcutPlatform::WaylandPortal,
            can_register: true,
            compositor: detect_compositor(),
        }
    }
}

// --- Fallback Backend ---

pub struct FallbackBackend;

impl FallbackBackend {
    pub fn new() -> Self {
        Self
    }
}

impl Default for FallbackBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl ShortcutBackend for FallbackBackend {
    fn register(&self, _shortcut: &str) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        Box::pin(async move { Ok(()) })
    }

    fn unregister(&self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        Box::pin(async move { Ok(()) })
    }

    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities {
            platform: ShortcutPlatform::WaylandFallback,
            can_register: false,
            compositor: detect_compositor(),
        }
    }
}

// ============================================================================
// Audio Recording
// ============================================================================

/// Audio recording device with configuration
pub struct AudioRecorder {
    device: Device,
    config: StreamConfig,
    sample_rate: u32,
}

/// Information about an available audio input device
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioDeviceInfo {
    pub name: String,
    pub supported_sample_rates: Vec<u32>,
}

/// Sample rate option with metadata for UI display
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SampleRateOption {
    pub value: u32,
    pub is_recommended: bool,
}

/// Supported sample rates for audio recording
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::enum_variant_names)]
pub enum SampleRate {
    Rate8kHz = 8000,
    Rate16kHz = 16000,
    Rate22kHz = 22050,
    Rate44kHz = 44100,
    Rate48kHz = 48000,
}

impl SampleRate {
    /// All available sample rates
    pub const ALL: [Self; 5] = [
        Self::Rate8kHz,
        Self::Rate16kHz,
        Self::Rate22kHz,
        Self::Rate44kHz,
        Self::Rate48kHz,
    ];

    /// Get all available sample rate options with UI metadata
    pub fn all_options() -> Vec<SampleRateOption> {
        Self::ALL.iter().map(|rate| rate.as_option()).collect()
    }

    /// Convert this sample rate to a SampleRateOption with metadata
    pub fn as_option(self) -> SampleRateOption {
        SampleRateOption {
            value: self.as_u32(),
            is_recommended: self.is_recommended(),
        }
    }

    /// Convert sample rate to u32 value
    pub const fn as_u32(self) -> u32 {
        self as u32
    }

    /// Whether this is the recommended rate
    pub const fn is_recommended(self) -> bool {
        matches!(self, Self::Rate16kHz)
    }
}

impl std::convert::TryFrom<u32> for SampleRate {
    type Error = anyhow::Error;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            8000 => Ok(Self::Rate8kHz),
            16000 => Ok(Self::Rate16kHz),
            22050 => Ok(Self::Rate22kHz),
            44100 => Ok(Self::Rate44kHz),
            48000 => Ok(Self::Rate48kHz),
            _ => Err(anyhow!(
                "Unsupported sample rate: {}. Supported rates: {:?}",
                value,
                Self::ALL.iter().map(|r| r.as_u32()).collect::<Vec<_>>()
            )),
        }
    }
}

impl From<SampleRate> for u32 {
    fn from(rate: SampleRate) -> Self {
        rate.as_u32()
    }
}

impl AudioRecorder {
    /// Create a new audio recorder with a specific device and sample rate
    ///
    /// # Arguments
    /// * `device_name` - Optional device name. If None, uses system default.
    /// * `sample_rate` - Target sample rate in Hz (e.g., 16000, 44100, 48000)
    pub fn new_with_device(device_name: Option<&str>, sample_rate: u32) -> Result<Self> {
        let host = cpal::default_host();

        let device = if let Some(name) = device_name {
            // Find device by name
            host.input_devices()?
                .find(|d| d.name().map(|n| n == name).unwrap_or(false))
                .ok_or_else(|| anyhow!("Audio device '{}' not found", name))?
        } else {
            // Use default device
            host.default_input_device()
                .ok_or_else(|| anyhow!("No default input device found"))?
        };

        let config = Self::get_optimal_config(&device, sample_rate)?;

        Ok(Self {
            device,
            config,
            sample_rate,
        })
    }

    /// Find the best audio configuration for the target sample rate
    fn get_optimal_config(device: &Device, target_sample_rate: u32) -> Result<StreamConfig> {
        let supported_configs = device.supported_input_configs()?;

        // Choose a supported configuration whose effective sample rate is as
        // close as possible to the requested target. If the device does not
        // support the exact target rate, we fall back to the nearest boundary
        // (min or max) within the reported range instead of forcing 16kHz.
        let mut best_config = None;
        let mut best_diff = u32::MAX;

        for config_range in supported_configs {
            let min = config_range.min_sample_rate().0;
            let max = config_range.max_sample_rate().0;

            // Pick a concrete rate within this range that is closest to target
            let candidate_rate = if target_sample_rate < min {
                min
            } else if target_sample_rate > max {
                max
            } else {
                target_sample_rate
            };

            let diff = candidate_rate.abs_diff(target_sample_rate);
            if diff < best_diff {
                best_diff = diff;
                // This is safe because candidate_rate is guaranteed to be within [min, max]
                let cfg = config_range.with_sample_rate(cpal::SampleRate(candidate_rate));
                best_config = Some(cfg);
            }
        }

        let config = best_config
            .ok_or_else(|| anyhow!("No suitable audio configuration found".to_string()))?;

        Ok(config.into())
    }

    /// Check if a device supports a specific sample rate
    fn device_supports_rate(device: &Device, rate: u32) -> bool {
        device
            .supported_input_configs()
            .map(|mut configs| {
                configs.any(|config| {
                    let min = config.min_sample_rate().0;
                    let max = config.max_sample_rate().0;
                    rate >= min && rate <= max
                })
            })
            .unwrap_or(false)
    }

    /// List all available audio input devices
    pub fn list_devices() -> Result<Vec<AudioDeviceInfo>> {
        let host = cpal::default_host();
        let devices = host.input_devices()?;

        let mut device_infos = Vec::new();

        for device in devices {
            let name = device.name().unwrap_or("Unknown Device".to_string());

            // Skip the virtual "default" device - it's just an alias
            if name == "default" {
                continue;
            }

            // Check which of our standard rates this device supports
            let supported_sample_rates: Vec<u32> = SampleRate::ALL
                .iter()
                .map(|r| r.as_u32())
                .filter(|&rate| Self::device_supports_rate(&device, rate))
                .collect();

            device_infos.push(AudioDeviceInfo {
                name,
                supported_sample_rates,
            });
        }

        Ok(device_infos)
    }

    /// Record a short audio sample and return the average volume level (0.0 to 1.0)
    pub fn get_audio_level(&self) -> Result<f32> {
        let buffer = Arc::new(std::sync::Mutex::new(Vec::new()));
        let stop_signal = Arc::new(AtomicBool::new(false));

        let stream =
            self.start_recording_background(buffer.clone(), stop_signal.clone(), None)?;

        stream.play()?;

        // Record for 100ms
        std::thread::sleep(std::time::Duration::from_millis(100));
        stop_signal.store(true, Ordering::Release);

        // Give it time to stop
        std::thread::sleep(std::time::Duration::from_millis(10));
        drop(stream);

        // Calculate RMS (root mean square) of the audio samples
        let samples = buffer.lock().unwrap();
        if samples.is_empty() {
            return Ok(0.0);
        }

        let sum_of_squares: f64 = samples
            .iter()
            .map(|&s| {
                let normalized = s as f64 / i16::MAX as f64;
                normalized * normalized
            })
            .sum();

        let rms = (sum_of_squares / samples.len() as f64).sqrt();
        Ok(rms as f32)
    }

    /// Start recording in background to a shared buffer (non-blocking)
    ///
    /// Optionally sends spectrum analysis updates via spectrum_tx channel.
    /// Recording can be stopped by setting stop_signal.
    pub fn start_recording_background(
        &self,
        audio_buffer: Arc<std::sync::Mutex<Vec<i16>>>,
        stop_signal: Arc<AtomicBool>,
        spectrum_tx: Option<tokio::sync::mpsc::UnboundedSender<Vec<f32>>>,
    ) -> Result<cpal::Stream> {
        let buffer_clone = audio_buffer.clone();
        let stop_clone = stop_signal.clone();

        // Create spectrum analyzer if we have a channel to send to
        let mut spectrum_analyzer = spectrum_tx
            .as_ref()
            .map(|_| SpectrumAnalyzer::new(self.sample_rate));

        let stream = self.device.build_input_stream(
            &self.config.clone(),
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                if stop_clone.load(Ordering::Acquire) {
                    return;
                }

                if let Ok(mut buffer) = buffer_clone.lock() {
                    for &sample in data {
                        let sample_i16 = (sample * i16::MAX as f32) as i16;
                        buffer.push(sample_i16);

                        // Calculate and send spectrum if analyzer exists
                        if let Some(ref mut analyzer) = spectrum_analyzer
                            && let Some(bands) = analyzer.push_sample(sample)
                            && let Some(ref tx) = spectrum_tx
                        {
                            let _ = tx.send(bands);
                        }
                    }
                }
            },
            |err| {
                eprintln!("Recording error: {}", err);
            },
            None,
        )?;

        Ok(stream)
    }
}

/// Convert audio buffer to WAV file
///
/// Writes a raw i16 audio buffer to a WAV file with the specified sample rate.
/// The output is always mono (single channel), 16-bit PCM.
///
/// # Arguments
/// * `buffer` - Raw audio samples as signed 16-bit integers
/// * `output_path` - Path where the WAV file should be written
/// * `sample_rate` - Sample rate in Hz (e.g., 16000 for 16kHz)
///
/// # Example
/// ```ignore
/// use crate::recording::buffer_to_wav;
///
/// let samples: Vec<i16> = vec![0; 16000]; // 1 second of silence at 16kHz
/// buffer_to_wav(&samples, "output.wav", 16000).unwrap();
/// ```
pub fn buffer_to_wav<P: AsRef<Path>>(
    buffer: &[i16],
    output_path: P,
    sample_rate: u32,
) -> Result<()> {
    let spec = WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };

    let mut writer = WavWriter::create(output_path, spec)?;
    for &sample in buffer {
        writer.write_sample(sample)?;
    }
    writer.finalize()?;
    Ok(())
}

// ============================================================================
// Spectrum Analysis
// ============================================================================

/// Type of frequency band, determines noise gate threshold
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BandType {
    /// Low frequencies, higher noise gate threshold (room noise)
    Bass,
    /// Mid-high frequencies, lower threshold (speech content)
    Speech,
}

/// FFT parameters that define frequency-to-bin mapping
#[derive(Debug, Clone, Copy)]
struct FftParams {
    sample_rate: u32,
    fft_size: usize,
}

impl FftParams {
    const fn new(sample_rate: u32, fft_size: usize) -> Self {
        Self {
            sample_rate,
            fft_size,
        }
    }

    /// Nyquist frequency (half the sample rate)
    #[inline]
    fn nyquist(&self) -> f32 {
        self.sample_rate as f32 / 2.0
    }

    /// Frequency resolution per FFT bin
    #[inline]
    fn bin_width(&self) -> f32 {
        self.nyquist() / (self.fft_size as f32 / 2.0)
    }
}

/// A range of FFT bins
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BinRange {
    low: usize,
    high: usize,
}

impl BinRange {
    fn new(low: usize, high: usize) -> Self {
        Self { low, high }
    }

    /// Calculate RMS magnitude for this bin range
    fn calculate_rms(&self, fft_data: &[Complex<f32>]) -> f32 {
        if self.low >= self.high {
            return 0.0;
        }

        let sum_squares: f32 = fft_data[self.low..self.high]
            .iter()
            .map(|c| {
                let mag = c.norm();
                mag * mag
            })
            .sum();

        let count = (self.high - self.low) as f32;
        (sum_squares / count).sqrt()
    }
}

/// Noise gate with floor and per-band thresholds
#[derive(Debug, Clone, Copy)]
struct NoiseGate {
    /// Minimum signal level to process (eliminates DC offset and low hum)
    noise_floor: f32,
    /// Threshold for bass frequencies (0-1, higher = more aggressive)
    bass_threshold: f32,
    /// Threshold for speech frequencies (0-1, lower = more sensitive)
    speech_threshold: f32,
}

impl NoiseGate {
    const fn new(noise_floor: f32, bass_threshold: f32, speech_threshold: f32) -> Self {
        Self {
            noise_floor,
            bass_threshold,
            speech_threshold,
        }
    }

    /// Get threshold for a band type
    #[inline]
    fn threshold_for(&self, band_type: BandType) -> f32 {
        match band_type {
            BandType::Bass => self.bass_threshold,
            BandType::Speech => self.speech_threshold,
        }
    }

    /// Apply noise gate to a signal
    fn gate(&self, signal: f32, band_type: BandType) -> f32 {
        let threshold = self.threshold_for(band_type);
        if signal < threshold {
            0.0
        } else {
            ((signal - threshold) / (1.0 - threshold)).clamp(0.0, 1.0)
        }
    }
}

impl Default for NoiseGate {
    fn default() -> Self {
        // Lower thresholds for more sensitive response to speech
        // noise_floor: 0.005 (was 0.01) - catch quieter signals
        // bass_threshold: 0.30 - keep bass gating aggressive
        // speech_threshold: 0.10 (was 0.20) - more sensitive to higher frequencies
        Self::new(0.005, 0.30, 0.10)
    }
}

/// A frequency band defined by acoustic properties only
#[derive(Debug, Clone, Copy)]
struct FrequencyBand {
    /// Lower frequency bound (Hz)
    low_hz: f32,
    /// Upper frequency bound (Hz)
    high_hz: f32,
    /// Band type (determines noise gate threshold)
    band_type: BandType,
}

impl FrequencyBand {
    const fn new(low_hz: f32, high_hz: f32, band_type: BandType) -> Self {
        Self {
            low_hz,
            high_hz,
            band_type,
        }
    }

    /// Convert this frequency band to FFT bin range
    fn to_bin_range(self, params: &FftParams) -> BinRange {
        let bin_width = params.bin_width();
        let low_bin = (self.low_hz / bin_width) as usize;
        let high_bin = ((self.high_hz / bin_width) as usize).min(params.fft_size / 2);

        BinRange::new(low_bin, high_bin)
    }
}

/// Couples a frequency band with visualization display settings
#[derive(Debug, Clone, Copy)]
struct BandVisualization {
    /// The acoustic frequency band
    band: FrequencyBand,
    /// Display amplification factor for UI visualization
    display_boost: f32,
}

impl BandVisualization {
    const fn new(low_hz: f32, high_hz: f32, display_boost: f32, band_type: BandType) -> Self {
        Self {
            band: FrequencyBand::new(low_hz, high_hz, band_type),
            display_boost,
        }
    }

    /// Process this band: bin range -> RMS -> signal processing
    fn process(
        self,
        fft_data: &[Complex<f32>],
        params: &FftParams,
        processing: &SignalProcessing,
    ) -> f32 {
        let bin_range = self.band.to_bin_range(params);
        let rms = bin_range.calculate_rms(fft_data);
        processing.process(rms, self.display_boost, self.band.band_type)
    }
}

/// Signal processing configuration and pipeline
#[derive(Debug, Clone, Copy)]
struct SignalProcessing {
    noise_gate: NoiseGate,
}

impl SignalProcessing {
    const fn new(noise_gate: NoiseGate) -> Self {
        Self { noise_gate }
    }

    /// Run the complete signal processing pipeline
    fn process(&self, rms: f32, weight: f32, band_type: BandType) -> f32 {
        let signal = (rms - self.noise_gate.noise_floor).max(0.0);
        let weighted = signal * weight;
        let compressed = weighted.sqrt();
        self.noise_gate.gate(compressed, band_type)
    }
}

/// FFT window size optimized for real-time speech processing
/// - 512 samples @ 16kHz = 32ms latency
/// - Provides 31.25 Hz frequency resolution
const FFT_SIZE: usize = 512;

/// Generate a Hann window to reduce spectral leakage in FFT
///
/// The Hann window smoothly tapers the signal at the edges to minimize
/// discontinuities that cause spectral leakage in the frequency domain.
///
/// Formula: w(n) = 0.5 * (1 - cos(2πn/N))
/// where n is the sample index and N is the window size
fn generate_hann_window(size: usize) -> Vec<f32> {
    (0..size)
        .map(|i| 0.5 * (1.0 - (2.0 * std::f32::consts::PI * i as f32 / size as f32).cos()))
        .collect()
}

/// Apply window function to samples, preparing them for FFT
///
/// Multiplies each sample by its corresponding window coefficient and
/// converts to complex numbers (with zero imaginary component) ready for FFT.
fn apply_window(samples: &[f32], window: &[f32]) -> Vec<Complex<f32>> {
    samples
        .iter()
        .zip(window.iter())
        .map(|(&s, &w)| Complex::new(s * w, 0.0))
        .collect()
}

/// Apply exponential moving average (EMA) temporal smoothing
///
/// Blends the current value with the previous value to reduce jitter in visualization.
/// Uses a smoothing factor of 0.7 (70% previous, 30% current) - tuned for speech visualization.
#[inline]
fn apply_temporal_smoothing(current: f32, previous: f32) -> f32 {
    const SMOOTHING_FACTOR: f32 = 0.7;
    SMOOTHING_FACTOR * previous + (1.0 - SMOOTHING_FACTOR) * current
}

/// Number of frequency bands produced by spectrum analysis
///
/// This constant defines how many frequency bands the audio analyzer produces.
/// The spectrum is divided into 8 bands optimized for speech visualization,
/// ranging from 20Hz to 8kHz.
pub const SPECTRUM_BANDS: usize = 8;

/// Speech-optimized frequency bands for 16kHz sample rate
/// Bass heavily reduced to filter environmental noise
/// Display boosts tuned for balanced frequency response visualization
const SPEECH_BANDS: [BandVisualization; SPECTRUM_BANDS] = [
    // Sub-bass (room noise) - 20-125 Hz
    BandVisualization::new(20.0, 125.0, 0.2, BandType::Bass),
    // Bass (room noise) - 125-250 Hz
    BandVisualization::new(125.0, 250.0, 0.3, BandType::Bass),
    // Low-mid - 250-500 Hz
    BandVisualization::new(250.0, 500.0, 1.2, BandType::Speech),
    // Mid (core speech) - 500-1000 Hz
    BandVisualization::new(500.0, 1000.0, 2.5, BandType::Speech),
    // High-mid (core speech) - 1000-2000 Hz
    BandVisualization::new(1000.0, 2000.0, 3.0, BandType::Speech),
    // Presence - 2000-4000 Hz - boosted for consonants
    BandVisualization::new(2000.0, 4000.0, 2.5, BandType::Speech),
    // Brilliance - 4000-6000 Hz - boosted for sibilants (1.0 -> 1.8)
    BandVisualization::new(4000.0, 6000.0, 1.8, BandType::Speech),
    // Air - 6000-8000 Hz - boosted for breath/air sounds (0.8 -> 1.5)
    BandVisualization::new(6000.0, 8000.0, 1.5, BandType::Speech),
];

/// FFT-based spectrum analyzer for frequency band visualization
///
/// Processes audio samples in real-time and produces frequency band levels
/// optimized for speech visualization in UI elements.
pub struct SpectrumAnalyzer {
    sample_buffer: Vec<f32>,
    fft_planner: FftPlanner<f32>,
    window: Vec<f32>,
    sample_rate: u32,
    noise_gate: NoiseGate,
    previous_frame_output: Vec<f32>,
}

impl SpectrumAnalyzer {
    /// Create a new spectrum analyzer for speech visualization
    pub fn new(sample_rate: u32) -> Self {
        Self {
            sample_buffer: Vec::with_capacity(FFT_SIZE),
            previous_frame_output: vec![0.0; SPEECH_BANDS.len()],
            window: generate_hann_window(FFT_SIZE),
            fft_planner: FftPlanner::new(),
            sample_rate,
            noise_gate: NoiseGate::default(),
        }
    }

    /// Push a single audio sample and optionally return frequency bands
    ///
    /// Returns `Some(bands)` when the FFT window is full and ready to process.
    /// Otherwise returns `None` to indicate more samples are needed.
    pub fn push_sample(&mut self, sample: f32) -> Option<Vec<f32>> {
        self.sample_buffer.push(sample);

        if self.sample_buffer.len() >= FFT_SIZE {
            let bands = self.compute_spectrum();
            self.sample_buffer.clear();
            Some(bands)
        } else {
            None
        }
    }

    /// Compute frequency spectrum from buffered samples
    fn compute_spectrum(&mut self) -> Vec<f32> {
        // Apply Hann window to reduce spectral leakage
        let mut windowed = apply_window(&self.sample_buffer, &self.window);

        let fft = self.fft_planner.plan_fft_forward(FFT_SIZE);
        fft.process(&mut windowed);

        let fft_params = FftParams::new(self.sample_rate, FFT_SIZE);
        let processing = SignalProcessing::new(self.noise_gate);

        let mut bands: Vec<f32> = SPEECH_BANDS
            .iter()
            .copied()
            .map(|band| band.process(&windowed, &fft_params, &processing))
            .collect();

        for (i, band) in bands.iter_mut().enumerate() {
            *band = apply_temporal_smoothing(*band, self.previous_frame_output[i]);
            self.previous_frame_output[i] = *band;
        }

        bands
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_hann_window() {
        let window = generate_hann_window(4);

        // Hann window should have 4 values
        assert_eq!(window.len(), 4);

        // Expected values for size 4: [0, 0.5, 1, 0.5]
        assert!(window[0].abs() < 0.0001); // Start at 0
        assert!((window[1] - 0.5).abs() < 0.0001);
        assert!((window[2] - 1.0).abs() < 0.0001); // Peak at center
        assert!((window[3] - 0.5).abs() < 0.0001);

        // Window should be symmetric around center
        assert!((window[0] + window[3] - 0.5).abs() < 0.0001);
        assert!((window[1] - window[3]).abs() < 0.0001);
    }

    #[test]
    fn test_apply_window() {
        let samples = vec![1.0, 2.0, 3.0, 4.0];
        let window = vec![0.5, 1.0, 1.0, 0.5];

        let windowed = apply_window(&samples, &window);

        assert_eq!(windowed.len(), 4);

        // Check that samples are multiplied by window coefficients
        assert!((windowed[0].re - 0.5).abs() < 0.0001); // 1.0 * 0.5
        assert!((windowed[1].re - 2.0).abs() < 0.0001); // 2.0 * 1.0
        assert!((windowed[2].re - 3.0).abs() < 0.0001); // 3.0 * 1.0
        assert!((windowed[3].re - 2.0).abs() < 0.0001); // 4.0 * 0.5

        // Imaginary parts should all be zero
        for val in windowed {
            assert_eq!(val.im, 0.0);
        }
    }

    #[test]
    fn test_noise_gate_threshold_selection() {
        let gate = NoiseGate::new(0.02, 0.5, 0.35);

        assert_eq!(gate.threshold_for(BandType::Bass), 0.5);
        assert_eq!(gate.threshold_for(BandType::Speech), 0.35);
    }

    #[test]
    fn test_noise_gate_gating() {
        let gate = NoiseGate::new(0.02, 0.5, 0.35);

        // Below threshold - gated to 0
        assert_eq!(gate.gate(0.2, BandType::Speech), 0.0);

        // At threshold - gated to 0
        assert_eq!(gate.gate(0.35, BandType::Speech), 0.0);

        // At maximum
        assert_eq!(gate.gate(1.0, BandType::Speech), 1.0);

        // Mid-range: (0.675 - 0.35) / (1.0 - 0.35) = 0.5
        let result = gate.gate(0.675, BandType::Speech);
        assert!((result - 0.5).abs() < 0.001);
    }

    #[test]
    fn test_fft_params() {
        let params = FftParams::new(16000, 512);

        assert_eq!(params.sample_rate, 16000);
        assert_eq!(params.fft_size, 512);
        assert_eq!(params.nyquist(), 8000.0);

        // bin_width = 8000 / 256 = 31.25
        assert!((params.bin_width() - 31.25).abs() < 0.01);
    }

    #[test]
    fn test_bin_range_calculate_rms_empty() {
        let fft_data = vec![Complex::new(0.0, 0.0); 256];
        let range = BinRange::new(10, 10); // Empty range

        assert_eq!(range.calculate_rms(&fft_data), 0.0);
    }

    #[test]
    fn test_bin_range_calculate_rms_with_signal() {
        let mut fft_data = vec![Complex::new(0.0, 0.0); 256];

        // Set bins 10-14 to have magnitude 1.0
        fft_data[10..15].fill(Complex::new(1.0, 0.0));

        let range = BinRange::new(10, 15);
        let rms = range.calculate_rms(&fft_data);

        // RMS of five 1.0 values: sqrt((5 * 1.0^2) / 5) = 1.0
        assert!((rms - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_frequency_band_to_bin_range() {
        let band = FrequencyBand::new(500.0, 1000.0, BandType::Speech);
        let params = FftParams::new(16000, 512);
        let range = band.to_bin_range(&params);

        // bin_width = 31.25, so 500Hz = bin 16, 1000Hz = bin 32
        assert_eq!(range.low, 16);
        assert_eq!(range.high, 32);
    }

    #[test]
    fn test_frequency_band_to_bin_range_clamps_at_nyquist() {
        let band = FrequencyBand::new(7000.0, 10000.0, BandType::Speech);
        let params = FftParams::new(16000, 512);
        let range = band.to_bin_range(&params);

        // High freq exceeds Nyquist, should clamp to fft_size/2 = 256
        assert_eq!(range.high, 256);
        assert!(range.low < range.high);
    }

    #[test]
    fn test_signal_processing_pipeline() {
        let gate = NoiseGate::new(0.02, 0.5, 0.35);
        let processing = SignalProcessing::new(gate);

        // Test with speech band
        let result = processing.process(0.1, 1.5, BandType::Speech);

        // Should go through: subtract floor -> weight -> compress -> gate
        // (0.1 - 0.02) = 0.08, * 1.5 = 0.12, sqrt = 0.346
        // Below speech threshold (0.35), should be gated to 0.0
        assert_eq!(result, 0.0);
    }

    #[test]
    fn test_frequency_band_const_creation() {
        const BAND: FrequencyBand = FrequencyBand::new(500.0, 1000.0, BandType::Speech);

        assert_eq!(BAND.low_hz, 500.0);
        assert_eq!(BAND.high_hz, 1000.0);
        assert_eq!(BAND.band_type, BandType::Speech);
    }

    #[test]
    fn test_band_visualization_const_creation() {
        const VIZ: BandVisualization = BandVisualization::new(500.0, 1000.0, 1.5, BandType::Speech);

        assert_eq!(VIZ.band.low_hz, 500.0);
        assert_eq!(VIZ.band.high_hz, 1000.0);
        assert_eq!(VIZ.display_boost, 1.5);
        assert_eq!(VIZ.band.band_type, BandType::Speech);
    }

    #[test]
    fn test_speech_bands_constant() {
        // Verify SPEECH_BANDS is properly defined
        assert_eq!(SPEECH_BANDS.len(), 8);

        // Check first band is bass
        assert_eq!(SPEECH_BANDS[0].band.low_hz, 20.0);
        assert_eq!(SPEECH_BANDS[0].band.high_hz, 125.0);
        assert_eq!(SPEECH_BANDS[0].band.band_type, BandType::Bass);

        // Check last band is speech
        assert_eq!(SPEECH_BANDS[7].band.low_hz, 6000.0);
        assert_eq!(SPEECH_BANDS[7].band.high_hz, 8000.0);
        assert_eq!(SPEECH_BANDS[7].band.band_type, BandType::Speech);

        // Verify bands are contiguous (no gaps)
        for i in 0..SPEECH_BANDS.len() - 1 {
            assert_eq!(
                SPEECH_BANDS[i].band.high_hz,
                SPEECH_BANDS[i + 1].band.low_hz,
                "Gap between band {} and {}",
                i,
                i + 1
            );
        }

        // Verify first two are bass, rest are speech
        assert_eq!(SPEECH_BANDS[0].band.band_type, BandType::Bass);
        assert_eq!(SPEECH_BANDS[1].band.band_type, BandType::Bass);
        for band_viz in &SPEECH_BANDS[2..8] {
            assert_eq!(band_viz.band.band_type, BandType::Speech);
        }
    }

    #[test]
    fn test_spectrum_analyzer_creation() {
        let analyzer = SpectrumAnalyzer::new(16000);
        assert_eq!(analyzer.sample_rate, 16000);
    }

    #[test]
    fn test_push_sample_returns_none_until_full() {
        let mut analyzer = SpectrumAnalyzer::new(16000);

        // Push samples until just before FFT size
        for _ in 0..511 {
            assert!(analyzer.push_sample(0.0).is_none());
        }

        // Last sample should trigger FFT
        assert!(analyzer.push_sample(0.0).is_some());
    }

    #[test]
    fn test_silence_produces_zero_bands() {
        let mut analyzer = SpectrumAnalyzer::new(16000);

        // Push 512 silent samples
        for _ in 0..512 {
            let _ = analyzer.push_sample(0.0);
        }

        // Should get bands back (might be Some or None depending on when we check)
        // All bands should be 0.0 due to noise gate
        if let Some(bands) = analyzer.push_sample(0.0) {
            for band in bands {
                assert!(band < 0.01, "Expected near-zero for silence, got {}", band);
            }
        }
    }
}

// ============================================================================
// Public API
// ============================================================================

use crate::broadcast::BroadcastServer;
use crate::conf::SettingsState;
use crate::db::Database;
use directories::ProjectDirs;

/// Result of stopping a recording
pub struct RecordedAudio {
    pub buffer: Vec<i16>,
    pub path: std::path::PathBuf,
    pub sample_rate: u32,
}

/// Toggle recording state - the main entry point
/// 
/// - If idle: starts recording
/// - If recording: stops, transcribes, and delivers output
/// - If transcribing: returns busy
pub async fn toggle_recording(app: &AppHandle) -> Result<String> {
    let recording: tauri::State<RecordingState> = app.state();
    let snapshot = recording.snapshot().await;

    match snapshot {
        RecordingSnapshot::Idle => {
            start_recording(app).await?;
            Ok("started".into())
        }
        RecordingSnapshot::Recording => {
            let broadcast: tauri::State<BroadcastServer> = app.state();
            broadcast
                .recording_status(
                    RecordingSnapshot::Transcribing,
                    None,
                    false,
                    recording.elapsed_ms().await,
                )
                .await;

            let app_clone = app.clone();
            tokio::spawn(async move {
                if let Err(e) = complete_recording(&app_clone).await {
                    eprintln!("[toggle_recording] Failed to complete recording: {}", e);
                    
                    let recording: tauri::State<RecordingState> = app_clone.state();
                    let broadcast: tauri::State<BroadcastServer> = app_clone.state();
                    recording.finish_transcription().await;
                    broadcast
                        .recording_status(RecordingSnapshot::Error, None, false, 0)
                        .await;
                }
            });

            Ok("stopping".into())
        }
        RecordingSnapshot::Transcribing | RecordingSnapshot::Error => Ok("busy".into()),
    }
}

async fn start_recording(app: &AppHandle) -> Result<()> {
    let settings: tauri::State<SettingsState> = app.state();
    let recording: tauri::State<RecordingState> = app.state();
    let broadcast: tauri::State<BroadcastServer> = app.state();

    let settings_data = settings.get().await;
    let device_name = settings_data.audio_device.clone();
    let sample_rate = settings_data.sample_rate;

    let recorder = AudioRecorder::new_with_device(device_name.as_deref(), sample_rate)?;

    let audio_buffer = Arc::new(std::sync::Mutex::new(Vec::new()));
    let stop_signal = Arc::new(AtomicBool::new(false));

    let (spectrum_tx, mut spectrum_rx) = tokio::sync::mpsc::unbounded_channel();

    let stream = recorder.start_recording_background(
        audio_buffer.clone(),
        stop_signal.clone(),
        Some(spectrum_tx),
    )?;

    stream.play().map_err(|e| anyhow!("Failed to play stream: {}", e))?;

    // Spawn spectrum broadcaster
    let broadcast_clone = broadcast.inner().clone();
    let start_time = std::time::Instant::now();
    tokio::spawn(async move {
        while let Some(spectrum) = spectrum_rx.recv().await {
            let ts = start_time.elapsed().as_millis() as u64;
            broadcast_clone
                .recording_status(RecordingSnapshot::Recording, Some(spectrum), false, ts)
                .await;
        }
    });

    recording
        .start_recording(stream, audio_buffer, stop_signal)
        .await;

    eprintln!("[recording] Recording started");
    Ok(())
}

async fn stop_recording(app: &AppHandle) -> Result<RecordedAudio> {
    let recording: tauri::State<RecordingState> = app.state();

    let audio_buffer = recording
        .stop_recording()
        .await
        .ok_or_else(|| anyhow!("No active recording"))?;

    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    let buffer = audio_buffer.lock().unwrap().clone();

    if buffer.is_empty() {
        recording.finish_transcription().await;
        return Err(anyhow!("No audio recorded"));
    }

    eprintln!("[recording] Recorded {} samples", buffer.len());

    let recordings_dir = {
        let project_dirs = ProjectDirs::from("com", "dictate", "dictate")
            .ok_or_else(|| anyhow!("Failed to get project directories"))?;
        let dir = project_dirs.data_dir().join("recordings");
        tokio::fs::create_dir_all(&dir).await?;
        dir
    };

    let timestamp = jiff::Zoned::now().strftime("%Y-%m-%d_%H-%M-%S");
    let audio_path = recordings_dir.join(format!("{}.wav", timestamp));
    buffer_to_wav(&buffer, &audio_path, 16000)?;

    eprintln!("[recording] Audio saved to: {:?}", audio_path);

    Ok(RecordedAudio {
        buffer,
        path: audio_path,
        sample_rate: 16000,
    })
}

async fn complete_recording(app: &AppHandle) -> Result<()> {
    let recording: tauri::State<RecordingState> = app.state();
    let _settings: tauri::State<SettingsState> = app.state();
    let broadcast: tauri::State<BroadcastServer> = app.state();
    let _db = app.try_state::<Database>();

    // Step 1: Stop and get audio
    let recorded_audio = stop_recording(app).await?;

    // Step 2: Transcribe and deliver
    let transcription = crate::transcription::transcribe_and_deliver(
        &recorded_audio.path,
        &recorded_audio.buffer,
        recorded_audio.sample_rate,
        app,
    ).await?;

    // Step 3: Broadcast completion
    let duration_secs = transcription.duration_ms.unwrap_or(0) as f32 / 1000.0;
    let model = transcription
        .model_id
        .map(|id| format!("{:?}", id))
        .unwrap_or_else(|| "unknown".to_string());

    broadcast
        .transcription_result(transcription.text.clone(), duration_secs, model)
        .await;

    recording.finish_transcription().await;

    broadcast
        .recording_status(RecordingSnapshot::Idle, None, true, 0)
        .await;

    Ok(())
}
