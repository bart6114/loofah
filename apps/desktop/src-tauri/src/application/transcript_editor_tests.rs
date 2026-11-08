use super::*;
use hypr_loofah_protocol::{
    AssignTranscriptSpeakersRequest, EnsurePersonRequest, MutationOutcome,
    TranscriptEditorSaveOutcome, TranscriptSpeakerAssignment,
};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

struct PhysicalHost {
    active: Arc<AtomicBool>,
    calls: Arc<AtomicUsize>,
}
impl hypr_loofah_core::LocalSessionLifecycleHost for PhysicalHost {
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
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.active.load(Ordering::SeqCst) {
                let mut error = hypr_loofah_client::failure(
                    ErrorCode::Conflict,
                    "Physical capture remains active",
                );
                error.mutation_outcome = Some(MutationOutcome::NotCommitted);
                return Err(error);
            }
            Ok(Box::new(()) as Box<dyn Send + Sync>)
        })
    }
}

#[tokio::test]
async fn local_checked_transcript_mutations_require_registered_physical_host_and_preserve_busy_sources()
 {
    let root = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let vault = root.path().join("vault");
    let session = vault.join("sessions/note");
    std::fs::create_dir_all(&session).unwrap();
    std::fs::write(
        session.join("_meta.json"),
        br#"{"id":"note","title":"Original","created_at":"2026-11-08T10:00:00Z","tags":[]}"#,
    )
    .unwrap();
    let raw = br#"{"transcripts":[{"id":"t","session_id":"note","words":[{"id":"w","text":"hello","start_ms":0,"end_ms":100,"channel":0}],"speaker_hints":[]}],"user_unknown":{"keep":true}}"#;
    std::fs::write(session.join("transcript.json"), raw).unwrap();
    let runtime = hypr_loofah_core::VaultRuntime::open(
        "local:checked-transcript".into(),
        &vault,
        &root.path().join("cache"),
        &root.path().join("operations"),
    )
    .await
    .unwrap();
    runtime.context.startup.wait_until_ready().await.unwrap();
    let context = runtime.context.clone();
    let app = tauri::test::mock_builder()
        .manage(context.clone())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    let snapshot = application_transcript_editor(app.handle().clone(), "note".into())
        .await
        .unwrap();
    let request = AssignTranscriptSpeakersRequest {
        expected_revision: snapshot.revision.clone(),
        assignments: vec![TranscriptSpeakerAssignment {
            transcript_id: "t".into(),
            channel: 0,
            speaker_index: None,
            speaker_label: "Alice".into(),
            anchor_word_id: "w".into(),
        }],
    };
    let missing = application_assign_transcript_speakers(
        app.handle().clone(),
        "note".into(),
        request.clone(),
    )
    .await
    .unwrap_err();
    assert_eq!(
        missing.mutation_outcome,
        Some(MutationOutcome::NotCommitted)
    );
    assert_eq!(
        application_ensure_person(
            app.handle().clone(),
            EnsurePersonRequest {
                name: "Alice".into()
            }
        )
        .await
        .unwrap_err()
        .mutation_outcome,
        Some(MutationOutcome::NotCommitted)
    );
    assert_eq!(std::fs::read(session.join("transcript.json")).unwrap(), raw);
    let active = Arc::new(AtomicBool::new(true));
    let calls = Arc::new(AtomicUsize::new(0));
    context
        .configure_local_session_lifecycle_host(Arc::new(PhysicalHost {
            active: active.clone(),
            calls: calls.clone(),
        }))
        .unwrap();
    let busy = application_assign_transcript_speakers(
        app.handle().clone(),
        "note".into(),
        request.clone(),
    )
    .await
    .unwrap_err();
    assert_eq!(busy.mutation_outcome, Some(MutationOutcome::NotCommitted));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(std::fs::read(session.join("transcript.json")).unwrap(), raw);
    assert_eq!(
        application_transcript_editor(app.handle().clone(), "note".into())
            .await
            .unwrap()
            .revision,
        snapshot.revision
    );
    active.store(false, Ordering::SeqCst);
    let person = application_ensure_person(
        app.handle().clone(),
        EnsurePersonRequest {
            name: "Alice".into(),
        },
    )
    .await
    .unwrap();
    assert_eq!(person.name, "Alice");
    let outcome = application_assign_transcript_speakers(
        app.handle().clone(),
        "note".into(),
        request.clone(),
    )
    .await
    .unwrap();
    let TranscriptEditorSaveOutcome::Saved(saved) = outcome else {
        panic!("original checked edit must save")
    };
    assert_ne!(saved.revision, snapshot.revision);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert!(matches!(
        application_assign_transcript_speakers(app.handle().clone(), "note".into(), request)
            .await
            .unwrap(),
        TranscriptEditorSaveOutcome::Conflict(_)
    ));
    let after: serde_json::Value =
        serde_json::from_slice(&std::fs::read(session.join("transcript.json")).unwrap()).unwrap();
    assert_eq!(after["user_unknown"]["keep"], true);
    drop(app);
    drop(context);
    runtime.shutdown().await.unwrap();
}
