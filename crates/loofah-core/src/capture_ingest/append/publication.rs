use std::{fs::File, future::Future, sync::Arc};

use hypr_capture_store::capture_spool::CaptureWriter;
use hypr_loofah_protocol::{
    ApplicationError, CaptureIngestStatus, CapturePrefixReceipt, CapturePublication, CaptureSeal,
    CaptureTarget, ErrorCode, MutationOutcome,
};
use hypr_vault_write::recording_append::{RecordingAppendArtifact, RecordingAppendOutcome};

use super::super::{rejected, status, wav};
use crate::{VaultContext, capture_append::NormalizedAppendAudio, vault::storage_error};

pub(in crate::vault::capture_ingest) async fn publish<G, F>(
    context: Arc<VaultContext>,
    writer: CaptureWriter,
    expected: CaptureSeal,
    authorize: impl Fn() -> F + Send + 'static,
) -> Result<CaptureIngestStatus, ApplicationError>
where
    G: Send + 'static,
    F: Future<Output = Result<G, ApplicationError>> + Send + 'static,
{
    #[cfg(test)]
    context.trace_append_publication("writer snapshot");
    let snapshot = writer.snapshot().await?;
    let CaptureTarget::AppendSession {
        session_id,
        admission,
    } = &snapshot.identity.target
    else {
        return Err(super::unavailable());
    };
    if snapshot.seal.as_ref() != Some(&expected) || snapshot.format != admission.baseline.format {
        return Err(rejected(
            ErrorCode::Conflict,
            "Append capture differs from its admission",
        ));
    }
    let session = session_id.clone();
    let _session = context.admit_session_work(&session).await?;
    let _request = context.admit_session_lifecycle_request(
        "capture-append-publish",
        &snapshot.identity.scope.member_id,
        uuid::Uuid::parse_str(&admission.request.admission_id).map_err(super::io)?,
    )?;
    #[cfg(test)]
    context.trace_append_publication("original source");
    let source = super::checked_source(&context, &snapshot.identity, false).await?;
    let root = context
        .operations
        .as_ref()
        .ok_or_else(super::unavailable)?
        .root
        .join("recording-appends");
    let identity = super::identity(&admission.request)?;
    #[cfg(test)]
    context.trace_append_publication("existing plan");
    let existing = context
        .store
        .prepare_recording_append_status(&root, identity.clone())
        .await
        .map_err(|cause| storage_error(cause, false))?;
    let plan = if let Some(plan) = existing {
        plan
    } else {
        context.ensure_recording_publication_ready(&session)?;
        let merged = if admission.request.processing.is_some() {
            #[cfg(test)]
            context.trace_append_publication("merged result");
            context
                .capture_append_merged(&snapshot.identity, &expected)
                .await?
                .bytes
        } else {
            source.raw_transcript().map(<[u8]>::to_vec)
        };
        #[cfg(test)]
        context.trace_append_publication("source recheck");
        if !source
            .recheck()
            .await
            .map_err(|cause| storage_error(cause, false))?
        {
            return Err(rejected(
                ErrorCode::Conflict,
                "Append original recording or transcript changed",
            ));
        }
        #[cfg(test)]
        context.trace_append_publication("verify suffix");
        let verified = wav::verify(&writer, &expected).await?;
        let baseline = admission.baseline.clone();
        let directory = source.directory().to_owned();
        let attempt = uuid::Uuid::new_v4();
        let suffix_path = directory.join(format!("core-suffix-{attempt}.wav"));
        let output_path = directory.join(format!("core-combined-{attempt}.wav"));
        let (mut reader, producer) = verified.reader(context.clone(), writer.clone());
        let pin = context.clone();
        let suffix = NormalizedAppendAudio {
            frames: expected.frames,
            bytes: verified.artifact_size_bytes,
            sha256: verified.artifact_sha256,
            format: verified.snapshot.format,
        };
        #[cfg(test)]
        context.trace_append_publication("combine artifact");
        let combined = tokio::task::spawn_blocking(move || {
            let _pin = pin;
            let mut output = File::create_new(&suffix_path).map_err(super::io)?;
            std::io::copy(&mut reader, &mut output).map_err(super::io)?;
            output.sync_all().map_err(super::io)?;
            drop(output);
            let prefix = NormalizedAppendAudio {
                frames: baseline.frames,
                bytes: baseline.normalized_bytes,
                sha256: baseline.normalized_sha256,
                format: baseline.format,
            };
            let combined = crate::capture_append::concatenate(
                &directory.join("core-normalized.wav"),
                &prefix,
                &suffix_path,
                &suffix,
                &output_path,
            )?;
            Ok::<_, ApplicationError>((File::open(output_path).map_err(super::io)?, combined))
        })
        .await;
        // Reader cancellation does not end the real source operation. Join its
        // owned task before releasing Context or propagating an artifact error.
        let produced = producer.await;
        let (file, combined) = combined.map_err(super::io)??;
        produced.map_err(super::io)??;
        let artifact = RecordingAppendArtifact {
            sha256: combined.sha256,
            size_bytes: combined.bytes,
            frames: combined.frames,
            layout: source.audio().layout,
        };
        #[cfg(test)]
        context.trace_append_publication("prepare compound plan");
        context
            .store
            .prepare_recording_append(&root, identity, source, file, artifact, merged)
            .await
            .map_err(|cause| storage_error(cause, false))?
    };
    #[cfg(test)]
    context.trace_append_publication("publication gate");
    let mut ticket = context
        .recording_publications
        .begin(
            &session,
            &super::super::publication::bootstrap::append_key(&super::identity(
                &admission.request,
            )?),
        )?
        .with_events(context.events.clone());
    #[cfg(test)]
    context.trace_append_publication("plan recheck");
    let plan = plan
        .recheck()
        .await
        .map_err(|cause| storage_error(cause, false))?;
    #[cfg(test)]
    context.trace_append_publication("final authorization");
    let _grant = authorize().await?;
    ticket.committing();
    #[cfg(test)]
    context.trace_append_publication("compound commit");
    let result = plan
        .commit()
        .await
        .map_err(|cause| storage_error(cause, true))?;
    let (sha256, size_bytes) = match result {
        RecordingAppendOutcome::Published {
            sha256, size_bytes, ..
        }
        | RecordingAppendOutcome::AlreadyCommitted {
            sha256, size_bytes, ..
        } => (sha256, size_bytes),
        RecordingAppendOutcome::Changed => {
            ticket.resolved();
            return Err(rejected(
                ErrorCode::Conflict,
                "Append source changed; retain the capture for review",
            ));
        }
        RecordingAppendOutcome::Busy => {
            return Err(unknown(
                ErrorCode::Unavailable,
                "Append transaction needs recovery",
            ));
        }
        RecordingAppendOutcome::RequiresReview => {
            return Err(unknown(
                ErrorCode::Conflict,
                "Append transaction requires review",
            ));
        }
    };
    ticket.resolved();
    let publication = CapturePublication {
        source: expected,
        session_id: session,
        canonical_audio_sha256: sha256,
        canonical_audio_bytes: size_bytes,
    };
    #[cfg(test)]
    context.trace_append_publication("publication receipt");
    writer
        .acknowledge_prefix(CapturePrefixReceipt {
            capture_fingerprint: publication.source.capture_fingerprint.clone(),
            chunk_count: publication.source.chunk_count,
            frames: publication.source.frames,
            pcm_bytes: publication.source.pcm_bytes,
            chain_digest: publication.source.chain_digest.clone(),
        })
        .await
        .map_err(persisted)?;
    status(
        writer
            .observe_publication(publication)
            .await
            .map_err(persisted)?,
    )
}
fn unknown(code: ErrorCode, message: &str) -> ApplicationError {
    let mut error = rejected(code, message);
    error.mutation_outcome = Some(MutationOutcome::Unknown);
    error
}
fn persisted(mut error: ApplicationError) -> ApplicationError {
    error.mutation_outcome = Some(MutationOutcome::Persisted);
    error
}
