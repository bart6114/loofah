use crate::{
    CaptureProcessingSummary, LocalMediaSourceSnapshot, LocalMediaStatus, MetadataSnapshot,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct MediaSessionSnapshot {
    pub metadata: MetadataSnapshot,
    pub transcript_revision: String,
    pub transcript_present: bool,
    pub recording: Option<LocalMediaSourceSnapshot>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct MediaUploadSource {
    pub sha256: String,
    pub size_bytes: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct MediaTranscriptionOptions {
    pub model: String,
    pub languages: Vec<String>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, specta::Type)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MediaOperation {
    ImportAudio {
        session_id: Option<String>,
        expected: Option<MediaSessionSnapshot>,
        filename: String,
        content_type: Option<String>,
        created_at: Option<String>,
        transcribe: bool,
        source: MediaUploadSource,
    },
    Transcribe {
        expected: LocalMediaSourceSnapshot,
    },
    ImportSubtitles {
        session_id: String,
        expected: MediaSessionSnapshot,
        filename: String,
        source: MediaUploadSource,
    },
}
impl MediaOperation {
    pub fn session_id(&self) -> Option<&str> {
        match self {
            Self::ImportAudio { session_id, .. } => session_id.as_deref(),
            Self::Transcribe { expected } => Some(&expected.session_id),
            Self::ImportSubtitles { session_id, .. } => Some(session_id),
        }
    }
    pub fn upload(&self) -> Option<(&str, &MediaUploadSource)> {
        match self {
            Self::ImportAudio {
                filename, source, ..
            }
            | Self::ImportSubtitles {
                filename, source, ..
            } => Some((filename, source)),
            Self::Transcribe { .. } => None,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct MediaDeclaration {
    pub request_id: String,
    pub operation: MediaOperation,
    pub transcription: Option<MediaTranscriptionOptions>,
    pub summary: Option<CaptureProcessingSummary>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum MediaPhase {
    Receiving,
    Ready,
    Running,
    Review,
    Succeeded,
    Failed,
    Cancelled,
}
#[derive(Clone, Debug, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct MediaRequestStatus {
    pub declaration: MediaDeclaration,
    pub revision: String,
    pub phase: MediaPhase,
    pub media: Option<LocalMediaStatus>,
}
#[derive(Clone, Debug, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct MediaAdmission {
    pub declaration: MediaDeclaration,
    pub revision: String,
    pub status: MediaRequestStatus,
}
pub fn validate_media_declaration(input: &MediaDeclaration) -> Result<(), &'static str> {
    if !canonical_uuid(&input.request_id) {
        return Err("Canonical media request UUID required");
    }
    let transcribe = match &input.operation {
        MediaOperation::ImportAudio {
            session_id,
            expected,
            filename,
            content_type,
            created_at,
            transcribe,
            source,
        } => {
            match (session_id, expected) {
                (None, None) => {}
                (Some(id), Some(expected)) if id == &expected.metadata.meta.id => {
                    validate_media_session_snapshot(expected)?
                }
                _ => return Err("Original import session baseline required"),
            }
            validate_upload(filename, source)?;
            if content_type.as_ref().is_some_and(|s| !bounded(s, 256))
                || created_at.as_ref().is_some_and(|s| !bounded(s, 128))
            {
                return Err("Invalid media metadata hints");
            }
            *transcribe
        }
        MediaOperation::Transcribe { expected } => {
            validate_recording(expected)?;
            true
        }
        MediaOperation::ImportSubtitles {
            session_id,
            expected,
            filename,
            source,
        } => {
            validate_media_session_snapshot(expected)?;
            if session_id != &expected.metadata.meta.id {
                return Err("Original subtitle session baseline required");
            }
            validate_upload(filename, source)?;
            let extension = filename.rsplit('.').next().unwrap_or_default();
            if !(extension.eq_ignore_ascii_case("srt") || extension.eq_ignore_ascii_case("vtt"))
                || source.size_bytes > 32 * 1024 * 1024
            {
                return Err("Invalid or oversized subtitle source");
            }
            false
        }
    };
    if transcribe != input.transcription.is_some() {
        return Err("Freeze transcription options before media admission");
    }
    if let Some(options) = &input.transcription {
        if !bounded(&options.model, 512)
            || options.languages.len() > 16
            || options.languages.iter().any(|s| !bounded(s, 64))
        {
            return Err("Invalid media transcription options");
        }
    }
    if let Some(CaptureProcessingSummary {
        target: crate::DocumentTarget::TemplateOutput { document_id },
        ..
    }) = &input.summary
    {
        crate::validate_session_id(document_id).map_err(|_| "Invalid media summary target")?;
    }
    Ok(())
}
pub fn validate_media_session_snapshot(value: &MediaSessionSnapshot) -> Result<(), &'static str> {
    crate::validate_session_id(&value.metadata.meta.id).map_err(|_| "Invalid media session")?;
    if !crate::is_transcription_sha256(&value.metadata.revision)
        || !crate::is_transcription_sha256(&value.transcript_revision)
        || !bounded(&value.metadata.meta.created_at, 128)
    {
        return Err("Invalid media source revision");
    }
    if let Some(recording) = &value.recording {
        validate_recording(recording)?;
        if recording.session_id != value.metadata.meta.id
            || recording.session_created_at != value.metadata.meta.created_at
            || recording.transcript_revision != value.transcript_revision
            || recording.transcript_present != value.transcript_present
        {
            return Err("Media source coordinates disagree");
        }
    }
    Ok(())
}
fn validate_recording(value: &LocalMediaSourceSnapshot) -> Result<(), &'static str> {
    crate::validate_session_id(&value.session_id).map_err(|_| "Invalid media session")?;
    if !bounded(&value.session_created_at, 128)
        || !crate::is_transcription_sha256(&value.recording_sha256)
        || !crate::is_transcription_sha256(&value.transcript_revision)
        || !matches!(
            value.recording_selection.as_str(),
            "audio.mp3"
                | "audio.wav"
                | "audio.ogg"
                | "audio/audio.mp3"
                | "audio/audio.wav"
                | "audio/audio.ogg"
        )
    {
        return Err("Invalid original recording baseline");
    }
    Ok(())
}
fn validate_upload(filename: &str, source: &MediaUploadSource) -> Result<(), &'static str> {
    crate::validate_upload_attachment_request(&crate::UploadAttachmentRequest {
        filename: filename.into(),
        sha256: source.sha256.clone(),
        size_bytes: source.size_bytes,
    })
}
fn bounded(value: &str, limit: usize) -> bool {
    !value.is_empty()
        && value.len() <= limit
        && value.trim() == value
        && !value.chars().any(char::is_control)
}
fn canonical_uuid(value: &str) -> bool {
    value.len() == 36
        && value != "00000000-0000-0000-0000-000000000000"
        && value.bytes().enumerate().all(|(i, c)| {
            if [8, 13, 18, 23].contains(&i) {
                c == b'-'
            } else {
                c.is_ascii_digit() || (b'a'..=b'f').contains(&c)
            }
        })
}
