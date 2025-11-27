use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WhisperModel {
    Tiny,
    Base,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Model {
    Whisper(WhisperModel),
}

impl Serialize for Model {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("Model", 2)?;
        match self {
            Model::Whisper(variant) => {
                state.serialize_field("engine", "whisper")?;
                state.serialize_field("id", variant)?;
            }
        }
        state.end()
    }
}

#[derive(Debug, Serialize)]
pub struct ModelSize {
    #[serde(flatten)]
    pub model: Model,
    pub size_bytes: u64,
}

fn main() {
    let size = ModelSize {
        model: Model::Whisper(WhisperModel::Tiny),
        size_bytes: 1234567,
    };
    let json = serde_json::to_string_pretty(&size).unwrap();
    println!("{}", json);
}
