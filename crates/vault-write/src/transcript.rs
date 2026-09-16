use hypr_fs_format::{
    TranscriptJson, TranscriptJsonStats, TranscriptSpeakerHint, TranscriptWithData, TranscriptWord,
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
    TranscriptSummary {
        transcript_ids: transcripts.iter().map(|t| t.id.clone()).collect(),
        has_words: transcripts.iter().any(|t| !t.words.is_empty()),
        word_count: transcripts.iter().map(|t| t.words.len() as u64).sum(),
        content_hash: content_hash(bytes),
    }
}

/// Summary straight from raw file bytes (rebuild path) -- lexes the whole file but
/// allocates nothing per word.
pub(crate) fn summarize_transcript_bytes(bytes: &[u8]) -> Result<TranscriptSummary, StoreError> {
    let stats: TranscriptJsonStats = serde_json::from_slice(bytes).map_err(|e| {
        StoreError::Serialize(format!("failed to deserialize transcript.json: {e}"))
    })?;
    Ok(TranscriptSummary {
        transcript_ids: stats.transcripts.iter().map(|t| t.id.clone()).collect(),
        has_words: stats.transcripts.iter().any(|t| t.words.0 > 0),
        word_count: stats.transcripts.iter().map(|t| t.words.0 as u64).sum(),
        content_hash: content_hash(bytes),
    })
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

        // A session moving on to a new transcript_id (new recording segment) must not carry
        // the previous transcript's words into the new one's file entry. If the outgoing
        // buffer still has unflushed changes, persist them first -- clearing an unflushed
        // buffer would silently drop words that never made it to disk.
        let mut live = self.live.lock().await;
        let needs_flush_before_switch = live.get(session_id).is_some_and(|buffer| {
            buffer.dirty
                && !buffer.transcript_id.is_empty()
                && buffer.transcript_id != delta.transcript_id
        });
        if needs_flush_before_switch {
            // flush_transcript takes this same lock internally, so the guard must be
            // released across the call; flush_transcript re-checks state under its own
            // guard, so the gap can't flush stale or already-clean content.
            drop(live);
            self.flush_transcript(session_id).await?;
            live = self.live.lock().await;
        }

        let needs_spawn = {
            let buffer = live.entry(session_id.to_string()).or_default();

            if !buffer.transcript_id.is_empty() && buffer.transcript_id != delta.transcript_id {
                buffer.words.clear();
                buffer.hints.clear();
            }

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
        let snapshot = {
            let mut live = self.live.lock().await;
            let Some(buffer) = live.get_mut(session_id) else {
                return Ok(());
            };
            if !buffer.dirty {
                // Nothing buffered since the last successful flush -- including right after
                // write_transcript's batch-supersedes-buffer guard cleared this entry. Flushing
                // clean state is a no-op by contract; persisting the (now-empty) snapshot
                // anyway would zero out a batch write that just landed via write_transcript.
                return Ok(());
            }
            // Clear dirty *before* doing I/O: if an append races in while we're writing, it
            // will see a clean buffer, flip it dirty again, and schedule its own flusher --
            // so nothing gets lost even though this in-flight flush won't see that append.
            buffer.dirty = false;
            // Cloning the full word/hint list on every flush is O(n) in transcript length;
            // fine at meeting scale (thousands of words, not millions).
            (
                buffer.transcript_id.clone(),
                buffer.started_at_ms,
                buffer.words.clone(),
                buffer.hints.clone(),
            )
        };

        let (transcript_id, started_at_ms, words, hints) = snapshot;
        let result = self
            .persist_transcript(session_id, &transcript_id, started_at_ms, words, hints)
            .await;

        if result.is_err() {
            // Persist failed: the words only exist in memory. Re-dirty so flush_all() and
            // the debounce retry loop pick this session back up -- a failed flush must never
            // look "clean" (that's the exact silent-loss shape this store exists to prevent).
            let mut live = self.live.lock().await;
            if let Some(buffer) = live.get_mut(session_id) {
                buffer.dirty = true;
            }
        }

        result
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

    /// Replace a whole transcript (batch/upload path) — writes file + index in one call.
    ///
    /// Clears any live (debounce-buffered) state for this transcript_id first: a batch
    /// overwrite supersedes whatever was buffered, and a still-pending debounced flush from
    /// `append_transcript` must not fire afterward and clobber this call's words with
    /// older, now-stale buffered content. Clearing (not just marking clean) also stops
    /// `append_transcript`'s dirty check from seeing anything left to flush.
    pub async fn write_transcript(
        &self,
        session_id: &str,
        t: TranscriptWithData,
    ) -> Result<(), StoreError> {
        validate_session_id(session_id)?;

        let transcript_id = t.id.clone();
        let started_at_ms = t.started_at;

        {
            let mut live = self.live.lock().await;
            if let Some(buffer) = live.get_mut(session_id) {
                if buffer.transcript_id == transcript_id {
                    buffer.words.clear();
                    buffer.hints.clear();
                    buffer.dirty = false;
                }
            }
        }

        self.persist_transcript(
            session_id,
            &transcript_id,
            started_at_ms,
            t.words,
            t.speaker_hints,
        )
        .await
    }

    /// Supersede primitive (E3): the incoming transcript REPLACES the session's whole
    /// transcript set (batch re-run / re-transcription path -- the old frontend
    /// tombstone-others UPDATE). The file is the truth, so superseded transcripts must
    /// leave `transcript.json`: the previous file moves to `.trash/<date>/sessions/<id>/`
    /// (hand-recoverable, same policy as enhanced-doc deletes) whenever it holds anything
    /// besides the incoming transcript id, then the file is rewritten to just the incoming
    /// transcript.
    ///
    /// Clears the session's whole live buffer first (not just a matching transcript_id,
    /// unlike `write_transcript`): every buffered transcript is superseded by definition,
    /// and a pending debounced flush must not resurrect one afterward.
    pub async fn replace_session_transcripts(
        &self,
        session_id: &str,
        t: TranscriptWithData,
    ) -> Result<(), StoreError> {
        validate_session_id(session_id)?;

        {
            let mut live = self.live.lock().await;
            if let Some(buffer) = live.get_mut(session_id) {
                buffer.words.clear();
                buffer.hints.clear();
                buffer.dirty = false;
            }
        }

        // One guard spans the "what's in the file today" read, the supersede-trash and the
        // rewrite, so a concurrent transcript write can't slip between them.
        let guard = self.lock_writes().await?;

        let previous = self.read_transcript_json(session_id).await?;
        let loses_content = previous
            .transcripts
            .iter()
            .any(|existing| existing.id != t.id);
        if loses_content {
            let vault_base = self.vault_base.clone();
            let session_dir = self.session_dir_locked(&guard, session_id).await?;
            let relative = paths::transcript_path_in(&session_dir);
            let lease = guard.clone();
            tokio::task::spawn_blocking(move || -> Result<(), StoreError> {
                let _lease = lease;
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

        let transcript_id = t.id.clone();
        let started_at_ms = t.started_at;

        // The file is now absent (trashed) or already holds only this transcript, so
        // persist_transcript's read-merge-write lands a file with exactly one entry --
        // and its index_set_transcripts/notify covers the removals too, since the index
        // gets the full (single-entry) list.
        self.persist_transcript_locked(
            &guard,
            session_id,
            &transcript_id,
            started_at_ms,
            t.words,
            t.speaker_hints,
        )
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

        let guard = self.lock_writes().await?;

        // Land any still-dirty live buffer first (same snapshot/re-dirty contract as
        // flush_transcript, but under this guard): the rename must apply on top of every
        // buffered word, and marking the buffer clean here means a pending retry flush
        // afterwards no-ops instead of replacing speaker_hints wholesale (live buffers
        // carry no label hints) and silently erasing the rename.
        let snapshot = {
            let mut live = self.live.lock().await;
            match live.get_mut(&session_id) {
                Some(buffer) if buffer.dirty => {
                    buffer.dirty = false;
                    Some((
                        buffer.transcript_id.clone(),
                        buffer.started_at_ms,
                        buffer.words.clone(),
                        buffer.hints.clone(),
                    ))
                }
                _ => None,
            }
        };
        if let Some((buffered_id, started_at_ms, words, hints)) = snapshot {
            let result = self
                .persist_transcript_locked(
                    &guard,
                    &session_id,
                    &buffered_id,
                    started_at_ms,
                    words,
                    hints,
                )
                .await;
            if let Err(err) = result {
                let mut live = self.live.lock().await;
                if let Some(buffer) = live.get_mut(&session_id) {
                    buffer.dirty = true;
                }
                return Err(err);
            }
        }

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
        entry.speaker_hints = next_hints;

        self.write_transcript_json_locked(&guard, &session_id, file)
            .await
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
        let guard = self.lock_writes().await?;
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
        guard: &WriteGuard,
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
        guard: &WriteGuard,
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
        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;

        let json: serde_json::Value = serde_json::from_slice(
            &std::fs::read(vault.path().join("sessions/s1/transcript.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(json["transcripts"][0]["words"].as_array().unwrap().len(), 2);
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
}
