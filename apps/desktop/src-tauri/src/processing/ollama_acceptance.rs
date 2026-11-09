use super::ProcessingState;
use crate::application;
use futures_util::FutureExt;
use hypr_loofah_core::{
    VaultContext, VaultRuntime,
    local_cli::{LocalCli, LocalCliArtifact},
    manual_media_intents::{
        ManualMediaKind, ManualMediaSummaryDeclaration, ManualMediaSummaryState,
    },
};
use hypr_loofah_protocol::{
    CaptureProcessingSummary, DocumentTarget, ListProcessingJobsInput, LocalCliOperation,
    ProcessingJobState, ProcessingSettingsSaveOutcome, ProcessingTitleStatus,
    SaveProcessingSettingsRequest, SummaryModelSelection, TitlePolicy,
};
use std::{fs::File, path::Path, sync::Arc, time::Duration};
use tauri::Manager;

const NOTE: &str = "# Project Saffron\n\nPersonal reminder: keep the original planning notes.\n\n- Confirm the restore drill before Friday.\n";
const TRANSCRIPT: &str = concat!(
    "Maya: We reviewed the Project Saffron migration. The pilot will start on Friday with the support team. ",
    "We agreed to keep the current service available until the restore drill passes.\n\n",
    "Jon: I will run the database restore drill on Thursday and document the results. ",
    "Maya will notify the support team after we confirm the rollback plan.\n\n",
    "Priya: The remaining risk is missing customer attachments during migration. ",
    "I will compare attachment counts on Wednesday. We will postpone the pilot if any files are missing.\n",
);

fn word_timed_subtitles() -> String {
    fn time(milliseconds: usize) -> String {
        format!(
            "{:02}:{:02}:{:02},{:03}",
            milliseconds / 3_600_000,
            milliseconds / 60_000 % 60,
            milliseconds / 1_000 % 60,
            milliseconds % 1_000,
        )
    }
    // Each SRT cue is one canonical transcript entry, even if its text has many words.
    TRANSCRIPT
        .split_whitespace()
        .enumerate()
        .map(|(index, word)| {
            let start = 1_000 + index * 500;
            format!(
                "{}\n{} --> {}\n{word}\n\n",
                index + 1,
                time(start),
                time(start + 450)
            )
        })
        .collect()
}

fn configured_model() -> SummaryModelSelection {
    let base_url = std::env::var("LOOFAH_TEST_OLLAMA_BASE_URL")
        .expect("set LOOFAH_TEST_OLLAMA_BASE_URL to the prepared loopback Ollama /v1 endpoint");
    let url = tauri::Url::parse(&base_url).expect("valid Ollama endpoint");
    assert_eq!(
        url.scheme(),
        "http",
        "this acceptance is offline loopback-only"
    );
    assert!(
        url.host_str()
            .and_then(|host| host
                .trim_matches(['[', ']'])
                .parse::<std::net::IpAddr>()
                .ok())
            .is_some_and(|address| address.is_loopback()),
        "use an explicit loopback IP; this test never contacts a remote provider"
    );
    assert!(url.username().is_empty() && url.password().is_none());
    assert!(url.query().is_none() && url.fragment().is_none());
    assert_eq!(url.path().trim_end_matches('/'), "/v1");
    let model_id = std::env::var("LOOFAH_TEST_OLLAMA_MODEL")
        .expect("set LOOFAH_TEST_OLLAMA_MODEL to an already downloaded local model");
    assert!(!model_id.trim().is_empty() && !model_id.chars().any(char::is_control));
    SummaryModelSelection {
        provider_id: "ollama".into(),
        model_id,
        base_url,
    }
}

async fn declare_and_import(
    context: &Arc<VaultContext>,
    cli: &Arc<LocalCli>,
    root: &Path,
    session: &str,
    text: &str,
) -> (String, String) {
    context
        .store
        .create_note_session(
            &serde_json::from_value(serde_json::json!({
                "id":session, "title":"", "created_at":"2026-11-08T10:00:00Z", "tags":[]
            }))
            .unwrap(),
            NOTE,
        )
        .await
        .unwrap();
    let request = uuid::Uuid::new_v4().to_string();
    let parent = context
        .declare_manual_media_summary(ManualMediaSummaryDeclaration {
            request_id: request.clone(),
            session_id: session.into(),
            fresh_session: false,
            expected_created_at: None,
            kind: ManualMediaKind::Subtitles,
            summary: Some(CaptureProcessingSummary {
                target: DocumentTarget::Summary,
                title_policy: TitlePolicy::GenerateIfEmpty,
            }),
            only_if_empty: false,
        })
        .await
        .unwrap();
    assert_eq!(parent.state, ManualMediaSummaryState::PendingMedia);
    let path = root.join(format!("{session}.srt"));
    std::fs::write(&path, text).unwrap();
    let descriptor = hypr_fs_sync_core::media::metadata_from_reader(
        "meeting.srt",
        "application/x-subrip",
        text.as_bytes(),
    )
    .unwrap();
    cli.execute(
        request.clone(),
        LocalCliOperation::ImportSubtitles {
            session_id: session.into(),
            filename: "meeting.srt".into(),
        },
        Some(LocalCliArtifact::new(
            File::open(&path).unwrap(),
            descriptor.size_bytes,
            descriptor.sha256,
            Some(path),
            Box::new(()),
        )),
        Arc::new(|_| {}),
    )
    .await
    .unwrap();
    (request, parent.child_request_id)
}

#[tokio::test]
#[ignore = "requires opt-in loopback Ollama URL/model with a previously downloaded local model"]
async fn configured_local_ollama_publishes_automatic_summary_and_title_without_login() {
    let model = configured_model();
    let root = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let vault = root.path().join("vault");
    std::fs::create_dir(&vault).unwrap();
    let hypr_storage::vault::LeaseAttempt::Acquired(lease) =
        hypr_storage::vault::VaultLease::try_acquire_existing(&vault).unwrap()
    else {
        panic!("acceptance vault must have its original exclusive lease")
    };
    let runtime = VaultRuntime::open_leased(
        "local:ollama-acceptance".into(),
        lease.clone(),
        &root.path().join("cache"),
        &root.path().join("operations"),
        hypr_vault_write::ContentLimits::unlimited(),
    )
    .await
    .unwrap();
    tokio::time::timeout(
        Duration::from_secs(30),
        runtime.context.startup.wait_until_ready(),
    )
    .await
    .unwrap()
    .unwrap();
    runtime
        .context
        .enable_standalone_local_session_lifecycle(&lease)
        .unwrap();
    let context = runtime.context.clone();
    let cli = Arc::new(LocalCli::new(context.clone()).unwrap());
    // The production constructor builds Native LocalModels and DeviceSecrets. Ollama
    // must resolve without a credential-store plugin, account state or API key.
    let app = tauri::test::mock_builder()
        .manage(ProcessingState::default())
        .manage(context.clone())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    app.state::<ProcessingState>()
        .start(app.handle().clone(), &context)
        .await
        .unwrap();
    let checked = std::panic::AssertUnwindSafe(async {
        let mut settings = application::application_processing_settings(app.handle().clone())
            .await
            .unwrap();
        settings.settings.summary_model = Some(model.clone());
        let saved = application::application_save_processing_settings(
            app.handle().clone(),
            SaveProcessingSettingsRequest {
                settings: settings.settings,
                expected_revision: settings.revision,
            },
        )
        .await
        .unwrap();
        assert!(matches!(saved, ProcessingSettingsSaveOutcome::Saved(_)));
        let (request, child) = declare_and_import(
            &context, &cli, root.path(), "eligible", &word_timed_subtitles(),
        )
        .await;
        let session = vault.join("sessions/eligible");
        let original_note = std::fs::read(session.join("notes.md")).unwrap();
        let original_transcript = std::fs::read(session.join("transcript.json")).unwrap();
        assert_eq!(original_note, NOTE.as_bytes());
        let transcripts = context.session_transcripts("eligible").await.unwrap();
        assert!(transcripts.iter().map(|t| t.words.len()).sum::<usize>() > 70);
        let admitted = context.complete_manual_media_summary(request.clone()).await.unwrap();
        assert_eq!(admitted.state, ManualMediaSummaryState::Child, "{admitted:?}");
        assert_eq!(admitted.child_request_id, child);
        let original_job = admitted.job.unwrap().id;
        let mut saw_generating_preview = false;
        let terminal = tokio::time::timeout(Duration::from_secs(300), async {
            loop {
                let status = context.manual_media_summary_status(&request).await.unwrap();
                let job = status.job.expect("original eligible child");
                assert_eq!(job.id, original_job);
                let preview = application::application_processing_job_preview(app.handle().clone(), original_job.clone()).await.unwrap();
                assert_eq!(preview.vault_id, "local:ollama-acceptance");
                assert_eq!(preview.job.id, original_job);
                assert_eq!(preview.job.session_id, "eligible");
                assert_eq!(preview.job.target, DocumentTarget::Summary);
                if let Some(partial) = preview.preview {
                    assert!(partial.markdown.len() <= hypr_loofah_protocol::MAX_PROCESSING_PREVIEW_BYTES);
                    assert!(!partial.markdown.contains("<think") && !partial.markdown.contains("<loofah-tags"));
                    if preview.job.phase == Some(hypr_loofah_protocol::ProcessingPhase::Generating) && !partial.markdown.trim().is_empty() {
                        saw_generating_preview = true;
                    }
                }
                if job.state.is_terminal() { break job; }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }).await.expect("local summary/title did not finish within five minutes");
        assert_eq!(terminal.state, ProcessingJobState::Succeeded, "{terminal:?}");
        assert!(saw_generating_preview, "the actual Native LocalModels path must expose a nonempty partial while generating the original summary");
        let result = application::application_processing_job_result(
            app.handle().clone(), original_job.clone(),
        ).await.unwrap().unwrap();
        let retained_preview = application::application_processing_job_preview(app.handle().clone(), original_job.clone()).await.unwrap();
        assert_eq!(retained_preview.job.state, ProcessingJobState::Succeeded);
        assert!(retained_preview.preview.as_ref().is_some_and(|preview| !preview.markdown.trim().is_empty()));
        assert!(!result.markdown.trim().is_empty());
        let normalized = result.markdown.to_lowercase();
        assert!(normalized.contains("saffron") && ["restore", "migration", "pilot", "attachment"].iter().any(|term| normalized.contains(term)), "summary lost the meeting subject or a relevant fact: {result:?}");
        let summary = context.read_document("eligible", &DocumentTarget::Summary).await.unwrap();
        let published_summary = summary.document.unwrap().markdown;
        let title = result.title.as_ref().expect("requested title stage must be retained");
        assert_eq!(title.status, ProcessingTitleStatus::Applied, "{title:?}");
        let meta = context.read_metadata("eligible").await.unwrap().meta;
        assert!(!meta.title.trim().is_empty());
        assert_eq!(Some(meta.title.as_str()), title.candidate.as_deref());
        let expected_summary = if result.markdown.trim_start().lines().next() == Some(&format!("# {}", meta.title.trim())) {
            result.markdown.clone()
        } else {
            format!("# {}\n\n{}", meta.title.trim(), result.markdown.trim_start()).trim().to_owned()
        };
        assert_eq!(published_summary, expected_summary, "title publication must preserve the complete generated summary body");
        let expected_note = format!("# {}\n\n{}", meta.title.trim(), std::str::from_utf8(&original_note).unwrap().trim_start()).trim().to_owned();
        assert_eq!(std::fs::read(session.join("notes.md")).unwrap(), expected_note.as_bytes(), "the generated title may stamp the note, but must preserve all original user prose");
        assert_eq!(std::fs::read(session.join("transcript.json")).unwrap(), original_transcript);
        let replay = context.complete_manual_media_summary(request.clone()).await.unwrap();
        assert_eq!(replay.child_request_id, child);
        assert_eq!(replay.job.unwrap().id, original_job);
        assert_eq!(application::application_processing_job_preview(app.handle().clone(), original_job.clone()).await.unwrap(), retained_preview);
        assert_eq!(context.read_document("eligible", &DocumentTarget::Summary).await.unwrap().document.unwrap().markdown, published_summary);
        assert_eq!(application::application_processing_job_result(app.handle().clone(), original_job).await.unwrap(), Some(result));

        let (short, _) = declare_and_import(&context, &cli, root.path(), "short", "1\n00:00:01,000 --> 00:00:02,000\nThank you.\n").await;
        let skipped = context.complete_manual_media_summary(short).await.unwrap();
        assert_eq!(skipped.state, ManualMediaSummaryState::Skipped, "{skipped:?}");
        assert!(skipped.job.is_none());
        assert!(context.read_document("short", &DocumentTarget::Summary).await.unwrap().document.is_none());
        let jobs = application::application_processing_jobs(app.handle().clone(), ListProcessingJobsInput::default()).await.unwrap();
        assert_eq!(jobs.items.len(), 1, "same parent replay and ineligible input must create no extra model job");
        assert_eq!(std::fs::read(session.join("notes.md")).unwrap(), expected_note.as_bytes());
        assert_eq!(std::fs::read(session.join("transcript.json")).unwrap(), original_transcript);
        eprintln!("Native Ollama acceptance passed: model={}, summary_bytes={}, title={:?}, original parent={request}, original child={child}", model.model_id, normalized.len(), meta.title);
    }).catch_unwind().await;
    // Always drain real model/title work before releasing the fixture, including failed assertions.
    let processing_drained = app.state::<ProcessingState>().shutdown().await;
    drop(cli);
    let runtime_drained = runtime.shutdown().await;
    if let Err(panic) = checked {
        std::panic::resume_unwind(panic);
    }
    processing_drained.unwrap();
    runtime_drained.unwrap();
}
