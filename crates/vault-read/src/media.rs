#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct AudioFileMetadata {
    pub filename: String,
    pub content_type: String,
    pub size_bytes: u64,
    pub sha256: String,
}

/// Precomputed waveform channels keep playback from decoding a full recording in the renderer.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct AudioPeaks {
    pub duration_sec: f64,
    pub channels: Vec<Vec<f32>>,
}
