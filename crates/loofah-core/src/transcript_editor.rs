use std::sync::Arc;

use hypr_loofah_protocol::{
    ApplicationError, AssignTranscriptSpeakersRequest, ErrorCode, MutationOutcome,
    TranscriptEditorSaveOutcome, TranscriptEditorSnapshot,
};

use crate::{
    VaultContext,
    vault::{error, storage_error},
};

impl VaultContext {
    pub async fn read_transcript_editor(
        self: &Arc<Self>,
        session_id: &str,
    ) -> Result<TranscriptEditorSnapshot, ApplicationError> {
        self.validate(session_id)?;
        self.ensure_recording_publication_ready(session_id)?;
        self.store
            .read_transcript_editor(session_id)
            .await
            .map_err(|cause| storage_error(cause, false))?
            .ok_or_else(|| error(ErrorCode::NotFound, "Session not found"))
    }

    pub async fn assign_transcript_speakers(
        self: &Arc<Self>,
        session_id: &str,
        input: AssignTranscriptSpeakersRequest,
    ) -> Result<TranscriptEditorSaveOutcome, ApplicationError> {
        self.validate(session_id)?;
        hypr_loofah_protocol::validate_transcript_speakers(&input)
            .map_err(|cause| error(ErrorCode::Validation, cause))?;
        self.ensure_recording_publication_ready(session_id)?;
        let context = self.clone();
        let id = session_id.to_owned();
        tokio::spawn(async move {
            let owner: Box<dyn Send> = if context.local_lifecycle_host.get().is_some() {
                let change = context.try_session_lifecycle_change(&id)?;
                context.ensure_session_jobs_idle(&id).await?;
                context.ensure_capture_session_lifecycle_idle(&id).await?;
                Box::new(context.local_lifecycle_proof(&id, change).await?)
            } else {
                Box::new(context.admit_session_work(&id).await?)
            };
            context
                .store
                .assign_transcript_speakers_checked_owned(&id, input, owner)
                .await
                .map_err(|cause| storage_error(cause, true))?
                .ok_or_else(|| error(ErrorCode::NotFound, "Session not found"))
        })
        .await
        .map_err(|cause| {
            tracing::error!(%cause, "transcript speaker worker stopped");
            let mut failure = error(
                ErrorCode::Internal,
                "Transcript speaker outcome could not be confirmed",
            );
            failure.mutation_outcome = Some(MutationOutcome::Unknown);
            failure
        })?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    struct NativeHost(Arc<AtomicBool>);
    impl crate::LocalSessionLifecycleHost for NativeHost {
        fn admit<'a>(
            &'a self,
            _: &'a Arc<VaultContext>,
            _: &'a str,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<Output = Result<Box<dyn Send + Sync>, ApplicationError>>
                    + Send
                    + 'a,
            >,
        > {
            Box::pin(async move {
                if self.0.load(Ordering::SeqCst) {
                    return Err(crate::session_lifecycle_gate::session_lifecycle_busy());
                }
                Ok(Box::new(()) as Box<dyn Send + Sync>)
            })
        }
    }
    #[tokio::test]
    async fn native_physical_work_blocks_checked_speaker_mutation_without_blocking_reads() {
        let root = tempfile::tempdir().unwrap();
        let vault = root.path().join("vault");
        let session = vault.join("sessions/note");
        std::fs::create_dir_all(&session).unwrap();
        std::fs::write(
            session.join("_meta.json"),
            r#"{"id":"note","title":"Original","created_at":"2026-11-08T10:00:00Z","tags":[]}"#,
        )
        .unwrap();
        let transcript=br#"{"transcripts":[{"id":"t","session_id":"note","words":[{"id":"w","text":"hello","start_ms":0,"end_ms":100,"channel":0}],"speaker_hints":[]}]}"#;
        std::fs::write(session.join("transcript.json"), transcript).unwrap();
        let runtime = crate::VaultRuntime::open(
            "vault".into(),
            &vault,
            &root.path().join("cache"),
            &root.path().join("operations"),
        )
        .await
        .unwrap();
        let context = runtime.context.clone();
        let active = Arc::new(AtomicBool::new(true));
        context
            .configure_local_session_lifecycle_host(Arc::new(NativeHost(active.clone())))
            .unwrap();
        let snapshot = context.read_transcript_editor("note").await.unwrap();
        let request = AssignTranscriptSpeakersRequest {
            expected_revision: snapshot.revision,
            assignments: vec![hypr_loofah_protocol::TranscriptSpeakerAssignment {
                transcript_id: "t".into(),
                channel: 0,
                speaker_index: None,
                speaker_label: "Alice".into(),
                anchor_word_id: "w".into(),
            }],
        };
        assert_eq!(
            context
                .assign_transcript_speakers("note", request.clone())
                .await
                .unwrap_err()
                .mutation_outcome,
            Some(MutationOutcome::NotCommitted)
        );
        assert_eq!(
            std::fs::read(session.join("transcript.json")).unwrap(),
            transcript
        );
        active.store(false, Ordering::SeqCst);
        assert!(matches!(
            context
                .assign_transcript_speakers("note", request)
                .await
                .unwrap(),
            TranscriptEditorSaveOutcome::Saved(_)
        ));
        drop(context);
        runtime.shutdown().await.unwrap();
    }
}

impl VaultContext {
    pub async fn ensure_person_checked(
        self: &Arc<Self>,
        input: hypr_loofah_protocol::EnsurePersonRequest,
    ) -> Result<hypr_loofah_protocol::PersonItem, ApplicationError> {
        hypr_loofah_protocol::validate_person_name(&input.name)
            .map_err(|cause| error(ErrorCode::Validation, cause))?;
        self.store
            .ensure_person_checked(input.name)
            .await
            .map_err(|cause| storage_error(cause, true))
    }
}
