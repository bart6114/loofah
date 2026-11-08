use hypr_loofah_protocol::TranscriptOverview;

use crate::{SessionStore, StoreError};

impl SessionStore {
    pub fn indexed_transcript_overview(
        &self,
        session_id: &str,
        budget: Option<(usize, usize)>,
    ) -> Result<Option<TranscriptOverview>, StoreError> {
        crate::validate_session_id(session_id)?;
        self.ensure_ready()?;
        let index = self.index.read().unwrap();
        if !index.sessions.contains_key(session_id) {
            return Ok(None);
        }
        let Some(summary) = index.transcripts.get(session_id) else {
            return Ok(Some(TranscriptOverview::default()));
        };
        let metadata = &summary.metadata;
        if let Some((max_labels, max_bytes)) = budget {
            if metadata.speaker_labels.len() > max_labels {
                return Err(StoreError::ResourceLimit(
                    "Transcript overview has too many speaker labels".into(),
                ));
            }
            let mut remaining = max_bytes;
            for label in &metadata.speaker_labels {
                remaining = remaining.checked_sub(label.len()).ok_or_else(|| {
                    StoreError::ResourceLimit("Transcript overview is too large".into())
                })?;
            }
        }
        Ok(Some(TranscriptOverview {
            has_words: summary.has_words,
            speaker_labels: metadata.speaker_labels.clone(),
            started_at: metadata.started_at,
            ended_at: metadata.ended_at,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::SessionEntry;
    use hypr_loofah_protocol::{
        TRANSCRIPT_OVERVIEW_LABEL_LIMIT, TRANSCRIPT_OVERVIEW_RESPONSE_LIMIT,
    };

    fn fixture() -> (SessionStore, tempfile::TempDir) {
        let vault = tempfile::tempdir().unwrap();
        let store = SessionStore::new(vault.path().into());
        let meta = serde_json::from_value(serde_json::json!({"id":"s","title":"Title","created_at":"2026-11-08T00:00:00Z","tags":[]})).unwrap();
        store.index.write().unwrap().sessions.insert(
            "s".into(),
            SessionEntry {
                meta,
                note_markdown: Some("PRIVATE NOTE BODY".into()),
            },
        );
        (store, vault)
    }
    fn put(store: &SessionStore, bytes: &[u8]) {
        let summary = crate::transcript::summarize_transcript_bytes(bytes).unwrap();
        store
            .index
            .write()
            .unwrap()
            .transcripts
            .insert("s".into(), summary);
    }
    #[test]
    fn missing_session_and_no_transcript_are_distinct_without_reading_files() {
        let (store, vault) = fixture();
        assert_eq!(
            store.indexed_transcript_overview("missing", None).unwrap(),
            None
        );
        assert_eq!(
            store.indexed_transcript_overview("s", None).unwrap(),
            Some(TranscriptOverview::default())
        );
        std::fs::write(
            vault.path().join("transcript.json"),
            b"not JSON; never inspect stray files",
        )
        .unwrap();
        put(&store, br#"{"transcripts":[{"id":"late","started_at":30,"ended_at":40,"words":[{"ignored":"unhydrated"}],"speaker_hints":[{"type":"speaker_label","value":"Bob"}]},{"id":"b","started_at":10,"ended_at":50,"speaker_hints":[{"type":"speaker_label","value":"Alice"}]},{"id":"a","started_at":10,"speaker_hints":[{"type":"speaker_label","value":"Carol"},{"type":"speaker_label","value":"Alice"}]}]}"#);
        let overview = store
            .indexed_transcript_overview("s", None)
            .unwrap()
            .unwrap();
        assert_eq!(
            overview,
            TranscriptOverview {
                has_words: true,
                speaker_labels: vec!["Carol".into(), "Alice".into(), "Bob".into()],
                started_at: Some(10.0),
                ended_at: Some(50.0)
            }
        );
        assert_eq!(
            store.session_transcript_metadata("s").speaker_labels,
            overview.speaker_labels
        );
        assert_eq!(store.session_has_transcript("s"), overview.has_words);
        store.index.write().unwrap().sessions.remove("s");
        assert!(
            store
                .indexed_transcript_overview("s", None)
                .unwrap()
                .is_none()
        );
    }
    #[test]
    fn absent_explicit_end_is_not_inferred_from_words() {
        let (store, _vault) = fixture();
        put(
            &store,
            br#"{"transcripts":[{"id":"t","started_at":0,"words":[{"end_ms":90000}]}]}"#,
        );
        assert_eq!(
            store
                .indexed_transcript_overview("s", None)
                .unwrap()
                .unwrap()
                .ended_at,
            None
        );
    }
    #[test]
    fn remote_admission_bounds_count_and_utf8_before_cloning_and_never_limits_local_metadata() {
        let (store, _vault) = fixture();
        put(&store, br#"{"transcripts":[{"id":"t"}]}"#);
        let budget = Some((
            TRANSCRIPT_OVERVIEW_LABEL_LIMIT,
            TRANSCRIPT_OVERVIEW_RESPONSE_LIMIT,
        ));
        for labels in [
            (0..=TRANSCRIPT_OVERVIEW_LABEL_LIMIT)
                .map(|i| i.to_string())
                .collect(),
            vec!["é".repeat(TRANSCRIPT_OVERVIEW_RESPONSE_LIMIT / 2 + 1)],
        ] {
            store
                .index
                .write()
                .unwrap()
                .transcripts
                .get_mut("s")
                .unwrap()
                .metadata
                .speaker_labels = labels;
            assert!(matches!(
                store.indexed_transcript_overview("s", budget),
                Err(StoreError::ResourceLimit(_))
            ));
            assert!(
                !store
                    .indexed_transcript_overview("s", None)
                    .unwrap()
                    .unwrap()
                    .speaker_labels
                    .is_empty()
            );
        }
        assert!(store.indexed_transcript_overview("../s", None).is_err());
    }
}
