use super::*;
use ledger::Record;
use std::time::Duration;

pub(super) async fn run(
    context: Arc<VaultContext>,
    service: Arc<CaptureProcessingService>,
) -> Result<(), ApplicationError> {
    tokio::select! {_=service.ready.cancelled()=>{},_=service.stop.cancelled()=>return Ok(())};
    let mut timer = tokio::time::interval(Duration::from_millis(500));
    timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        for previous in service.journal.pending()? {
            if service.stop.is_cancelled() {
                return Ok(());
            }
            let _work = context.admit_session_work(&previous.target_session).await?;
            // Terminal cancellation still reconciles exact child cancellations;
            // startup never requeues an interrupted/unknown native or paid job.
            let mut next = previous.clone();
            if next.cancel_requested {
                match cancel_children(&context, &next).await {
                    Ok(settled) => {
                        next.cancel_reconciled = settled;
                        service.journal.update(previous, next).await?;
                    }
                    Err(cause) => {
                        if !matches!(
                            cause.code,
                            ErrorCode::Unavailable | ErrorCode::ResourceLimit
                        ) {
                            return Err(cause);
                        }
                    }
                }
                continue;
            }
            let grant = match service.authorizer.authorize(next.member()).await {
                Ok(grant) => grant,
                Err(cause) => {
                    next.cancel_requested = true;
                    next.snapshot.state = if cause == TranscriptionFailure::Interrupted {
                        CaptureProcessingState::Interrupted
                    } else {
                        CaptureProcessingState::Cancelled
                    };
                    next.snapshot.failure = Some(if cause == TranscriptionFailure::Interrupted {
                        CaptureProcessingFailure::Interrupted
                    } else {
                        CaptureProcessingFailure::AccessDenied
                    });
                    service.journal.update(previous, next).await?;
                    continue;
                }
            };
            if !service.model_matches(&next) {
                next.snapshot.state = CaptureProcessingState::Uncertain;
                next.snapshot.failure = Some(CaptureProcessingFailure::ModelChanged);
                next.cancel_requested = true;
            } else if let Err(cause) = step(&context, &service, &mut next).await
                && !matches!(
                    cause.code,
                    ErrorCode::Unavailable | ErrorCode::ResourceLimit
                )
            {
                next.snapshot.state = if cause.mutation_outcome == Some(MutationOutcome::Unknown) {
                    CaptureProcessingState::Uncertain
                } else if cause.code == ErrorCode::Conflict {
                    CaptureProcessingState::Conflict
                } else {
                    CaptureProcessingState::Failed
                };
                next.snapshot.failure = Some(if cause.code == ErrorCode::Conflict {
                    CaptureProcessingFailure::SourceChanged
                } else {
                    CaptureProcessingFailure::Storage
                });
                next.cancel_requested = true;
            }
            drop(grant);
            service.journal.update(previous, next).await?;
        }
        tokio::select! {_=service.stop.cancelled()=>return Ok(()),_=service.wake.notified()=>{},_=timer.tick()=>{}}
    }
}

async fn step(
    context: &Arc<VaultContext>,
    service: &Arc<CaptureProcessingService>,
    record: &mut Record,
) -> Result<(), ApplicationError> {
    let member = record.member().to_owned();
    let capture = record.identity.capture_id.clone();
    let source = context
        .capture_writer(member.clone(), capture.clone())
        .await?
        .snapshot()
        .await?;
    if source.identity != record.identity
        || source.format != record.format
        || source.capture_fingerprint != record.snapshot.capture_fingerprint
    {
        return Err(conflict("Capture processing source changed"));
    }
    record.snapshot.session_id = source.session_id.clone();
    let availability = (service.policy.availability)();
    let ready = availability == TranscriptionAvailability::Ready;
    if matches!(
        availability,
        TranscriptionAvailability::Unavailable
            | TranscriptionAvailability::Disabled
            | TranscriptionAvailability::Stopping
    ) {
        record.snapshot.failure = Some(CaptureProcessingFailure::ModelUnavailable);
    } else if record.snapshot.failure == Some(CaptureProcessingFailure::ModelUnavailable) {
        record.snapshot.failure = None;
    }

    if matches!(record.identity.target, CaptureTarget::AppendSession { .. })
        && !record.transcript_ready
        && !super::append::step(context, service, record, &source, ready).await?
    {
        return Ok(());
    }
    if let Some(generation) = record.snapshot.live_generation_id.clone() {
        match context
            .capture_live_processing_outcome(member.clone(), generation.clone())
            .await
        {
            Ok((live, drained, safe)) => {
                if !live.state.is_terminal() {
                    record.snapshot.state = CaptureProcessingState::Live;
                    return Ok(());
                }
                if !drained {
                    return Ok(());
                }
                if !safe {
                    record.snapshot.state = CaptureProcessingState::Uncertain;
                    record.snapshot.failure = Some(CaptureProcessingFailure::Interrupted);
                    return Ok(());
                }
                if matches!(
                    live.state,
                    CaptureLiveState::Completed | CaptureLiveState::Empty
                ) {
                    let Some(seal) = &source.seal else {
                        return Err(conflict("Live coverage has no sealed source"));
                    };
                    if live.completed_through_frame != Some(seal.frames)
                        || live.admitted_end_frame != seal.frames
                        || live.input_consumed_end_frame != seal.frames
                        || !live.result_available
                    {
                        return Err(conflict("Live coverage is incomplete"));
                    }
                    record.transcript_ready = true;
                    record.empty = live.state == CaptureLiveState::Empty;
                    record.snapshot.transcript_publication = live.publication;
                }
            }
            Err(cause) if cause.code == ErrorCode::NotFound => {
                if source.seal.is_none() {
                    if !ready {
                        return Ok(());
                    }
                    start_live(context, record).await?;
                    return Ok(());
                }
            }
            Err(cause) => return Err(cause),
        }
    } else if context.capture_live.get().is_some()
        && source.seal.is_none()
        && matches!(
            source.identity.target,
            CaptureTarget::ExistingSession {
                expected_audio_sha256: None,
                ..
            }
        )
    {
        if ready {
            // Persist the generation plan first; the next iteration replays the
            // exact start even when a response or process dies after admission.
            record.snapshot.live_generation_id = Some(record.identity.request_id.clone());
            record.snapshot.state = CaptureProcessingState::Live;
        }
        return Ok(());
    }
    let Some(publication) = source.publication else {
        record.snapshot.state = CaptureProcessingState::WaitingForPublication;
        return Ok(());
    };
    if source.seal.as_ref() != Some(&publication.source)
        || publication.session_id != record.target_session
    {
        return Err(conflict("Capture publication binding changed"));
    }
    context.ensure_recording_publication_ready(&record.target_session)?;
    if !record.transcript_ready {
        if let Some(request) = record.repair_request.clone() {
            if request.expected_recording_sha256 != publication.canonical_audio_sha256 {
                return Err(conflict(
                    "Repair source no longer matches capture publication",
                ));
            }
            let job = context
                .submit_capture_repair_job(
                    member.clone(),
                    child_request_id(record, "repair"),
                    request,
                    child_origin(record, "repair")?,
                )
                .await?;
            record.snapshot.repair_job_id = Some(job.id.clone());
            record.snapshot.state = CaptureProcessingState::Repairing;
            match job.state {
                TranscriptionJobState::Succeeded | TranscriptionJobState::Empty => {
                    record.transcript_ready = true;
                    record.empty = job.state == TranscriptionJobState::Empty;
                    record.snapshot.transcript_publication = job.publication;
                }
                TranscriptionJobState::Failed
                    if matches!(
                        job.failure,
                        Some(
                            TranscriptionFailure::EngineUnavailable
                                | TranscriptionFailure::ResourceLimit
                        )
                    ) =>
                {
                    if ready {
                        context
                            .retry_unstarted_capture_repair(&member, &job.id)
                            .await?;
                    }
                    return Ok(());
                }
                state if state.is_terminal() => {
                    record.snapshot.state = if state == TranscriptionJobState::Uncertain {
                        CaptureProcessingState::Uncertain
                    } else if state == TranscriptionJobState::Conflict {
                        CaptureProcessingState::Conflict
                    } else if state == TranscriptionJobState::Interrupted {
                        CaptureProcessingState::Interrupted
                    } else {
                        CaptureProcessingState::Failed
                    };
                    record.snapshot.failure = Some(CaptureProcessingFailure::ChildFailed);
                    return Ok(());
                }
                _ => return Ok(()),
            }
        } else {
            let guard = context
                .store
                .guard_processing_source(&record.target_session, None)
                .await
                .map_err(|cause| crate::vault::storage_error(cause, false))?
                .ok_or_else(|| unavailable("Published recording unavailable"))?;
            if guard.binding().recording_sha256 != publication.canonical_audio_sha256 {
                return Err(conflict("Published recording changed"));
            }
            record.repair_request = Some(TranscriptionJobRequest {
                session_id: record.target_session.clone(),
                expected_recording_sha256: publication.canonical_audio_sha256,
                expected_transcript: AbsentTranscriptExpectation::Absent,
                model: record.snapshot.model.clone(),
                languages: record.snapshot.request.languages.clone(),
                keywords: record.snapshot.request.keywords.clone(),
            });
            record.snapshot.state = CaptureProcessingState::Repairing;
            return Ok(());
        }
    }
    let Some(summary) = record.snapshot.request.summary.clone() else {
        record.snapshot.state = if record.empty {
            CaptureProcessingState::Empty
        } else {
            CaptureProcessingState::Succeeded
        };
        return Ok(());
    };
    if record.summary_request.is_none() {
        let guard = context
            .store
            .guard_processing_source(&record.target_session, None)
            .await
            .map_err(|cause| crate::vault::storage_error(cause, false))?
            .ok_or_else(|| unavailable("Summary source unavailable"))?;
        if guard.binding().recording_sha256 != publication.canonical_audio_sha256
            || guard.binding().transcript_present == record.empty
            || (!record.empty
                && record
                    .snapshot
                    .transcript_publication
                    .as_ref()
                    .is_none_or(|proof| proof.revision != guard.binding().transcript_revision))
        {
            return Err(conflict("Transcript or recording changed before summary"));
        }
        if record.empty {
            let note = context
                .read_note(&record.target_session)
                .await?
                .markdown
                .unwrap_or_default();
            if !tokio::task::spawn_blocking(move || crate::jobs::source::has_note_text(&note))
                .await
                .map_err(storage)?
            {
                record.snapshot.state = CaptureProcessingState::Failed;
                record.snapshot.failure = Some(CaptureProcessingFailure::SourceUnavailable);
                return Ok(());
            }
        }
        let document = context
            .read_document(&record.target_session, &summary.target)
            .await?;
        let revision = match &record.summary_revision {
            Some(expected) if *expected != document.revision => {
                return Err(conflict("Summary target changed during capture"));
            }
            Some(expected) => expected.clone(),
            None => {
                if document.document.is_some() {
                    return Err(conflict("Pending summary target was edited"));
                }
                document.revision
            }
        };
        record.summary_revision = Some(revision.clone());
        record.summary_source = Some(guard.binding().clone());
        record.summary_request = Some(SummaryJobRequest {
            only_if_empty: false,
            session_id: record.target_session.clone(),
            target: summary.target,
            expected_revision: revision,
            title_policy: summary.title_policy,
        });
        record.snapshot.state = CaptureProcessingState::Summarizing;
        return Ok(());
    }
    let job = context
        .submit_capture_summary_job(
            member,
            child_request_id(record, "summary"),
            record.summary_request.clone().unwrap(),
            child_origin(record, "summary")?,
        )
        .await?;
    record.snapshot.summary_job_id = Some(job.id);
    record.snapshot.state = match job.state {
        ProcessingJobState::Succeeded => {
            if record.empty {
                CaptureProcessingState::Empty
            } else {
                CaptureProcessingState::Succeeded
            }
        }
        ProcessingJobState::Conflict => CaptureProcessingState::Conflict,
        ProcessingJobState::Interrupted => CaptureProcessingState::Interrupted,
        ProcessingJobState::Failed => CaptureProcessingState::Failed,
        ProcessingJobState::Cancelled => CaptureProcessingState::Cancelled,
        _ => CaptureProcessingState::Summarizing,
    };
    if matches!(
        record.snapshot.state,
        CaptureProcessingState::Failed | CaptureProcessingState::Interrupted
    ) {
        record.snapshot.failure = Some(CaptureProcessingFailure::ChildFailed);
    }
    Ok(())
}

async fn start_live(context: &Arc<VaultContext>, record: &Record) -> Result<(), ApplicationError> {
    let result = context
        .start_capture_live_authorized(
            record.member().to_owned(),
            record.identity.capture_id.clone(),
            record.identity.request_id.clone(),
            CaptureLiveRequest {
                capture_fingerprint: record.snapshot.capture_fingerprint.clone(),
                expected_transcript: AbsentTranscriptExpectation::Absent,
                model: record.snapshot.model.clone(),
                languages: record.snapshot.request.languages.clone(),
                keywords: record.snapshot.request.keywords.clone(),
            },
            || async { Ok(()) },
        )
        .await;
    match result {
        Ok(_) => Ok(()),
        Err(cause) if cause.code == ErrorCode::Conflict => {
            // A seal can race the exact first start. No native attempt was admitted;
            // the declared repair stage handles the immutable completed recording.
            let source = context
                .capture_writer(
                    record.member().to_owned(),
                    record.identity.capture_id.clone(),
                )
                .await?
                .snapshot()
                .await?;
            if source.seal.is_some() {
                Ok(())
            } else {
                Err(cause)
            }
        }
        Err(cause) => Err(cause),
    }
}
async fn cancel_children(
    context: &Arc<VaultContext>,
    record: &Record,
) -> Result<bool, ApplicationError> {
    let mut settled = true;
    if matches!(record.identity.target, CaptureTarget::AppendSession { .. }) {
        settled &= super::append::cancel(context, record).await?;
    }
    if let Some(generation) = &record.snapshot.live_generation_id {
        match context
            .cancel_capture_live_authorized(
                record.member().to_owned(),
                record.identity.capture_id.clone(),
                generation.clone(),
                || async { Ok(()) },
            )
            .await
        {
            Ok(_) => {
                let (snapshot, drained, safe) = context
                    .capture_live_processing_outcome(record.member().to_owned(), generation.clone())
                    .await?;
                settled &= snapshot.state.is_terminal() && drained && safe;
            }
            Err(cause) if cause.code == ErrorCode::NotFound => {}
            Err(cause) => return Err(cause),
        }
    }
    if let Some(request) = &record.repair_request {
        settled &= context
            .cancel_capture_repair_request(
                record.member().to_owned(),
                child_request_id(record, "repair"),
                request.clone(),
            )
            .await?;
    }
    if let (Some(request), Some(service)) = (&record.summary_request, context.processing.get()) {
        settled &= service
            .journal
            .cancel_capture_summary_request(
                record.member().to_owned(),
                child_request_id(record, "summary"),
                request.clone(),
            )
            .await?;
    } else if record.summary_request.is_some() {
        settled = false;
    }
    Ok(settled)
}
