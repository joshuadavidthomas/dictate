use std::sync::Arc;
use std::sync::Mutex;

use crate::spectrum::SPECTRUM_BANDS;

#[derive(Clone, Debug)]
pub struct DictationState {
    spectrum: SpectrumLevels,
}

impl DictationState {
    pub fn initial() -> Self {
        Self {
            spectrum: SpectrumLevels::new(),
        }
    }

    pub fn spectrum(&self) -> SpectrumLevels {
        self.spectrum.clone()
    }

    pub fn spectrum_bands(&self) -> [f32; SPECTRUM_BANDS] {
        self.spectrum.bands()
    }
}

#[derive(Clone, Debug)]
pub struct SpectrumLevels {
    bands: Arc<Mutex<[f32; SPECTRUM_BANDS]>>,
}

impl SpectrumLevels {
    fn new() -> Self {
        Self {
            bands: Arc::new(Mutex::new([0.0; SPECTRUM_BANDS])),
        }
    }

    pub fn set(&self, bands: [f32; SPECTRUM_BANDS]) {
        *self.bands.lock().unwrap() = bands;
    }

    pub fn bands(&self) -> [f32; SPECTRUM_BANDS] {
        *self.bands.lock().unwrap()
    }
}
