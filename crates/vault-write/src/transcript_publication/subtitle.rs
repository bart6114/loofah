use super::*;

pub struct SubtitleSourceSnapshot {
    baseline: TranscriptPublicationBaseline,
    raw: Option<Vec<u8>>,
}
impl SubtitleSourceSnapshot {
    pub fn transcript_revision(&self) -> &str {
        &self.baseline.0.binding.transcript_revision
    }
    pub fn transcript_present(&self) -> bool {
        self.baseline.0.binding.transcript_present
    }
    pub fn session_created_at(&self) -> &str {
        &self
            .baseline
            .0
            .binding
            .subtitle
            .as_ref()
            .expect("subtitle source")
            .session_created_at
    }
}
pub enum SubtitleSourcePreparation {
    Ready(SubtitleSourceSnapshot),
    Busy,
}
impl SessionStore {
    pub async fn completed_subtitle_publication(
        &self,
        operations: &Path,
        identity: TranscriptPublicationIdentity,
    ) -> Result<Option<TranscriptPublicationOutcome>, StoreError> {
        identity.validate()?;
        if !operations.is_absolute() {
            return Err(StoreError::Serialize(
                "Subtitle operation path must be absolute".into(),
            ));
        }
        let path = io::paths(operations, &identity).0;
        self.spawn_blocking(move || {
            let Some(receipt) = io::read_receipt(&path, &identity)? else {
                return Ok(None);
            };
            if receipt.binding.subtitle.is_none() {
                return Err(StoreError::Conflict(
                    "Publication is not a subtitle import".into(),
                ));
            }
            Ok(match receipt.state {
                State::Committed => Some(TranscriptPublicationOutcome::AlreadyCommitted {
                    revision: receipt.result_sha256,
                    transcript_id: receipt.transcript_id,
                }),
                State::Empty => Some(TranscriptPublicationOutcome::Empty),
                State::Conflict => Some(TranscriptPublicationOutcome::Conflict),
                State::Review => Some(TranscriptPublicationOutcome::RequiresReview),
                _ => None,
            })
        })
        .await
        .map_err(io::error)?
    }
    pub async fn prepare_subtitle_source(
        &self,
        id: &str,
        source_sha256: String,
    ) -> Result<Option<SubtitleSourcePreparation>, StoreError> {
        crate::validate_session_id(id)?;
        if !io::valid_hash(&source_sha256) {
            return Err(StoreError::Serialize(
                "Invalid subtitle source digest".into(),
            ));
        }
        let store = self.clone();
        let id = id.to_owned();
        tokio::spawn(async move {
            let audio = match store.lock_session_audio(&id).await {
                Ok(audio) => audio,
                Err(StoreError::Conflict(_)) => return Ok(Some(SubtitleSourcePreparation::Busy)),
                Err(cause) => return Err(cause),
            };
            let transcript = store.transcript_operations.lock(&id).await;
            let snapshot = store.lock_snapshot().await;
            store.ensure_ready()?;
            let Some(meta) = store
                .metadata_snapshot_locked(&snapshot._guard, &id)
                .await?
            else {
                return Ok(None);
            };
            if store.deleted_sessions.lock().unwrap().contains(&id) {
                return Ok(None);
            }
            if busy(&store, &id, &meta.meta).await {
                return Ok(Some(SubtitleSourcePreparation::Busy));
            }
            let directory = store.vault_base.join(store.session_dir(&id).await?);
            let owner = store.write_lock.clone();
            store
                .spawn_blocking(move || {
                    let _audio = audio;
                    let _transcript = transcript;
                    let _snapshot = snapshot;
                    let raw = io::read(&directory.join("transcript.json"), result_limit(true))?;
                    let revision = raw
                        .as_ref()
                        .map(|bytes| crate::sha256(bytes))
                        .unwrap_or_else(io::absent_revision);
                    Ok(Some(SubtitleSourcePreparation::Ready(
                        SubtitleSourceSnapshot {
                            baseline: TranscriptPublicationBaseline(Arc::new(Baseline {
                                owner,
                                recording: None,
                                binding: Binding {
                                    session_id: id,
                                    recording_selection: String::new(),
                                    recording_sha256: String::new(),
                                    transcript_revision: revision,
                                    replacement: true,
                                    transcript_present: raw.is_some(),
                                    audio: Default::default(),
                                    subtitle: Some(io::SubtitleBinding {
                                        session_created_at: meta.meta.created_at,
                                        source_sha256,
                                    }),
                                },
                            })),
                            raw,
                        },
                    )))
                })
                .await
                .map_err(io::error)?
        })
        .await
        .map_err(io::error)?
    }
    pub async fn prepare_subtitle_publication(
        &self,
        operations: &Path,
        identity: TranscriptPublicationIdentity,
        source: SubtitleSourceSnapshot,
        generated: TranscriptWithData,
    ) -> Result<TranscriptPublicationPlan, StoreError> {
        identity.validate()?;
        if !operations.is_absolute()
            || !Arc::ptr_eq(&self.write_lock, &source.baseline.0.owner)
            || source.baseline.0.binding.subtitle.is_none()
            || generated.session_id != source.baseline.0.binding.session_id
        {
            return Err(StoreError::Conflict(
                "Subtitle source belongs to another Store or session".into(),
            ));
        }
        validate_generated(&generated, false)?;
        let transcript_id = generated.id.clone();
        let store = self.clone();
        let root = operations.to_owned();
        tokio::spawn(async move {
            let SubtitleSourceSnapshot { baseline, raw } = source;
            let result = store
                .spawn_blocking(move || merge(raw, generated))
                .await
                .map_err(io::error)??;
            let wanted = Receipt {
                schema_version: 2,
                identity: identity.clone(),
                binding: baseline.0.binding.clone(),
                result_sha256: crate::sha256(&result),
                transcript_id,
                stage_name: io::stage_name(),
                state: State::Prepared,
            };
            let (receipt_path, result_path) = io::paths(&root, &identity);
            let snapshot = store.lock_snapshot().await;
            store.ensure_ready()?;
            let receipt = store
                .spawn_blocking(move || {
                    let _snapshot = snapshot;
                    if let Some(existing) = io::read_receipt(&receipt_path, &identity)
                        .map_err(StoreError::after_commit)?
                    {
                        if existing.binding != wanted.binding
                            || existing.result_sha256 != wanted.result_sha256
                            || existing.transcript_id != wanted.transcript_id
                        {
                            return Err(StoreError::Conflict(
                                "Subtitle publication request changed".into(),
                            ));
                        }
                        return Ok(existing);
                    }
                    io::save_result(&result_path, &result, result_limit(true))?;
                    io::persist(&receipt_path, &wanted)?;
                    Ok(wanted)
                })
                .await
                .map_err(io::error)??;
            prepare_plan(store, root, receipt, Some(baseline)).await
        })
        .await
        .map_err(io::error)?
    }
}
fn merge(raw: Option<Vec<u8>>, generated: TranscriptWithData) -> Result<Vec<u8>, StoreError> {
    let mut value = match raw {
        Some(bytes) => {
            serde_json::from_slice::<crate::metadata_revision::UniqueValue>(&bytes)
                .map_err(io::serialization)?
                .0
        }
        None => serde_json::json!({"transcripts":[]}),
    };
    let original: TranscriptJson =
        serde_json::from_value(value.clone()).map_err(io::serialization)?;
    let mut ids = std::collections::HashSet::new();
    let mut words = std::collections::HashSet::new();
    for transcript in &original.transcripts {
        if transcript.session_id != generated.session_id
            || !ids.insert(&transcript.id)
            || transcript.id == generated.id
        {
            return Err(StoreError::Conflict(
                "Subtitle transcript identities conflict".into(),
            ));
        }
        words.extend(
            transcript
                .words
                .iter()
                .filter_map(|word| word.id.as_deref()),
        );
    }
    if generated
        .words
        .iter()
        .filter_map(|word| word.id.as_deref())
        .any(|id| words.contains(id))
    {
        return Err(StoreError::Conflict(
            "Subtitle word identities conflict".into(),
        ));
    }
    let transcripts = value
        .as_object_mut()
        .ok_or_else(|| StoreError::Serialize("Transcript must be an object".into()))?
        .entry("transcripts")
        .or_insert_with(|| serde_json::json!([]));
    if transcripts.is_null() {
        *transcripts = serde_json::json!([]);
    }
    transcripts
        .as_array_mut()
        .ok_or_else(|| StoreError::Serialize("Transcript list is invalid".into()))?
        .push(serde_json::to_value(generated).map_err(io::serialization)?);
    serde_json::to_vec_pretty(&value).map_err(io::serialization)
}
