#[derive(
    Debug, Default, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, specta::Type,
)]
#[serde(rename_all = "snake_case")]
pub enum AudioSource {
    Import,
    Recording,
    #[default]
    Unknown,
}

#[derive(
    Debug, Default, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, specta::Type,
)]
#[serde(rename_all = "snake_case")]
pub enum AudioLayout {
    #[default]
    Mixed,
    MicSystem,
}

#[derive(
    Debug, Default, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, specta::Type,
)]
pub struct SessionAudio {
    pub source: AudioSource,
    pub layout: AudioLayout,
}
