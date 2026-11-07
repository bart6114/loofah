use std::sync::Mutex;

use super::super::{
    JobAuthorizer,
    generation::{ModelChunk, ModelRequest, ModelStream, SummaryGenerator, TextModel},
};
use super::*;
use crate::{VaultRuntime, search::SearchService, settings::ConfigState, startup::StartupState};
use hypr_loofah_protocol::{DocumentTarget, ProcessingJobState, SummaryJobRequest};
use hypr_vault_write::SessionStore;

struct Fixture {
    root: tempfile::TempDir,
    context: Arc<VaultContext>,
}
impl Fixture {
    async fn new(note: &str) -> Self {
        let root = tempfile::tempdir().unwrap();
        let context = Arc::new(
            VaultContext::new(
                "vault-one".into(),
                Arc::new(SessionStore::new(root.path().into())),
                StartupState::new(Some(root.path())),
                Arc::new(ConfigState::load_or_default(root.path())),
                Arc::new(SearchService::default()),
            )
            .with_operation_storage(root.path().join("runtime")),
        );
        let meta = serde_json::from_value(serde_json::json!({"id":"note","title":"Release planning","created_at":"2026-11-07T10:00:00Z","tags":["release"]})).unwrap();
        context
            .store
            .create_note_session(&meta, note)
            .await
            .unwrap();
        Self { root, context }
    }
    async fn prepare(
        &self,
        identity: SummaryIdentity,
        principal: &str,
        images: bool,
    ) -> Result<SummaryPrompt, ProcessingJobFailure> {
        VaultSummarySource::new(&self.context, identity, images)
            .prepare(work(principal), CancellationToken::new())
            .await
    }
}

fn work(principal: &str) -> JobWork {
    JobWork {
        principal: principal.into(),
        request: SummaryJobRequest {
            session_id: "note".into(),
            target: DocumentTarget::Summary,
            expected_revision: "revision".into(),
        },
    }
}

fn transcript(owner: &str) -> serde_json::Value {
    serde_json::json!({"transcripts":[{"id":"t1","session_id":"note","user_id":owner,"started_at":1000,"ended_at":2000,"memo_md":"Context before the meeting.","words":[{"id":"w1","text":"Ship Friday.","start_ms":0,"end_ms":1000,"channel":0,"metadata":{"capture_source":"recording"}}]}]})
}

#[test]
fn source_availability_matches_existing_note_markdown_and_legacy_editor_cases() {
    for note in [
        "",
        " \n",
        "&nbsp;",
        "---",
        "# ",
        "![photo](attachments/photo.png)",
        r#"{"type":"doc","content":[{"type":"paragraph"}]}"#,
    ] {
        assert!(!has_note_text(note), "unexpected source: {note}");
    }
    for note in [
        "Go",
        "# Decision\n\nShip it.",
        r#"{"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"Go"}]}]}"#,
        "{not-json",
        "{}",
    ] {
        assert!(has_note_text(note), "missing source: {note}");
    }
}

#[tokio::test]
async fn stock_source_reads_files_and_settings_without_a_desktop_or_login() {
    let f = Fixture::new("Stale index body").await;
    std::fs::write(
        f.root.path().join("sessions/note/notes.md"),
        "Fresh file-backed note.",
    )
    .unwrap();
    let mut values = std::collections::HashMap::new();
    values.insert("ai_language".into(), "nl".into());
    values.insert(
        "auto_summary_prompt".into(),
        "Decisions in {{ language }}.".into(),
    );
    f.context.settings.set_values(values).await.unwrap();
    let prepared = f
        .prepare(SummaryIdentity::LocalOwner, "trusted-local-device", false)
        .await
        .unwrap();
    assert!(prepared.custom_prompt);
    assert!(prepared.system.contains("Decisions in Dutch."));
    assert!(
        prepared
            .prompt
            .contains("# Notes\n\nFresh file-backed note.")
    );
    assert!(prepared.prompt.contains("- release"));
    assert!(prepared.images.is_empty());
    assert!(!prepared.prompt.contains("Stale index body"));
    assert_eq!(
        std::fs::read_to_string(f.root.path().join("sessions/note/notes.md")).unwrap(),
        "Fresh file-backed note."
    );
}

#[tokio::test]
async fn summary_identity_uses_recording_ownership_and_never_a_speaker_name_or_unknown_capture() {
    let f = Fixture::new("").await;
    let alice = "00000000-0000-0000-0000-000000000001";
    let bob = "00000000-0000-0000-0000-000000000002";
    std::fs::write(
        f.root.path().join("people.json"),
        serde_json::json!({"people":[{"id":alice,"name":"Alice"},{"id":bob,"name":"You"}]})
            .to_string(),
    )
    .unwrap();
    let path = f.root.path().join("sessions/note/transcript.json");
    std::fs::write(&path, transcript(alice).to_string()).unwrap();
    let own = f
        .prepare(SummaryIdentity::JobMember, alice, false)
        .await
        .unwrap();
    assert!(own.prompt.contains("[current user]: Ship Friday."));
    let other = f
        .prepare(SummaryIdentity::JobMember, bob, false)
        .await
        .unwrap();
    assert!(other.prompt.contains("Alice: Ship Friday."));
    assert!(!other.prompt.contains("Alice [current user]"));
    let mut unknown = transcript(bob);
    unknown["transcripts"][0]["words"][0]["metadata"] = serde_json::json!({});
    std::fs::write(&path, unknown.to_string()).unwrap();
    let unknown = f
        .prepare(SummaryIdentity::JobMember, bob, false)
        .await
        .unwrap();
    assert!(!unknown.prompt.contains("[current user]:"));
    std::fs::write(&path, transcript("").to_string()).unwrap();
    let legacy = f
        .prepare(SummaryIdentity::LocalOwner, "device", false)
        .await
        .unwrap();
    assert!(legacy.prompt.contains("[current user]: Ship Friday."));
    let remote = f
        .prepare(SummaryIdentity::JobMember, bob, false)
        .await
        .unwrap();
    assert!(!remote.prompt.contains("[current user]:"));
    assert_eq!(
        f.prepare(SummaryIdentity::JobMember, "forged-member", false)
            .await
            .unwrap_err(),
        ProcessingJobFailure::AccessDenied
    );
}

#[tokio::test]
async fn buffered_transcripts_are_committed_before_prompt_preparation_and_empty_sources_fail() {
    let f = Fixture::new("").await;
    assert_eq!(
        f.prepare(SummaryIdentity::LocalOwner, "device", false)
            .await
            .unwrap_err(),
        ProcessingJobFailure::SourceUnavailable
    );
    let delta = serde_json::from_value(serde_json::json!({"transcript_id":"live","started_at_ms":1000,"new_words":[{"id":"w1","text":"Go","start_ms":0,"end_ms":10,"channel":0,"metadata":{"capture_source":"recording"}}],"replaced_ids":[],"new_hints":[]})).unwrap();
    f.context
        .store
        .append_transcript("note", delta)
        .await
        .unwrap();
    let prepared = f
        .prepare(SummaryIdentity::LocalOwner, "device", false)
        .await
        .unwrap();
    assert!(prepared.prompt.contains("# Transcript"));
    assert!(prepared.prompt.contains("Go"));
    let saved: serde_json::Value = serde_json::from_slice(
        &std::fs::read(f.root.path().join("sessions/note/transcript.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(saved["transcripts"][0]["words"][0]["text"], "Go");
}

pub(super) fn png() -> Vec<u8> {
    let image = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
        2,
        2,
        image::Rgb([20, 100, 200]),
    ));
    let mut bytes = std::io::Cursor::new(Vec::new());
    image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
    bytes.into_inner()
}

#[tokio::test]
async fn images_use_referenced_managed_files_and_never_follow_remote_or_device_paths() {
    let f = Fixture::new("Context\n![ok](attachments/chart.png)\n![remote](https://example.com/private.png)\n![loose](user.png)\n![outside](file:///outside/private.png)").await;
    let directory = f.root.path().join("sessions/note/attachments");
    std::fs::create_dir(&directory).unwrap();
    let bytes = png();
    std::fs::write(directory.join("chart.png"), &bytes).unwrap();
    std::fs::write(directory.join("unreferenced.png"), &bytes).unwrap();
    std::fs::write(f.root.path().join("sessions/note/user.png"), &bytes).unwrap();
    let prepared = f
        .prepare(SummaryIdentity::LocalOwner, "device", true)
        .await
        .unwrap();
    assert_eq!(prepared.images.len(), 1);
    assert_eq!(prepared.images[0].bytes, bytes);
    assert!(
        prepared
            .prompt
            .contains("Attached note images are included as visual context.")
    );
    assert!(
        f.prepare(SummaryIdentity::LocalOwner, "device", false)
            .await
            .unwrap()
            .images
            .is_empty()
    );
    assert_eq!(std::fs::read(directory.join("chart.png")).unwrap(), bytes);
    #[cfg(unix)]
    {
        std::fs::remove_file(directory.join("chart.png")).unwrap();
        std::os::unix::fs::symlink(
            directory.join("unreferenced.png"),
            directory.join("chart.png"),
        )
        .unwrap();
        assert!(
            f.prepare(SummaryIdentity::LocalOwner, "device", true)
                .await
                .unwrap()
                .images
                .is_empty()
        );
    }
}

#[tokio::test]
async fn image_context_samples_the_full_note_and_enforces_the_aggregate_budget() {
    let note = format!(
        "Context\n{}",
        (0..20)
            .map(|index| format!("![image](attachments/chart-{index}.jpg)\n"))
            .collect::<String>()
    );
    let f = Fixture::new(&note).await;
    let directory = f.root.path().join("sessions/note/attachments");
    std::fs::create_dir(&directory).unwrap();
    for index in 0..20 {
        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 90)
            .encode_image(&image::RgbImage::from_pixel(
                2,
                3,
                image::Rgb([index * 10, 100, 40]),
            ))
            .unwrap();
        let mut bytes = jpeg[..2].to_vec();
        for _ in 0..2 {
            bytes.extend_from_slice(&[0xff, 0xfe]);
            bytes.extend_from_slice(&((55 * 1024 + 2) as u16).to_be_bytes());
            bytes.extend_from_slice(&vec![b'x'; 55 * 1024]);
        }
        bytes.extend_from_slice(&jpeg[2..]);
        std::fs::write(directory.join(format!("chart-{index}.jpg")), bytes).unwrap();
    }
    let prepared = f
        .prepare(SummaryIdentity::LocalOwner, "device", true)
        .await
        .unwrap();
    assert_eq!(prepared.images.len(), 10);
    assert!(
        prepared
            .images
            .iter()
            .all(|image| image.bytes.len() <= 128 * 1024)
    );
    assert!(
        prepared
            .images
            .iter()
            .map(|image| image.bytes.len())
            .sum::<usize>()
            <= 768 * 1024
    );
    assert!(prepared.images.iter().any(|image| image.bytes.len() < 4096));
    let indexes: Vec<_> = prepared
        .images
        .iter()
        .map(|image| {
            let decoded = image::load_from_memory(&image.bytes).unwrap().to_rgb8();
            (decoded.get_pixel(0, 0).0[0] as f64 / 10.0).round() as usize
        })
        .collect();
    assert_eq!(indexes, [0, 2, 4, 6, 8, 11, 13, 15, 17, 19]);
}

#[tokio::test]
async fn budgets_cancellation_and_expired_contexts_cannot_start_a_model_source() {
    let f = Fixture::new("Source text").await;
    let reader = VaultSummarySource::new(&f.context, SummaryIdentity::LocalOwner, false)
        .with_limits(SourceReadLimits {
            note_bytes: 2,
            ..Default::default()
        });
    assert_eq!(
        reader
            .prepare(work("device"), CancellationToken::new())
            .await
            .unwrap_err(),
        ProcessingJobFailure::ResourceLimit
    );
    assert!(f.context.read_note("note").await.is_ok());
    let reader = VaultSummarySource::new(&f.context, SummaryIdentity::LocalOwner, false);
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert_eq!(
        reader.prepare(work("device"), cancel).await.unwrap_err(),
        ProcessingJobFailure::Interrupted
    );
    drop(f);
    assert_eq!(
        reader
            .prepare(work("device"), CancellationToken::new())
            .await
            .unwrap_err(),
        ProcessingJobFailure::SourceUnavailable
    );
}

#[tokio::test]
async fn identical_session_and_attachment_names_stay_isolated_between_vaults() {
    let a = Fixture::new("Source from vault A.\n![chart](attachments/chart.png)").await;
    let b = Fixture::new("Source from vault B.\n![chart](attachments/chart.png)").await;
    let first = png();
    let mut second = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(2, 2, image::Rgb([200, 40, 10])))
        .write_to(&mut second, image::ImageFormat::Png)
        .unwrap();
    let second = second.into_inner();
    for (fixture, bytes) in [(&a, &first), (&b, &second)] {
        let directory = fixture.root.path().join("sessions/note/attachments");
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(directory.join("chart.png"), bytes).unwrap();
    }
    for (fixture, own, other, bytes) in [
        (&a, "vault A", "vault B", first.as_slice()),
        (&b, "vault B", "vault A", second.as_slice()),
        (&a, "vault A", "vault B", first.as_slice()),
    ] {
        let prepared = fixture
            .prepare(SummaryIdentity::LocalOwner, "device", true)
            .await
            .unwrap();
        assert!(prepared.prompt.contains(own));
        assert!(!prepared.prompt.contains(other));
        assert_eq!(prepared.images[0].bytes, bytes);
    }
}

struct Model(Mutex<Vec<ModelRequest>>);
impl TextModel for Model {
    fn stream(
        &self,
        request: ModelRequest,
        _: CancellationToken,
    ) -> JobFuture<'_, Result<ModelStream, ProcessingJobFailure>> {
        self.0.lock().unwrap().push(request);
        Box::pin(async {
            Ok(Box::pin(futures_util::stream::iter([
                Ok(ModelChunk::Text(
                    "# Release plan\n- Ship Friday.<loofah-tags>{\"tags\":[]}</loofah-tags>".into(),
                )),
                Ok(ModelChunk::Finish { truncated: false }),
            ])) as ModelStream)
        })
    }
}
struct Authorizer;
impl JobAuthorizer for Authorizer {
    fn authorize<'a>(
        &'a self,
        _: &'a JobWork,
    ) -> JobFuture<'a, Result<Box<dyn Send>, ProcessingJobFailure>> {
        Box::pin(async { Ok(Box::new(()) as Box<dyn Send>) })
    }
}

#[tokio::test]
async fn a_headless_job_prepares_canonical_source_and_applies_the_shared_generation_result() {
    let root = tempfile::tempdir().unwrap();
    let mut runtime = VaultRuntime::open_with_changes(
        "vault-one".into(),
        &root.path().join("vault"),
        &root.path().join("cache"),
        &root.path().join("operations"),
        tokio::sync::mpsc::unbounded_channel().1,
    )
    .await
    .unwrap();
    let meta = serde_json::from_value(serde_json::json!({"id":"note","title":"Release","created_at":"2026-11-07T10:00:00Z","tags":[]})).unwrap();
    runtime
        .context
        .store
        .create_note_session(&meta, "Ship Friday. Preserve this source note.")
        .await
        .unwrap();
    runtime.context.store.ensure_summary("note").await.unwrap();
    let model = Arc::new(Model(Mutex::new(Vec::new())));
    let source = Arc::new(VaultSummarySource::new(
        &runtime.context,
        SummaryIdentity::LocalOwner,
        false,
    ));
    runtime
        .enable_processing(
            Arc::new(SummaryGenerator::new(source.clone(), model.clone())),
            Arc::new(Authorizer),
        )
        .await
        .unwrap();
    let baseline = runtime
        .context
        .read_document("note", &DocumentTarget::Summary)
        .await
        .unwrap();
    let mut request = work("device");
    request.request.expected_revision = baseline.revision;
    let journal = runtime.context.processing_jobs().unwrap().clone();
    let job = journal
        .submit(
            request.principal,
            uuid::Uuid::new_v4().to_string(),
            request.request,
        )
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let snapshot = journal.get(job.id.clone()).await.unwrap().unwrap();
            if snapshot.state.is_terminal() {
                assert_eq!(snapshot.state, ProcessingJobState::Succeeded);
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(
        model.0.lock().unwrap()[0]
            .prompt
            .contains("Ship Friday. Preserve this source note.")
    );
    assert_eq!(
        runtime
            .context
            .read_document("note", &DocumentTarget::Summary)
            .await
            .unwrap()
            .document
            .unwrap()
            .markdown,
        "# Release plan\n- Ship Friday."
    );
    runtime.shutdown().await.unwrap();
    assert_eq!(
        source
            .prepare(work("device"), CancellationToken::new())
            .await
            .unwrap_err(),
        ProcessingJobFailure::SourceUnavailable
    );
    drop(journal);
    let reopened = VaultRuntime::open_with_changes(
        "vault-one".into(),
        &root.path().join("vault"),
        &root.path().join("cache"),
        &root.path().join("operations"),
        tokio::sync::mpsc::unbounded_channel().1,
    )
    .await
    .unwrap();
    reopened.shutdown().await.unwrap();
}
