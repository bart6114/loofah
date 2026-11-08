use super::*;
use futures_util::FutureExt;
use hypr_vault_read::{
    AudioLayout, AudioSource, TranscriptSpeakerHint, TranscriptWithData, TranscriptWord,
};
use hypr_vault_write::transcript_publication::{
    TranscriptPublicationIdentity, TranscriptPublicationOutcome, TranscriptSourcePreparation,
};
use sha2::{Digest, Sha256};
use std::{panic::AssertUnwindSafe, time::Duration};
use tokio::sync::watch;

struct Released(watch::Sender<bool>);
impl Drop for Released {
    fn drop(&mut self) {
        self.0.send_replace(true);
    }
}
struct SourceOwner {
    _context: Arc<VaultContext>,
    _released: Released,
}
struct Cancel(Arc<dyn CaptureLiveControl>);
impl Drop for Cancel {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

struct AccessSignals {
    cancelled: CancellationToken,
    interrupted: CancellationToken,
}

pub(super) async fn run(
    context: Arc<VaultContext>,
    service: Arc<CaptureLiveService>,
    writer: CaptureWriter,
    mut record: ledger::Record,
    cancel: CancellationToken,
) {
    let work = record.work();
    let access = match service.authorizer.observe(&work).await {
        Ok(access) => access,
        Err(cause) => {
            finish(
                &context,
                &service,
                &record,
                CaptureLiveState::Interrupted,
                Some(auth_failure(cause)),
            )
            .await;
            return;
        }
    };
    let access_owner = access.owner;
    let access = AccessSignals {
        cancelled: access.cancelled,
        interrupted: access.interrupted,
    };
    if cancel.is_cancelled()
        || service.stop.is_cancelled()
        || access.interrupted.is_cancelled()
        || access.cancelled.is_cancelled()
    {
        let failure = if access.cancelled.is_cancelled() {
            CaptureLiveFailure::AccessDenied
        } else {
            CaptureLiveFailure::Interrupted
        };
        finish(
            &context,
            &service,
            &record,
            CaptureLiveState::Interrupted,
            Some(failure),
        )
        .await;
        return;
    }
    let (released, mut release) = watch::channel(false);
    let source_owner = Box::new(SourceOwner {
        _context: context.clone(),
        _released: Released(released),
    });
    let started = AssertUnwindSafe(async {
        service
            .engine
            .start(
                CaptureLiveEngineSpec {
                    generation_id: work.generation_id.clone(),
                    format: record.format.clone(),
                    request: record.request.clone(),
                },
                source_owner,
            )
            .await
    })
    .catch_unwind()
    .await;
    let mut session = match started {
        Ok(Ok(session)) => session,
        outcome => {
            await_release(&mut release).await;
            let failure = match outcome {
                Ok(Err(failure)) => failure,
                _ => CaptureLiveFailure::EngineFailed,
            };
            finish(
                &context,
                &service,
                &record,
                CaptureLiveState::Interrupted,
                Some(failure),
            )
            .await;
            return;
        }
    };
    let _cancel = Cancel(session.control.clone());
    let result = AssertUnwindSafe(drive(
        &context,
        &service,
        &writer,
        &mut record,
        &mut session,
        &cancel,
        &access,
    ))
    .catch_unwind()
    .await;
    let failure = match result {
        Ok(Ok(())) => None,
        Ok(Err(failure)) => Some(failure),
        Err(_) => Some(CaptureLiveFailure::EngineFailed),
    };
    if failure.is_some() {
        session.control.cancel();
    }
    // Even a failed ledger or panic cannot turn a cancellation request into
    // completion. Keep the Context and source until both receipts are proven.
    let receipt = loop {
        if let Some(receipt) = session.completion.borrow().clone() {
            break receipt;
        }
        if session.completion.changed().await.is_err() {
            tracing::error!("Live native completion channel closed without receipt");
            std::future::pending::<()>().await;
        }
    };
    if !receipt.owners_released || (!receipt.native_joined && receipt.failure.is_none()) {
        tracing::error!("Live native ownership was not confirmed");
        std::future::pending::<()>().await;
    }
    await_release(&mut release).await;
    let mut failure = failure.or(receipt.failure);
    if failure == Some(CaptureLiveFailure::Storage) && !record.commit_pending {
        if access.cancelled.is_cancelled() {
            failure = Some(CaptureLiveFailure::AccessDenied);
        } else if service.stop.is_cancelled()
            || access.interrupted.is_cancelled()
            || cancel.is_cancelled()
        {
            failure = Some(CaptureLiveFailure::Interrupted);
        }
    }
    if failure == Some(CaptureLiveFailure::Storage) || record.commit_pending {
        service
            .failed
            .store(true, std::sync::atomic::Ordering::Release);
    }
    if let Some(failure) = failure {
        let state = if matches!(
            failure,
            CaptureLiveFailure::Backlog
                | CaptureLiveFailure::Idle
                | CaptureLiveFailure::SourceChanged
                | CaptureLiveFailure::InvalidOutput
        ) {
            CaptureLiveState::RepairRequired
        } else if cancel.is_cancelled()
            && !service.stop.is_cancelled()
            && !access.cancelled.is_cancelled()
            && !access.interrupted.is_cancelled()
        {
            CaptureLiveState::Cancelled
        } else {
            CaptureLiveState::Interrupted
        };
        finish(&context, &service, &record, state, Some(failure)).await;
    }
    drop(access_owner);
}

async fn await_release(release: &mut watch::Receiver<bool>) {
    loop {
        if *release.borrow_and_update() {
            return;
        }
        if release.changed().await.is_err() {
            tracing::error!("Live source owner vanished without release receipt");
            std::future::pending::<()>().await;
        }
    }
}

async fn drive(
    context: &Arc<VaultContext>,
    service: &Arc<CaptureLiveService>,
    writer: &CaptureWriter,
    record: &mut ledger::Record,
    session: &mut CaptureLiveSession,
    cancel: &CancellationToken,
    access: &AccessSignals,
) -> Result<(), CaptureLiveFailure> {
    let id = record.snapshot.generation_id.clone();
    *record = service
        .journal
        .change(id.clone(), |r| r.snapshot.state = CaptureLiveState::Running)
        .await
        .map_err(|_| CaptureLiveFailure::Storage)?;
    let mut chunk = 0_u64;
    let mut pending = Vec::<f32>::new();
    let mut offset = 0;
    let mut pending_block: Option<Arc<[f32]>> = None;
    let mut frame = 0_u64;
    let mut pcm_bytes = 0_u64;
    let mut chain = capture_chain_seed(&record.request.capture_fingerprint);
    let mut source_hash = Sha256::new();
    let count = record.format.channels.len();
    let mut finished_input = false;
    let mut source = writer
        .snapshot()
        .await
        .map_err(|_| CaptureLiveFailure::Storage)?;
    let mut newest = source.frames;
    let mut last_audio = tokio::time::Instant::now();
    let mut tick = tokio::time::interval(Duration::from_millis(20));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut polls = 0;
    let mut captions_open = true;
    let changes = context.capture_changes().unwrap_or_default();
    let receipt = loop {
        tokio::select! {
            biased;
            _ = service.stop.cancelled() => return Err(CaptureLiveFailure::Interrupted),
            _ = access.interrupted.cancelled() => return Err(CaptureLiveFailure::Interrupted),
            _ = access.cancelled.cancelled() => return Err(CaptureLiveFailure::AccessDenied),
            _ = cancel.cancelled() => return Err(CaptureLiveFailure::Interrupted),
            caption = session.captions.recv(), if captions_open => {
                let Some(caption) = caption else {
                    if let Some(receipt) = session.completion.borrow().clone() { break receipt; }
                    captions_open = false;
                    continue;
                };
                *record = retry_append(&service.journal, &id, caption).await?;
                changed(context, record);
            },
            receipt = session.completion.changed() => {
                if let Some(receipt) = session.completion.borrow().clone() { break receipt; }
                if receipt.is_err() { return Err(CaptureLiveFailure::EngineFailed); }
            },
            _ = changes.notified(), if !finished_input => { polls = 0; },
            _ = tick.tick(), if !finished_input => {
                polls += 1;
                if polls % 10 == 1 {
                    source = match writer.snapshot().await {
                        Ok(source) => source,
                        Err(cause) if cause.code == ErrorCode::Unavailable => continue,
                        Err(_) => return Err(CaptureLiveFailure::Storage),
                    };
                    if source.identity != record.identity || source.format != record.format || source.capture_fingerprint != record.request.capture_fingerprint {
                        return Err(CaptureLiveFailure::SourceChanged);
                    }
                    if source.frames > newest { newest = source.frames; last_audio = tokio::time::Instant::now(); }
                    if source.frames.saturating_sub(frame) > 60 * 16000 { return Err(CaptureLiveFailure::Backlog); }
                    if source.seal.is_none() && last_audio.elapsed() > Duration::from_secs(120) { return Err(CaptureLiveFailure::Idle); }
                }
                if offset >= pending.len() && chunk < source.chunk_count {
                    let read = match writer.read_chunk(chunk).await {
                        Ok(read) => read,
                        Err(cause) if cause.code == ErrorCode::Unavailable => continue,
                        Err(_) => return Err(CaptureLiveFailure::Storage),
                    };
                    if read.descriptor.sequence != chunk || read.descriptor.first_frame != frame
                        || read.descriptor.pcm_offset != pcm_bytes || read.descriptor.previous_digest != chain
                        || read.descriptor.capture_fingerprint != record.request.capture_fingerprint {
                        return Err(CaptureLiveFailure::SourceChanged);
                    }
                    source_hash.update(&read.bytes);
                    pcm_bytes = pcm_bytes.checked_add(read.bytes.len() as u64).ok_or(CaptureLiveFailure::ResourceLimit)?;
                    chain = read.descriptor.digest.clone();
                    pending = read.bytes.chunks_exact(4).map(|bytes| f32::from_le_bytes(bytes.try_into().unwrap())).collect();
                    offset = 0;
                    if pending.len() != read.descriptor.frames as usize * count || pending.iter().any(|sample| !sample.is_finite()) { return Err(CaptureLiveFailure::SourceChanged); }
                }
                if offset < pending.len() {
                    let end = (offset + 16000 * count).min(pending.len());
                    let block = pending_block.get_or_insert_with(|| pending[offset..end].into());
                    match session.control.try_accept(frame, block)? {
                        CaptureLiveIngress::Busy => continue,
                        CaptureLiveIngress::Accepted | CaptureLiveIngress::Duplicate => {
                            frame += ((end - offset) / count) as u64;
                            pending_block = None;
                            offset = end;
                            if offset == pending.len() { chunk += 1; pending.clear(); }
                            *record = service.journal.change(id.clone(), move |r| r.snapshot.admitted_end_frame = frame).await.map_err(|_| CaptureLiveFailure::Storage)?;
                        }
                    }
                }
                if chunk == source.chunk_count && source.seal.as_ref().is_some_and(|seal| seal.frames == frame) {
                    session.control.finish(frame)?;
                    finished_input = true;
                    *record = service.journal.change(id.clone(), |r| r.snapshot.state = CaptureLiveState::Finishing).await.map_err(|_| CaptureLiveFailure::Storage)?;
                }
            }
        }
    };
    while let Some(caption) = session.captions.recv().await {
        *record = retry_append(&service.journal, &id, caption).await?;
    }
    if let Some(failure) = receipt.failure {
        return Err(failure);
    }
    if !finished_input
        || !receipt.native_joined
        || !receipt.owners_released
        || receipt.generation_id != id
        || receipt.caption_count != record.snapshot.next_caption_sequence
        || receipt.admitted_end_frame != frame
        || receipt.input_consumed_end_frame != frame
        || receipt.completed_through_frame != Some(frame)
        || source.seal.as_ref().is_none_or(|seal| {
            seal.frames != frame
                || seal.chunk_count != chunk
                || seal.pcm_bytes != pcm_bytes
                || seal.chain_digest != chain
                || seal.capture_fingerprint != record.request.capture_fingerprint
                || seal.source_sha256
                    != crate::transcription_jobs::ledger::hex(&source_hash.clone().finalize())
        })
    {
        return Err(CaptureLiveFailure::InvalidOutput);
    }
    let transcript = materialize(&service.journal, record, frame).await?;
    *record = service
        .journal
        .retain_result(
            id.clone(),
            transcript,
            source.seal.clone().expect("verified full source seal"),
        )
        .await
        .map_err(|_| CaptureLiveFailure::Storage)?;
    *record = service
        .journal
        .change(id.clone(), move |r| {
            r.snapshot.input_consumed_end_frame = frame;
            r.snapshot.completed_through_frame = Some(frame);
            r.snapshot.state = CaptureLiveState::AwaitingRecording;
        })
        .await
        .map_err(|_| CaptureLiveFailure::Storage)?;
    changed(context, record);
    let waiting = tokio::time::Instant::now();
    loop {
        if service.stop.is_cancelled() || access.interrupted.is_cancelled() || cancel.is_cancelled()
        {
            return Err(CaptureLiveFailure::Interrupted);
        }
        if access.cancelled.is_cancelled() {
            return Err(CaptureLiveFailure::AccessDenied);
        }
        let current = writer
            .snapshot()
            .await
            .map_err(|_| CaptureLiveFailure::Storage)?;
        if let Some(publication) = &current.publication {
            if current.identity != record.identity
                || current.seal != source.seal
                || current.seal.as_ref() != Some(&publication.source)
            {
                return Err(CaptureLiveFailure::SourceChanged);
            }
            promote(context, service, record, publication, cancel, access).await?;
            return Ok(());
        }
        if waiting.elapsed() > Duration::from_secs(120) {
            return Err(CaptureLiveFailure::Idle);
        }
        tokio::select! { _ = service.stop.cancelled() => {}, _ = cancel.cancelled() => {}, _ = tokio::time::sleep(Duration::from_millis(100)) => {} }
    }
}

async fn retry_append(
    journal: &ledger::Journal,
    id: &str,
    caption: CaptureLiveCaption,
) -> Result<ledger::Record, CaptureLiveFailure> {
    loop {
        match journal.append(id.into(), caption.clone()).await {
            Ok(record) => return Ok(record),
            Err(cause) if cause.code == ErrorCode::Unavailable => {
                tokio::time::sleep(Duration::from_millis(10)).await
            }
            Err(_) => return Err(CaptureLiveFailure::Storage),
        }
    }
}

async fn materialize(
    journal: &ledger::Journal,
    record: &ledger::Record,
    frames: u64,
) -> Result<TranscriptWithData, CaptureLiveFailure> {
    let mut after = None;
    let mut words = BTreeMap::new();
    loop {
        let page = journal
            .page(
                record.identity.scope.member_id.clone(),
                record.snapshot.generation_id.clone(),
                after,
                128,
            )
            .await
            .map_err(|_| CaptureLiveFailure::Storage)?;
        for caption in page.captions {
            for id in caption.replaced_ids {
                words.remove(&id);
            }
            for word in caption.new_words {
                if word.end_ms as u64 > frames.saturating_mul(1000) / 16000 + 1 {
                    return Err(CaptureLiveFailure::InvalidOutput);
                }
                words.insert(word.id.clone(), word);
                if words.len() > 250_000 {
                    return Err(CaptureLiveFailure::ResourceLimit);
                }
            }
        }
        let Some(next) = page.next_cursor else {
            break;
        };
        after = Some(next);
    }
    let mut values: Vec<_> = words.into_values().collect();
    values.sort_by(|a, b| (a.start_ms, a.channel, &a.id).cmp(&(b.start_ms, b.channel, &b.id)));
    let mut canonical = Vec::new();
    let mut hints = Vec::new();
    for word in values {
        if let Some(speaker) = word.speaker_index {
            hints.push(TranscriptSpeakerHint {
                id: Some(format!("{}:provider_speaker_index", word.id)),
                word_id: word.id.clone(),
                hint_type: "provider_speaker_index".into(),
                value: serde_json::Value::String(
                    serde_json::json!({"channel":word.channel,"speaker_index":speaker}).to_string(),
                ),
            });
        }
        canonical.push(TranscriptWord {
            id: Some(word.id),
            text: word.text,
            start_ms: word.start_ms as f64,
            end_ms: word.end_ms as f64,
            channel: f64::from(word.channel),
            speaker: None,
            metadata: Some(serde_json::Map::from_iter([(
                "capture_source".into(),
                serde_json::json!("recording"),
            )])),
        });
    }
    Ok(TranscriptWithData {
        id: uuid::Uuid::new_v4().to_string(),
        user_id: record.identity.scope.member_id.clone(),
        created_at: record.created_at.clone(),
        session_id: record.snapshot.session_id.clone(),
        started_at: record.observation_ms as f64,
        ended_at: None,
        memo_md: journal
            .memo(record)
            .await
            .map_err(|_| CaptureLiveFailure::Storage)?,
        words: canonical,
        speaker_hints: hints,
    })
}

async fn promote(
    context: &Arc<VaultContext>,
    service: &Arc<CaptureLiveService>,
    record: &mut ledger::Record,
    publication: &CapturePublication,
    cancel: &CancellationToken,
    access: &AccessSignals,
) -> Result<(), CaptureLiveFailure> {
    context
        .ensure_recording_publication_ready(&record.snapshot.session_id)
        .map_err(|_| CaptureLiveFailure::SourceChanged)?;
    let source = match context
        .store
        .prepare_transcript_source(&record.snapshot.session_id)
        .await
        .map_err(|_| CaptureLiveFailure::Storage)?
    {
        Some(TranscriptSourcePreparation::Ready(source)) => source,
        _ => {
            finish(context, service, record, CaptureLiveState::Conflict, None).await;
            return Ok(());
        }
    };
    let expected_layout = if record.format.channels.len() == 2 {
        AudioLayout::MicSystem
    } else {
        AudioLayout::Mixed
    };
    if source.recording_sha256() != publication.canonical_audio_sha256
        || source.audio().source != AudioSource::Recording
        || source.audio().layout != expected_layout
    {
        return Err(CaptureLiveFailure::SourceChanged);
    }
    let result = service
        .journal
        .result(
            record.identity.scope.member_id.clone(),
            record.snapshot.generation_id.clone(),
        )
        .await
        .map_err(|_| CaptureLiveFailure::Storage)?
        .ok_or(CaptureLiveFailure::Storage)?;
    // Hashing/source preparation releases Store guards before this fresh grant;
    // actual commit follows the established Catalog -> Store lock order.
    let _grant = service
        .authorizer
        .authorize(&record.work())
        .await
        .map_err(auth_failure)?;
    if cancel.is_cancelled() || service.stop.is_cancelled() || access.interrupted.is_cancelled() {
        return Err(CaptureLiveFailure::Interrupted);
    }
    if access.cancelled.is_cancelled() {
        return Err(CaptureLiveFailure::AccessDenied);
    }
    context
        .ensure_recording_publication_ready(&record.snapshot.session_id)
        .map_err(|_| CaptureLiveFailure::SourceChanged)?;
    let root = context
        .operations
        .as_ref()
        .ok_or(CaptureLiveFailure::Storage)?
        .root
        .join("capture-live-transcript-publications");
    let identity = TranscriptPublicationIdentity {
        vault_id: record.identity.scope.vault_id.clone(),
        namespace: record.identity.scope.member_id.clone(),
        request_id: uuid::Uuid::parse_str(&record.snapshot.generation_id).unwrap(),
    };
    let plan = context
        .store
        .prepare_transcript_publication(&root, identity, source.baseline(), result)
        .await
        .map_err(|_| CaptureLiveFailure::Storage)?;
    let plan = plan
        .recheck()
        .await
        .map_err(|_| CaptureLiveFailure::Storage)?;
    if cancel.is_cancelled() || service.stop.is_cancelled() || access.interrupted.is_cancelled() {
        return Err(CaptureLiveFailure::Interrupted);
    }
    if access.cancelled.is_cancelled() {
        return Err(CaptureLiveFailure::AccessDenied);
    }
    *record = service
        .journal
        .change(record.snapshot.generation_id.clone(), |r| {
            r.commit_pending = true;
            r.snapshot.state = CaptureLiveState::Publishing
        })
        .await
        .map_err(|_| CaptureLiveFailure::Storage)?;
    let outcome = plan
        .commit()
        .await
        .map_err(|_| CaptureLiveFailure::Storage)?;
    let (state, publication) = match outcome {
        TranscriptPublicationOutcome::Published {
            revision,
            transcript_id,
        }
        | TranscriptPublicationOutcome::AlreadyCommitted {
            revision,
            transcript_id,
        } => (
            CaptureLiveState::Completed,
            Some(TranscriptionPublication {
                revision,
                transcript_id,
            }),
        ),
        TranscriptPublicationOutcome::Empty => (CaptureLiveState::Empty, None),
        TranscriptPublicationOutcome::Conflict => (CaptureLiveState::Conflict, None),
        _ => (CaptureLiveState::Uncertain, None),
    };
    *record = service
        .journal
        .change(record.snapshot.generation_id.clone(), move |r| {
            r.snapshot.state = state;
            r.snapshot.publication = publication;
            r.commit_pending = state == CaptureLiveState::Uncertain;
        })
        .await
        .map_err(|_| CaptureLiveFailure::Storage)?;
    changed(context, record);
    Ok(())
}

fn auth_failure(failure: TranscriptionFailure) -> CaptureLiveFailure {
    if failure == TranscriptionFailure::Interrupted {
        CaptureLiveFailure::Interrupted
    } else {
        CaptureLiveFailure::AccessDenied
    }
}

async fn finish(
    context: &VaultContext,
    service: &CaptureLiveService,
    record: &ledger::Record,
    state: CaptureLiveState,
    failure: Option<CaptureLiveFailure>,
) {
    match service
        .journal
        .change(record.snapshot.generation_id.clone(), move |r| {
            if r.commit_pending {
                r.snapshot.state = CaptureLiveState::Uncertain;
            } else {
                r.snapshot.state = state;
            }
            r.snapshot.failure = failure;
            r.snapshot.completed_through_frame = None;
        })
        .await
    {
        Ok(record) => changed(context, &record),
        Err(cause) => {
            service
                .failed
                .store(true, std::sync::atomic::Ordering::Release);
            tracing::error!(%cause, "Live state could not be confirmed; retained ledger requires review")
        }
    }
}

fn changed(context: &VaultContext, record: &ledger::Record) {
    context.events.publish_for(
        &record.identity.scope.member_id,
        Change::JobChanged {
            job_id: record.snapshot.generation_id.clone(),
        },
    );
}
