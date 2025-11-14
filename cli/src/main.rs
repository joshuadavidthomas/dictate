use anyhow::{Result, anyhow};
use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Lightweight CLI voice transcription for Linux
#[derive(Parser)]
#[command(name = "dictate")]
#[command(about = "Voice transcription CLI for dictate Tauri app")]
#[command(version = "0.1.0")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Toggle recording (start if idle, stop if recording)
    Toggle {
        /// Tauri app HTTP port
        #[arg(long, default_value = "7777")]
        port: u16,
    },
    
    /// Start recording
    Start {
        /// Tauri app HTTP port
        #[arg(long, default_value = "7777")]
        port: u16,
    },
    
    /// Stop current recording
    Stop {
        /// Tauri app HTTP port
        #[arg(long, default_value = "7777")]
        port: u16,
    },
    
    /// Get current status
    Status {
        /// Tauri app HTTP port
        #[arg(long, default_value = "7777")]
        port: u16,
    },
}

#[derive(Serialize, Deserialize, Debug)]
struct StatusResponse {
    status: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct CommandResponse {
    success: bool,
    message: String,
}

/// Check if Tauri app is running
async fn is_tauri_running(port: u16) -> bool {
    let client = reqwest::Client::new();
    let url = format!("http://127.0.0.1:{}/health", port);
    
    client
        .get(&url)
        .timeout(Duration::from_millis(500))
        .send()
        .await
        .is_ok()
}

/// Start Tauri app in background if not running
async fn ensure_tauri_running(port: u16) -> Result<()> {
    if is_tauri_running(port).await {
        return Ok(());
    }
    
    eprintln!("Starting dictate app...");
    
    // Try to start the Tauri app
    // Look for the binary in common locations
    let possible_paths = vec![
        "/usr/bin/dictate",
        "/usr/local/bin/dictate",
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|p| p.join("../dictate")))
            .and_then(|p| p.canonicalize().ok())
            .as_ref()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default(),
    ];
    
    for path in possible_paths {
        if std::path::Path::new(&path).exists() {
            std::process::Command::new(&path)
                .arg("--headless")
                .spawn()
                .ok();
            
            // Wait for app to start
            for _ in 0..20 {
                tokio::time::sleep(Duration::from_millis(250)).await;
                if is_tauri_running(port).await {
                    eprintln!("Dictate app started");
                    return Ok(());
                }
            }
        }
    }
    
    Err(anyhow!(
        "Could not start dictate app. Please start it manually:\n  dictate (GUI)\nor install it to /usr/bin/dictate"
    ))
}

/// Send a command to the Tauri app
async fn send_command(port: u16, endpoint: &str) -> Result<()> {
    let client = reqwest::Client::new();
    let url = format!("http://127.0.0.1:{}/{}", port, endpoint);
    
    let response = client
        .post(&url)
        .timeout(Duration::from_secs(5))
        .send()
        .await?;
    
    if response.status().is_success() {
        Ok(())
    } else {
        Err(anyhow!("Command failed: {}", response.status()))
    }
}

/// Get status from Tauri app
async fn get_status(port: u16) -> Result<String> {
    let client = reqwest::Client::new();
    let url = format!("http://127.0.0.1:{}/status", port);
    
    let response = client
        .get(&url)
        .timeout(Duration::from_secs(2))
        .send()
        .await?
        .json::<StatusResponse>()
        .await?;
    
    Ok(response.status)
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    
    match cli.command {
        Commands::Toggle { port } => {
            ensure_tauri_running(port).await?;
            send_command(port, "toggle").await?;
        }
        
        Commands::Start { port } => {
            ensure_tauri_running(port).await?;
            send_command(port, "start").await?;
        }
        
        Commands::Stop { port } => {
            ensure_tauri_running(port).await?;
            send_command(port, "stop").await?;
        }
        
        Commands::Status { port } => {
            if !is_tauri_running(port).await {
                println!("Dictate app is not running");
                return Ok(());
            }
            
            let status = get_status(port).await?;
            println!("Status: {}", status);
        }
    }
    
    Ok(())
}
