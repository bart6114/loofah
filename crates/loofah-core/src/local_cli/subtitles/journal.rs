use super::*;
use aspasia::{Subtitle, TimedSubtitleFile, WebVttSubtitle};
use std::io::{Read, Seek, SeekFrom};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ParsedReceipt {
    intent_sha256: String,
    result_sha256: String,
    word_count: usize,
}

pub(super) fn load(directory: &Path) -> Result<Option<Intent>, ApplicationError> {
    if !ledger::existing_directory(directory)? {
        return Ok(None);
    }
    let path = directory.join("intent.json");
    if !ledger::check_file(&path)? {
        return Ok(None);
    }
    let intent: Intent =
        serde_json::from_slice(&ledger::read(&path, 17 * 1024 * 1024)?).map_err(unknown)?;
    validate(&intent)?;
    Ok(Some(intent))
}
fn validate(intent: &Intent) -> Result<(), ApplicationError> {
    canonical_uuid(&intent.request_id)?;
    canonical_uuid(&intent.transcript_id)?;
    validate_filename(&intent.filename)?;
    if intent.version != 1
        || intent
            .principal
            .as_ref()
            .is_some_and(|p| p.is_empty() || p.len() > 512 || p.chars().any(char::is_control))
        || intent.source_bytes > SOURCE_LIMIT
        || !is_transcription_sha256(&intent.source_sha256)
        || !is_transcription_sha256(&intent.transcript_revision)
        || chrono::DateTime::parse_from_rfc3339(&intent.session_created_at).is_err()
        || chrono::DateTime::parse_from_rfc3339(&intent.created_at).is_err()
        || !intent.started_at.is_finite()
        || intent.started_at < 0.0
        || intent.memo.len() > 16 * 1024 * 1024
    {
        return Err(unknown("Original subtitle declaration requires review"));
    }
    Ok(())
}
fn intent_bytes(intent: &Intent) -> Result<Vec<u8>, ApplicationError> {
    validate(intent)?;
    ledger::encode_bounded_result(intent, 17 * 1024 * 1024)
}
pub(super) fn prepare(
    directory: &Path,
    intent: &Intent,
    artifact: Option<LocalCliArtifact>,
    progress: &dyn Fn(LocalCliProgress),
) -> Result<Parsed, ApplicationError> {
    ledger::ensure_directory(directory)?;
    let bytes = intent_bytes(intent)?;
    let intent_path = directory.join("intent.json");
    if ledger::check_file(&intent_path)? {
        if ledger::read(&intent_path, 17 * 1024 * 1024)? != bytes {
            return Err(unknown("Original subtitle declaration changed"));
        }
    } else {
        ledger::write_new(&intent_path, &bytes)?;
    }
    progress(LocalCliProgress {
        schema_version: "2".into(),
        phase: "preparing media".into(),
        processed: 0,
        total: None,
        added: 0,
        updated: 0,
        deleted: 0,
        elapsed_ms: 0,
    });
    let result_path = directory.join("parsed.json");
    if ledger::check_file(&result_path)? {
        return read_parsed(directory, intent);
    }
    let extension = Path::new(&intent.filename)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap()
        .to_ascii_lowercase();
    let source_path = directory.join(format!("source.{extension}"));
    let source = if ledger::check_file(&source_path)? {
        ledger::read(&source_path, SOURCE_LIMIT)?
    } else {
        let mut artifact = artifact.ok_or_else(|| {
            unknown("Original subtitle source was not retained; request requires review")
        })?;
        artifact.seek(SeekFrom::Start(0)).map_err(unknown)?;
        let mut bytes = Vec::new();
        (&mut artifact)
            .take(SOURCE_LIMIT + 1)
            .read_to_end(&mut bytes)
            .map_err(unknown)?;
        if bytes.len() as u64 != intent.source_bytes || digest(&bytes) != intent.source_sha256 {
            return Err(error(
                ErrorCode::Conflict,
                "Subtitle source changed before retention",
            ));
        }
        ledger::write_new(&source_path, &bytes)?;
        bytes
    };
    if source.len() as u64 != intent.source_bytes || digest(&source) != intent.source_sha256 {
        return Err(unknown("Retained subtitle source changed"));
    }
    let subtitle = TimedSubtitleFile::new(&source_path).map_err(|cause| {
        error(
            ErrorCode::Validation,
            &format!("Cannot parse subtitles: {cause}"),
        )
    })?;
    if digest(&ledger::read(&source_path, SOURCE_LIMIT)?) != intent.source_sha256 {
        return Err(unknown("Retained subtitle source changed while parsing"));
    }
    let vtt: WebVttSubtitle = subtitle.into();
    if vtt.events().len() > 250_000 {
        return Err(error(
            ErrorCode::Validation,
            "Subtitle file has too many cues",
        ));
    }
    let mut words = Vec::with_capacity(vtt.events().len());
    for cue in vtt.events() {
        let start = i64::from(cue.start);
        let end = i64::from(cue.end);
        if start < 0 || end < start {
            return Err(error(
                ErrorCode::Validation,
                "Subtitle cue timing is invalid",
            ));
        }
        words.push(serde_json::json!({"id":uuid::Uuid::new_v4().to_string(),"text":cue.text,"start_ms":start,"end_ms":end,"channel":2}));
    }
    let transcript:TranscriptWithData=serde_json::from_value(serde_json::json!({
        "id":intent.transcript_id,"session_id":intent.session_id,"user_id":intent.principal.clone().unwrap_or_else(|| uuid::Uuid::nil().to_string()),
        "created_at":intent.created_at,"started_at":intent.started_at,"ended_at":null,
        "memo_md":intent.memo,"words":words,"speaker_hints":[]
    })).map_err(unknown)?;
    ledger::validate_result(&transcript)?;
    let parsed = Parsed {
        intent_sha256: digest(&bytes),
        transcript,
    };
    let bytes = ledger::encode_bounded_result(&parsed, RESULT_LIMIT)?;
    ledger::write_new(&result_path, &bytes)?;
    let receipt = ParsedReceipt {
        intent_sha256: parsed.intent_sha256,
        result_sha256: digest(&bytes),
        word_count: parsed.transcript.words.len(),
    };
    ledger::write_new(
        &directory.join("parsed-receipt.json"),
        &ledger::encode_bounded_result(&receipt, 1024)?,
    )?;
    read_parsed(directory, intent)
}
pub(super) fn read_parsed(directory: &Path, intent: &Intent) -> Result<Parsed, ApplicationError> {
    let bytes = ledger::read(&directory.join("parsed.json"), RESULT_LIMIT as u64)?;
    let receipt: ParsedReceipt =
        serde_json::from_slice(&ledger::read(&directory.join("parsed-receipt.json"), 1024)?)
            .map_err(unknown)?;
    if receipt.intent_sha256 != digest(&intent_bytes(intent)?)
        || receipt.result_sha256 != digest(&bytes)
    {
        return Err(unknown("Retained subtitle result digest changed"));
    }
    let parsed: Parsed = serde_json::from_slice(&bytes).map_err(unknown)?;
    let transcript = &parsed.transcript;
    if parsed.intent_sha256 != digest(&intent_bytes(intent)?)
        || transcript.id != intent.transcript_id
        || transcript.session_id != intent.session_id
        || transcript.created_at != intent.created_at
        || transcript.started_at != intent.started_at
        || transcript.memo_md != intent.memo
        || transcript.user_id
            != intent
                .principal
                .clone()
                .unwrap_or_else(|| uuid::Uuid::nil().to_string())
    {
        return Err(unknown("Retained subtitle result identity changed"));
    }
    ledger::validate_result(transcript)?;
    if transcript.words.len() != receipt.word_count {
        return Err(unknown("Retained subtitle word count changed"));
    }
    Ok(parsed)
}

pub(super) fn parsed_word_count(
    directory: &Path,
    intent: &Intent,
) -> Result<usize, ApplicationError> {
    let receipt: ParsedReceipt =
        serde_json::from_slice(&ledger::read(&directory.join("parsed-receipt.json"), 1024)?)
            .map_err(unknown)?;
    if receipt.intent_sha256 != digest(&intent_bytes(intent)?)
        || !is_transcription_sha256(&receipt.result_sha256)
        || receipt.word_count > 250_000
    {
        return Err(unknown("Retained subtitle receipt changed"));
    }
    Ok(receipt.word_count)
}
