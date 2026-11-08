//! Recording readers must share this Context's gates: a canonical rename can precede
//! its durable publication receipt. Runtime bootstrap resolves admitted filesystem
//! intents before readiness; unresolved recordings stay hidden while notes remain usable.

use std::{future::Future, sync::Arc};

use hypr_loofah_protocol::{
    ApplicationError, CaptureIngestStatus, CapturePrefixReceipt, CapturePublication, CaptureSeal,
    CaptureTarget, ErrorCode, MutationOutcome,
};
use hypr_vault_write::recording_publish::{
    RecordingExpectation, RecordingPublication, RecordingPublicationDiscoveryLimits,
    RecordingPublicationRecoveryPreparation, RecordingPublicationRequest,
};

use super::{CaptureScope, CaptureSpool, find, hash, not_committed, rejected, status, wav};

mod bootstrap;
mod gates;
pub(crate) use gates::PublicationGates;

impl crate::VaultContext {
    pub(crate) fn ensure_recording_publication_ready(
        &self,
        session: &str,
    ) -> Result<(), ApplicationError> {
        self.recording_publications.check(session)
    }

    pub async fn publish_capture_authorized<G, F>(
        self: &Arc<Self>,
        member_id: String,
        capture_id: String,
        expected: CaptureSeal,
        authorize: impl Fn() -> F + Send + 'static,
    ) -> Result<CaptureIngestStatus, ApplicationError>
    where
        G: Send + 'static,
        F: Future<Output = Result<G, ApplicationError>> + Send + 'static,
    {
        self.store
            .ensure_ready()
            .map_err(|cause| crate::vault::storage_error(cause, false))?;
        super::validate_id(&capture_id)?;
        if member_id.is_empty() || member_id.len() > 512 || member_id.chars().any(char::is_control)
        {
            return Err(rejected(ErrorCode::Validation, "Invalid capture member"));
        }
        if self.lease.is_none() || !self.recording_publications.initialized() {
            return Err(rejected(
                ErrorCode::Unavailable,
                "Capture publication requires an exclusively owned vault runtime",
            ));
        }
        let settings = self.capture_ingestion.get().ok_or_else(|| {
            rejected(
                ErrorCode::Unavailable,
                "Capture ingestion is not configured",
            )
        })?;
        let permit = settings
            .operations
            .clone()
            .try_acquire_owned()
            .map_err(|_| {
                rejected(
                    ErrorCode::Unavailable,
                    "Capture storage is busy; retry shortly",
                )
            })?;
        let operations = self
            .operations
            .as_ref()
            .ok_or_else(|| {
                rejected(
                    ErrorCode::Unavailable,
                    "Operation storage is not configured",
                )
            })?
            .root
            .clone();
        let context = self.clone();
        // A pending note can persist before its recording commit is admitted.
        let creation = Arc::new(std::sync::atomic::AtomicU8::new(0));
        let attempted_creation = creation.clone();
        let result = tokio::spawn(async move {
            let _permit = permit;
            let settings = context
                .capture_ingestion
                .get()
                .expect("configured capture ingestion");
            let scope = CaptureScope {
                server: settings.server.clone(),
                member_id,
                vault_id: context.events.vault_id(),
            };
            let spool = settings
                .spool
                .get_or_try_init(|| {
                    CaptureSpool::open(operations.join("captures"), settings.limits.clone())
                })
                .await?
                .clone();
            let grant = authorize().await.map_err(not_committed)?;
            let writer = find(&spool, scope, &capture_id).await?;
            let snapshot = writer.snapshot().await?;
            let session = target_session(&snapshot)?;
            let _admission = context.admit_session_work(&session).await?;
            if snapshot.seal.as_ref() != Some(&expected) {
                return Err(rejected(ErrorCode::Conflict, "Capture seal changed"));
            }
            if snapshot.session_id.is_some() {
                context
                    .store
                    .read_metadata_snapshot(&session)
                    .await
                    .map_err(|cause| crate::vault::storage_error(cause, false))?
                    .ok_or_else(|| rejected(ErrorCode::NotFound, "Session not found"))?;
            }
            drop(grant);
            let mut verified = wav::verify(&writer, &expected)
                .await
                .map_err(not_committed)?;
            if let CaptureTarget::PendingSession { create_request_id } =
                &verified.snapshot.identity.target
            {
                let grant = authorize().await.map_err(not_committed)?;
                attempted_creation.store(1, std::sync::atomic::Ordering::Release);
                let created = context
                    .create_capture_session(
                        verified.snapshot.identity.scope.member_id.clone(),
                        uuid::Uuid::parse_str(create_request_id).map_err(|_| {
                            rejected(ErrorCode::Validation, "Invalid capture creation identifier")
                        })?,
                        verified
                            .snapshot
                            .first_admitted_audio_observed_at_ms
                            .ok_or_else(|| {
                                rejected(ErrorCode::Conflict, "Capture has no producer observation")
                            })?,
                    )
                    .await?;
                attempted_creation.store(2, std::sync::atomic::Ordering::Release);
                if created.meta.id != session {
                    return Err(unconfirmed(
                        ErrorCode::Conflict,
                        "Capture creation resolved to another session",
                    ));
                }
                let bound = writer.bind_session(session.clone()).await?;
                if bound.identity != verified.snapshot.identity
                    || bound.format != verified.snapshot.format
                    || bound.seal != verified.snapshot.seal
                    || bound.first_admitted_audio_observed_at_ms
                        != verified.snapshot.first_admitted_audio_observed_at_ms
                {
                    return Err(unconfirmed(
                        ErrorCode::Conflict,
                        "Capture declaration changed during session binding",
                    ));
                }
                verified.snapshot = bound;
                drop(grant);
            }
            let request = publication_request(&verified)?;
            let key = operation_key(&request);
            let root = operations.join("recording-publications");
            let inventory = match context
                .store
                .discover_recording_publications(
                    &root,
                    &context.events.vault_id(),
                    RecordingPublicationDiscoveryLimits::default(),
                )
                .await
            {
                Ok(inventory) => inventory,
                Err(cause) => {
                    context.recording_publications.block_all();
                    return Err(before_commit_error(cause));
                }
            };
            bootstrap::retain_inventory(&context.recording_publications, &inventory);
            let ticket = context
                .recording_publications
                .begin(&session, &key)?
                .with_events(context.events.clone());
            let pending = inventory.pending.into_iter().find(|pending| {
                let existing = pending.request();
                existing.namespace == request.namespace && existing.request_id == request.request_id
            });
            let prepared = if let Some(pending) = pending {
                if pending.request() != &request {
                    return Err(rejected(
                        ErrorCode::Conflict,
                        "Capture publication declaration changed",
                    ));
                }
                match context
                    .store
                    .prepare_recording_publication_recovery(
                        &root,
                        &context.events.vault_id(),
                        pending,
                    )
                    .await
                    .map_err(before_commit_error)?
                {
                    RecordingPublicationRecoveryPreparation::Prepared(plan) => Some(*plan),
                    RecordingPublicationRecoveryPreparation::NoLongerPending => context
                        .store
                        .prepare_recording_publication_status(&root, request.clone())
                        .await
                        .map_err(before_commit_error)?,
                    RecordingPublicationRecoveryPreparation::Busy => {
                        return Err(rejected(
                            ErrorCode::Unavailable,
                            "Recording recovery is busy",
                        ));
                    }
                    RecordingPublicationRecoveryPreparation::RequiresReview(_) => {
                        context.recording_publications.review(&session);
                        return Err(rejected(
                            ErrorCode::Conflict,
                            "Recording recovery requires review",
                        ));
                    }
                }
            } else {
                context
                    .store
                    .prepare_recording_publication_status(&root, request.clone())
                    .await
                    .map_err(before_commit_error)?
            };
            let plan = if let Some(plan) = prepared {
                plan
            } else {
                let (reader, producer) = verified.reader(context.clone(), writer.clone());
                let prepared = context
                    .store
                    .prepare_recording_publication(&root, request.clone(), reader)
                    .await;
                // Preparation may reject or reuse a receipt without consuming the reader.
                // Always join the real producer, including its current storage read.
                let produced = producer.await;
                let plan = prepared.map_err(before_commit_error)?;
                produced
                    .map_err(|_| rejected(ErrorCode::Internal, "Capture reader stopped"))?
                    .map_err(not_committed)?;
                plan
            };
            let plan = plan.recheck().await.map_err(before_commit_error)?;
            let grant = authorize().await.map_err(not_committed)?;
            let mut ticket = ticket;
            ticket.committing();
            let published = plan.commit().await;
            match published {
                Ok(RecordingPublication::Published {
                    sha256, size_bytes, ..
                }) => {
                    ticket.resolved();
                    let publication = CapturePublication {
                        source: expected,
                        session_id: session,
                        canonical_audio_sha256: sha256,
                        canonical_audio_bytes: size_bytes,
                    };
                    let observed = async {
                        writer
                            .acknowledge_prefix(CapturePrefixReceipt {
                                capture_fingerprint: publication.source.capture_fingerprint.clone(),
                                chunk_count: publication.source.chunk_count,
                                frames: publication.source.frames,
                                pcm_bytes: publication.source.pcm_bytes,
                                chain_digest: publication.source.chain_digest.clone(),
                            })
                            .await?;
                        writer.observe_publication(publication).await
                    }
                    .await
                    .map_err(|mut cause| {
                        cause.mutation_outcome = Some(MutationOutcome::Persisted);
                        cause
                    });
                    drop(grant);
                    status(observed?)
                }
                other => {
                    drop(grant);
                    drop(ticket);
                    reconcile_missing(&context, &root, &request).await;
                    match other {
                        Ok(RecordingPublication::Changed) => Err(unconfirmed(
                            ErrorCode::Conflict,
                            "Recording changed; review the capture before retrying",
                        )),
                        Ok(RecordingPublication::Busy) => Err(unconfirmed(
                            ErrorCode::Unavailable,
                            "Recording publication is busy; retry after recovery",
                        )),
                        Ok(RecordingPublication::Uncertain) => Err(unconfirmed(
                            ErrorCode::Conflict,
                            "Recording publication requires review",
                        )),
                        Err(cause) => {
                            let mut cause = crate::vault::storage_error(cause, true);
                            cause.mutation_outcome = Some(MutationOutcome::Unknown);
                            Err(cause)
                        }
                        Ok(RecordingPublication::Published { .. }) => unreachable!(),
                    }
                }
            }
        })
        .await
        .map_err(|cause| {
            tracing::error!(%cause, "capture publication worker stopped");
            unconfirmed(
                ErrorCode::Internal,
                "Capture publication outcome could not be confirmed",
            )
        })?;
        result.map_err(|mut cause| {
            let creation = creation.load(std::sync::atomic::Ordering::Acquire);
            if creation > 0
                && matches!(
                    cause.mutation_outcome,
                    None | Some(MutationOutcome::NotCommitted)
                )
            {
                cause.mutation_outcome = Some(if creation == 2 {
                    MutationOutcome::Persisted
                } else {
                    MutationOutcome::Unknown
                });
            }
            cause
        })
    }
}

fn target_session(snapshot: &super::CaptureSnapshot) -> Result<String, ApplicationError> {
    match &snapshot.identity.target {
        CaptureTarget::ExistingSession {
            session_id,
            expected_audio_sha256: None,
        } if snapshot.session_id.as_deref() == Some(session_id.as_str()) => Ok(session_id.clone()),
        CaptureTarget::ExistingSession { .. } => Err(rejected(
            ErrorCode::Validation,
            "Capture publication only supports an absent recording target",
        )),
        CaptureTarget::PendingSession { .. } => {
            let expected = super::target_session_id(&snapshot.identity)?;
            if snapshot
                .session_id
                .as_ref()
                .is_some_and(|bound| bound != &expected)
            {
                return Err(rejected(
                    ErrorCode::Conflict,
                    "Pending capture is bound to another session",
                ));
            }
            Ok(expected)
        }
    }
}

fn publication_request(
    verified: &wav::VerifiedWave,
) -> Result<RecordingPublicationRequest, ApplicationError> {
    let snapshot = &verified.snapshot;
    let source_binding = hash(
        &serde_json::to_vec(&(
            "loofah/capture-wav-f32le/v1",
            &snapshot.identity,
            &snapshot.format,
            &snapshot.seal,
            snapshot.first_admitted_audio_observed_at_ms,
        ))
        .map_err(|_| rejected(ErrorCode::Internal, "Capture binding could not be encoded"))?,
    );
    Ok(RecordingPublicationRequest {
        vault_id: snapshot.identity.scope.vault_id.clone(),
        namespace: snapshot.identity.scope.member_id.clone(),
        request_id: uuid::Uuid::parse_str(&snapshot.identity.request_id)
            .map_err(|_| rejected(ErrorCode::Validation, "Invalid capture request identifier"))?,
        session_id: target_session(snapshot)?,
        source_binding,
        expected: RecordingExpectation::Absent,
        artifact_sha256: verified.artifact_sha256.clone(),
        artifact_size_bytes: verified.artifact_size_bytes,
        layout: if snapshot.format.channels.len() == 2 {
            hypr_vault_read::AudioLayout::MicSystem
        } else {
            hypr_vault_read::AudioLayout::Mixed
        },
        first_audio_observed_at_ms: snapshot
            .first_admitted_audio_observed_at_ms
            .ok_or_else(|| rejected(ErrorCode::Conflict, "Capture has no producer observation"))?,
        capture_ended_at_ms: None,
    })
}

fn operation_key(request: &RecordingPublicationRequest) -> String {
    hash(
        &serde_json::to_vec(&(
            &request.vault_id,
            &request.namespace,
            request.request_id,
            &request.source_binding,
        ))
        .expect("recording publication identity serializes"),
    )
}

async fn reconcile_missing(
    context: &crate::VaultContext,
    root: &std::path::Path,
    request: &RecordingPublicationRequest,
) {
    let Ok(inventory) = context
        .store
        .discover_recording_publications(
            root,
            &context.events.vault_id(),
            RecordingPublicationDiscoveryLimits::default(),
        )
        .await
    else {
        context.recording_publications.block_all();
        return;
    };
    bootstrap::retain_inventory(&context.recording_publications, &inventory);
    let key = operation_key(request);
    if !inventory
        .pending
        .iter()
        .any(|pending| operation_key(pending.request()) == key)
        && !inventory.issues.iter().any(|issue| {
            issue
                .session_id
                .as_deref()
                .is_none_or(|session| session == request.session_id.as_str())
        })
    {
        context
            .recording_publications
            .resolve(&request.session_id, &key);
        if context
            .ensure_recording_publication_ready(&request.session_id)
            .is_ok()
        {
            invalidate_recording(&context.events, &request.session_id);
        }
    }
}

fn invalidate_recording(events: &crate::events::EventHub, session: &str) {
    use hypr_loofah_protocol::{ArtifactChange, ArtifactTarget, Change, IndexEntity};
    events.publish(Change::IndexChanged {
        entity: IndexEntity::Artifacts,
        ids: vec![session.into()],
        artifacts: Some(vec![ArtifactChange {
            session_id: session.into(),
            target: ArtifactTarget::Recording,
        }]),
    });
}

fn before_commit_error(cause: hypr_vault_write::StoreError) -> ApplicationError {
    not_committed(crate::vault::storage_error(cause, false))
}

fn unconfirmed(code: ErrorCode, message: &str) -> ApplicationError {
    let mut cause = rejected(code, message);
    cause.mutation_outcome = Some(MutationOutcome::Unknown);
    cause
}

#[cfg(test)]
mod tests;
