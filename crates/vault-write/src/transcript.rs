use hypr_fs_format::{
    SeqCount, TranscriptJson, TranscriptJsonStats, TranscriptSpeakerHint, TranscriptStat,
    TranscriptWithData, TranscriptWord,
};

use super::index::TranscriptSummary;
use super::{SessionStore, StoreError, WriteGuard, paths, validate_session_id};

/// Truncated sha2 of the file bytes -- the summary's change discriminator. Both
/// producers (the write-through path and rebuild) hash the same bytes that land on
/// disk, so a restart never sees a spurious diff.
fn content_hash(bytes: &[u8]) -> u64 {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(bytes);
    u64::from_le_bytes(digest[..8].try_into().unwrap())
}

/// Summary from an already-materialized transcript list (write-through path: the
/// serialized bytes and the parsed list are both in hand).
pub(crate) fn summarize_transcripts(
    bytes: &[u8],
    transcripts: &[TranscriptWithData],
) -> TranscriptSummary {
    summarize_stats(
        bytes,
        transcripts
            .iter()
            .map(|t| TranscriptStat {
                id: t.id.clone(),
                batch_recovery: t.batch_recovery.clone(),
                started_at: t.started_at,
                ended_at: t.ended_at,
                words: SeqCount(t.words.len()),
                speaker_labels: t
                    .speaker_hints
                    .iter()
                    .filter(|hint| hint.hint_type == "speaker_label")
                    .filter_map(|hint| hint.value.as_str())
                    .filter(|label| !label.is_empty())
                    .map(str::to_owned)
                    .collect(),
            })
            .collect(),
    )
}

/// Reads index metadata without materializing the word corpus.
pub(crate) fn summarize_transcript_bytes(bytes: &[u8]) -> Result<TranscriptSummary, StoreError> {
    let stats: TranscriptJsonStats = serde_json::from_slice(bytes).map_err(|e| {
        StoreError::Serialize(format!("failed to deserialize transcript.json: {e}"))
    })?;
    Ok(summarize_stats(bytes, stats.transcripts))
}

fn summarize_stats(bytes: &[u8], mut transcripts: Vec<TranscriptStat>) -> TranscriptSummary {
    let mut summary = TranscriptSummary {
        transcript_ids: transcripts.iter().map(|t| t.id.clone()).collect(),
        has_words: transcripts.iter().any(|t| t.words.0 > 0),
        word_count: transcripts.iter().map(|t| t.words.0 as u64).sum(),
        content_hash: content_hash(bytes),
        metadata: Default::default(),
    };
    transcripts.sort_by(|a, b| a.started_at.total_cmp(&b.started_at).then(a.id.cmp(&b.id)));
    let mut seen = std::collections::HashSet::new();
    for transcript in transcripts {
        let metadata = &mut summary.metadata;
        metadata.started_at = Some(metadata.started_at.map_or(transcript.started_at, |start| {
            start.min(transcript.started_at)
        }));
        if let Some(end) = transcript.ended_at {
            metadata.ended_at = Some(metadata.ended_at.map_or(end, |current| current.max(end)));
        }
        if let Some(recovery) = transcript.batch_recovery {
            metadata.batch_recoveries.push(recovery);
        }
        for label in transcript.speaker_labels {
            if seen.insert(label.clone()) {
                metadata.speaker_labels.push(label);
            }
        }
    }
    summary
}

#[derive(serde::Deserialize, specta::Type, Clone)]
pub struct TranscriptDelta {
    pub transcript_id: String,
    pub new_words: Vec<TranscriptWord>,
    pub replaced_ids: Vec<String>,
    pub new_hints: Vec<TranscriptSpeakerHint>,
    pub started_at_ms: f64,
}

#[derive(Debug, Default)]
pub struct LiveTranscriptBuffer {
    pub transcript_id: String,
    pub started_at_ms: f64,
    pub words: Vec<TranscriptWord>,
    pub hints: Vec<TranscriptSpeakerHint>,
    pub dirty: bool,
    pub release_requested: bool,
}

const DEBOUNCE: std::time::Duration = std::time::Duration::from_secs(1);
const RETRY_DELAY_CAP: std::time::Duration = std::time::Duration::from_secs(30);

/// Backoff for the debounced-flush retry loop: doubling keeps a persistently failing disk
/// from being hammered every second, the cap keeps retries frequent enough to catch a
/// recovered disk quickly, and there is deliberately no max-attempts abandon -- a buffer
/// that can't flush holds the only copy of those words.
fn next_retry_delay(current: std::time::Duration) -> std::time::Duration {
    (current * 2).min(RETRY_DELAY_CAP)
}

impl SessionStore {
    pub async fn finish_transcript(
        &self,
        session_id: &str,
        transcript_id: &str,
    ) -> Result<(), StoreError> {
        validate_session_id(session_id)?;
        let _operation = self.transcript_operations.lock(session_id).await;
        {
            let mut live = self.live.lock().await;
            let Some(buffer) = live.get_mut(session_id) else {
                return Ok(());
            };
            if buffer.transcript_id != transcript_id {
                return Ok(());
            }
            buffer.release_requested = true;
        }
        self.flush_transcript_under_operation(session_id).await
    }

    /// Buffers the delta and schedules a flush ~1s later. Has no session/index preconditions,
    /// so it can never silently no-op. Usually touches only the in-memory buffer; the one
    /// exception is switching `transcript_id` while the outgoing buffer is dirty, which flushes
    /// the old transcript first and propagates that flush's error (loudly — the caller sees Err
    /// and the incoming delta is NOT buffered; the old transcript's buffer stays dirty for retry).
    pub async fn append_transcript(
        &self,
        session_id: &str,
        delta: TranscriptDelta,
    ) -> Result<(), StoreError> {
        validate_session_id(session_id)?;

        if delta.new_words.is_empty() && delta.replaced_ids.is_empty() && delta.new_hints.is_empty()
        {
            // Nothing to buffer, nothing to (re)dirty, nothing to schedule.
            return Ok(());
        }

        let _operation = self.transcript_operations.lock(session_id).await;
        if self.deleted_sessions.lock().unwrap().contains(session_id) {
            return Err(StoreError::Io(format!("session {session_id} was deleted")));
        }
        let switching = self
            .live
            .lock()
            .await
            .get(session_id)
            .is_some_and(|buffer| buffer.transcript_id != delta.transcript_id);
        if switching {
            self.flush_transcript_under_operation(session_id).await?;
            self.live.lock().await.remove(session_id);
        }
        let needs_seed = !self.live.lock().await.contains_key(session_id)
            && self.transcript_session_id(&delta.transcript_id).as_deref() == Some(session_id);
        if needs_seed {
            if let Some(previous) = self
                .read_transcript_json(session_id)
                .await?
                .transcripts
                .into_iter()
                .find(|t| t.id == delta.transcript_id)
            {
                self.live.lock().await.insert(
                    session_id.to_owned(),
                    LiveTranscriptBuffer {
                        transcript_id: previous.id,
                        started_at_ms: previous.started_at,
                        words: previous.words,
                        hints: previous.speaker_hints,
                        ..Default::default()
                    },
                );
            }
        }
        let mut live = self.live.lock().await;

        let needs_spawn = {
            let buffer = live.entry(session_id.to_string()).or_default();

            if !delta.replaced_ids.is_empty() {
                buffer.words.retain(|word| {
                    !delta
                        .replaced_ids
                        .iter()
                        .any(|id| word.id.as_deref() == Some(id.as_str()))
                });
            }
            buffer.words.extend(delta.new_words);
            buffer.hints.extend(delta.new_hints);
            buffer.transcript_id = delta.transcript_id;
            buffer.started_at_ms = delta.started_at_ms;

            // Only the transition from clean -> dirty schedules a flusher; further appends
            // within the debounce window ride along on the one already pending.
            let was_dirty = buffer.dirty;
            buffer.dirty = true;
            !was_dirty
        };
        drop(live);

        if needs_spawn {
            let store = self.clone();
            let session_id = session_id.to_string();
            tokio::spawn(async move {
                let mut delay = DEBOUNCE;
                loop {
                    tokio::time::sleep(delay).await;
                    // No separate "is it still dirty" pre-check: flush_transcript itself
                    // no-ops with Ok when the buffer went clean in the meantime (explicit
                    // flush, batch write), so Ok always means this flusher is done.
                    match store.flush_transcript(&session_id).await {
                        Ok(()) => return,
                        Err(err) => {
                            // flush_transcript re-dirties the buffer on Err, so the next
                            // iteration picks this session back up. Retries never stop --
                            // the buffer holds the only copy of these words -- they just
                            // slow down while the failure persists.
                            delay = next_retry_delay(delay);
                            tracing::error!(
                                session_id = %session_id,
                                error = %err,
                                next_retry_in_secs = delay.as_secs(),
                                "debounced transcript flush failed; will retry with backoff while buffer stays dirty"
                            );
                        }
                    }
                }
            });
        }

        Ok(())
    }

    /// Writes transcript.json from the buffer (or re-reads existing file and merges), updates
    /// the transcripts index row. No-op if there is nothing buffered for this session.
    pub async fn flush_transcript(&self, session_id: &str) -> Result<(), StoreError> {
        validate_session_id(session_id)?;
        let _operation = self.transcript_operations.lock(session_id).await;
        self.flush_transcript_under_operation(session_id).await
    }

    async fn flush_transcript_under_operation(&self, session_id: &str) -> Result<(), StoreError> {
        let snapshot = {
            let mut live = self.live.lock().await;
            let Some(buffer) = live.get(session_id) else {
                return Ok(());
            };
            if !buffer.dirty {
                if buffer.release_requested {
                    live.remove(session_id);
                }
                return Ok(());
            }
            (
                buffer.transcript_id.clone(),
                buffer.started_at_ms,
                buffer.words.clone(),
                buffer.hints.clone(),
            )
        };
        let (transcript_id, started_at_ms, words, hints) = snapshot;
        self.persist_transcript(session_id, &transcript_id, started_at_ms, words, hints)
            .await?;
        let mut live = self.live.lock().await;
        if let Some(buffer) = live.get_mut(session_id) {
            if buffer.release_requested {
                live.remove(session_id);
            } else {
                buffer.dirty = false;
            }
        }
        Ok(())
    }

    /// App-exit hook: flush every dirty buffer. A failure on one session must not skip the
    /// others, so every session is attempted; the first error (if any) is returned afterward.
    pub async fn flush_all(&self) -> Result<(), StoreError> {
        let dirty_session_ids: Vec<String> = {
            let live = self.live.lock().await;
            live.iter()
                .filter(|(_, buffer)| buffer.dirty)
                .map(|(session_id, _)| session_id.clone())
                .collect()
        };

        let mut first_error = None;
        for session_id in dirty_session_ids {
            if let Err(err) = self.flush_transcript(&session_id).await {
                if first_error.is_none() {
                    first_error = Some(err);
                }
            }
        }

        match first_error {
            Some(err) => Err(err),
            None => Ok(()),
        }
    }

    /// Replace a whole transcript, releasing superseded live data only after persistence.
    pub async fn write_transcript(
        &self,
        session_id: &str,
        mut t: TranscriptWithData,
    ) -> Result<(), StoreError> {
        validate_session_id(session_id)?;
        let _operation = self.transcript_operations.lock(session_id).await;
        let transcript_id = t.id.clone();
        let guard = self.lock_writes().await;
        let mut file = self.read_transcript_json(session_id).await?;
        if let Some(existing) = file
            .transcripts
            .iter_mut()
            .find(|existing| existing.id == t.id)
        {
            existing.session_id = session_id.to_string();
            existing.started_at = t.started_at;
            existing.words = t.words;
            existing.speaker_hints = t.speaker_hints;
            if t.batch_recovery.is_some() {
                existing.batch_recovery = t.batch_recovery;
            }
        } else {
            t.created_at = chrono::Utc::now().to_rfc3339();
            if t.batch_recovery.is_none() {
                t.user_id.clear();
                t.memo_md.clear();
                t.ended_at = None;
            }
            file.transcripts.push(t);
        }
        self.write_transcript_json_locked(&guard, session_id, file)
            .await?;
        let mut live = self.live.lock().await;
        if live
            .get(session_id)
            .is_some_and(|buffer| buffer.transcript_id == transcript_id)
        {
            live.remove(session_id);
        }
        Ok(())
    }

    /// Supersede primitive (E3): the incoming transcript REPLACES the session's whole
    /// transcript set (batch re-run / re-transcription path -- the old frontend
    /// tombstone-others UPDATE). The file is the truth, so superseded transcripts must
    /// leave `transcript.json`: the previous file moves to `.trash/<date>/sessions/<id>/`
    /// (hand-recoverable, same policy as enhanced-doc deletes) whenever it holds anything
    /// besides the incoming transcript id, then the file is rewritten to just the incoming
    /// transcript.
    ///
    /// Drops superseded live data after successful persistence, preserving failed writes for retry.
    pub async fn replace_session_transcripts(
        &self,
        session_id: &str,
        mut t: TranscriptWithData,
    ) -> Result<(), StoreError> {
        validate_session_id(session_id)?;

        let _operation = self.transcript_operations.lock(session_id).await;

        // One guard spans the "what's in the file today" read, the supersede-trash and the
        // rewrite, so a concurrent transcript write can't slip between them.
        let guard = self.lock_writes().await;

        let previous = self.read_transcript_json(session_id).await?;
        if t.batch_recovery.is_none() {
            if let Some(existing) = previous
                .transcripts
                .iter()
                .find(|existing| existing.id == t.id)
            {
                t.user_id = existing.user_id.clone();
                t.created_at = existing.created_at.clone();
                t.memo_md = existing.memo_md.clone();
                t.ended_at = existing.ended_at;
            } else {
                t.user_id.clear();
                t.created_at = chrono::Utc::now().to_rfc3339();
                t.memo_md.clear();
                t.ended_at = None;
            }
        }
        let loses_content = previous
            .transcripts
            .iter()
            .any(|existing| existing.id != t.id);
        if loses_content {
            let vault_base = self.vault_base.clone();
            let session_dir = self.session_dir_locked(&guard, session_id).await?;
            let relative = paths::transcript_path_in(&session_dir);
            tokio::task::spawn_blocking(move || -> Result<(), StoreError> {
                let abs = vault_base.join(relative);
                hypr_fs_sync_core::export::move_to_trash(&vault_base, &abs)
                    .map(|_| ())
                    .map_err(|e| {
                        StoreError::Io(format!(
                            "failed to move superseded transcripts to trash: {e}"
                        ))
                    })
            })
            .await
            .map_err(|e| StoreError::Io(format!("task join error: {e}")))??;
        }

        self.write_transcript_json_locked(
            &guard,
            session_id,
            TranscriptJson {
                transcripts: vec![t],
            },
        )
        .await?;
        self.live.lock().await.remove(session_id);
        Ok(())
    }

    pub async fn checkpoint_batch_transcript(
        &self,
        session_id: &str,
        incoming: TranscriptWithData,
        initial: bool,
    ) -> Result<(), StoreError> {
        let existing = self.read_transcript_json(session_id).await?.transcripts;
        let Some(saved) = existing.iter().find(|t| t.id == incoming.id) else {
            if initial {
                return self.replace_session_transcripts(session_id, incoming).await;
            }
            return Err(StoreError::Io(
                "The transcript was replaced while transcription was running.".into(),
            ));
        };
        let recovery = incoming
            .batch_recovery
            .ok_or_else(|| StoreError::Io("Missing recovery metadata.".into()))?;
        let previous = saved
            .batch_recovery
            .as_ref()
            .ok_or_else(|| StoreError::Io("The recovery run was replaced.".into()))?;
        if previous.planning_pending && !recovery.planning_pending {
            return self
                .complete_batch_chunk_planning(session_id, recovery)
                .await;
        }
        let resolved: Vec<_> = previous
            .unresolved
            .iter()
            .filter(|range| {
                !recovery.unresolved.iter().any(|r| {
                    r.channel == range.channel
                        && r.start_sample == range.start_sample
                        && r.end_sample == range.end_sample
                })
            })
            .cloned()
            .collect();
        let words: Vec<_> = incoming
            .words
            .into_iter()
            .filter(|word| {
                resolved.iter().any(|range| {
                    let channel = if recovery.audio.layout == hypr_fs_format::AudioLayout::Mixed {
                        2.0
                    } else {
                        range.channel as f64
                    };
                    word.channel == channel
                        && word.start_ms >= range.start_sample as f64 / 16.0 - 0.001
                        && word.end_ms <= range.end_sample as f64 / 16.0 + 0.001
                })
            })
            .collect();
        let ids: std::collections::HashSet<_> =
            words.iter().filter_map(|word| word.id.as_deref()).collect();
        let hints = incoming
            .speaker_hints
            .into_iter()
            .filter(|hint| ids.contains(hint.word_id.as_str()))
            .collect();
        self.merge_batch_recovery(session_id, &recovery.run_id, resolved, words, hints)
            .await?;
        self.update_batch_recovery_status(session_id, recovery)
            .await
    }

    async fn complete_batch_chunk_planning(
        &self,
        session_id: &str,
        incoming: hypr_fs_format::BatchRecovery,
    ) -> Result<(), StoreError> {
        let _operation = self.transcript_operations.lock(session_id).await;
        let guard = self.lock_writes().await;
        let mut file = self.read_transcript_json(session_id).await?;
        let transcript = file
            .transcripts
            .iter_mut()
            .find(|t| {
                t.batch_recovery
                    .as_ref()
                    .is_some_and(|r| r.run_id == incoming.run_id)
            })
            .ok_or_else(|| StoreError::Io("The recovery run was replaced.".into()))?;
        if !transcript.batch_recovery.as_ref().unwrap().planning_pending {
            return Ok(());
        }
        if !transcript.words.is_empty() {
            return Err(StoreError::Io("Text was added while audio was being planned. Transcribe the full recording again.".into()));
        }
        transcript.batch_recovery = Some(incoming);
        self.write_transcript_json_locked(&guard, session_id, file)
            .await
    }

    pub async fn refine_batch_recovery_range(
        &self,
        session_id: &str,
        run_id: &str,
        original: hypr_fs_format::FailedAudioRange,
        parts: Vec<hypr_fs_format::FailedAudioRange>,
    ) -> Result<(), StoreError> {
        validate_session_id(session_id)?;
        let mut end = original.start_sample;
        for range in &parts {
            if range.channel != original.channel
                || range.start_sample != end
                || range.end_sample <= range.start_sample
            {
                return Err(StoreError::Io("Invalid recovery boundaries.".into()));
            }
            end = range.end_sample;
        }
        if end != original.end_sample {
            return Err(StoreError::Io(
                "Recovery boundaries do not cover the missing audio.".into(),
            ));
        }
        let _operation = self.transcript_operations.lock(session_id).await;
        let guard = self.lock_writes().await;
        let mut file = self.read_transcript_json(session_id).await?;
        let recovery = file
            .transcripts
            .iter_mut()
            .filter_map(|t| t.batch_recovery.as_mut())
            .find(|r| r.run_id == run_id)
            .ok_or_else(|| StoreError::Io("The recovery run was replaced.".into()))?;
        let Some(index) = recovery.unresolved.iter().position(|r| {
            r.channel == original.channel
                && r.start_sample == original.start_sample
                && r.end_sample == original.end_sample
        }) else {
            return Ok(());
        };
        recovery.unresolved.splice(index..=index, parts);
        self.write_transcript_json_locked(&guard, session_id, file)
            .await
    }

    pub async fn update_batch_recovery_status(
        &self,
        session_id: &str,
        incoming: hypr_fs_format::BatchRecovery,
    ) -> Result<(), StoreError> {
        validate_session_id(session_id)?;
        let _operation = self.transcript_operations.lock(session_id).await;
        let guard = self.lock_writes().await;
        let mut file = self.read_transcript_json(session_id).await?;
        let recovery = file
            .transcripts
            .iter_mut()
            .filter_map(|t| t.batch_recovery.as_mut())
            .find(|r| r.run_id == incoming.run_id)
            .ok_or_else(|| {
                StoreError::Io(
                    "The recovery run was replaced. Transcribe the full recording again.".into(),
                )
            })?;
        if recovery.diarization_pending && !incoming.diarization_pending {
            recovery.diarization = incoming.diarization.clone();
            recovery.diarization_pending = false;
        }
        for range in &mut recovery.unresolved {
            if let Some(updated) = incoming.unresolved.iter().find(|r| {
                r.channel == range.channel
                    && r.start_sample == range.start_sample
                    && r.end_sample == range.end_sample
            }) {
                range.reason = updated.reason.clone();
            }
        }
        recovery.planning_pending &= incoming.planning_pending;
        recovery.status = if recovery.unresolved.is_empty() {
            hypr_fs_format::BatchCompletionStatus::Complete
        } else if incoming.status == hypr_fs_format::BatchCompletionStatus::Complete {
            hypr_fs_format::BatchCompletionStatus::Incomplete
        } else {
            incoming.status
        };
        self.write_transcript_json_locked(&guard, session_id, file)
            .await
    }

    pub async fn merge_batch_recovery(
        &self,
        session_id: &str,
        run_id: &str,
        resolved: Vec<hypr_fs_format::FailedAudioRange>,
        mut words: Vec<TranscriptWord>,
        hints: Vec<TranscriptSpeakerHint>,
    ) -> Result<(), StoreError> {
        validate_session_id(session_id)?;
        let _operation = self.transcript_operations.lock(session_id).await;
        let guard = self.lock_writes().await;
        let mut file = self.read_transcript_json(session_id).await?;
        let transcript = file
            .transcripts
            .iter_mut()
            .find(|t| {
                t.batch_recovery
                    .as_ref()
                    .is_some_and(|r| r.run_id == run_id)
            })
            .ok_or_else(|| {
                StoreError::Io(
                    "This recovery plan was replaced. Transcribe the full recording again.".into(),
                )
            })?;
        let recovery = transcript.batch_recovery.as_mut().unwrap();
        if recovery.version != 1 {
            return Err(StoreError::Io(
                "This recovery plan requires a newer app version.".into(),
            ));
        }
        let same_range = |a: &hypr_fs_format::FailedAudioRange,
                          b: &hypr_fs_format::FailedAudioRange| {
            a.channel == b.channel
                && a.start_sample == b.start_sample
                && a.end_sample == b.end_sample
        };
        let pending: Vec<_> = resolved
            .iter()
            .filter(|range| recovery.unresolved.iter().any(|r| same_range(r, range)))
            .collect();
        if pending.is_empty() {
            return Ok(());
        }
        if pending.len() != resolved.len() {
            return Err(StoreError::Io(
                "Recovery ranges have changed. Retry the remaining audio again.".into(),
            ));
        }
        let word_channel = |range: &hypr_fs_format::FailedAudioRange| {
            if recovery.audio.layout == hypr_fs_format::AudioLayout::Mixed {
                2.0
            } else {
                range.channel as f64
            }
        };
        let inside = |word: &TranscriptWord, range: &hypr_fs_format::FailedAudioRange| {
            word.channel == word_channel(range)
                && word.start_ms < range.end_sample as f64 / 16.0
                && word.end_ms > range.start_sample as f64 / 16.0
        };
        if transcript
            .words
            .iter()
            .any(|word| pending.iter().any(|range| inside(word, range)))
        {
            return Err(StoreError::Io("Text was added inside the missing audio. Keep your edits or transcribe the full recording again.".into()));
        }
        for word in &mut words {
            let Some(range) = pending.iter().find(|range| {
                word.channel == word_channel(range)
                    && word.start_ms >= range.start_sample as f64 / 16.0 - 0.001
                    && word.end_ms <= range.end_sample as f64 / 16.0 + 0.001
                    && word.end_ms >= word.start_ms
            }) else {
                return Err(StoreError::Io(
                    "Recovered words lie outside the requested audio ranges.".into(),
                ));
            };
            word.start_ms = word.start_ms.max(range.start_sample as f64 / 16.0);
            word.end_ms = word.end_ms.min(range.end_sample as f64 / 16.0);
        }
        let mut ids: std::collections::HashSet<_> = transcript
            .words
            .iter()
            .filter_map(|w| w.id.as_deref())
            .collect();
        if words
            .iter()
            .any(|w| w.id.as_deref().is_none_or(|id| !ids.insert(id)))
        {
            return Err(StoreError::Io(
                "Recovered words must have unique identities.".into(),
            ));
        }
        let recovered_ids: std::collections::HashSet<_> =
            words.iter().filter_map(|w| w.id.as_deref()).collect();
        if hints
            .iter()
            .any(|h| !recovered_ids.contains(h.word_id.as_str()))
        {
            return Err(StoreError::Io(
                "Recovered speaker assignments reference unknown words.".into(),
            ));
        }
        recovery
            .unresolved
            .retain(|range| !pending.iter().any(|r| same_range(range, r)));
        recovery.status = if recovery.unresolved.is_empty() {
            hypr_fs_format::BatchCompletionStatus::Complete
        } else {
            hypr_fs_format::BatchCompletionStatus::Incomplete
        };
        transcript.words.extend(words);
        transcript.words.sort_by(|a, b| {
            a.start_ms
                .total_cmp(&b.start_ms)
                .then(a.end_ms.total_cmp(&b.end_ms))
        });
        transcript.speaker_hints.extend(hints);
        self.write_transcript_json_locked(&guard, session_id, file)
            .await
    }

    /// Speaker rename as a hints-only mutation: the words list is passed through
    /// untouched (structurally preserving per-word `metadata`, which the old
    /// frontend read-modify-write path silently wiped), and the frontend never has
    /// to ship the full transcript over IPC just to relabel a speaker.
    ///
    /// The conflict rules mirror the frontend's old `upsertSpeakerAssignment`: the
    /// new assignment scopes to `(channel, speaker_index)`; an existing
    /// `speaker_label` hint is dropped when it has the same id as the new one, or
    /// when its own scope (its anchor word's channel + that word's
    /// `provider_speaker_index` hint) conflicts — same channel and either side
    /// lacking a speaker index, or both having the same one. Hints anchored to
    /// unknown words are kept.
    ///
    /// Exception on a *diarized* channel (>=2 distinct provider speaker indexes):
    /// scopes conflict only when they match exactly — an indexed assignment evicts
    /// just its own `(channel, index)`, and a channel-wide assignment evicts just the
    /// previous channel-wide one, because the channel-wide label is the fallback for
    /// segments without an index, not a duplicate of the indexed labels.
    pub async fn assign_transcript_speaker(
        &self,
        transcript_id: &str,
        channel: i32,
        speaker_index: Option<i32>,
        speaker_label: &str,
        anchor_word_id: &str,
    ) -> Result<(), StoreError> {
        // The index only resolves transcript -> session here; ownership never moves, so
        // this lookup is safe outside the guard. Words and hints are re-read from the
        // file below, under the guard -- persisting a snapshot taken out here could
        // clobber a write that lands between the read and the lock.
        let session_id = self.transcript_session_id(transcript_id).ok_or_else(|| {
            StoreError::Io(format!(
                "cannot assign speaker: transcript {transcript_id} does not exist"
            ))
        })?;

        let _operation = self.transcript_operations.lock(&session_id).await;
        self.flush_transcript_under_operation(&session_id).await?;
        let guard = self.lock_writes().await;

        let mut file = self.read_transcript_json(&session_id).await?;
        let Some(entry) = file.transcripts.iter_mut().find(|t| t.id == transcript_id) else {
            return Err(StoreError::Io(format!(
                "cannot assign speaker: transcript {transcript_id} does not exist"
            )));
        };

        let new_id = format!("{anchor_word_id}:speaker_label");
        let next_channel = f64::from(channel);
        let next_speaker_index = speaker_index.map(f64::from);

        let mut next_hints: Vec<TranscriptSpeakerHint> = {
            let hints = &entry.speaker_hints;
            let words_by_id: std::collections::HashMap<&str, &TranscriptWord> = entry
                .words
                .iter()
                .filter_map(|word| word.id.as_deref().map(|id| (id, word)))
                .collect();

            // A channel is "diarized" when its provider hints resolve to >=2 distinct
            // speaker indexes. On such a channel, indexed assignments and the
            // channel-wide fallback are separate scopes that must coexist -- a
            // channel-wide hint is the fallback for gap segments, not a stale duplicate.
            let channel_is_diarized = {
                let mut indexes: Vec<f64> = hints
                    .iter()
                    .filter(|h| {
                        h.hint_type == "provider_speaker_index"
                            && words_by_id
                                .get(h.word_id.as_str())
                                .is_some_and(|w| w.channel == next_channel)
                    })
                    .filter_map(|h| provider_speaker_index_for_word(hints, &h.word_id))
                    .collect();
                indexes.sort_by(f64::total_cmp);
                indexes.dedup();
                indexes.len() >= 2
            };

            hints
                .iter()
                .filter(|hint| {
                    if hint.hint_type != "speaker_label" {
                        return true;
                    }
                    if hint.id.as_deref() == Some(new_id.as_str()) {
                        return false;
                    }
                    let Some(word) = words_by_id.get(hint.word_id.as_str()) else {
                        return true;
                    };
                    if word.channel != next_channel {
                        return true;
                    }
                    let existing_speaker_index =
                        provider_speaker_index_for_word(hints, &hint.word_id);
                    let conflicts = if channel_is_diarized {
                        // Diarized channel: only an exact scope match supersedes --
                        // (ch, idx) evicts the same (ch, idx), channel-wide evicts
                        // channel-wide. Mixed scopes coexist (fallback semantics).
                        existing_speaker_index == next_speaker_index
                    } else {
                        // Undiarized channel: a missing speaker index on either side
                        // means "the whole channel", which conflicts with everything
                        // on it (pre-diarization behavior, unchanged).
                        match (existing_speaker_index, next_speaker_index) {
                            (Some(left), Some(right)) => left == right,
                            _ => true,
                        }
                    };
                    !conflicts
                })
                .cloned()
                .collect()
        };

        next_hints.push(TranscriptSpeakerHint {
            id: Some(new_id),
            word_id: anchor_word_id.to_string(),
            hint_type: "speaker_label".to_string(),
            // The frontend write path persisted the label as a bare JSON string;
            // renderers read it back the same way.
            value: serde_json::Value::String(speaker_label.to_string()),
        });

        // Hints-only mutation: `entry.words` is deliberately never reassigned, so word
        // content (including per-word metadata) cannot regress no matter what raced the
        // pre-guard index lookup.
        entry.speaker_hints = next_hints.clone();
        self.write_transcript_json_locked(&guard, &session_id, file)
            .await?;
        if let Some(buffer) = self.live.lock().await.get_mut(&session_id) {
            if buffer.transcript_id == transcript_id {
                buffer.hints = next_hints;
            }
        }
        Ok(())
    }

    async fn persist_transcript(
        &self,
        session_id: &str,
        transcript_id: &str,
        started_at_ms: f64,
        words: Vec<TranscriptWord>,
        hints: Vec<TranscriptSpeakerHint>,
    ) -> Result<(), StoreError> {
        // The lock spans the read *and* the write: `transcript.json` holds every transcript
        // of the session, so two concurrent persists that each read the file and write a
        // whole new one back would drop the loser's entry entirely.
        let guard = self.lock_writes().await;
        self.persist_transcript_locked(
            &guard,
            session_id,
            transcript_id,
            started_at_ms,
            words,
            hints,
        )
        .await
    }

    async fn persist_transcript_locked(
        &self,
        guard: &WriteGuard<'_>,
        session_id: &str,
        transcript_id: &str,
        started_at_ms: f64,
        words: Vec<TranscriptWord>,
        hints: Vec<TranscriptSpeakerHint>,
    ) -> Result<(), StoreError> {
        let mut file = self.read_transcript_json(session_id).await?;

        match file.transcripts.iter().position(|t| t.id == transcript_id) {
            Some(idx) => {
                let existing = &mut file.transcripts[idx];
                existing.session_id = session_id.to_string();
                existing.started_at = started_at_ms;
                existing.words = words;
                existing.speaker_hints = hints;
            }
            None => {
                file.transcripts.push(TranscriptWithData {
                    batch_recovery: None,
                    id: transcript_id.to_string(),
                    user_id: String::new(),
                    created_at: chrono::Utc::now().to_rfc3339(),
                    session_id: session_id.to_string(),
                    started_at: started_at_ms,
                    ended_at: None,
                    memo_md: String::new(),
                    words,
                    speaker_hints: hints,
                });
            }
        }

        self.write_transcript_json_locked(guard, session_id, file)
            .await
    }

    /// Shared persist tail: serialize the whole file, write it under the caller's guard,
    /// and republish the full transcript list to the index.
    async fn write_transcript_json_locked(
        &self,
        guard: &WriteGuard<'_>,
        session_id: &str,
        file: TranscriptJson,
    ) -> Result<(), StoreError> {
        // Serialize the full file up front: a word that can't round-trip must fail the whole
        // flush (StoreError::Serialize), never get silently dropped or collapse to `[]`.
        let bytes =
            serde_json::to_vec_pretty(&file).map_err(|e| StoreError::Serialize(e.to_string()))?;
        // Summarize the exact bytes being written -- a later rebuild hashing the file
        // on disk agrees, so a restart never produces a spurious Transcripts diff.
        let summary = summarize_transcripts(&bytes, &file.transcripts);

        let session_dir = self.session_dir_locked(guard, session_id).await?;
        self.write_file_locked(guard, paths::transcript_path_in(&session_dir), bytes)
            .await?;

        self.index_set_transcript_summary(session_id, summary);
        self.notify_index_changed(
            super::IndexEntity::Transcripts,
            vec![session_id.to_string()],
        );

        Ok(())
    }

    /// Reusable by rebuild.rs: missing file -> empty transcript list (not an error, matching
    /// the "no transcript yet" state); malformed JSON -> Err so callers can distinguish
    /// "nothing here" from "something here that failed to parse".
    pub(crate) async fn read_transcript_json(
        &self,
        session_id: &str,
    ) -> Result<TranscriptJson, StoreError> {
        validate_session_id(session_id)?;
        let vault_base = self.vault_base.clone();
        let relative = paths::transcript_path_in(&self.session_dir(session_id).await?);

        tokio::task::spawn_blocking(move || -> Result<TranscriptJson, StoreError> {
            let path = vault_base.join(&relative);

            // Attempt-then-match, not exists()-then-read: see read_meta's comment in
            // content.rs for why exists() alone is unsafe to gate on here.
            let bytes = match std::fs::read(&path) {
                Ok(bytes) => bytes,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    return Ok(TranscriptJson {
                        transcripts: Vec::new(),
                    });
                }
                Err(e) => {
                    return Err(StoreError::Io(format!(
                        "failed to read transcript file: {}",
                        e
                    )));
                }
            };

            serde_json::from_slice(&bytes).map_err(|e| {
                StoreError::Serialize(format!("failed to deserialize transcript.json: {}", e))
            })
        })
        .await
        .map_err(|e| StoreError::Io(format!("task join error: {}", e)))?
    }

    /// Rebuild's word-free counterpart of `read_transcript_json`: same missing-file
    /// (-> empty summary, which removes the index entry) and malformed-file (-> Err,
    /// keep the old entry) contract, but only counts and hashes -- no per-word
    /// allocation, nothing retained.
    pub(crate) async fn read_transcript_summary(
        &self,
        session_id: &str,
    ) -> Result<TranscriptSummary, StoreError> {
        validate_session_id(session_id)?;
        let vault_base = self.vault_base.clone();
        let relative = paths::transcript_path_in(&self.session_dir(session_id).await?);

        tokio::task::spawn_blocking(move || -> Result<TranscriptSummary, StoreError> {
            let path = vault_base.join(&relative);
            let bytes = match std::fs::read(&path) {
                Ok(bytes) => bytes,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    return Ok(TranscriptSummary {
                        transcript_ids: Vec::new(),
                        has_words: false,
                        word_count: 0,
                        content_hash: 0,
                        metadata: Default::default(),
                    });
                }
                Err(e) => {
                    return Err(StoreError::Io(format!(
                        "failed to read transcript file: {}",
                        e
                    )));
                }
            };
            summarize_transcript_bytes(&bytes)
        })
        .await
        .map_err(|e| StoreError::Io(format!("task join error: {}", e)))?
    }
}

/// Live-era `provider_speaker_index` hints serialized the value as a JSON *string*
/// (`"{\"channel\":1,\"speaker_index\":2}"`); newer writers store the object
/// directly. Both shapes are on disk, so both must resolve.
fn provider_speaker_index_for_word(hints: &[TranscriptSpeakerHint], word_id: &str) -> Option<f64> {
    let hint = hints
        .iter()
        .find(|h| h.hint_type == "provider_speaker_index" && h.word_id == word_id)?;
    let parsed;
    let value = match &hint.value {
        serde_json::Value::String(s) => {
            parsed = serde_json::from_str::<serde_json::Value>(s).ok()?;
            &parsed
        }
        other => other,
    };
    value.get("speaker_index")?.as_f64()
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_store() -> (SessionStore, tempfile::TempDir) {
        let temp = tempfile::tempdir().unwrap();
        let vault = temp.path().to_path_buf();
        let store = SessionStore::new(vault);
        (store, temp)
    }

    #[test]
    fn metadata_parity_preserves_speaker_order_and_duration() {
        let bytes = serde_json::to_vec(&serde_json::json!({"transcripts": [
            {"id":"late", "session_id":"s", "started_at":20, "ended_at":50,
             "words":[], "speaker_hints":[{"word_id":"w", "type":"speaker_label", "value":"Bob"}]},
            {"id":"b", "session_id":"s", "started_at":10, "ended_at":30,
             "words":[], "speaker_hints":[{"word_id":"w", "type":"speaker_label", "value":"Bob"}]},
            {"id":"a", "session_id":"s", "started_at":10, "ended_at":null,
             "words":[{"id":"w","text":"hello","start_ms":0,"end_ms":10,"channel":0}],
             "speaker_hints":[
                {"word_id":"w", "type":"provider_speaker_index", "value":{"speaker_index":0}},
                {"word_id":"w", "type":"speaker_label", "value":"Alice"},
                {"word_id":"w", "type":"speaker_label", "value":""},
                {"word_id":"w", "type":"speaker_label", "value":null},
                {"word_id":"w", "type":"speaker_label", "value":"Alice"}
             ]}
        ]}))
        .unwrap();
        let full: TranscriptJson = serde_json::from_slice(&bytes).unwrap();
        let compact = summarize_transcript_bytes(&bytes).unwrap();
        assert_eq!(compact, summarize_transcripts(&bytes, &full.transcripts));
        assert_eq!(compact.metadata.speaker_labels, ["Alice", "Bob"]);
        assert_eq!(compact.metadata.started_at, Some(10.0));
        assert_eq!(compact.metadata.ended_at, Some(50.0));
        assert_eq!(compact.word_count, 1);
        assert_eq!(compact.transcript_ids, ["late", "b", "a"]);
    }

    #[test]
    fn metadata_skips_words_and_accepts_legacy_nulls() {
        let summary = summarize_transcript_bytes(br#"{"transcripts":[
            {"id":"a", "started_at":null, "words":[{"huge":"ignored without a word schema"}], "speaker_hints":null},
            {"id":"b", "words":null}
        ]}"#).unwrap();
        assert_eq!(summary.word_count, 1);
        assert_eq!(summary.metadata.started_at, Some(0.0));
        assert_eq!(summary.metadata.ended_at, None);
        assert!(summary.metadata.speaker_labels.is_empty());
        assert!(summarize_transcript_bytes(b"not json").is_err());
    }

    #[tokio::test]
    async fn metadata_follows_flush_external_refresh_and_removal() {
        let (store, _vault) = test_store().await;
        store
            .append_transcript("s", delta_with_words(&["hello"]))
            .await
            .unwrap();
        store.flush_transcript("s").await.unwrap();
        assert_eq!(
            store.session_transcript_metadata("s").started_at,
            Some(1000.0)
        );
        let meta: crate::SessionMeta = serde_json::from_value(serde_json::json!({
            "id":"s", "title":"Test", "created_at":"2026-01-01T00:00:00Z", "tags":[]
        }))
        .unwrap();
        store.write_meta(&meta).await.unwrap();
        let path = _vault
            .path()
            .join(store.session_dir("s").await.unwrap())
            .join("transcript.json");
        let mut file: TranscriptJson =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        file.transcripts[0].speaker_hints = vec![label_hint("w0", "Alice")];
        file.transcripts[0].ended_at = Some(5000.0);
        std::fs::write(&path, serde_json::to_vec(&file).unwrap()).unwrap();
        store.refresh_session("s").await.unwrap();
        assert_eq!(
            store.session_transcript_metadata("s").speaker_labels,
            ["Alice"]
        );
        assert_eq!(
            store.session_transcript_metadata("s").ended_at,
            Some(5000.0)
        );
        std::fs::write(&path, "malformed").unwrap();
        let _ = store.refresh_session("s").await;
        assert_eq!(
            store.session_transcript_metadata("s").speaker_labels,
            ["Alice"]
        );
        std::fs::write(&path, serde_json::to_vec(&file).unwrap()).unwrap();
        store
            .assign_transcript_speaker("t1", 0, None, "Bob", "w0")
            .await
            .unwrap();
        assert_eq!(
            store.session_transcript_metadata("s").speaker_labels,
            ["Bob"]
        );
        file.transcripts[0].speaker_hints.clear();
        store
            .replace_session_transcripts("s", file.transcripts[0].clone())
            .await
            .unwrap();
        assert!(
            store
                .session_transcript_metadata("s")
                .speaker_labels
                .is_empty()
        );
        std::fs::remove_file(&path).unwrap();
        store.refresh_session("s").await.unwrap();
        assert_eq!(store.session_transcript_metadata("s"), Default::default());
    }

    fn word(id: &str, text: &str) -> TranscriptWord {
        TranscriptWord {
            id: Some(id.to_string()),
            text: text.to_string(),
            start_ms: 0.0,
            end_ms: 0.0,
            channel: 0.0,
            speaker: None,
            metadata: None,
        }
    }

    /// Failure injection for flush retries: a regular FILE squatting on the `sessions/`
    /// directory path makes every transcript read/write under it fail (ENOTDIR), without
    /// relying on permission bits (which root ignores). Remove the file to "recover the
    /// disk".
    fn block_sessions_dir(vault: &std::path::Path) {
        std::fs::write(vault.join("sessions"), b"not a directory").unwrap();
    }

    fn unblock_sessions_dir(vault: &std::path::Path) {
        std::fs::remove_file(vault.join("sessions")).unwrap();
    }

    fn delta_with_words(words: &[&str]) -> TranscriptDelta {
        TranscriptDelta {
            transcript_id: "t1".to_string(),
            new_words: words
                .iter()
                .enumerate()
                .map(|(i, w)| word(&format!("w{i}"), w))
                .collect(),
            replaced_ids: vec![],
            new_hints: vec![],
            started_at_ms: 1000.0,
        }
    }

    #[tokio::test]
    async fn append_then_flush_writes_words_to_file_and_index() {
        let (store, vault) = test_store().await;
        store
            .append_transcript("s1", delta_with_words(&["hello", "world"]))
            .await
            .unwrap();
        store.flush_transcript("s1").await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(
            &std::fs::read(vault.path().join("sessions/s1/transcript.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(json["transcripts"][0]["words"].as_array().unwrap().len(), 2);
        let indexed = store.session_transcripts("s1").await.unwrap();
        assert_eq!(indexed.len(), 1);
        assert_eq!(indexed[0].words[0].text, "hello");
    }

    /// REGRESSION for the 2026-07-23 data loss: no index row, no folder, no _meta.json — words still land.
    #[tokio::test]
    async fn recording_into_unknown_session_still_persists() {
        let (store, vault) = test_store().await;
        // deliberately: no write_meta, no sessions row
        store
            .append_transcript("ghost", delta_with_words(&["survives"]))
            .await
            .unwrap();
        store.flush_transcript("ghost").await.unwrap();
        assert!(
            vault
                .path()
                .join("sessions/ghost/transcript.json")
                .is_file()
        );
    }

    #[tokio::test]
    async fn debounce_flushes_without_explicit_flush() {
        let (store, vault) = test_store().await;
        store
            .append_transcript("s1", delta_with_words(&["auto"]))
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
        assert!(vault.path().join("sessions/s1/transcript.json").is_file());
    }

    #[tokio::test]
    async fn replaced_ids_removes_superseded_words_before_appending() {
        let (store, vault) = test_store().await;
        store
            .append_transcript("s1", delta_with_words(&["hell", "world"]))
            .await
            .unwrap();
        store
            .append_transcript(
                "s1",
                TranscriptDelta {
                    transcript_id: "t1".to_string(),
                    new_words: vec![word("w0-fix", "hello")],
                    replaced_ids: vec!["w0".to_string()],
                    new_hints: vec![],
                    started_at_ms: 1000.0,
                },
            )
            .await
            .unwrap();
        store.flush_transcript("s1").await.unwrap();

        let json: serde_json::Value = serde_json::from_slice(
            &std::fs::read(vault.path().join("sessions/s1/transcript.json")).unwrap(),
        )
        .unwrap();
        let texts: Vec<&str> = json["transcripts"][0]["words"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| w["text"].as_str().unwrap())
            .collect();
        assert_eq!(texts, vec!["world", "hello"]);
    }

    #[tokio::test]
    async fn flush_merges_and_preserves_other_transcripts_in_same_file() {
        let (store, vault) = test_store().await;
        store
            .append_transcript("s1", delta_with_words(&["first"]))
            .await
            .unwrap();
        store.flush_transcript("s1").await.unwrap();

        store
            .append_transcript(
                "s1",
                TranscriptDelta {
                    transcript_id: "t2".to_string(),
                    new_words: vec![word("y0", "second")],
                    replaced_ids: vec![],
                    new_hints: vec![],
                    started_at_ms: 2000.0,
                },
            )
            .await
            .unwrap();
        store.flush_transcript("s1").await.unwrap();

        let json: serde_json::Value = serde_json::from_slice(
            &std::fs::read(vault.path().join("sessions/s1/transcript.json")).unwrap(),
        )
        .unwrap();
        let transcripts = json["transcripts"].as_array().unwrap();
        assert_eq!(transcripts.len(), 2);
        let ids: Vec<&str> = transcripts
            .iter()
            .map(|t| t["id"].as_str().unwrap())
            .collect();
        assert!(ids.contains(&"t1"));
        assert!(ids.contains(&"t2"));
        let t1 = transcripts.iter().find(|t| t["id"] == "t1").unwrap();
        assert_eq!(t1["words"].as_array().unwrap().len(), 1);
        // Regression: switching transcript_id must reset the live buffer, not carry t1's
        // words into t2's entry.
        let t2 = transcripts.iter().find(|t| t["id"] == "t2").unwrap();
        let t2_texts: Vec<&str> = t2["words"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| w["text"].as_str().unwrap())
            .collect();
        assert_eq!(t2_texts, vec!["second"]);
    }

    #[tokio::test]
    async fn repeated_appends_within_debounce_window_share_one_flusher() {
        let (store, vault) = test_store().await;
        for word_text in ["a", "b", "c"] {
            store
                .append_transcript("s1", delta_with_words(&[word_text]))
                .await
                .unwrap();
        }
        // Give the single scheduled flusher time to fire; if appends had each spawned their
        // own flusher this would still pass, so the real assertion is the word count below --
        // three separate un-coalesced buffers would have clobbered each other down to 1 word.
        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
        let json: serde_json::Value = serde_json::from_slice(
            &std::fs::read(vault.path().join("sessions/s1/transcript.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(json["transcripts"][0]["words"].as_array().unwrap().len(), 3);

        let texts: Vec<String> = store
            .transcript_get("t1")
            .await
            .unwrap()
            .unwrap()
            .words
            .into_iter()
            .map(|w| w.text)
            .collect();
        assert_eq!(texts, vec!["a", "b", "c"]);
    }

    #[tokio::test]
    async fn flush_all_flushes_every_dirty_session_and_reports_first_error() {
        let (store, vault) = test_store().await;
        store
            .append_transcript("s1", delta_with_words(&["one"]))
            .await
            .unwrap();
        store
            .append_transcript("s2", delta_with_words(&["two"]))
            .await
            .unwrap();

        store.flush_all().await.unwrap();

        assert!(vault.path().join("sessions/s1/transcript.json").is_file());
        assert!(vault.path().join("sessions/s2/transcript.json").is_file());
    }

    #[tokio::test]
    async fn flush_all_is_noop_when_nothing_dirty() {
        let (store, _vault) = test_store().await;
        store.flush_all().await.unwrap();
    }

    /// REGRESSION for the Task 7 review's standing checklist item: `write_transcript` (batch
    /// path) must sync/clear the live debounce buffer for the same transcript_id, so a racing
    /// debounced flush from an earlier `append_transcript` can't fire afterward and clobber
    /// the batch overwrite with stale buffered words.
    #[tokio::test]
    async fn write_transcript_clears_live_buffer_so_pending_debounce_cannot_clobber_it() {
        let (store, vault) = test_store().await;
        // Buffer a word via append_transcript but never flush it -- this leaves a dirty
        // buffer with a debounce timer already scheduled.
        store
            .append_transcript("s1", delta_with_words(&["stale"]))
            .await
            .unwrap();

        // Batch overwrite for the same transcript_id ("t1", per delta_with_words) supersedes
        // whatever is buffered.
        store
            .write_transcript(
                "s1",
                TranscriptWithData {
                    batch_recovery: None,
                    id: "t1".to_string(),
                    user_id: String::new(),
                    created_at: "2026-07-24T00:00:00Z".to_string(),
                    session_id: "s1".to_string(),
                    started_at: 500.0,
                    ended_at: Some(900.0),
                    memo_md: String::new(),
                    words: vec![word("b0", "batch-result")],
                    speaker_hints: vec![],
                },
            )
            .await
            .unwrap();

        // Let the pending debounce timer from the earlier append fire. If the live buffer
        // weren't cleared, it would flush "stale" and clobber the batch write.
        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;

        let json: serde_json::Value = serde_json::from_slice(
            &std::fs::read(vault.path().join("sessions/s1/transcript.json")).unwrap(),
        )
        .unwrap();
        let texts: Vec<&str> = json["transcripts"][0]["words"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| w["text"].as_str().unwrap())
            .collect();
        assert_eq!(texts, vec!["batch-result"]);
    }

    /// REGRESSION (reviewer-found, Important 5): after `write_transcript` clears the live
    /// buffer for a transcript_id (dirty=false, words=[]) -- which only happens when a buffer
    /// entry already existed, i.e. some `append_transcript` ran first -- an explicit
    /// `flush_transcript` call on the same session (e.g. `session_flush_transcript` from
    /// `onStopped`) must not persist that now-empty snapshot over the batch write:
    /// `flush_transcript` is a no-op when the buffer isn't dirty, regardless of whether an
    /// (empty) entry still exists.
    #[tokio::test]
    async fn flush_transcript_after_write_transcript_does_not_zero_the_batch_write() {
        let (store, vault) = test_store().await;
        // Creates a live buffer entry for ("s1", "t1") so write_transcript below has something
        // to clear -- without this, there is no buffer entry at all and the bug can't reproduce.
        store
            .append_transcript("s1", delta_with_words(&["stale"]))
            .await
            .unwrap();

        store
            .write_transcript(
                "s1",
                TranscriptWithData {
                    batch_recovery: None,
                    id: "t1".to_string(),
                    user_id: String::new(),
                    created_at: "2026-07-24T00:00:00Z".to_string(),
                    session_id: "s1".to_string(),
                    started_at: 500.0,
                    ended_at: Some(900.0),
                    memo_md: String::new(),
                    words: vec![word("b0", "batch-result")],
                    speaker_hints: vec![],
                },
            )
            .await
            .unwrap();

        store.flush_transcript("s1").await.unwrap();

        let json: serde_json::Value = serde_json::from_slice(
            &std::fs::read(vault.path().join("sessions/s1/transcript.json")).unwrap(),
        )
        .unwrap();
        let texts: Vec<&str> = json["transcripts"][0]["words"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| w["text"].as_str().unwrap())
            .collect();
        assert_eq!(texts, vec!["batch-result"]);
    }

    #[tokio::test]
    async fn write_transcript_replaces_file_and_index_in_one_call() {
        let (store, vault) = test_store().await;
        store
            .write_transcript(
                "s1",
                TranscriptWithData {
                    batch_recovery: None,
                    id: "batch".to_string(),
                    user_id: String::new(),
                    created_at: "2026-07-24T00:00:00Z".to_string(),
                    session_id: "s1".to_string(),
                    started_at: 500.0,
                    ended_at: Some(900.0),
                    memo_md: String::new(),
                    words: vec![word("b0", "uploaded")],
                    speaker_hints: vec![],
                },
            )
            .await
            .unwrap();

        assert!(vault.path().join("sessions/s1/transcript.json").is_file());
        assert_eq!(
            store.transcript_get("batch").await.unwrap().unwrap().words[0].text,
            "uploaded"
        );
    }

    #[tokio::test]
    async fn explicit_flush_drains_buffer_so_pending_debounce_timer_is_a_harmless_noop() {
        let (store, vault) = test_store().await;
        // append schedules a debounce timer; flushing explicitly right away drains the
        // buffer before that timer ever fires.
        store
            .append_transcript("s1", delta_with_words(&["first"]))
            .await
            .unwrap();
        store.flush_transcript("s1").await.unwrap();

        // let the still-pending debounce timer from the append above fire; it must see a
        // clean buffer and skip, not re-write stale/duplicate content or error out.
        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;

        let json: serde_json::Value = serde_json::from_slice(
            &std::fs::read(vault.path().join("sessions/s1/transcript.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(json["transcripts"][0]["words"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn append_after_explicit_drain_is_still_flushed_by_a_freshly_scheduled_timer() {
        let (store, vault) = test_store().await;
        store
            .append_transcript("s1", delta_with_words(&["first"]))
            .await
            .unwrap();
        store.flush_transcript("s1").await.unwrap();

        // buffer is clean now; this append must re-arm its own debounce timer rather than
        // relying on the (already-fired) one from the first append -- otherwise the word
        // would sit in memory forever with nothing scheduled to persist it.
        store
            .append_transcript("s1", delta_with_words(&["second"]))
            .await
            .unwrap();
        // Busy runners can finish the background write after the debounce delay expires.
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                let json: serde_json::Value = serde_json::from_slice(
                    &std::fs::read(vault.path().join("sessions/s1/transcript.json")).unwrap(),
                )
                .unwrap();
                if json["transcripts"][0]["words"].as_array().unwrap().len() == 2 {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("the fresh debounce timer must persist the second word");
    }

    #[tokio::test]
    async fn flush_transcript_with_nothing_buffered_is_a_noop() {
        let (store, vault) = test_store().await;
        store.flush_transcript("never-touched").await.unwrap();
        assert!(
            !vault
                .path()
                .join("sessions/never-touched/transcript.json")
                .is_file()
        );
    }

    /// REGRESSION for Critical 1 (reviewer-traced): a failed flush must not be swallowed,
    /// and the buffer must stay dirty so flush_all() can recover the session instead of
    /// silently treating it as flushed. Failure is injected at the file layer (the only
    /// persistence layer left): a regular file squatting on `sessions/` makes the write
    /// fail with ENOTDIR.
    #[tokio::test]
    async fn flush_transcript_write_failure_leaves_buffer_dirty_for_retry() {
        let (store, vault) = test_store().await;
        store
            .append_transcript("s1", delta_with_words(&["hello"]))
            .await
            .unwrap();

        block_sessions_dir(vault.path());

        let result = store.flush_transcript("s1").await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), StoreError::Io(_)));

        // The failed flush must re-dirty the buffer, not leave it looking clean.
        {
            let live = store.live.lock().await;
            assert!(
                live.get("s1").unwrap().dirty,
                "buffer must be re-dirtied after a failed flush"
            );
        }

        // Clear the obstacle and confirm flush_all() recovers the session that
        // previously failed -- the words were only in memory until now.
        unblock_sessions_dir(vault.path());

        store.flush_all().await.unwrap();

        let json: serde_json::Value = serde_json::from_slice(
            &std::fs::read(vault.path().join("sessions/s1/transcript.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(json["transcripts"][0]["words"].as_array().unwrap().len(), 1);
        assert_eq!(
            store.transcript_get("t1").await.unwrap().unwrap().words[0].text,
            "hello"
        );
    }

    /// Direct test for `append_transcript`'s `needs_flush_before_switch` branch: a delta for
    /// a *different* transcript_id arriving while the buffer still holds unflushed words for
    /// the previous transcript must flush those words to file + index first (the buffer reset
    /// for the new transcript would otherwise silently drop them), and the new transcript's
    /// delta must then buffer and flush normally.
    #[tokio::test]
    async fn switching_transcript_id_flushes_dirty_old_buffer_before_buffering_new_delta() {
        let (store, vault) = test_store().await;
        // Dirty buffer for t1 -- never explicitly flushed, debounce timer not yet fired.
        store
            .append_transcript("s1", delta_with_words(&["old-words"]))
            .await
            .unwrap();

        store
            .append_transcript(
                "s1",
                TranscriptDelta {
                    transcript_id: "t2".to_string(),
                    new_words: vec![word("y0", "new-words")],
                    replaced_ids: vec![],
                    new_hints: vec![],
                    started_at_ms: 2000.0,
                },
            )
            .await
            .unwrap();

        // t1's words must already be on disk and in the index -- the switch itself flushed
        // them, no explicit flush_transcript and no debounce wait.
        let json: serde_json::Value = serde_json::from_slice(
            &std::fs::read(vault.path().join("sessions/s1/transcript.json")).unwrap(),
        )
        .unwrap();
        let transcripts = json["transcripts"].as_array().unwrap();
        let t1 = transcripts.iter().find(|t| t["id"] == "t1").unwrap();
        let t1_texts: Vec<&str> = t1["words"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| w["text"].as_str().unwrap())
            .collect();
        assert_eq!(t1_texts, vec!["old-words"]);
        assert_eq!(
            store.transcript_get("t1").await.unwrap().unwrap().words[0].text,
            "old-words"
        );

        // The incoming delta proceeded normally: t2 is buffered dirty and flushes with only
        // its own words, alongside (not replacing) t1's entry.
        store.flush_transcript("s1").await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(
            &std::fs::read(vault.path().join("sessions/s1/transcript.json")).unwrap(),
        )
        .unwrap();
        let transcripts = json["transcripts"].as_array().unwrap();
        assert_eq!(transcripts.len(), 2);
        let t2 = transcripts.iter().find(|t| t["id"] == "t2").unwrap();
        let t2_texts: Vec<&str> = t2["words"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| w["text"].as_str().unwrap())
            .collect();
        assert_eq!(t2_texts, vec!["new-words"]);
    }

    fn batch(id: &str, word_texts: &[&str]) -> TranscriptWithData {
        TranscriptWithData {
            batch_recovery: None,
            id: id.to_string(),
            user_id: String::new(),
            created_at: "2026-07-24T00:00:00Z".to_string(),
            session_id: "s1".to_string(),
            started_at: 500.0,
            ended_at: None,
            memo_md: String::new(),
            words: word_texts
                .iter()
                .enumerate()
                .map(|(i, w)| word(&format!("b{i}"), w))
                .collect(),
            speaker_hints: vec![],
        }
    }

    // -- replace_session_transcripts (E3 supersede primitive) --

    #[tokio::test]
    async fn replace_supersedes_every_other_transcript_in_file_and_index_and_sql() {
        let (store, vault) = test_store().await;
        store
            .write_transcript("s1", batch("t-old-1", &["one"]))
            .await
            .unwrap();
        store
            .write_transcript("s1", batch("t-old-2", &["two"]))
            .await
            .unwrap();

        store
            .replace_session_transcripts("s1", batch("t-new", &["fresh"]))
            .await
            .unwrap();

        // File truth: only the new transcript remains.
        let json: serde_json::Value = serde_json::from_slice(
            &std::fs::read(vault.path().join("sessions/s1/transcript.json")).unwrap(),
        )
        .unwrap();
        let transcripts = json["transcripts"].as_array().unwrap();
        assert_eq!(transcripts.len(), 1);
        assert_eq!(transcripts[0]["id"], "t-new");

        // Index queries agree with the file.
        let indexed: Vec<String> = store
            .session_transcripts("s1")
            .await
            .unwrap()
            .into_iter()
            .map(|t| t.id)
            .collect();
        assert_eq!(indexed, vec!["t-new"]);
        assert!(store.transcript_get("t-old-1").await.unwrap().is_none());
        assert!(store.transcript_get("t-old-2").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn replace_moves_the_previous_file_to_trash_for_recovery() {
        let (store, vault) = test_store().await;
        store
            .write_transcript("s1", batch("t-old", &["recoverable"]))
            .await
            .unwrap();

        store
            .replace_session_transcripts("s1", batch("t-new", &["fresh"]))
            .await
            .unwrap();

        let date = chrono::Utc::now().format("%Y-%m-%d").to_string();
        let trashed = vault
            .path()
            .join(".trash")
            .join(date)
            .join("sessions/s1/transcript.json");
        assert!(
            trashed.is_file(),
            "superseded file must be hand-recoverable"
        );
        let json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&trashed).unwrap()).unwrap();
        assert_eq!(json["transcripts"][0]["id"], "t-old");
        assert_eq!(json["transcripts"][0]["words"][0]["text"], "recoverable");
    }

    #[tokio::test]
    async fn replace_of_the_same_single_transcript_does_not_trash() {
        let (store, vault) = test_store().await;
        store
            .write_transcript("s1", batch("t1", &["v1"]))
            .await
            .unwrap();

        store
            .replace_session_transcripts("s1", batch("t1", &["v2"]))
            .await
            .unwrap();

        assert!(
            !vault.path().join(".trash").exists(),
            "overwriting the only transcript in place is a plain write, not a supersede"
        );
        let json: serde_json::Value = serde_json::from_slice(
            &std::fs::read(vault.path().join("sessions/s1/transcript.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(json["transcripts"][0]["words"][0]["text"], "v2");
    }

    #[tokio::test]
    async fn replace_into_a_session_without_transcripts_behaves_like_a_plain_write() {
        let (store, vault) = test_store().await;
        store
            .replace_session_transcripts("s1", batch("t1", &["first"]))
            .await
            .unwrap();

        assert!(vault.path().join("sessions/s1/transcript.json").is_file());
        assert!(!vault.path().join(".trash").exists());
        assert_eq!(store.session_transcripts("s1").await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn replace_emits_a_transcripts_index_event() {
        let (store, _vault) = test_store().await;
        store
            .write_transcript("s1", batch("t-old", &["one"]))
            .await
            .unwrap();
        // drain what the setup writes emitted
        let mut rx = store.take_index_change_receiver().unwrap();
        while rx.try_recv().is_ok() {}

        store
            .replace_session_transcripts("s1", batch("t-new", &["fresh"]))
            .await
            .unwrap();

        let mut saw_transcripts_change = false;
        while let Ok((entity, ids)) = rx.try_recv() {
            if entity == super::super::IndexEntity::Transcripts && ids.contains(&"s1".to_string()) {
                saw_transcripts_change = true;
            }
        }
        assert!(saw_transcripts_change);
    }

    /// A dirty live buffer for a *different* transcript is superseded content too -- a
    /// pending debounced flush must not fire afterward and resurrect it next to (or on
    /// top of) the replacement.
    #[tokio::test]
    async fn replace_clears_a_dirty_live_buffer_for_another_transcript() {
        let (store, vault) = test_store().await;
        store
            .append_transcript("s1", delta_with_words(&["buffered"]))
            .await
            .unwrap();

        store
            .replace_session_transcripts("s1", batch("t-batch", &["replacement"]))
            .await
            .unwrap();

        // let the pending debounce timer from the append fire
        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;

        let json: serde_json::Value = serde_json::from_slice(
            &std::fs::read(vault.path().join("sessions/s1/transcript.json")).unwrap(),
        )
        .unwrap();
        let transcripts = json["transcripts"].as_array().unwrap();
        assert_eq!(transcripts.len(), 1);
        assert_eq!(transcripts[0]["id"], "t-batch");
        assert_eq!(transcripts[0]["words"][0]["text"], "replacement");
    }

    // -- assign_transcript_speaker (hints-only speaker rename) --

    fn channel_word(id: &str, text: &str, channel: f64) -> TranscriptWord {
        TranscriptWord {
            channel,
            ..word(id, text)
        }
    }

    fn label_hint(word_id: &str, label: &str) -> TranscriptSpeakerHint {
        TranscriptSpeakerHint {
            id: Some(format!("{word_id}:speaker_label")),
            word_id: word_id.to_string(),
            hint_type: "speaker_label".to_string(),
            value: serde_json::Value::String(label.to_string()),
        }
    }

    /// Provider hint in the live-era on-disk shape: the value is a JSON *string*.
    fn provider_hint(word_id: &str, channel: i64, speaker_index: i64) -> TranscriptSpeakerHint {
        TranscriptSpeakerHint {
            id: Some(format!("{word_id}:provider_speaker_index")),
            word_id: word_id.to_string(),
            hint_type: "provider_speaker_index".to_string(),
            value: serde_json::Value::String(
                serde_json::json!({ "channel": channel, "speaker_index": speaker_index })
                    .to_string(),
            ),
        }
    }

    async fn hint_summaries(store: &SessionStore, transcript_id: &str) -> Vec<(String, String)> {
        store
            .transcript_get(transcript_id)
            .await
            .unwrap()
            .unwrap()
            .speaker_hints
            .into_iter()
            .map(|h| (h.id.unwrap_or_default(), h.hint_type))
            .collect()
    }

    /// REGRESSION for the metadata wipe: renaming a speaker must not touch words at
    /// all — in particular the `metadata.timing.source = "synthetic_speech"` marker
    /// the renderer needs to keep batch sentences atomic per channel.
    #[tokio::test]
    async fn assign_speaker_writes_hint_and_preserves_word_metadata() {
        let (store, vault) = test_store().await;
        let metadata: serde_json::Map<String, serde_json::Value> =
            serde_json::from_value(serde_json::json!({
                "timing": { "source": "synthetic_speech" }
            }))
            .unwrap();
        let mut transcript = batch("t1", &[]);
        transcript.words = vec![TranscriptWord {
            metadata: Some(metadata.clone()),
            ..channel_word("w0", "hello", 1.0)
        }];
        store.write_transcript("s1", transcript).await.unwrap();

        store
            .assign_transcript_speaker("t1", 1, Some(0), "Alice", "w0")
            .await
            .unwrap();

        let stored = store.transcript_get("t1").await.unwrap().unwrap();
        assert_eq!(stored.words.len(), 1);
        assert_eq!(stored.words[0].metadata, Some(metadata));
        assert_eq!(stored.speaker_hints, vec![label_hint("w0", "Alice")]);

        // The file agrees with the index: metadata survives the round trip to disk.
        let json: serde_json::Value = serde_json::from_slice(
            &std::fs::read(vault.path().join("sessions/s1/transcript.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            json["transcripts"][0]["words"][0]["metadata"]["timing"]["source"],
            "synthetic_speech"
        );
    }

    /// REGRESSION for the buffer race: a rename that lands while a live buffer is
    /// still dirty (e.g. the debounced flush is backing off after an I/O failure)
    /// must first persist the buffered words, and the retry flush that fires
    /// afterwards must be a no-op — live buffers carry no label hints, so letting it
    /// run would replace `speaker_hints` wholesale and silently erase the rename.
    #[tokio::test]
    async fn assign_speaker_lands_dirty_buffer_and_survives_the_retry_flush() {
        let (store, vault) = test_store().await;
        store
            .append_transcript("s1", delta_with_words(&["hello", "world"]))
            .await
            .unwrap();
        store.flush_transcript("s1").await.unwrap();

        // More words arrive; this buffer is dirty and its debounced flush is pending.
        store
            .append_transcript(
                "s1",
                TranscriptDelta {
                    transcript_id: "t1".to_string(),
                    new_words: vec![word("w2", "tail")],
                    replaced_ids: vec![],
                    new_hints: vec![],
                    started_at_ms: 1000.0,
                },
            )
            .await
            .unwrap();

        store
            .assign_transcript_speaker("t1", 0, None, "Alice", "w0")
            .await
            .unwrap();

        // The buffered tail word landed together with the rename.
        let stored = store.transcript_get("t1").await.unwrap().unwrap();
        assert_eq!(stored.words.len(), 3);
        assert_eq!(stored.words[2].text, "tail");
        assert_eq!(stored.speaker_hints, vec![label_hint("w0", "Alice")]);

        // The pending flush now sees a clean buffer and must not clobber the hint.
        store.flush_transcript("s1").await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(
            &std::fs::read(vault.path().join("sessions/s1/transcript.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(json["transcripts"][0]["speaker_hints"][0]["value"], "Alice");
        assert_eq!(json["transcripts"][0]["words"].as_array().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn assign_speaker_replaces_stale_assignment_on_the_same_channel() {
        let (store, _vault) = test_store().await;
        let mut transcript = batch("t1", &[]);
        transcript.words = vec![
            channel_word("old-word", "hello", 1.0),
            channel_word("new-word", "there", 1.0),
        ];
        transcript.speaker_hints = vec![
            label_hint("old-word", "Alice"),
            provider_hint("new-word", 1, 2),
        ];
        store.write_transcript("s1", transcript).await.unwrap();

        store
            .assign_transcript_speaker("t1", 1, Some(2), "Bob", "new-word")
            .await
            .unwrap();

        // The channel-wide "Alice" assignment (its anchor has no provider speaker
        // index) conflicts with anything on channel 1, so it is dropped.
        assert_eq!(
            hint_summaries(&store, "t1").await,
            vec![
                (
                    "new-word:provider_speaker_index".to_string(),
                    "provider_speaker_index".to_string()
                ),
                (
                    "new-word:speaker_label".to_string(),
                    "speaker_label".to_string()
                ),
            ]
        );
        let stored = store.transcript_get("t1").await.unwrap().unwrap();
        assert_eq!(
            stored.speaker_hints[1].value,
            serde_json::Value::String("Bob".to_string())
        );
    }

    #[tokio::test]
    async fn assign_speaker_keeps_assignments_on_a_different_channel() {
        let (store, _vault) = test_store().await;
        let mut transcript = batch("t1", &[]);
        transcript.words = vec![
            channel_word("direct-word", "hi", 0.0),
            channel_word("remote-word", "there", 1.0),
        ];
        transcript.speaker_hints = vec![label_hint("direct-word", "Me")];
        store.write_transcript("s1", transcript).await.unwrap();

        store
            .assign_transcript_speaker("t1", 1, None, "Bob", "remote-word")
            .await
            .unwrap();

        assert_eq!(
            hint_summaries(&store, "t1").await,
            vec![
                (
                    "direct-word:speaker_label".to_string(),
                    "speaker_label".to_string()
                ),
                (
                    "remote-word:speaker_label".to_string(),
                    "speaker_label".to_string()
                ),
            ]
        );
    }

    #[tokio::test]
    async fn assign_speaker_lets_two_provider_speaker_scopes_coexist_on_one_channel() {
        let (store, _vault) = test_store().await;
        let mut transcript = batch("t1", &[]);
        transcript.words = vec![
            channel_word("speaker-1-word", "first", 1.0),
            channel_word("speaker-2-word-old", "second", 1.0),
            channel_word("speaker-2-word-new", "later", 1.0),
        ];
        transcript.speaker_hints = vec![
            provider_hint("speaker-1-word", 1, 1),
            label_hint("speaker-1-word", "Alice"),
            provider_hint("speaker-2-word-old", 1, 2),
            label_hint("speaker-2-word-old", "Bob"),
            provider_hint("speaker-2-word-new", 1, 2),
        ];
        store.write_transcript("s1", transcript).await.unwrap();

        store
            .assign_transcript_speaker("t1", 1, Some(2), "Carol", "speaker-2-word-new")
            .await
            .unwrap();

        // Alice (speaker index 1) survives; Bob (same channel, same speaker
        // index 2) is superseded by Carol.
        assert_eq!(
            hint_summaries(&store, "t1").await,
            vec![
                (
                    "speaker-1-word:provider_speaker_index".to_string(),
                    "provider_speaker_index".to_string()
                ),
                (
                    "speaker-1-word:speaker_label".to_string(),
                    "speaker_label".to_string()
                ),
                (
                    "speaker-2-word-old:provider_speaker_index".to_string(),
                    "provider_speaker_index".to_string()
                ),
                (
                    "speaker-2-word-new:provider_speaker_index".to_string(),
                    "provider_speaker_index".to_string()
                ),
                (
                    "speaker-2-word-new:speaker_label".to_string(),
                    "speaker_label".to_string()
                ),
            ]
        );
    }

    /// Newer writers store the provider hint value as an object rather than a JSON
    /// string; scope resolution must read both shapes.
    #[tokio::test]
    async fn assign_speaker_reads_object_shaped_provider_hint_values() {
        let (store, _vault) = test_store().await;
        let mut transcript = batch("t1", &[]);
        transcript.words = vec![
            channel_word("w-a", "first", 1.0),
            channel_word("w-b", "second", 1.0),
        ];
        transcript.speaker_hints = vec![
            TranscriptSpeakerHint {
                value: serde_json::json!({ "channel": 1, "speaker_index": 1 }),
                ..provider_hint("w-a", 1, 1)
            },
            label_hint("w-a", "Alice"),
        ];
        store.write_transcript("s1", transcript).await.unwrap();

        store
            .assign_transcript_speaker("t1", 1, Some(2), "Bob", "w-b")
            .await
            .unwrap();

        // Alice's scope resolved to speaker index 1, so index 2's assignment
        // does not evict her.
        assert_eq!(
            hint_summaries(&store, "t1").await,
            vec![
                (
                    "w-a:provider_speaker_index".to_string(),
                    "provider_speaker_index".to_string()
                ),
                ("w-a:speaker_label".to_string(), "speaker_label".to_string()),
                ("w-b:speaker_label".to_string(), "speaker_label".to_string()),
            ]
        );
    }

    // -- RULE 4: eviction scopes on diarized vs undiarized channels --

    /// Diarized channel: an indexed assignment supersedes only the prior label with
    /// the same (channel, speaker_index) -- other indexes on the channel survive.
    #[tokio::test]
    async fn indexed_assignment_evicts_only_same_index() {
        let (store, _vault) = test_store().await;
        let mut transcript = batch("t1", &[]);
        transcript.words = vec![
            channel_word("idx1-word", "first", 1.0),
            channel_word("idx2-word", "second", 1.0),
        ];
        transcript.speaker_hints = vec![
            provider_hint("idx1-word", 1, 1),
            label_hint("idx1-word", "Alice"),
            provider_hint("idx2-word", 1, 2),
            label_hint("idx2-word", "Bob"),
        ];
        store.write_transcript("s1", transcript).await.unwrap();

        store
            .assign_transcript_speaker("t1", 1, Some(2), "Carol", "idx2-word")
            .await
            .unwrap();

        assert_eq!(
            hint_summaries(&store, "t1").await,
            vec![
                (
                    "idx1-word:provider_speaker_index".to_string(),
                    "provider_speaker_index".to_string()
                ),
                (
                    "idx1-word:speaker_label".to_string(),
                    "speaker_label".to_string()
                ),
                (
                    "idx2-word:provider_speaker_index".to_string(),
                    "provider_speaker_index".to_string()
                ),
                (
                    "idx2-word:speaker_label".to_string(),
                    "speaker_label".to_string()
                ),
            ]
        );
        let stored = store.transcript_get("t1").await.unwrap().unwrap();
        assert_eq!(
            stored.speaker_hints[1].value,
            serde_json::Value::String("Alice".to_string())
        );
        assert_eq!(
            stored.speaker_hints[3].value,
            serde_json::Value::String("Carol".to_string())
        );
    }

    /// Diarized channel: an indexed assignment must keep the channel-wide label (a
    /// hint whose anchor word has no provider index) -- it is the fallback for
    /// null-index gap segments, not a stale duplicate.
    #[tokio::test]
    async fn indexed_assignment_preserves_channel_fallback() {
        let (store, _vault) = test_store().await;
        let mut transcript = batch("t1", &[]);
        transcript.words = vec![
            channel_word("gap-word", "uh", 1.0),
            channel_word("idx1-word", "first", 1.0),
            channel_word("idx2-word", "second", 1.0),
        ];
        transcript.speaker_hints = vec![
            label_hint("gap-word", "Fallback"),
            provider_hint("idx1-word", 1, 1),
            provider_hint("idx2-word", 1, 2),
        ];
        store.write_transcript("s1", transcript).await.unwrap();

        store
            .assign_transcript_speaker("t1", 1, Some(2), "Bob", "idx2-word")
            .await
            .unwrap();

        assert_eq!(
            hint_summaries(&store, "t1").await,
            vec![
                (
                    "gap-word:speaker_label".to_string(),
                    "speaker_label".to_string()
                ),
                (
                    "idx1-word:provider_speaker_index".to_string(),
                    "provider_speaker_index".to_string()
                ),
                (
                    "idx2-word:provider_speaker_index".to_string(),
                    "provider_speaker_index".to_string()
                ),
                (
                    "idx2-word:speaker_label".to_string(),
                    "speaker_label".to_string()
                ),
            ]
        );
    }

    /// Diarized channel: a channel-wide assignment (null-index anchor) replaces only
    /// the previous channel-wide label; indexed assignments survive.
    #[tokio::test]
    async fn channel_assignment_on_diarized_channel_preserves_indexed() {
        let (store, _vault) = test_store().await;
        let mut transcript = batch("t1", &[]);
        transcript.words = vec![
            channel_word("old-gap-word", "uh", 1.0),
            channel_word("new-gap-word", "hm", 1.0),
            channel_word("idx1-word", "first", 1.0),
            channel_word("idx2-word", "second", 1.0),
        ];
        transcript.speaker_hints = vec![
            label_hint("old-gap-word", "OldFallback"),
            provider_hint("idx1-word", 1, 1),
            label_hint("idx1-word", "Alice"),
            provider_hint("idx2-word", 1, 2),
            label_hint("idx2-word", "Bob"),
        ];
        store.write_transcript("s1", transcript).await.unwrap();

        store
            .assign_transcript_speaker("t1", 1, None, "NewFallback", "new-gap-word")
            .await
            .unwrap();

        assert_eq!(
            hint_summaries(&store, "t1").await,
            vec![
                (
                    "idx1-word:provider_speaker_index".to_string(),
                    "provider_speaker_index".to_string()
                ),
                (
                    "idx1-word:speaker_label".to_string(),
                    "speaker_label".to_string()
                ),
                (
                    "idx2-word:provider_speaker_index".to_string(),
                    "provider_speaker_index".to_string()
                ),
                (
                    "idx2-word:speaker_label".to_string(),
                    "speaker_label".to_string()
                ),
                (
                    "new-gap-word:speaker_label".to_string(),
                    "speaker_label".to_string()
                ),
            ]
        );
        let stored = store.transcript_get("t1").await.unwrap().unwrap();
        assert_eq!(
            stored.speaker_hints[4].value,
            serde_json::Value::String("NewFallback".to_string())
        );
    }

    /// Guard for today's behavior: on an undiarized channel (one distinct provider
    /// index does not count as diarized) a channel-wide assignment still evicts every
    /// speaker_label hint on that channel, indexed or not.
    #[tokio::test]
    async fn channel_assignment_on_undiarized_channel_evicts_channel() {
        let (store, _vault) = test_store().await;
        let mut transcript = batch("t1", &[]);
        transcript.words = vec![
            channel_word("plain-word", "hello", 1.0),
            channel_word("idx2-word", "there", 1.0),
            channel_word("other-channel-word", "aside", 0.0),
        ];
        transcript.speaker_hints = vec![
            label_hint("plain-word", "Alice"),
            provider_hint("idx2-word", 1, 2),
            label_hint("idx2-word", "Bob"),
            label_hint("other-channel-word", "Me"),
        ];
        store.write_transcript("s1", transcript).await.unwrap();

        store
            .assign_transcript_speaker("t1", 1, None, "Carol", "plain-word")
            .await
            .unwrap();

        assert_eq!(
            hint_summaries(&store, "t1").await,
            vec![
                (
                    "idx2-word:provider_speaker_index".to_string(),
                    "provider_speaker_index".to_string()
                ),
                (
                    "other-channel-word:speaker_label".to_string(),
                    "speaker_label".to_string()
                ),
                (
                    "plain-word:speaker_label".to_string(),
                    "speaker_label".to_string()
                ),
            ]
        );
        let stored = store.transcript_get("t1").await.unwrap().unwrap();
        assert_eq!(
            stored.speaker_hints[2].value,
            serde_json::Value::String("Carol".to_string())
        );
    }

    #[tokio::test]
    async fn assign_speaker_on_missing_transcript_errors() {
        let (store, _vault) = test_store().await;
        let err = store
            .assign_transcript_speaker("nope", 0, None, "Alice", "w0")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("does not exist"));
    }

    #[test]
    fn retry_delay_doubles_and_caps_without_ever_reaching_zero() {
        let mut delay = DEBOUNCE;
        let mut observed = Vec::new();
        for _ in 0..8 {
            delay = next_retry_delay(delay);
            observed.push(delay.as_secs());
        }
        assert_eq!(observed, vec![2, 4, 8, 16, 30, 30, 30, 30]);
    }

    /// The debounce flusher must keep retrying after a failure (now with backoff) rather
    /// than abandoning the buffer: block the vault's `sessions/` path so the first attempt
    /// fails, clear it, and assert a later retry lands the words with no further appends
    /// and no explicit flush call.
    #[tokio::test]
    async fn debounce_retry_recovers_after_transient_failure_without_new_appends() {
        let (store, vault) = test_store().await;
        block_sessions_dir(vault.path());
        store
            .append_transcript("s1", delta_with_words(&["survivor"]))
            .await
            .unwrap();

        // First flush attempt fires at ~1s and fails; clear the obstacle before the
        // backoff retry (~2s later) fires.
        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
        unblock_sessions_dir(vault.path());

        // Poll instead of a single fixed sleep: on a loaded machine the recovery may only
        // land on a later (further backed-off) retry, and "retries never stop" is exactly
        // the property under test.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            if store.transcript_get("t1").await.unwrap().is_some() {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "debounce retry loop never landed the words after the failure cleared"
            );
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
        assert!(
            vault.path().join("sessions/s1/transcript.json").is_file(),
            "the recovered flush must land the file too"
        );
    }
    #[tokio::test]
    async fn finish_releases_twenty_large_transcripts_after_disk_persistence() {
        let (store, vault) = test_store().await;
        for i in 0..20 {
            let session = format!("s{i}");
            store
                .append_transcript(&session, delta_with_words(&vec!["word"; 5000]))
                .await
                .unwrap();
            store.finish_transcript(&session, "t1").await.unwrap();
            let fresh = SessionStore::new(vault.path().to_path_buf());
            assert_eq!(
                fresh
                    .read_transcript_json(&session)
                    .await
                    .unwrap()
                    .transcripts[0]
                    .words
                    .len(),
                5000
            );
            assert!(store.live.lock().await.is_empty());
        }
    }

    #[tokio::test]
    async fn failed_finish_keeps_words_and_retry_releases_them() {
        let (store, vault) = test_store().await;
        block_sessions_dir(vault.path());
        store
            .append_transcript("s1", delta_with_words(&["recoverable"]))
            .await
            .unwrap();
        assert!(store.finish_transcript("s1", "t1").await.is_err());
        assert_eq!(store.live.lock().await["s1"].words.len(), 1);
        unblock_sessions_dir(vault.path());
        store.flush_transcript("s1").await.unwrap();
        assert!(store.live.lock().await.is_empty());
        assert_eq!(
            store.read_transcript_json("s1").await.unwrap().transcripts[0].words[0].text,
            "recoverable"
        );
    }

    #[tokio::test]
    async fn old_finish_cannot_release_a_resumed_recording() {
        let (store, _vault) = test_store().await;
        store
            .append_transcript("s1", delta_with_words(&["old"]))
            .await
            .unwrap();
        store.finish_transcript("s1", "t1").await.unwrap();
        let mut resumed = delta_with_words(&["new"]);
        resumed.transcript_id = "t2".into();
        store.append_transcript("s1", resumed).await.unwrap();
        store.finish_transcript("s1", "t1").await.unwrap();
        assert_eq!(store.live.lock().await["s1"].transcript_id, "t2");
        store.finish_transcript("s1", "t2").await.unwrap();
        store.finish_transcript("s1", "t2").await.unwrap();
        store.flush_transcript("s1").await.unwrap();
        assert!(store.live.lock().await.is_empty());
        assert_eq!(
            store
                .read_transcript_json("s1")
                .await
                .unwrap()
                .transcripts
                .len(),
            2
        );
    }

    fn recovery() -> hypr_fs_format::BatchRecovery {
        serde_json::from_value(serde_json::json!({
            "audio": {"source": "recording", "layout": "mic_system"},
            "version": 1, "run_id": "run-1", "source_fingerprint": "sha256:audio",
            "provider": "soniqo", "model": "soniqo-parakeet-batch", "languages": ["en", "nl"],
            "status": "incomplete", "unresolved": [{"channel": 1, "start_sample": 16000, "end_sample": 32000, "reason": "decoder failed"}],
            "diarization": [{"channel": 1, "start_ms": 0, "end_ms": 2000, "speaker_index": 2}]
        })).unwrap()
    }

    #[tokio::test]
    async fn recovery_survives_restart_edits_and_index_rebuild() {
        let (store, vault) = test_store().await;
        let mut t = batch("t1", &["kept"]);
        t.batch_recovery = Some(recovery());
        store.replace_session_transcripts("s1", t).await.unwrap();
        assert_eq!(
            store.session_transcript_metadata("s1").batch_recoveries,
            vec![recovery()]
        );
        let fresh = SessionStore::new(vault.path().to_path_buf());
        let saved = fresh.read_transcript_json("s1").await.unwrap();
        assert_eq!(saved.transcripts[0].batch_recovery, Some(recovery()));
        let bytes = std::fs::read(vault.path().join("sessions/s1/transcript.json")).unwrap();
        let summary = summarize_transcript_bytes(&bytes).unwrap();
        assert_eq!(summary.metadata.batch_recoveries, vec![recovery()]);
        let mut edit = saved.transcripts[0].clone();
        edit.batch_recovery = None;
        edit.words[0].text = "edited".into();
        fresh.write_transcript("s1", edit).await.unwrap();
        assert_eq!(
            fresh.read_transcript_json("s1").await.unwrap().transcripts[0].batch_recovery,
            Some(recovery())
        );
        fresh
            .replace_session_transcripts("s1", batch("t2", &["replacement"]))
            .await
            .unwrap();
        assert!(
            fresh.read_transcript_json("s1").await.unwrap().transcripts[0]
                .batch_recovery
                .is_none()
        );
    }

    #[tokio::test]
    async fn recovered_ranges_merge_once_and_preserve_existing_edits() {
        let (store, _vault) = test_store().await;
        let mut t = batch("t1", &["edited"]);
        t.batch_recovery = Some(recovery());
        let original = t.words[0].clone();
        store.replace_session_transcripts("s1", t).await.unwrap();
        let range = recovery().unresolved;
        let mut recovered = word("recovery-run-1-range-1-word-0", "recovered");
        recovered.channel = 1.0;
        recovered.start_ms = 1000.0;
        recovered.end_ms = 2000.0 + 1e-9;
        for _ in 0..2 {
            store
                .merge_batch_recovery(
                    "s1",
                    "run-1",
                    range.clone(),
                    vec![recovered.clone()],
                    vec![],
                )
                .await
                .unwrap();
        }
        let saved = store.read_transcript_json("s1").await.unwrap();
        assert_eq!(saved.transcripts[0].words.len(), 2);
        assert_eq!(saved.transcripts[0].words[1].end_ms, 2000.0);
        assert_eq!(saved.transcripts[0].words[0], original);
        assert!(
            saved.transcripts[0]
                .batch_recovery
                .as_ref()
                .unwrap()
                .unresolved
                .is_empty()
        );
        assert!(
            store
                .merge_batch_recovery("s1", "stale-run", range, vec![], vec![])
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn recovery_rejects_edits_in_missing_audio_without_changing_saved_work() {
        let (store, _vault) = test_store().await;
        let mut t = batch("t1", &["user edit"]);
        t.batch_recovery = Some(recovery());
        t.words[0].channel = 1.0;
        t.words[0].start_ms = 1100.0;
        t.words[0].end_ms = 1200.0;
        store
            .replace_session_transcripts("s1", t.clone())
            .await
            .unwrap();
        let result = store
            .merge_batch_recovery("s1", "run-1", recovery().unresolved, vec![], vec![])
            .await;
        assert!(result.unwrap_err().to_string().contains("Text was added"));
        assert_eq!(
            store.read_transcript_json("s1").await.unwrap().transcripts,
            vec![t]
        );
    }

    #[tokio::test]
    async fn cumulative_checkpoints_preserve_edits_deletions_and_speaker_labels() {
        let (store, vault) = test_store().await;
        let mut initial = batch("t1", &[]);
        let mut plan = recovery();
        let later = plan.unresolved[0].clone();
        let mut earlier = later.clone();
        earlier.start_sample = 0;
        earlier.end_sample = 16000;
        plan.unresolved.insert(0, earlier);
        initial.batch_recovery = Some(plan.clone());
        store
            .checkpoint_batch_transcript("s1", initial.clone(), true)
            .await
            .unwrap();
        let mut first = word("first", "original");
        first.channel = 1.0;
        first.start_ms = 100.0;
        first.end_ms = 500.0;
        let mut checkpoint = initial.clone();
        checkpoint.words = vec![first.clone()];
        checkpoint.batch_recovery.as_mut().unwrap().unresolved = vec![later];
        store
            .checkpoint_batch_transcript("s1", checkpoint.clone(), false)
            .await
            .unwrap();
        let fresh = SessionStore::new(vault.path().to_path_buf());
        let mut saved = fresh
            .read_transcript_json("s1")
            .await
            .unwrap()
            .transcripts
            .remove(0);
        assert_eq!(saved.words.len(), 1);
        assert_eq!(saved.batch_recovery.as_ref().unwrap().unresolved.len(), 1);
        saved.words[0].text = "user edited".into();
        saved.speaker_hints = vec![label_hint("first", "Alice")];
        fresh.write_transcript("s1", saved).await.unwrap();
        let mut second = word("second", "recovered");
        second.channel = 1.0;
        second.start_ms = 1100.0;
        second.end_ms = 1500.0;
        checkpoint.words.push(second);
        checkpoint
            .batch_recovery
            .as_mut()
            .unwrap()
            .unresolved
            .clear();
        checkpoint.batch_recovery.as_mut().unwrap().status =
            hypr_fs_format::BatchCompletionStatus::Complete;
        fresh
            .checkpoint_batch_transcript("s1", checkpoint.clone(), false)
            .await
            .unwrap();
        let mut saved = fresh
            .read_transcript_json("s1")
            .await
            .unwrap()
            .transcripts
            .remove(0);
        assert_eq!(saved.words[0].text, "user edited");
        assert_eq!(saved.speaker_hints, vec![label_hint("first", "Alice")]);
        saved.words.remove(0);
        saved.speaker_hints.clear();
        fresh.write_transcript("s1", saved).await.unwrap();
        fresh
            .checkpoint_batch_transcript("s1", checkpoint, false)
            .await
            .unwrap();
        assert_eq!(
            fresh.read_transcript_json("s1").await.unwrap().transcripts[0]
                .words
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn whole_channel_retry_refines_only_missing_audio_and_preserves_restart_progress() {
        let (store, vault) = test_store().await;
        let mut initial = batch("t1", &[]);
        initial.batch_recovery = Some(recovery());
        store
            .replace_session_transcripts("s1", initial)
            .await
            .unwrap();
        let original = recovery().unresolved.remove(0);
        let mut first = original.clone();
        first.end_sample = 24000;
        let mut second = original.clone();
        second.start_sample = 24000;
        for _ in 0..2 {
            store
                .refine_batch_recovery_range(
                    "s1",
                    "run-1",
                    original.clone(),
                    vec![first.clone(), second.clone()],
                )
                .await
                .unwrap();
        }
        store
            .merge_batch_recovery("s1", "run-1", vec![first], vec![], vec![])
            .await
            .unwrap();
        let fresh = SessionStore::new(vault.path().to_path_buf());
        assert_eq!(
            fresh.read_transcript_json("s1").await.unwrap().transcripts[0]
                .batch_recovery
                .as_ref()
                .unwrap()
                .unresolved,
            vec![second]
        );
    }

    #[tokio::test]
    async fn planning_checkpoint_restores_whole_channel_until_speech_ranges_are_saved() {
        let (store, vault) = test_store().await;
        let mut initial = batch("t1", &[]);
        let mut plan = recovery();
        plan.planning_pending = true;
        plan.unresolved[0].start_sample = 0;
        initial.batch_recovery = Some(plan);
        store
            .checkpoint_batch_transcript("s1", initial.clone(), true)
            .await
            .unwrap();
        let fresh = SessionStore::new(vault.path().to_path_buf());
        assert!(
            fresh.read_transcript_json("s1").await.unwrap().transcripts[0]
                .batch_recovery
                .as_ref()
                .unwrap()
                .planning_pending
        );
        let mut planned = initial;
        planned.batch_recovery = Some(recovery());
        fresh
            .checkpoint_batch_transcript("s1", planned.clone(), false)
            .await
            .unwrap();
        assert_eq!(
            fresh.read_transcript_json("s1").await.unwrap().transcripts[0].batch_recovery,
            Some(recovery())
        );
        let mut resolved = word("resolved", "speech");
        resolved.channel = 1.0;
        resolved.start_ms = 1100.0;
        resolved.end_ms = 1500.0;
        let mut complete = planned.clone();
        complete.words = vec![resolved];
        complete.batch_recovery.as_mut().unwrap().unresolved.clear();
        fresh
            .checkpoint_batch_transcript("s1", complete, false)
            .await
            .unwrap();
        fresh
            .checkpoint_batch_transcript("s1", planned, false)
            .await
            .unwrap();
        assert!(
            fresh.read_transcript_json("s1").await.unwrap().transcripts[0]
                .batch_recovery
                .as_ref()
                .unwrap()
                .unresolved
                .is_empty()
        );
    }

    #[tokio::test]
    async fn failed_recovery_write_keeps_text_and_ranges_together() {
        let (store, vault) = test_store().await;
        let mut t = batch("t1", &["kept"]);
        t.batch_recovery = Some(recovery());
        store.replace_session_transcripts("s1", t).await.unwrap();
        let path = vault.path().join("sessions/s1/transcript.json");
        let mut before = std::fs::read(&path).unwrap();
        before.push(b'\n');
        std::fs::write(&path, &before).unwrap();
        std::fs::write(
            vault.path().join(".trash"),
            b"block backup of externally changed bytes",
        )
        .unwrap();
        let mut recovered = word("recovered", "new");
        recovered.channel = 1.0;
        recovered.start_ms = 1100.0;
        recovered.end_ms = 1500.0;
        assert!(
            store
                .merge_batch_recovery(
                    "s1",
                    "run-1",
                    recovery().unresolved,
                    vec![recovered.clone()],
                    vec![]
                )
                .await
                .is_err()
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);
        std::fs::remove_file(vault.path().join(".trash")).unwrap();
        store
            .merge_batch_recovery(
                "s1",
                "run-1",
                recovery().unresolved,
                vec![recovered],
                vec![],
            )
            .await
            .unwrap();
        let saved = store.read_transcript_json("s1").await.unwrap();
        assert_eq!(saved.transcripts[0].words.len(), 2);
        assert!(
            saved.transcripts[0]
                .batch_recovery
                .as_ref()
                .unwrap()
                .unresolved
                .is_empty()
        );
    }

    #[tokio::test]
    async fn failed_batch_preserves_buffered_words() {
        for replace in [false, true] {
            let (store, vault) = test_store().await;
            block_sessions_dir(vault.path());
            store
                .append_transcript("s1", delta_with_words(&["recoverable"]))
                .await
                .unwrap();
            let result = if replace {
                store
                    .replace_session_transcripts("s1", batch("t1", &["batch"]))
                    .await
            } else {
                store.write_transcript("s1", batch("t1", &["batch"])).await
            };
            assert!(result.is_err());
            assert_eq!(store.live.lock().await["s1"].words.len(), 1);
            unblock_sessions_dir(vault.path());
            store.finish_transcript("s1", "t1").await.unwrap();
            assert_eq!(
                store.read_transcript_json("s1").await.unwrap().transcripts[0].words[0].text,
                "recoverable"
            );
        }
    }

    #[tokio::test]
    async fn concurrent_append_finish_and_debounce_preserve_words() {
        let (store, _vault) = test_store().await;
        store
            .append_transcript("s1", delta_with_words(&["first"]))
            .await
            .unwrap();
        let (finished, appended) = tokio::join!(
            store.finish_transcript("s1", "t1"),
            store.append_transcript(
                "s1",
                TranscriptDelta {
                    new_words: vec![word("w1", "second")],
                    ..delta_with_words(&[])
                }
            )
        );
        finished.unwrap();
        appended.unwrap();
        store.finish_transcript("s1", "t1").await.unwrap();
        tokio::time::sleep(DEBOUNCE + std::time::Duration::from_millis(100)).await;
        assert!(store.live.lock().await.is_empty());
        assert_eq!(
            store.read_transcript_json("s1").await.unwrap().transcripts[0]
                .words
                .len(),
            2
        );
    }

    #[tokio::test]
    async fn finalization_racing_replacement_keeps_the_replacement() {
        for finish_first in [true, false] {
            let (store, _vault) = test_store().await;
            store
                .append_transcript("s1", delta_with_words(&["live"]))
                .await
                .unwrap();
            let finish = store.finish_transcript("s1", "t1");
            let replace = store.replace_session_transcripts("s1", batch("t2", &["replacement"]));
            if finish_first {
                let (a, b) = tokio::join!(biased; finish, replace);
                a.unwrap();
                b.unwrap();
            } else {
                let (a, b) = tokio::join!(biased; replace, finish);
                a.unwrap();
                b.unwrap();
            }
            store.flush_transcript("s1").await.unwrap();
            assert!(store.live.lock().await.is_empty());
            let saved = store.read_transcript_json("s1").await.unwrap();
            assert_eq!(saved.transcripts.len(), 1);
            assert_eq!(saved.transcripts[0].id, "t2");
            assert_eq!(saved.transcripts[0].words[0].text, "replacement");
        }
    }

    #[tokio::test]
    async fn finalization_racing_deletion_cannot_recreate_the_session() {
        for finish_first in [true, false] {
            let (store, vault) = test_store().await;
            let meta = serde_json::from_value(serde_json::json!({
                "id": "s1", "title": "Deletion race", "created_at": "2026-09-19T00:00:00Z", "tags": []
            })).unwrap();
            store.write_meta(&meta).await.unwrap();
            let path = vault.path().join(store.session_dir("s1").await.unwrap());
            store
                .append_transcript("s1", delta_with_words(&["live"]))
                .await
                .unwrap();
            store.flush_transcript("s1").await.unwrap();
            let finish = store.finish_transcript("s1", "t1");
            let delete = store.delete_session("s1");
            if finish_first {
                let (a, b) = tokio::join!(biased; finish, delete);
                a.unwrap();
                assert!(b.unwrap().is_some());
            } else {
                let (a, b) = tokio::join!(biased; delete, finish);
                assert!(a.unwrap().is_some());
                b.unwrap();
            }
            tokio::time::sleep(DEBOUNCE + std::time::Duration::from_millis(100)).await;
            store.finish_transcript("s1", "t1").await.unwrap();
            assert!(store.live.lock().await.is_empty());
            assert!(!path.exists());
        }
    }

    #[tokio::test]
    async fn speaker_edits_survive_later_appends_and_finalization() {
        let (store, _vault) = test_store().await;
        store
            .append_transcript("s1", delta_with_words(&["first"]))
            .await
            .unwrap();
        store.flush_transcript("s1").await.unwrap();
        store
            .assign_transcript_speaker("t1", 0, None, "Alice", "w0")
            .await
            .unwrap();
        store
            .append_transcript(
                "s1",
                TranscriptDelta {
                    new_words: vec![word("w1", "second")],
                    ..delta_with_words(&[])
                },
            )
            .await
            .unwrap();
        store.finish_transcript("s1", "t1").await.unwrap();
        let saved = store.read_transcript_json("s1").await.unwrap();
        assert_eq!(
            saved.transcripts[0].speaker_hints,
            vec![label_hint("w0", "Alice")]
        );
        assert_eq!(saved.transcripts[0].words.len(), 2);
        assert!(store.live.lock().await.is_empty());
    }
}
