use super::{ModelId, ParakeetModel, WhisperModel};

/// Static metadata for a single model.
///
/// Contains all immutable properties needed to locate, download, and identify
/// a model on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelDescriptor {
    pub id: ModelId,
    pub storage_name: &'static str,
    pub is_directory: bool,
    pub download_url: &'static str,
}

/// All supported models in the catalog.
const ALL_MODELS: &[ModelDescriptor] = &[
    ModelDescriptor {
        id: ModelId::Whisper(WhisperModel::Tiny),
        storage_name: "whisper-tiny",
        is_directory: false,
        download_url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.bin",
    },
    ModelDescriptor {
        id: ModelId::Whisper(WhisperModel::Base),
        storage_name: "whisper-base",
        is_directory: false,
        download_url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin",
    },
    ModelDescriptor {
        id: ModelId::Whisper(WhisperModel::Small),
        storage_name: "whisper-small",
        is_directory: false,
        download_url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin",
    },
    ModelDescriptor {
        id: ModelId::Whisper(WhisperModel::Medium),
        storage_name: "whisper-medium",
        is_directory: false,
        download_url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-medium.bin",
    },
    ModelDescriptor {
        id: ModelId::Parakeet(ParakeetModel::V2),
        storage_name: "parakeet-v2",
        is_directory: true,
        download_url: "https://blob.handy.computer/parakeet-v2-int8.tar.gz",
    },
    ModelDescriptor {
        id: ModelId::Parakeet(ParakeetModel::V3),
        storage_name: "parakeet-v3",
        is_directory: true,
        download_url: "https://blob.handy.computer/parakeet-v3-int8.tar.gz",
    },
];

/// Returns the complete catalog of all supported models.
pub fn all_models() -> &'static [ModelDescriptor] {
    ALL_MODELS
}

/// Looks up a model descriptor by ID.
///
/// Returns `None` if the model is not in the catalog.
pub fn find(id: ModelId) -> Option<&'static ModelDescriptor> {
    ALL_MODELS.iter().find(|desc| desc.id == id)
}

/// Resolves the preferred model or falls back to defaults.
///
/// Fallback order:
/// 1. Preferred model (if provided and exists in catalog)
/// 2. Parakeet V3
/// 3. Whisper Base
pub fn preferred_or_default(pref: Option<ModelId>) -> &'static ModelDescriptor {
    // Try preferred model first
    if let Some(pref_id) = pref
        && let Some(desc) = find(pref_id)
    {
        return desc;
    }

    // Fall back to Parakeet V3
    if let Some(desc) = find(ModelId::Parakeet(ParakeetModel::V3)) {
        return desc;
    }

    // Final fallback to Whisper Base
    find(ModelId::Whisper(WhisperModel::Base))
        .expect("Whisper Base must exist in catalog as final fallback")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_all_models_count() {
        assert_eq!(all_models().len(), 6);
    }

    #[test]
    fn test_find_existing_model() {
        let desc = find(ModelId::Whisper(WhisperModel::Base));
        assert!(desc.is_some());
        assert_eq!(desc.unwrap().storage_name, "whisper-base");
    }

    #[test]
    fn test_preferred_or_default_with_valid_preference() {
        let desc = preferred_or_default(Some(ModelId::Whisper(WhisperModel::Small)));
        assert_eq!(desc.id, ModelId::Whisper(WhisperModel::Small));
    }

    #[test]
    fn test_preferred_or_default_fallback_to_parakeet_v3() {
        let desc = preferred_or_default(None);
        assert_eq!(desc.id, ModelId::Parakeet(ParakeetModel::V3));
    }

    #[test]
    fn test_whisper_models_are_files() {
        for desc in all_models() {
            if matches!(desc.id, ModelId::Whisper(_)) {
                assert!(!desc.is_directory, "{:?} should be a file", desc.id);
            }
        }
    }

    #[test]
    fn test_parakeet_models_are_directories() {
        for desc in all_models() {
            if matches!(desc.id, ModelId::Parakeet(_)) {
                assert!(desc.is_directory, "{:?} should be a directory", desc.id);
            }
        }
    }

    #[test]
    fn test_all_models_have_download_urls() {
        for desc in all_models() {
            assert!(!desc.download_url.is_empty(), "{:?} missing download URL", desc.id);
        }
    }
}
