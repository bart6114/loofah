use super::*;
use futures_util::FutureExt;
use hypr_vault_write::transcript_publication::{
    TranscriptPublicationBaseline, TranscriptPublicationOutcome,
};
use std::panic::AssertUnwindSafe;

struct Signals {
    cancel: CancellationToken,
    access: CancellationToken,
    interrupted: CancellationToken,
    stop: CancellationToken,
}

impl Signals {
    fn cancelled(&self) -> bool {
        self.cancel.is_cancelled()
            || self.access.is_cancelled()
            || self.interrupted.is_cancelled()
            || self.stop.is_cancelled()
    }
}

pub struct TranscriptionWorker {
    pub(super) stop: CancellationToken,
    pub(super) finished: CancellationToken,
    task: Option<tokio::task::JoinHandle<Result<(), ApplicationError>>>,
}
impl Drop for TranscriptionWorker {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}
impl TranscriptionWorker {
    pub(super) async fn start(
        context: Arc<VaultContext>,
        journal: ledger::Journal,
        engine: Arc<dyn TranscriptionEngine>,
        authorizer: Arc<dyn TranscriptionAuthorizer>,
    ) -> Result<Self, ApplicationError> {
        let stop = CancellationToken::new();
        let stopping = stop.clone();
        let finished = CancellationToken::new();
        let done = finished.clone().drop_guard();
        let task = tokio::spawn(async move {
            let _done = done;
            let result = AssertUnwindSafe(run(
                context.clone(),
                journal.clone(),
                engine,
                authorizer,
                stopping,
            ))
            .catch_unwind()
            .await
            .unwrap_or_else(|_| Err(storage("transcription worker panicked")));
            journal.drain_sources().await;
            if result.is_err() {
                let _ = journal.interrupt().await;
            }
            drop(context);
            result
        });
        Ok(Self {
            stop,
            finished,
            task: Some(task),
        })
    }
    pub fn request_shutdown(&self) {
        self.stop.cancel();
    }
    pub async fn shutdown(mut self) -> Result<(), ApplicationError> {
        self.stop.cancel();
        if let Some(task) = self.task.take() {
            task.await.map_err(storage)?
        } else {
            Ok(())
        }
    }
}

async fn run(
    context: Arc<VaultContext>,
    journal: ledger::Journal,
    engine: Arc<dyn TranscriptionEngine>,
    authorizer: Arc<dyn TranscriptionAuthorizer>,
    stop: CancellationToken,
) -> Result<(), ApplicationError> {
    loop {
        if stop.is_cancelled() {
            return Ok(());
        }
        let wake = journal.notify().notified();
        tokio::pin!(wake);
        wake.as_mut().enable();
        let cancel = stop.child_token();
        let Some(record) = journal.claim(cancel.clone()).await? else {
            tokio::select! {_ = stop.cancelled()=>return Ok(()),_ = wake=>{}}
            continue;
        };
        journal.fault("claimed");
        execute(
            context.clone(),
            &journal,
            engine.as_ref(),
            authorizer.as_ref(),
            record,
            cancel,
            &stop,
        )
        .await?;
    }
}

async fn execute(
    context: Arc<VaultContext>,
    journal: &ledger::Journal,
    engine: &dyn TranscriptionEngine,
    authorizer: &dyn TranscriptionAuthorizer,
    mut record: ledger::Record,
    cancel: CancellationToken,
    stop: &CancellationToken,
) -> Result<(), ApplicationError> {
    let _work = context
        .admit_session_work(&record.request.session_id)
        .await?;
    let work = record.work();
    let access = match authorizer.observe(&work).await {
        Ok(access) => access,
        Err(cause) => {
            return journal
                .finish(
                    record.snapshot.id,
                    authorization_failure(cause),
                    Some(cause),
                    None,
                )
                .await;
        }
    };
    let TranscriptionAccess {
        cancelled: access_cancelled,
        interrupted,
        owner: access_owner,
    } = access;
    let monitor_cancel = cancel.clone();
    let revoked = access_cancelled.clone();
    let host_interrupted = interrupted.clone();
    let monitor = tokio::spawn(async move {
        tokio::select! {_ = monitor_cancel.cancelled()=>{},_ = revoked.cancelled()=>monitor_cancel.cancel(),_ = host_interrupted.cancelled()=>monitor_cancel.cancel()}
    });
    let signals = Signals {
        cancel: cancel.clone(),
        access: access_cancelled,
        interrupted,
        stop: stop.clone(),
    };
    let result =
        execute_observed(context, journal, engine, authorizer, &mut record, &signals).await;
    cancel.cancel();
    let _ = monitor.await;
    drop(access_owner);
    result
}

fn authorization_failure(cause: TranscriptionFailure) -> TranscriptionJobState {
    if cause == TranscriptionFailure::Interrupted {
        TranscriptionJobState::Interrupted
    } else {
        TranscriptionJobState::Failed
    }
}

fn cancellation(signals: &Signals) -> (TranscriptionJobState, Option<TranscriptionFailure>) {
    if signals.access.is_cancelled() {
        (
            TranscriptionJobState::Cancelled,
            Some(TranscriptionFailure::AccessDenied),
        )
    } else if signals.stop.is_cancelled() || signals.interrupted.is_cancelled() {
        (
            TranscriptionJobState::Interrupted,
            Some(TranscriptionFailure::Interrupted),
        )
    } else {
        (TranscriptionJobState::Cancelled, None)
    }
}

async fn execute_observed(
    context: Arc<VaultContext>,
    journal: &ledger::Journal,
    engine: &dyn TranscriptionEngine,
    authorizer: &dyn TranscriptionAuthorizer,
    record: &mut ledger::Record,
    signals: &Signals,
) -> Result<(), ApplicationError> {
    let id = record.snapshot.id.clone();
    if context
        .capture_child_admission(
            &record.member,
            &record.identity().request_id.to_string(),
            record.capture_processing.as_ref(),
        )
        .await
        .is_err()
    {
        return journal
            .finish(
                id,
                TranscriptionJobState::Interrupted,
                Some(TranscriptionFailure::Interrupted),
                None,
            )
            .await;
    }
    let mut baseline = None;
    if signals.cancelled() {
        let (s, f) = cancellation(signals);
        return journal.finish(id, s, f, None).await;
    }
    if record.action == ledger::Action::Generate {
        let prepared = match engine
            .prepare(&record.request, signals.cancel.clone())
            .await
        {
            Ok(prepared) => prepared,
            Err(cause) => {
                if signals.cancelled() {
                    let (s, f) = cancellation(signals);
                    return journal.finish(id, s, f, None).await;
                }
                return journal
                    .finish(id, TranscriptionJobState::Failed, Some(cause), None)
                    .await;
            }
        };
        let source =
            match source::prepare(context.clone(), journal, record, signals.cancel.clone()).await {
                Ok(source) => source,
                Err(cause) => {
                    drop(prepared);
                    return fail(journal, record, &cause, signals).await;
                }
            };
        if signals.cancelled() {
            drop(source);
            drop(prepared);
            let (state, failure) = cancellation(signals);
            return journal.finish(id, state, failure, None).await;
        }
        let binding = source.binding.clone();
        *record = journal
            .change(id.clone(), move |r| {
                r.source = Some(binding);
                if r.snapshot.state != TranscriptionJobState::Cancelling {
                    r.snapshot.phase = Some(TranscriptionPhase::Transcribing);
                }
            })
            .await?;
        let source::Prepared {
            input,
            metadata,
            baseline: original,
            released,
            ..
        } = source;
        journal.track_source(record.request.session_id.clone(), released.clone());
        // This future owns the host reservation. Even if its adapter panics, the
        // actual source owner must exit before we terminalize or finish shutdown.
        let generated = AssertUnwindSafe(prepared.transcribe(input, signals.cancel.clone()))
            .catch_unwind()
            .await
            .unwrap_or(Err(TranscriptionFailure::EngineFailed));
        let mut released = released;
        loop {
            if *released.borrow_and_update() {
                break;
            }
            if released.changed().await.is_err() {
                return Err(storage(
                    "native source owner disappeared without completion",
                ));
            }
        }
        let transcript = match generated {
            Ok(transcript) => transcript,
            Err(cause) => {
                if signals.cancelled() {
                    let (s, f) = cancellation(signals);
                    return journal.finish(id, s, f, None).await;
                }
                return journal
                    .finish(id, TranscriptionJobState::Failed, Some(cause), None)
                    .await;
            }
        };
        if transcript.session_id != metadata.session_id
            || transcript.user_id != metadata.user_id
            || transcript.created_at != metadata.created_at
            || transcript.memo_md != metadata.memo_md
            || transcript.started_at != metadata.started_at_ms()
            || transcript.ended_at.is_some()
            || uuid(&transcript.id).is_err()
        {
            return journal
                .finish(
                    id,
                    TranscriptionJobState::Failed,
                    Some(TranscriptionFailure::InvalidOutput),
                    None,
                )
                .await;
        }
        *record = journal.retain_result(id.clone(), transcript).await?;
        baseline = Some(original);
    }
    if signals.cancelled() {
        let (s, f) = cancellation(signals);
        return journal.finish(id, s, f, None).await;
    }
    publish(context, journal, authorizer, record, baseline, signals).await
}

async fn fail(
    journal: &ledger::Journal,
    record: &ledger::Record,
    cause: &ApplicationError,
    signals: &Signals,
) -> Result<(), ApplicationError> {
    if signals.cancelled() {
        let (s, f) = cancellation(signals);
        return journal.finish(record.snapshot.id.clone(), s, f, None).await;
    }
    let state = if cause.mutation_outcome == Some(MutationOutcome::Unknown) {
        TranscriptionJobState::Uncertain
    } else if cause.code == ErrorCode::Conflict {
        TranscriptionJobState::Conflict
    } else {
        TranscriptionJobState::Failed
    };
    let failure = match cause.code {
        ErrorCode::ResourceLimit => TranscriptionFailure::ResourceLimit,
        ErrorCode::Unauthenticated | ErrorCode::Forbidden => TranscriptionFailure::AccessDenied,
        _ => TranscriptionFailure::SourceUnavailable,
    };
    journal
        .finish(record.snapshot.id.clone(), state, Some(failure), None)
        .await
}

async fn publish(
    context: Arc<VaultContext>,
    journal: &ledger::Journal,
    authorizer: &dyn TranscriptionAuthorizer,
    record: &mut ledger::Record,
    baseline: Option<TranscriptPublicationBaseline>,
    signals: &Signals,
) -> Result<(), ApplicationError> {
    let id = record.snapshot.id.clone();
    let work = record.work();
    let grant = match authorizer.authorize(&work).await {
        Ok(grant) => grant,
        Err(cause) => {
            return journal
                .finish(id, authorization_failure(cause), Some(cause), None)
                .await;
        }
    };
    if signals.cancelled() {
        let (s, f) = cancellation(signals);
        return journal.finish(id, s, f, None).await;
    }
    if let Err(cause) = context.ensure_recording_publication_ready(&record.request.session_id) {
        return fail(journal, record, &cause, signals).await;
    }
    *record = journal
        .change(id.clone(), |r| {
            if r.snapshot.state != TranscriptionJobState::Cancelling {
                r.snapshot.phase = Some(TranscriptionPhase::Publishing);
            }
        })
        .await?;
    let root = context
        .operations
        .as_ref()
        .ok_or_else(unavailable)?
        .root
        .join("transcript-publications");
    let existing = context
        .store
        .prepare_transcript_publication_status(&root, record.identity())
        .await;
    let existing = match existing {
        Ok(existing) => existing,
        Err(cause) => {
            return journal
                .finish(
                    id,
                    TranscriptionJobState::Uncertain,
                    Some(failure_for_store(&cause)),
                    None,
                )
                .await;
        }
    };
    let plan = if let Some(plan) = existing {
        plan
    } else {
        if record.uncertain {
            return journal
                .finish(
                    id,
                    TranscriptionJobState::Uncertain,
                    Some(TranscriptionFailure::Storage),
                    None,
                )
                .await;
        }
        let baseline = if let Some(baseline) = baseline {
            baseline
        } else {
            let source = match source::snapshot(&context, &record.request.session_id).await {
                Ok(source) => source,
                Err(cause) => return fail(journal, record, &cause, signals).await,
            };
            let meta = context.read_metadata(&record.request.session_id).await?;
            if record
                .source
                .as_ref()
                .is_none_or(|before| !before.matches(&source, &meta.meta.created_at))
            {
                return journal
                    .finish(id, TranscriptionJobState::Conflict, None, None)
                    .await;
            }
            source.baseline()
        };
        let Some(result) = journal.result(record).await? else {
            return journal
                .finish(
                    id,
                    TranscriptionJobState::Uncertain,
                    Some(TranscriptionFailure::Storage),
                    None,
                )
                .await;
        };
        match context
            .store
            .prepare_transcript_publication(&root, record.identity(), baseline, result)
            .await
        {
            Ok(plan) => plan,
            Err(cause) => {
                return journal
                    .finish(
                        id,
                        if matches!(cause, hypr_vault_write::StoreError::Conflict(_)) {
                            TranscriptionJobState::Conflict
                        } else {
                            TranscriptionJobState::Uncertain
                        },
                        Some(failure_for_store(&cause)),
                        None,
                    )
                    .await;
            }
        }
    };
    let plan = match plan.recheck().await {
        Ok(plan) => plan,
        Err(cause) => {
            return journal
                .finish(
                    id,
                    TranscriptionJobState::Uncertain,
                    Some(failure_for_store(&cause)),
                    None,
                )
                .await;
        }
    };
    if signals.cancelled() {
        drop(plan);
        drop(grant);
        let (s, f) = cancellation(signals);
        return journal.finish(id, s, f, None).await;
    }
    if let Err(cause) = context.ensure_recording_publication_ready(&record.request.session_id) {
        drop(plan);
        drop(grant);
        return fail(journal, record, &cause, signals).await;
    }
    if context
        .capture_child_admission(
            &record.member,
            &record.identity().request_id.to_string(),
            record.capture_processing.as_ref(),
        )
        .await
        .is_err()
    {
        drop(plan);
        drop(grant);
        return journal
            .finish(
                id,
                TranscriptionJobState::Interrupted,
                Some(TranscriptionFailure::Interrupted),
                None,
            )
            .await;
    }
    // Persist uncertainty before crossing the Store commit boundary. If the host
    // disappears here, only identical receipt reconciliation may resolve it.
    *record = journal.change(id.clone(), |r| r.uncertain = true).await?;
    let published = plan.commit().await;
    drop(grant);
    journal.fault("publication_commit");
    let (state, failure, publication) = match published {
        Ok(
            TranscriptPublicationOutcome::Published {
                revision,
                transcript_id,
            }
            | TranscriptPublicationOutcome::AlreadyCommitted {
                revision,
                transcript_id,
            },
        ) => (
            TranscriptionJobState::Succeeded,
            None,
            Some(TranscriptionPublication {
                revision,
                transcript_id,
            }),
        ),
        Ok(TranscriptPublicationOutcome::Empty) => (TranscriptionJobState::Empty, None, None),
        Ok(TranscriptPublicationOutcome::Conflict) => (TranscriptionJobState::Conflict, None, None),
        Ok(TranscriptPublicationOutcome::Busy | TranscriptPublicationOutcome::RequiresReview) => {
            (TranscriptionJobState::Uncertain, None, None)
        }
        Err(cause) => (
            TranscriptionJobState::Uncertain,
            Some(failure_for_store(&cause)),
            None,
        ),
    };
    journal.finish(id, state, failure, publication).await
}
fn failure_for_store(cause: &hypr_vault_write::StoreError) -> TranscriptionFailure {
    match cause {
        hypr_vault_write::StoreError::ResourceLimit(_) => TranscriptionFailure::ResourceLimit,
        hypr_vault_write::StoreError::Serialize(_) => TranscriptionFailure::InvalidOutput,
        _ => TranscriptionFailure::Storage,
    }
}
