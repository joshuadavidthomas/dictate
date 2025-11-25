mod catalog;
mod storage;

pub use catalog::*;
pub use storage::*;

use serde::{Deserialize, Serialize};

/// Engine families for transcription models.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelEngine {
    Whisper,
    Parakeet,
}

/// Whisper model variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WhisperModel {
    Tiny,
    Base,
    Small,
    Medium,
}

/// Parakeet model variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ParakeetModel {
    V2,
    V3,
}

/// Global identifier for all supported models.
///
/// This encodes the invariant that every model belongs to exactly one engine
/// and to a finite set of variants within that engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "engine", content = "id", rename_all = "lowercase")]
pub enum ModelId {
    Whisper(WhisperModel),
    Parakeet(ParakeetModel),
}

impl ModelId {
    /// Returns the engine family for this model.
    pub fn engine(self) -> ModelEngine {
        match self {
            ModelId::Whisper(_) => ModelEngine::Whisper,
            ModelId::Parakeet(_) => ModelEngine::Parakeet,
        }
    }
}
