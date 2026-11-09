use std::{
    collections::VecDeque,
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use futures_util::stream;
use hypr_template_app::{EnhanceTagContext, Session};

use super::*;

struct Source {
    prompt: SummaryPrompt,
    calls: AtomicUsize,
}
impl SummarySource for Source {
    fn prepare(
        &self,
        _: JobWork,
        _: CancellationToken,
    ) -> JobFuture<'_, Result<SummaryPrompt, ProcessingJobFailure>> {
        Box::pin(async {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.prompt.clone())
        })
    }
}

struct Model {
    scripts: Mutex<VecDeque<Vec<Result<ModelChunk, ProcessingJobFailure>>>>,
    requests: Mutex<Vec<ModelRequest>>,
    tokens: Mutex<Vec<CancellationToken>>,
    drained: Arc<AtomicUsize>,
}
impl Model {
    fn new(scripts: Vec<Vec<Result<ModelChunk, ProcessingJobFailure>>>) -> Arc<Self> {
        Arc::new(Self {
            scripts: Mutex::new(scripts.into()),
            requests: Mutex::new(Vec::new()),
            tokens: Mutex::new(Vec::new()),
            drained: Arc::new(AtomicUsize::new(0)),
        })
    }
}
impl TextModel for Model {
    fn stream(
        &self,
        request: ModelRequest,
        cancel: CancellationToken,
    ) -> JobFuture<'_, Result<ModelStream, ProcessingJobFailure>> {
        Box::pin(async move {
            self.requests.lock().unwrap().push(request);
            self.tokens.lock().unwrap().push(cancel);
            let chunks = self.scripts.lock().unwrap().pop_front().unwrap();
            let drained = self.drained.clone();
            Ok(Box::pin(stream::iter(chunks).inspect(move |_| {
                drained.fetch_add(1, Ordering::SeqCst);
            })) as ModelStream)
        })
    }
}

fn input(note: &str) -> EnhanceUser {
    EnhanceUser {
        tag_context: EnhanceTagContext {
            available: vec!["release".into()],
            attached: vec!["planning".into()],
            dismissed: vec!["old".into()],
        },
        session: Session {
            title: Some("Release planning".into()),
            started_at: None,
            ended_at: None,
            event: None,
        },
        participants: Vec::new(),
        transcripts: Vec::new(),
        pre_meeting_memo: String::new(),
        post_meeting_memo: note.into(),
    }
}
fn prompt(custom: bool) -> SummaryPrompt {
    SummaryPrompt::render(
        EnhanceSystem {
            language: Some("nl".into()),
            prompt_override: if custom {
                "Use compact prose in {{ language }}.".into()
            } else {
                String::new()
            },
        },
        input("Ship on Friday."),
        Vec::new(),
    )
    .unwrap()
}
fn work() -> JobWork {
    JobWork {
        principal: "local-owner".into(),
        request: hypr_loofah_protocol::SummaryJobRequest {
            only_if_empty: false,
            title_policy: Default::default(),
            session_id: "note".into(),
            target: hypr_loofah_protocol::DocumentTarget::Summary,
            expected_revision: "revision".into(),
        },
    }
}
fn source(custom: bool) -> Arc<Source> {
    Arc::new(Source {
        prompt: prompt(custom),
        calls: AtomicUsize::new(0),
    })
}
fn text(body: &str) -> Result<ModelChunk, ProcessingJobFailure> {
    Ok(ModelChunk::Text(body.into()))
}
fn finished() -> Result<ModelChunk, ProcessingJobFailure> {
    Ok(ModelChunk::Finish { truncated: false })
}

#[test]
fn rendered_prompts_reuse_existing_templates_and_preserve_note_transcript_tag_and_image_context() {
    let system = EnhanceSystem {
        language: Some("nl".into()),
        prompt_override: String::new(),
    };
    let user = input("Ship on Friday.");
    let expected_system =
        hypr_template_app::render(Template::EnhanceSystem(system.clone())).unwrap();
    let expected_user =
        hypr_template_app::render(Template::EnhanceUser(Box::new(user.clone()))).unwrap();
    let prepared = SummaryPrompt::render(system.clone(), user.clone(), Vec::new()).unwrap();
    assert_eq!(prepared.system, expected_system);
    assert_eq!(
        prepared.prompt,
        format!("{expected_user}\n\n{CONCISION_NOTE}")
    );
    assert!(prepared.system.contains("Dutch"));
    assert!(prepared.prompt.contains("# Notes\n\nShip on Friday."));
    assert!(!prepared.prompt.contains("# Transcript"));
    assert!(prepared.prompt.contains("- release"));
    assert!(prepared.prompt.contains("- planning"));
    assert!(prepared.prompt.contains("- old"));
    let mut user = user;
    user.pre_meeting_memo = "Existing context".into();
    user.transcripts.push(hypr_template_app::Transcript {
        segments: vec![
            hypr_template_app::Segment {
                speaker: "Alice".into(),
                text: "I will ship it".into(),
                is_current_user: Some(true),
            },
            hypr_template_app::Segment {
                speaker: "You".into(),
                text: "Please review it".into(),
                is_current_user: Some(false),
            },
        ],
        started_at: Some(1000),
        ended_at: Some(2000),
    });
    let prepared = SummaryPrompt::render(
        system,
        user,
        vec![SummaryImage {
            bytes: vec![1],
            mime_type: "image/jpeg".into(),
        }],
    )
    .unwrap();
    assert!(
        prepared
            .prompt
            .contains("# Pre-Meeting Notes\n\nExisting context")
    );
    assert!(prepared.prompt.contains("# Meeting Notes"));
    assert!(
        prepared
            .prompt
            .contains("Alice [current user]: I will ship it")
    );
    assert!(prepared.prompt.contains("You: Please review it"));
    assert!(!prepared.prompt.contains("You [current user]"));
    assert!(
        prepared
            .prompt
            .ends_with(&format!("{IMAGE_CONTEXT_NOTE}\n\n{CONCISION_NOTE}"))
    );
}

#[test]
fn prompt_limits_and_custom_template_validation_fail_before_a_model_request() {
    let mut prepared = prompt(false);
    prepared.images = vec![
        SummaryImage {
            bytes: vec![1; 128 * 1024],
            mime_type: "image/png".into()
        };
        7
    ];
    assert_eq!(
        prepared.validate(),
        Err(ProcessingJobFailure::ResourceLimit)
    );
    prepared.images.truncate(6);
    assert!(prepared.validate().is_ok());
    prepared.images[0].bytes.push(1);
    assert_eq!(
        prepared.validate(),
        Err(ProcessingJobFailure::ResourceLimit)
    );
    prepared.images.clear();
    prepared.images.push(SummaryImage {
        bytes: vec![1],
        mime_type: "text/html".into(),
    });
    assert_eq!(
        prepared.validate(),
        Err(ProcessingJobFailure::SourceUnavailable)
    );
    let result = SummaryPrompt::render(
        EnhanceSystem {
            language: None,
            prompt_override: "{{ unsupported }}".into(),
        },
        input("Note"),
        Vec::new(),
    );
    assert_eq!(result.unwrap_err(), ProcessingJobFailure::SourceUnavailable);
}

#[test]
fn footer_splitting_and_unicode_are_independent_of_network_chunk_boundaries() {
    let raw = "# Summary\n\n  • Ship café on Friday.\n\t• Review it.\n<loofah-tags>{\"tags\":[{\"name\":\"réunion\",\"confidence\":0.93}]}</loofah-tags>\n";
    for split in raw
        .char_indices()
        .map(|(index, _)| index)
        .chain(std::iter::once(raw.len()))
    {
        let mut output = Output::default();
        output.push(&raw[..split]).unwrap();
        output.push(&raw[split..]).unwrap();
        output.finished = true;
        let result = output.finish().unwrap();
        assert_eq!(
            result.markdown,
            "# Summary\n\n  - Ship café on Friday.\n\t- Review it."
        );
        assert_eq!(
            result.suggested_tags.unwrap(),
            vec![ScoredTagSuggestion {
                name: "réunion".into(),
                confidence: 0.93
            }]
        );
    }
    let mut output = Output::default();
    for character in raw.chars() {
        output.push(&character.to_string()).unwrap();
    }
    output.finished = true;
    assert_eq!(output.finish().unwrap().suggested_tags.unwrap().len(), 1);
}

#[test]
fn malformed_oversized_truncated_and_missing_footers_never_leak_into_the_document() {
    let suffixes = [
        "",
        "<loofah",
        "<loofah-tags",
        "<loofah-tags>{bad}</loofah-tags>",
        "<loofah-tags>{\"tags\":[{\"name\":\"x\",\"confidence\":2}]}</loofah-tags>",
        "<loofah-tags>{\"tags\":[]}</loofah-tags>extra",
    ];
    for suffix in suffixes {
        let mut output = Output::default();
        output.push(&format!("# Body\n{suffix}")).unwrap();
        output.finished = true;
        let result = output.finish().unwrap();
        assert_eq!(result.markdown, "# Body");
        assert!(result.suggested_tags.is_none());
    }
    let mut output = Output::default();
    output.push("# Body\n<loofah-tags>").unwrap();
    output.push(&"x".repeat(MAX_FOOTER_UNITS)).unwrap();
    assert!(output.oversized);
    output.push("</loofah-tags>").unwrap();
    output.finished = true;
    assert!(output.finish().unwrap().suggested_tags.is_none());
    let mut output = Output::default();
    output
        .push("# Body\n<loofah-tags>{\"tags\":[]}</loofah-tags>")
        .unwrap();
    output.truncated = true;
    output.finished = true;
    assert!(output.finish().unwrap().suggested_tags.is_none());
    let mut output = Output::default();
    output.push("# Literal <").unwrap();
    output.finished = true;
    assert_eq!(output.finish().unwrap().markdown, "# Literal <");
}

#[tokio::test]
async fn invalid_prefix_retries_once_after_draining_and_resets_body_and_tags() {
    let source = source(false);
    let model = Model::new(vec![
        vec![
            text("Invalid content without a heading"),
            text(
                " late text<loofah-tags>{\"tags\":[{\"name\":\"stale\",\"confidence\":1}]}</loofah-tags>",
            ),
            finished(),
        ],
        vec![
            text("# Valid heading\n"),
            text("• Ship Friday.\n<loofah-tags>{\"tags\":[]}</loofah-tags>"),
            finished(),
        ],
    ]);
    let result = SummaryGenerator::new(source.clone(), model.clone())
        .generate(work(), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(result.markdown, "# Valid heading\n- Ship Friday.");
    assert_eq!(result.suggested_tags, Some(Vec::new()));
    assert_eq!(source.calls.load(Ordering::SeqCst), 1);
    assert_eq!(model.drained.load(Ordering::SeqCst), 6);
    let requests = model.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].max_output_tokens, 8192);
    assert_eq!(requests[0].max_retries, 4);
    assert_eq!(
        requests[1].prompt,
        format!(
            "{}\n\nIMPORTANT: Previous attempt failed. {VALIDATION_FEEDBACK}",
            requests[0].prompt
        )
    );
    assert!(model.tokens.lock().unwrap()[0].is_cancelled());
}

#[tokio::test]
async fn custom_short_and_second_attempt_outputs_preserve_existing_validation_policy() {
    for (custom, scripts, expected_calls) in [
        (
            true,
            vec![vec![
                text("Compact plain prose without a heading."),
                finished(),
            ]],
            1,
        ),
        (false, vec![vec![text("Short"), finished()]], 1),
        (
            false,
            vec![
                vec![text("Invalid prefix once"), finished()],
                vec![text("Invalid prefix twice"), finished()],
            ],
            2,
        ),
    ] {
        let model = Model::new(scripts);
        let output = SummaryGenerator::new(source(custom), model.clone())
            .generate(work(), CancellationToken::new())
            .await
            .unwrap();
        assert!(!output.markdown.is_empty());
        assert_eq!(model.requests.lock().unwrap().len(), expected_calls);
    }
}

#[tokio::test]
async fn fence_only_generation_fails_without_an_additional_model_attempt() {
    for scripts in [
        vec![vec![text("```html\n"), finished()]],
        vec![
            vec![text("Invalid prefix once"), finished()],
            vec![text("```html\n"), finished()],
        ],
    ] {
        let attempts = scripts.len();
        let model = Model::new(scripts);
        let result = SummaryGenerator::new(source(false), model.clone())
            .generate(work(), CancellationToken::new())
            .await;
        assert_eq!(result.unwrap_err(), ProcessingJobFailure::InvalidOutput);
        assert_eq!(model.requests.lock().unwrap().len(), attempts);
        assert_eq!(model.drained.load(Ordering::SeqCst), attempts * 2);
    }
}

#[test]
fn final_generation_rejects_empty_fences_but_preserves_short_prose_and_code_content() {
    for body in [
        "```html",
        "```",
        "~~~text\n\n~~~",
        "  ```html\n  ```",
        "```html\n```\n~~~text\n~~~",
        "```html\n<loofah-tags>{\"tags\":[]}</loofah-tags>",
    ] {
        let mut output = Output::default();
        output.push(body).unwrap();
        output.finished = true;
        assert_eq!(
            output.finish().unwrap_err(),
            ProcessingJobFailure::InvalidOutput,
            "{body:?}"
        );
    }
    for body in [
        "Short",
        "é",
        "# Heading",
        "```html\n<p>Ship Friday.</p>\n```",
        "~~~text\nShip Friday.\n~~~",
        "```text\n é \n```",
        "Before\n```html\n```",
        "```html\n```\nAfter",
        "`` literal ticks",
    ] {
        let mut output = Output::default();
        output.push(body).unwrap();
        output.finished = true;
        assert_eq!(output.finish().unwrap().markdown, body.trim(), "{body:?}");
    }
}

#[test]
fn final_generation_preserves_literal_fences_in_first_indented_code_line() {
    for (body, expected) in [
        ("    ```html", "    ```html"),
        (" \n    ```html\n  \n", "    ```html"),
        ("\n\t~~~\n", "\t~~~"),
        ("  \t```html", "  \t```html"),
        ("\n    code\n    ```\n", "    code\n    ```"),
        ("  ordinary prose \n", "ordinary prose"),
    ] {
        let mut output = Output::default();
        output.push(body).unwrap();
        output.finished = true;
        assert_eq!(output.finish().unwrap().markdown, expected, "{body:?}");
    }
}

#[tokio::test]
async fn provider_failure_unconfirmed_eof_and_invalid_stream_order_do_not_return_partial_success() {
    for script in [
        vec![
            text("# Valid heading\nPartial body"),
            Err(ProcessingJobFailure::ProviderFailed),
        ],
        vec![text("# Valid heading\nMissing confirmation")],
        vec![text("# Valid heading"), finished(), text("Trailing data")],
        vec![text("# Valid heading"), finished(), finished()],
    ] {
        let model = Model::new(vec![script]);
        let failure = SummaryGenerator::new(source(false), model.clone())
            .generate(work(), CancellationToken::new())
            .await
            .unwrap_err();
        assert_eq!(failure, ProcessingJobFailure::ProviderFailed);
        assert_eq!(model.requests.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn reasoning_blocks_are_removed_before_validation_and_footer_parsing_at_every_boundary() {
    let raw = "<think>private reasoning <loofah-tags>bad</loofah-tags></think><thinking>分析</thinking># Valid heading\nBefore<thinking>hidden</thinking>after<think>hidden too</think>end\n<loofah-tags>{\"tags\":[]}</loofah-tags>";
    let source = source(false);
    for split in raw
        .char_indices()
        .map(|(index, _)| index)
        .chain(std::iter::once(raw.len()))
    {
        let model = Model::new(vec![vec![
            text(&raw[..split]),
            text(&raw[split..]),
            finished(),
        ]]);
        let output = SummaryGenerator::new(source.clone(), model.clone())
            .generate(work(), CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(output.markdown, "# Valid heading\nBefore\nafter\nend");
        assert_eq!(output.suggested_tags, Some(Vec::new()));
        assert_eq!(model.requests.lock().unwrap().len(), 1);
    }
    let mut script: Vec<_> = raw
        .chars()
        .map(|character| text(&character.to_string()))
        .collect();
    script.push(finished());
    let model = Model::new(vec![script]);
    let output = SummaryGenerator::new(source, model)
        .generate(work(), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(output.markdown, "# Valid heading\nBefore\nafter\nend");
}

struct HeldModel {
    started: Arc<AtomicBool>,
    cancelled: Arc<AtomicBool>,
    released: Arc<tokio::sync::Semaphore>,
    drained: Arc<AtomicBool>,
}
impl TextModel for HeldModel {
    fn stream(
        &self,
        _: ModelRequest,
        cancel: CancellationToken,
    ) -> JobFuture<'_, Result<ModelStream, ProcessingJobFailure>> {
        let cancelled = self.cancelled.clone();
        let started = self.started.clone();
        let released = self.released.clone();
        let drained = self.drained.clone();
        Box::pin(async move {
            started.store(true, Ordering::SeqCst);
            let stream = stream::once(async { text("# Valid heading\nWaiting") }).chain(
                stream::once(async move {
                    cancel.cancelled().await;
                    cancelled.store(true, Ordering::SeqCst);
                    released.acquire().await.unwrap().forget();
                    drained.store(true, Ordering::SeqCst);
                    finished()
                }),
            );
            Ok(Box::pin(stream) as ModelStream)
        })
    }
}

#[tokio::test]
async fn cancellation_waits_for_actual_model_stream_completion() {
    let model = Arc::new(HeldModel {
        started: Arc::new(AtomicBool::new(false)),
        cancelled: Arc::new(AtomicBool::new(false)),
        released: Arc::new(tokio::sync::Semaphore::new(0)),
        drained: Arc::new(AtomicBool::new(false)),
    });
    let generator = Arc::new(SummaryGenerator::new(source(false), model.clone()));
    let cancel = CancellationToken::new();
    let stopping = cancel.clone();
    let task = tokio::spawn(async move { generator.generate(work(), stopping).await });
    cancel.cancel();
    // Cancellation before generation is admitted must not start a provider request.
    assert_eq!(
        task.await.unwrap().unwrap_err(),
        ProcessingJobFailure::Interrupted
    );
    assert!(!model.cancelled.load(Ordering::SeqCst));
    let generator = Arc::new(SummaryGenerator::new(source(false), model.clone()));
    let cancel = CancellationToken::new();
    let stopping = cancel.clone();
    let task = tokio::spawn(async move { generator.generate(work(), stopping).await });
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while !model.started.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    cancel.cancel();
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while !model.cancelled.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(!task.is_finished());
    assert!(!model.drained.load(Ordering::SeqCst));
    model.released.add_permits(1);
    let retained = task.await.unwrap().unwrap();
    assert_eq!(retained.markdown, "# Valid heading\nWaiting");
    assert!(model.drained.load(Ordering::SeqCst));
}

#[test]
fn legacy_results_remain_readable_and_generated_metadata_is_validated() {
    let output: GeneratedDocument =
        serde_json::from_str("{\"markdown\":\"# Older output\"}").unwrap();
    assert!(output.suggested_tags.is_none());
    for confidence in [f64::NAN, f64::INFINITY, -0.1, 1.1] {
        let output = GeneratedDocument {
            markdown: "# Body".into(),
            suggested_tags: Some(vec![ScoredTagSuggestion {
                name: "topic".into(),
                confidence,
            }]),
        };
        assert!(super::super::result::validate_output(&output).is_err());
    }
}

#[test]
fn output_and_discarded_metadata_cannot_exceed_the_stream_budget() {
    let mut output = Output::default();
    assert!(output.push(&"x".repeat(MAX_OUTPUT_BYTES + 1)).is_err());
    let mut output = Output::default();
    output.push("# Summary\n<loofah-tags>").unwrap();
    let block = "x".repeat(128 * 1024);
    let mut rejected = false;
    for _ in 0..34 {
        if output.push(&block).is_err() {
            rejected = true;
            break;
        }
    }
    assert!(rejected);
    assert!(output.footer.unwrap().is_empty());
}

struct Authorizer;
impl super::super::JobAuthorizer for Authorizer {
    fn authorize<'a>(
        &'a self,
        _: &'a JobWork,
    ) -> JobFuture<'a, Result<Box<dyn Send>, ProcessingJobFailure>> {
        Box::pin(async { Ok(Box::new(()) as Box<dyn Send>) })
    }
}

#[tokio::test]
async fn fence_only_job_preserves_canonical_summary_and_retains_no_result() {
    use crate::VaultRuntime;
    use hypr_loofah_protocol::{
        DocumentTarget, ProcessingJobState, SaveDocumentRequest, SummaryJobRequest,
    };
    let root = tempfile::tempdir().unwrap();
    let vault = root.path().join("vault");
    let mut runtime = VaultRuntime::open_with_changes(
        "vault-one".into(),
        &vault,
        &root.path().join("cache"),
        &root.path().join("operations"),
        tokio::sync::mpsc::unbounded_channel().1,
    )
    .await
    .unwrap();
    let meta = serde_json::from_value(serde_json::json!({"id":"note","title":"Original title","created_at":"2026-11-07T10:00:00Z","tags":[]})).unwrap();
    runtime
        .context
        .store
        .create_note_session(&meta, "Ship Friday.")
        .await
        .unwrap();
    runtime.context.store.ensure_summary("note").await.unwrap();
    let empty = runtime
        .context
        .read_document("note", &DocumentTarget::Summary)
        .await
        .unwrap();
    runtime
        .context
        .save_document(
            "note",
            &DocumentTarget::Summary,
            SaveDocumentRequest {
                markdown: "# Existing summary\nPreserve the original.".into(),
                expected_revision: empty.revision,
            },
        )
        .await
        .unwrap();
    let baseline = runtime
        .context
        .read_document("note", &DocumentTarget::Summary)
        .await
        .unwrap();
    let original_bytes = std::fs::read(vault.join("sessions/note/summary.md")).unwrap();
    let model = Model::new(vec![vec![text("```html\n"), finished()]]);
    runtime
        .enable_processing(
            Arc::new(SummaryGenerator::new(source(false), model.clone())),
            Arc::new(Authorizer),
        )
        .await
        .unwrap();
    let journal = runtime.context.processing_jobs().unwrap().clone();
    let job = journal
        .submit(
            "local-owner".into(),
            uuid::Uuid::new_v4().to_string(),
            SummaryJobRequest {
                only_if_empty: false,
                title_policy: Default::default(),
                session_id: "note".into(),
                target: DocumentTarget::Summary,
                expected_revision: baseline.revision.clone(),
            },
        )
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let snapshot = journal.get(job.id.clone()).await.unwrap().unwrap();
            if snapshot.state.is_terminal() {
                assert_eq!(snapshot.state, ProcessingJobState::Failed);
                assert_eq!(snapshot.failure, Some(ProcessingJobFailure::InvalidOutput));
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(journal.result(job.id).await.unwrap().is_none());
    assert_eq!(
        runtime
            .context
            .read_document("note", &DocumentTarget::Summary)
            .await
            .unwrap(),
        baseline
    );
    assert_eq!(
        std::fs::read(vault.join("sessions/note/summary.md")).unwrap(),
        original_bytes
    );
    assert_eq!(model.requests.lock().unwrap().len(), 1);
    assert_eq!(model.drained.load(Ordering::SeqCst), 2);
    runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn shared_generation_runs_without_a_frontend_and_retains_tag_results_across_runtime_restart()
{
    use crate::VaultRuntime;
    use hypr_loofah_protocol::{DocumentTarget, ProcessingJobState, SummaryJobRequest};
    let root = tempfile::tempdir().unwrap();
    let vault = root.path().join("vault");
    let cache = root.path().join("cache");
    let operations = root.path().join("operations");
    let mut runtime = VaultRuntime::open_with_changes(
        "vault-one".into(),
        &vault,
        &cache,
        &operations,
        tokio::sync::mpsc::unbounded_channel().1,
    )
    .await
    .unwrap();
    let meta = serde_json::from_value(serde_json::json!({"id":"note","title":"Original title","created_at":"2026-11-07T10:00:00Z","tags":[]})).unwrap();
    runtime
        .context
        .store
        .create_note_session(&meta, "Ship Friday.")
        .await
        .unwrap();
    runtime.context.store.ensure_summary("note").await.unwrap();
    let unknown = vault.join("sessions/note/user-owned.txt");
    std::fs::write(&unknown, b"Preserve user content").unwrap();
    let original = runtime.context.read_note("note").await.unwrap();
    let prompt = SummaryPrompt::render(
        EnhanceSystem {
            language: Some("nl".into()),
            prompt_override: String::new(),
        },
        input(original.markdown.as_deref().unwrap()),
        Vec::new(),
    )
    .unwrap();
    let source = Arc::new(Source {
        prompt,
        calls: AtomicUsize::new(0),
    });
    let model = Model::new(vec![vec![
        text(
            "# Release plan\n• Ship Friday.\n<loofah-tags>{\"tags\":[{\"name\":\"release\",\"confidence\":0.93}]}</loofah-tags>",
        ),
        finished(),
    ]]);
    let generator = Arc::new(SummaryGenerator::new(source, model.clone()));
    runtime
        .enable_processing(generator.clone(), Arc::new(Authorizer))
        .await
        .unwrap();
    let journal = runtime.context.processing_jobs().unwrap().clone();
    let baseline = runtime
        .context
        .read_document("note", &DocumentTarget::Summary)
        .await
        .unwrap();
    let job = journal
        .submit(
            "local-owner".into(),
            uuid::Uuid::new_v4().to_string(),
            SummaryJobRequest {
                only_if_empty: false,
                title_policy: Default::default(),
                session_id: "note".into(),
                target: DocumentTarget::Summary,
                expected_revision: baseline.revision,
            },
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
    assert_eq!(runtime.context.read_note("note").await.unwrap(), original);
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
    let generated = journal.result(job.id.clone()).await.unwrap().unwrap();
    assert_eq!(
        generated.suggested_tags.as_ref().unwrap()[0].name,
        "release"
    );
    assert_eq!(std::fs::read(&unknown).unwrap(), b"Preserve user content");
    runtime.shutdown().await.unwrap();
    drop(journal);
    let mut reopened = VaultRuntime::open_with_changes(
        "vault-one".into(),
        &vault,
        &cache,
        &operations,
        tokio::sync::mpsc::unbounded_channel().1,
    )
    .await
    .unwrap();
    reopened
        .enable_processing(generator, Arc::new(Authorizer))
        .await
        .unwrap();
    assert_eq!(
        reopened
            .context
            .processing_jobs()
            .unwrap()
            .result(job.id)
            .await
            .unwrap(),
        Some(generated)
    );
    assert_eq!(model.requests.lock().unwrap().len(), 1);
    reopened.shutdown().await.unwrap();
}

#[tokio::test]
async fn cancellation_retains_confirmed_late_generation_as_a_private_draft_without_applying_it() {
    use crate::VaultRuntime;
    use hypr_loofah_protocol::{DocumentTarget, ProcessingJobState, SummaryJobRequest};
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
    let meta = serde_json::from_value(serde_json::json!({"id":"note","title":"Original title","created_at":"2026-11-07T10:00:00Z","tags":[]})).unwrap();
    runtime
        .context
        .store
        .create_note_session(&meta, "Ship Friday.")
        .await
        .unwrap();
    runtime.context.store.ensure_summary("note").await.unwrap();
    let baseline = runtime
        .context
        .read_document("note", &DocumentTarget::Summary)
        .await
        .unwrap();
    let model = Arc::new(HeldModel {
        started: Arc::new(AtomicBool::new(false)),
        cancelled: Arc::new(AtomicBool::new(false)),
        released: Arc::new(tokio::sync::Semaphore::new(0)),
        drained: Arc::new(AtomicBool::new(false)),
    });
    runtime
        .enable_processing(
            Arc::new(SummaryGenerator::new(source(false), model.clone())),
            Arc::new(Authorizer),
        )
        .await
        .unwrap();
    let journal = runtime.context.processing_jobs().unwrap().clone();
    let job = journal
        .submit(
            "local-owner".into(),
            uuid::Uuid::new_v4().to_string(),
            SummaryJobRequest {
                only_if_empty: false,
                title_policy: Default::default(),
                session_id: "note".into(),
                target: DocumentTarget::Summary,
                expected_revision: baseline.revision.clone(),
            },
        )
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while !model.started.load(Ordering::SeqCst) {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        journal.cancel(job.id.clone()).await.unwrap().state,
        ProcessingJobState::Cancelling
    );
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while !model.cancelled.load(Ordering::SeqCst) {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(journal.result(job.id.clone()).await.unwrap().is_none());
    model.released.add_permits(1);
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let state = journal.get(job.id.clone()).await.unwrap().unwrap().state;
            if state.is_terminal() {
                assert_eq!(state, ProcessingJobState::Cancelled);
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        journal.result(job.id).await.unwrap().unwrap().markdown,
        "# Valid heading\nWaiting"
    );
    assert_eq!(
        runtime
            .context
            .read_document("note", &DocumentTarget::Summary)
            .await
            .unwrap(),
        baseline
    );
    assert!(model.drained.load(Ordering::SeqCst));
    runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn preview_retains_only_filtered_partial_body_after_provider_failure_and_drain() {
    let model = Model::new(vec![vec![
        text("<thi"),
        text("nking>PRIVATE-REASONING-CANARY</thinking># Public heading\n"),
        text("Decision.\n<loofah-"),
        text("tags>{\"tags\":[{\"name\":\"PRIVATE-TAG-CANARY\",\"confidence\":1}]}</loofah-tags>"),
        Err(ProcessingJobFailure::ProviderFailed),
    ]]);
    let (preview, _) = GenerationPreview::channel();
    let error = SummaryGenerator::new(source(false), model.clone())
        .generate_observed(work(), CancellationToken::new(), Some(preview.clone()))
        .await
        .unwrap_err();
    assert_eq!(error, ProcessingJobFailure::ProviderFailed);
    assert_eq!(model.drained.load(Ordering::SeqCst), 5);
    assert_eq!(model.requests.lock().unwrap().len(), 1);
    let partial = preview.current().unwrap();
    assert_eq!(partial.markdown, "# Public heading\nDecision.\n");
    assert!(!partial.markdown.contains("CANARY"));
    assert!(!partial.markdown.contains("loofah-tags"));
}

#[tokio::test]
async fn preview_retry_resets_attempt_without_adding_model_requests() {
    let model = Model::new(vec![
        vec![text("Invalid original content"), finished()],
        vec![text("# New attempt\nResult"), finished()],
    ]);
    let (preview, _) = GenerationPreview::channel();
    preview.reset();
    let before = preview.current().unwrap().cursor;
    let result = SummaryGenerator::new(source(false), model.clone())
        .generate_observed(work(), CancellationToken::new(), Some(preview.clone()))
        .await
        .unwrap();
    assert_eq!(model.requests.lock().unwrap().len(), 2);
    assert_eq!(model.drained.load(Ordering::SeqCst), 4);
    let after = preview.current().unwrap();
    assert_eq!(after.markdown, result.markdown);
    assert_ne!(
        after.cursor.split_once(':').unwrap().0,
        before.split_once(':').unwrap().0
    );
    assert!(!after.markdown.contains("Invalid original"));
}
