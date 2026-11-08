use crate::vault::error;
use hypr_loofah_protocol::{ApplicationError, ErrorCode, TranscriptWithData};
use serde_json::{Value, json};

/// Existing JSON stays lossless at every unknown key. Empty suffixes return the
/// original bytes, including the distinction between absence and an empty file.
pub fn merge_transcript(
    original: Option<&[u8]>,
    mut suffix: TranscriptWithData,
    session_id: &str,
    prefix_frames: u64,
    suffix_frames: u64,
) -> Result<Option<Vec<u8>>, ApplicationError> {
    if suffix.session_id != session_id
        || suffix.id.is_empty()
        || !suffix.started_at.is_finite()
        || suffix.ended_at.is_some()
        || prefix_frames == 0
        || prefix_frames > u32::MAX as u64
        || suffix_frames == 0
        || suffix_frames > u32::MAX as u64
    {
        return Err(invalid());
    }
    if suffix.words.is_empty() {
        return Ok(original.map(<[u8]>::to_vec));
    }
    let mut value: Value = match original {
        Some(bytes) => serde_json::from_slice(bytes).map_err(|_| invalid())?,
        None => json!({"transcripts": []}),
    };
    // Deserialize for known schema validation, while retaining the raw tree for
    // serialization. Rebuilding old records from typed structs loses user data.
    let existing: hypr_vault_read::TranscriptJson =
        serde_json::from_value(value.clone()).map_err(|_| invalid())?;
    if existing.transcripts.iter().any(|entry| {
        entry.id == suffix.id || entry.session_id != session_id || !entry.started_at.is_finite()
    }) {
        return Err(invalid());
    }
    let base = existing
        .transcripts
        .iter()
        .map(|entry| entry.started_at)
        .min_by(f64::total_cmp)
        .unwrap_or(suffix.started_at);
    let offset = prefix_frames as f64 / 16.0;
    let duration = suffix_frames as f64 / 16.0;
    for word in &mut suffix.words {
        if !word.start_ms.is_finite()
            || !word.end_ms.is_finite()
            || word.start_ms < 0.0
            || word.end_ms < word.start_ms
            || word.end_ms > duration
            || !word.channel.is_finite()
        {
            return Err(invalid());
        }
        word.start_ms += offset;
        word.end_ms += offset;
    }
    // All word timestamps share the oldest original clock, avoiding a second
    // renderer offset while retaining the exact fractional-millisecond frame.
    suffix.started_at = base;
    let object = value.as_object_mut().ok_or_else(invalid)?;
    let transcripts = object.entry("transcripts").or_insert_with(|| json!([]));
    if transcripts.is_null() {
        *transcripts = json!([]);
    }
    transcripts
        .as_array_mut()
        .ok_or_else(invalid)?
        .push(serde_json::to_value(suffix).map_err(|_| invalid())?);
    serde_json::to_vec(&value).map(Some).map_err(|_| invalid())
}

fn invalid() -> ApplicationError {
    error(
        ErrorCode::Conflict,
        "Append transcript cannot be merged with its original baseline",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn suffix() -> TranscriptWithData {
        serde_json::from_value(json!({"id":"suffix","session_id":"s","started_at":9000,
            "words":[{"id":"word","text":"New","start_ms":0,"end_ms":10,"channel":0,"metadata":{"future":true}}],
            "speaker_hints":[{"word_id":"word","type":"speaker_label","value":"Speaker"}]})).unwrap()
    }
    #[test]
    fn preserves_unknown_original_fields_and_exact_frame_offset_without_clock_gap() {
        let original = json!({"future":{"version":42},"transcripts":[{"id":"original","session_id":"s","started_at":1000,
            "unknown":"keep","words":[{"text":"Old","start_ms":1,"end_ms":2,"channel":0,"future":[42]}],
            "speaker_hints":[{"word_id":"old","type":"future","value":{"opaque":true},"unknown":7}]}]});
        let merged = merge_transcript(
            Some(&serde_json::to_vec(&original).unwrap()),
            suffix(),
            "s",
            16001,
            160,
        )
        .unwrap()
        .unwrap();
        let merged: Value = serde_json::from_slice(&merged).unwrap();
        assert_eq!(merged["future"], original["future"]);
        assert_eq!(merged["transcripts"][0], original["transcripts"][0]);
        assert_eq!(merged["transcripts"][1]["started_at"], 1000.0);
        assert_eq!(merged["transcripts"][1]["words"][0]["start_ms"], 1000.0625);
        assert_eq!(merged["transcripts"][1]["words"][0]["end_ms"], 1010.0625);
        assert_eq!(
            merged["transcripts"][1]["words"][0]["metadata"]["future"],
            true
        );
    }
    #[test]
    fn empty_suffix_preserves_exact_bytes_and_absence() {
        let mut empty = suffix();
        empty.words.clear();
        for original in [
            None,
            Some(&b" {\"transcripts\":null,\"future\":true} \n"[..]),
            Some(&b""[..]),
        ] {
            assert_eq!(
                merge_transcript(original, empty.clone(), "s", 1, 1).unwrap(),
                original.map(<[u8]>::to_vec)
            );
        }
    }
    #[test]
    fn rejects_malformed_original_duplicate_identity_and_out_of_suffix_range() {
        assert!(merge_transcript(Some(b""), suffix(), "s", 1, 160).is_err());
        let original = serde_json::to_vec(&json!({"transcripts":[suffix()]})).unwrap();
        assert!(merge_transcript(Some(&original), suffix(), "s", 1, 160).is_err());
        assert!(merge_transcript(None, suffix(), "s", 1, 159).is_err());
    }
}
