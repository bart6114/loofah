use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
};

use super::*;
use crate::jobs::generation::ModelStream;

struct Model {
    requests: Mutex<Vec<ModelRequest>>,
    chunks: Mutex<Vec<Result<ModelChunk, ProcessingJobFailure>>>,
}

impl TextModel for Model {
    fn stream(
        &self,
        request: ModelRequest,
        _: CancellationToken,
    ) -> JobFuture<'_, Result<ModelStream, ProcessingJobFailure>> {
        self.requests.lock().unwrap().push(request);
        let chunks = std::mem::take(&mut *self.chunks.lock().unwrap());
        Box::pin(async move { Ok(Box::pin(futures_util::stream::iter(chunks)) as ModelStream) })
    }
}

fn model(chunks: Vec<Result<ModelChunk, ProcessingJobFailure>>) -> Arc<Model> {
    Arc::new(Model {
        requests: Mutex::new(vec![]),
        chunks: Mutex::new(chunks),
    })
}

fn text(value: &str) -> Result<ModelChunk, ProcessingJobFailure> {
    Ok(ModelChunk::Text(value.into()))
}

fn finish() -> Result<ModelChunk, ProcessingJobFailure> {
    Ok(ModelChunk::Finish { truncated: false })
}

#[tokio::test]
async fn title_generation_reuses_the_exact_templates_language_model_and_budgets() {
    let model = model(vec![text("  Release planning  \n"), finish()]);
    let generator = TitleGenerator::new(model.clone(), Some("nl".into()), Some(0.0));
    assert_eq!(
        generator
            .generate("# Decisions\nShip Friday".into(), CancellationToken::new())
            .await
            .unwrap(),
        Some("Release planning".into())
    );
    let requests = model.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(
        request.system,
        hypr_template_app::render(Template::TitleSystem(TitleSystem {
            language: Some("nl".into())
        }))
        .unwrap()
    );
    assert_eq!(
        request.prompt,
        hypr_template_app::render(Template::TitleUser(TitleUser {
            enhanced_note: "# Decisions\nShip Friday".into()
        }))
        .unwrap()
    );
    assert!(request.system.contains("Dutch"));
    assert_eq!(request.max_output_tokens, 128);
    assert_eq!(request.max_retries, 4);
    assert_eq!(request.temperature, Some(0.0));
    assert!(request.images.is_empty());
}

#[tokio::test]
async fn title_generation_filters_split_reasoning_trims_and_preserves_existing_plaintext_acceptance()
 {
    for chunks in [
        vec![text("  \n"), finish()],
        vec![text("  <EMPTY> \n"), finish()],
        vec![
            text("<think>hidden</think><thinking>hidden</thinking>"),
            finish(),
        ],
    ] {
        assert_eq!(
            TitleGenerator::new(model(chunks), None, None)
                .generate("".into(), CancellationToken::new())
                .await
                .unwrap(),
            None
        );
    }
    let input = "<thinking>private reasoning</thinking><think>more reasoning</think>Roadmap: Q4";
    for split in 0..=input.len() {
        let chunks = vec![text(&input[..split]), text(&input[split..]), finish()];
        assert_eq!(
            TitleGenerator::new(model(chunks), None, None)
                .generate("Meeting".into(), CancellationToken::new())
                .await
                .unwrap(),
            Some("Roadmap: Q4".into()),
            "split {split}"
        );
    }
    let model = model(vec![
        text("# A quoted \"title\""),
        Ok(ModelChunk::Finish { truncated: true }),
    ]);
    assert_eq!(
        TitleGenerator::new(model.clone(), None, None)
            .generate("Meeting".into(), CancellationToken::new())
            .await
            .unwrap(),
        Some("# A quoted \"title\"".into())
    );
    assert_eq!(model.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn title_generation_rejects_malformed_or_unbounded_results_without_validation_retries() {
    for (chunks, expected) in [
        (vec![text("Title")], ProcessingJobFailure::ProviderFailed),
        (
            vec![finish(), finish()],
            ProcessingJobFailure::ProviderFailed,
        ),
        (
            vec![finish(), text("late")],
            ProcessingJobFailure::ProviderFailed,
        ),
        (
            vec![text("Title\nInjected"), finish()],
            ProcessingJobFailure::InvalidOutput,
        ),
        (
            vec![text(&"x".repeat(MAX_TITLE_BYTES + 1)), finish()],
            ProcessingJobFailure::InvalidOutput,
        ),
        (
            vec![
                text(&format!(
                    "<think>{}</think>Title",
                    "x".repeat(MAX_OUTPUT_BYTES)
                )),
                finish(),
            ],
            ProcessingJobFailure::InvalidOutput,
        ),
        (
            vec![Err(ProcessingJobFailure::ProviderUnavailable)],
            ProcessingJobFailure::ProviderUnavailable,
        ),
    ] {
        let model = model(chunks);
        assert_eq!(
            TitleGenerator::new(model.clone(), None, None)
                .generate("Meeting".into(), CancellationToken::new())
                .await
                .unwrap_err(),
            expected
        );
        assert_eq!(model.requests.lock().unwrap().len(), 1);
    }
    let title = "é".repeat(MAX_TITLE_BYTES / 2);
    assert_eq!(
        TitleGenerator::new(model(vec![text(&title), finish()]), None, None)
            .generate("Meeting".into(), CancellationToken::new())
            .await
            .unwrap(),
        Some(title)
    );
}

#[tokio::test]
async fn title_generation_checks_source_settings_and_early_cancellation_before_model_dispatch() {
    for (source, language, temperature, expected) in [
        (
            "x".repeat(MAX_SOURCE_BYTES + 1),
            None,
            None,
            ProcessingJobFailure::ResourceLimit,
        ),
        (
            "x".repeat(MAX_SOURCE_BYTES),
            None,
            None,
            ProcessingJobFailure::ResourceLimit,
        ),
        (
            "Meeting".into(),
            Some("x".repeat(129)),
            None,
            ProcessingJobFailure::ResourceLimit,
        ),
        (
            "Meeting".into(),
            Some("nl\nInjected".into()),
            None,
            ProcessingJobFailure::SourceUnavailable,
        ),
        (
            "Meeting".into(),
            None,
            Some(f64::NAN),
            ProcessingJobFailure::SourceUnavailable,
        ),
        (
            "Meeting".into(),
            None,
            Some(-0.1),
            ProcessingJobFailure::SourceUnavailable,
        ),
        (
            "Meeting".into(),
            None,
            Some(2.1),
            ProcessingJobFailure::SourceUnavailable,
        ),
    ] {
        let model = model(vec![finish()]);
        assert_eq!(
            TitleGenerator::new(model.clone(), language, temperature)
                .generate(source, CancellationToken::new())
                .await
                .unwrap_err(),
            expected
        );
        assert!(model.requests.lock().unwrap().is_empty());
    }
    let model = model(vec![finish()]);
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert_eq!(
        TitleGenerator::new(model.clone(), None, None)
            .generate("Meeting".into(), cancel)
            .await
            .unwrap_err(),
        ProcessingJobFailure::Interrupted
    );
    assert!(model.requests.lock().unwrap().is_empty());
}

#[test]
fn title_temperature_matches_the_existing_frontend_policy_without_generic_model_guesses() {
    for (provider, id) in [
        ("chatgpt_subscription", "any"),
        ("anthropic", "claude-opus-4-8"),
        ("anthropic.messages", "claude-sonnet-4.8-20261001"),
        ("anthropic", "CLAUDE-HAIKU-4-8"),
        ("openrouter", "anthropic/claude-opus-4-8"),
        ("openrouter", "anthropic/vendor/claude-sonnet-4.8"),
    ] {
        assert_eq!(deterministic_title_temperature(provider, id), None);
    }
    for (provider, id) in [
        ("anthropic", "claude-opus-4-5"),
        ("anthropic", "claude-opus-4-80"),
        ("custom", "claude-opus-4-8"),
        ("openai", "gpt-4o"),
        ("openai", "o3"),
        ("openrouter", "openai/gpt-5"),
    ] {
        assert_eq!(deterministic_title_temperature(provider, id), Some(0.0));
    }
}

#[tokio::test]
async fn title_fragmented_whitespace_uses_the_normalized_limit_with_a_bounded_raw_stream() {
    let mut chunks = vec![];
    for _ in 0..1024 {
        chunks.push(text("                                "));
    }
    chunks.push(text("Title"));
    for _ in 0..1023 {
        chunks.push(text("                                "));
    }
    chunks.push(finish());
    assert_eq!(
        TitleGenerator::new(model(chunks), None, None)
            .generate("Meeting".into(), CancellationToken::new())
            .await
            .unwrap(),
        Some("Title".into())
    );
    let chunks = vec![
        text(&format!("Title{}Suffix", " ".repeat(MAX_TITLE_BYTES))),
        finish(),
    ];
    assert_eq!(
        TitleGenerator::new(model(chunks), None, None)
            .generate("Meeting".into(), CancellationToken::new())
            .await
            .unwrap_err(),
        ProcessingJobFailure::InvalidOutput
    );
}

#[tokio::test]
async fn title_trim_matches_javascript_bom_and_whitespace_without_trimming_nel() {
    for whitespace in [
        '\u{0009}', '\u{000a}', '\u{000b}', '\u{000c}', '\u{000d}', '\u{0020}', '\u{00a0}',
        '\u{1680}', '\u{2000}', '\u{2001}', '\u{2002}', '\u{2003}', '\u{2004}', '\u{2005}',
        '\u{2006}', '\u{2007}', '\u{2008}', '\u{2009}', '\u{200a}', '\u{2028}', '\u{2029}',
        '\u{202f}', '\u{205f}', '\u{3000}', '\u{feff}',
    ] {
        assert_eq!(
            trim_candidate(&format!("{whitespace}<EMPTY>{whitespace}")),
            "<EMPTY>"
        );
    }
    assert_eq!(
        trim_candidate("\u{0085}<EMPTY>\u{0085}"),
        "\u{0085}<EMPTY>\u{0085}"
    );
    assert_eq!(
        trim_candidate("\u{180e}Title\u{200b}"),
        "\u{180e}Title\u{200b}"
    );
    for value in ["\u{feff}<EMPTY>\u{feff}", "\u{00a0}\u{feff} \u{3000}"] {
        assert_eq!(
            TitleGenerator::new(model(vec![text(value), finish()]), None, None)
                .generate("Meeting".into(), CancellationToken::new())
                .await
                .unwrap(),
            None
        );
    }
    for value in ["\u{0085}<EMPTY>\u{0085}", "\u{0085}Title"] {
        assert_eq!(
            TitleGenerator::new(model(vec![text(value), finish()]), None, None)
                .generate("Meeting".into(), CancellationToken::new())
                .await
                .unwrap_err(),
            ProcessingJobFailure::InvalidOutput
        );
    }
    let title = "é".repeat(MAX_TITLE_BYTES / 2);
    assert_eq!(
        TitleGenerator::new(
            model(vec![text(&format!("\u{feff} {title} \u{feff}")), finish()]),
            None,
            None
        )
        .generate("Meeting".into(), CancellationToken::new())
        .await
        .unwrap(),
        Some(title)
    );
}

struct Drain {
    started: tokio::sync::Semaphore,
    cancelled: tokio::sync::Semaphore,
    release: tokio::sync::Semaphore,
    finished: AtomicBool,
}

impl Default for Drain {
    fn default() -> Self {
        Self {
            started: tokio::sync::Semaphore::new(0),
            cancelled: tokio::sync::Semaphore::new(0),
            release: tokio::sync::Semaphore::new(0),
            finished: AtomicBool::new(false),
        }
    }
}

struct HeldModel {
    drain: Arc<Drain>,
    startup: bool,
    invalid: bool,
    _admission: tokio::sync::OwnedSemaphorePermit,
}

impl TextModel for HeldModel {
    fn stream(
        &self,
        _: ModelRequest,
        cancel: CancellationToken,
    ) -> JobFuture<'_, Result<ModelStream, ProcessingJobFailure>> {
        let drain = self.drain.clone();
        let startup = self.startup;
        let invalid = self.invalid;
        Box::pin(async move {
            if startup {
                drain.started.add_permits(1);
                cancel.cancelled().await;
                drain.cancelled.add_permits(1);
                drain.release.acquire().await.unwrap().forget();
                drain.finished.store(true, Ordering::Release);
                return Err(ProcessingJobFailure::ProviderFailed);
            }
            Ok(Box::pin(async_stream::stream! {
                yield Ok(ModelChunk::Text(if invalid {
                    "x".repeat(MAX_TITLE_BYTES + 1)
                } else {
                    "Candidate".into()
                }));
                yield finish();
                drain.started.add_permits(1);
                cancel.cancelled().await;
                drain.cancelled.add_permits(1);
                drain.release.acquire().await.unwrap().forget();
                drain.finished.store(true, Ordering::Release);
            }) as ModelStream)
        })
    }
}

async fn signal(semaphore: &tokio::sync::Semaphore) {
    tokio::time::timeout(std::time::Duration::from_secs(10), semaphore.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
}

#[tokio::test]
async fn title_cancellation_and_dropped_callers_hold_the_same_model_admission_until_actual_drain() {
    for startup in [false, true] {
        for drop_caller in [false, true] {
            let admission = Arc::new(tokio::sync::Semaphore::new(1));
            let drain = Arc::new(Drain::default());
            let model = Arc::new(HeldModel {
                drain: drain.clone(),
                startup,
                invalid: false,
                _admission: admission.clone().acquire_owned().await.unwrap(),
            });
            let weak = Arc::downgrade(&model);
            let generator = TitleGenerator::new(model, None, None);
            let cancel = CancellationToken::new();
            let stopping = cancel.clone();
            let caller =
                tokio::spawn(async move { generator.generate("Meeting".into(), stopping).await });
            signal(&drain.started).await;
            if drop_caller {
                caller.abort();
            } else {
                cancel.cancel();
            }
            signal(&drain.cancelled).await;
            assert_eq!(admission.available_permits(), 0);
            assert!(weak.upgrade().is_some());
            assert!(!drain.finished.load(Ordering::Acquire));
            if !drop_caller {
                assert!(!caller.is_finished());
            }
            drain.release.add_permits(1);
            if drop_caller {
                assert!(caller.await.unwrap_err().is_cancelled());
            } else {
                assert_eq!(
                    caller.await.unwrap().unwrap_err(),
                    ProcessingJobFailure::Interrupted
                );
            }
            tokio::time::timeout(std::time::Duration::from_secs(10), async {
                while !drain.finished.load(Ordering::Acquire) || weak.upgrade().is_some() {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            assert_eq!(admission.available_permits(), 1);
        }
    }
}

#[tokio::test]
async fn invalid_title_cancels_and_drains_before_releasing_the_supplied_model() {
    let admission = Arc::new(tokio::sync::Semaphore::new(1));
    let drain = Arc::new(Drain::default());
    let model = Arc::new(HeldModel {
        drain: drain.clone(),
        startup: false,
        invalid: true,
        _admission: admission.clone().acquire_owned().await.unwrap(),
    });
    let generator = TitleGenerator::new(model, None, None);
    let caller = tokio::spawn(async move {
        generator
            .generate("Meeting".into(), CancellationToken::new())
            .await
    });
    signal(&drain.cancelled).await;
    assert_eq!(admission.available_permits(), 0);
    assert!(!caller.is_finished());
    drain.release.add_permits(1);
    assert_eq!(
        caller.await.unwrap().unwrap_err(),
        ProcessingJobFailure::InvalidOutput
    );
    assert!(drain.finished.load(Ordering::Acquire));
    assert_eq!(admission.available_permits(), 1);
}
