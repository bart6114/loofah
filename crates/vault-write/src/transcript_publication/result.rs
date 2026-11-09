use super::*;

impl SessionStore {
    /// Reads only the original retained result, without inspecting or adopting current content.
    pub async fn retained_transcript_publication_result(
        &self,
        operations: &Path,
        identity: TranscriptPublicationIdentity,
    ) -> Result<Option<TranscriptWithData>, StoreError> {
        identity.validate()?;
        if !operations.is_absolute() {
            return Err(StoreError::Serialize(
                "Transcript operation path must be absolute".into(),
            ));
        }
        let root = operations.to_owned();
        self.spawn_blocking(move || {
            let (receipt_path, result_path) = io::paths(&root, &identity);
            let Some(receipt) = io::read_receipt(&receipt_path, &identity)? else {
                return Ok(None);
            };
            let limit = result_limit(receipt.binding.replacement).min(64 * 1024 * 1024);
            let bytes = io::read(&result_path, limit)?.ok_or_else(|| {
                StoreError::Conflict("Retained transcript result disappeared".into())
            })?;
            if crate::sha256(&bytes) != receipt.result_sha256 {
                return Err(StoreError::Conflict(
                    "Retained transcript result changed".into(),
                ));
            }
            let raw = serde_json::from_slice::<crate::metadata_revision::UniqueValue>(&bytes)
                .map_err(io::serialization)?
                .0;
            let file: TranscriptJson = serde_json::from_value(raw).map_err(io::serialization)?;
            if (receipt.binding.subtitle.is_none() && file.transcripts.len() != 1)
                || file
                    .transcripts
                    .iter()
                    .any(|transcript| transcript.session_id != receipt.binding.session_id)
            {
                return Err(StoreError::Conflict(
                    "Retained transcript source identity changed".into(),
                ));
            }
            let mut matches = file
                .transcripts
                .into_iter()
                .filter(|transcript| transcript.id == receipt.transcript_id);
            let transcript = matches.next().ok_or_else(|| {
                StoreError::Conflict("Retained transcript identity changed".into())
            })?;
            if matches.next().is_some() {
                return Err(StoreError::Conflict(
                    "Duplicate retained transcript identity".into(),
                ));
            }
            validate_generated(&transcript, true)?;
            Ok(Some(transcript))
        })
        .await
        .map_err(io::error)?
    }
}
