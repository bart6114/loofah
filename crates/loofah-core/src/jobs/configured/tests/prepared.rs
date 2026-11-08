use std::sync::atomic::AtomicBool;

use super::*;

struct Legacy(AtomicUsize);

impl JobGenerator for Legacy {
    fn generate(
        &self,
        _: JobWork,
        cancel: CancellationToken,
    ) -> JobFuture<'_, Result<GeneratedDocument, ProcessingJobFailure>> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            if cancel.is_cancelled() {
                Err(ProcessingJobFailure::Interrupted)
            } else {
                Ok(GeneratedDocument {
                    markdown: "# Legacy body".into(),
                    suggested_tags: None,
                })
            }
        })
    }
}

#[tokio::test]
async fn default_prepared_generation_calls_the_existing_body_method_without_a_title_or_recursion() {
    let legacy = Arc::new(Legacy(AtomicUsize::new(0)));
    let generator: Arc<dyn JobGenerator> = legacy.clone();
    let prepared = generator
        .generate_prepared(work(), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(prepared.document.markdown, "# Legacy body");
    assert!(prepared.title.is_none());
    assert_eq!(legacy.0.load(Ordering::SeqCst), 1);
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert_eq!(
        generator.generate_prepared(work(), cancel).await.err(),
        Some(ProcessingJobFailure::Interrupted)
    );
    assert_eq!(legacy.0.load(Ordering::SeqCst), 2);
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Blocking {
    None,
    TitleStartup,
    TitleEof,
    BodyEof,
}

struct BridgeModel {
    context: Weak<VaultContext>,
    requests: Mutex<Vec<ModelRequest>>,
    blocking: Blocking,
    started: Arc<Semaphore>,
    cancelled: Arc<Semaphore>,
    drain: Arc<Semaphore>,
    drained: Arc<AtomicBool>,
}

impl BridgeModel {
    fn new(context: &Arc<VaultContext>, blocking: Blocking) -> Arc<Self> {
        Arc::new(Self {
            context: Arc::downgrade(context),
            requests: Mutex::new(vec![]),
            blocking,
            started: Arc::new(Semaphore::new(0)),
            cancelled: Arc::new(Semaphore::new(0)),
            drain: Arc::new(Semaphore::new(0)),
            drained: Arc::new(AtomicBool::new(false)),
        })
    }
}

impl TextModel for BridgeModel {
    fn stream(
        &self,
        request: ModelRequest,
        cancel: CancellationToken,
    ) -> JobFuture<'_, Result<ModelStream, ProcessingJobFailure>> {
        let title = request.max_output_tokens == 128;
        self.requests.lock().unwrap().push(request);
        let (blocking, started, cancelled, drain, drained) = (
            self.blocking,
            self.started.clone(),
            self.cancelled.clone(),
            self.drain.clone(),
            self.drained.clone(),
        );
        Box::pin(async move {
            if title && blocking == Blocking::TitleStartup {
                started.add_permits(1);
                cancel.cancelled().await;
                cancelled.add_permits(1);
                drain.acquire().await.unwrap().forget();
                drained.store(true, Ordering::Release);
                return Err(ProcessingJobFailure::ProviderFailed);
            }
            Ok(Box::pin(async_stream::stream! {
                yield Ok(ModelChunk::Text(if title { "Release planning" } else { "# Release\n- Ship Friday." }.into()));
                yield Ok(ModelChunk::Finish { truncated: false });
                if title && blocking == Blocking::TitleEof || !title && blocking == Blocking::BodyEof {
                    started.add_permits(1);
                    cancel.cancelled().await;
                    cancelled.add_permits(1);
                    drain.acquire().await.unwrap().forget();
                    drained.store(true, Ordering::Release);
                }
            }) as ModelStream)
        })
    }
}

struct BridgeModels {
    model: Arc<BridgeModel>,
    selections: Mutex<Vec<LlmSelection>>,
    mutate: bool,
    images: bool,
}

impl BridgeModels {
    fn new(model: Arc<BridgeModel>, mutate: bool, images: bool) -> Arc<Self> {
        Arc::new(Self {
            model,
            selections: Mutex::new(vec![]),
            mutate,
            images,
        })
    }
}

impl SummaryModels for BridgeModels {
    fn resolve(
        &self,
        selection: LlmSelection,
        _: CancellationToken,
    ) -> JobFuture<'_, Result<SummaryModel, ProcessingJobFailure>> {
        self.selections.lock().unwrap().push(selection);
        Box::pin(async move {
            if self.mutate {
                self.model
                    .context
                    .upgrade()
                    .unwrap()
                    .settings
                    .set_values(HashMap::from([
                        ("current_llm_provider".into(), json!("openai")),
                        ("current_llm_model".into(), json!("later-model")),
                        ("ai_language".into(), json!("ko")),
                        (
                            "auto_summary_prompt".into(),
                            json!("Later custom instructions."),
                        ),
                    ]))
                    .await
                    .unwrap();
            }
            Ok(SummaryModel {
                model: self.model.clone(),
                include_images: self.images,
            })
        })
    }
}

async fn signal(semaphore: &Semaphore) {
    tokio::time::timeout(std::time::Duration::from_secs(10), semaphore.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
}

fn lease(fixture: &mut Fixture) -> std::fs::File {
    let path = fixture._root.path().join("prepared-test-lease");
    let owner = std::fs::File::create(&path).unwrap();
    owner.try_lock().unwrap();
    Arc::get_mut(&mut fixture.context).unwrap().lease = Some(owner.into());
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .unwrap()
}

async fn image_note(fixture: &Fixture) -> Vec<u8> {
    let directory = fixture._root.path().join("sessions/note/attachments");
    std::fs::create_dir_all(&directory).unwrap();
    let image = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
        2,
        3,
        image::Rgb([20, 100, 200]),
    ));
    let mut encoded = std::io::Cursor::new(vec![]);
    image
        .write_to(&mut encoded, image::ImageFormat::Png)
        .unwrap();
    let bytes = encoded.into_inner();
    std::fs::write(directory.join("chart.png"), &bytes).unwrap();
    fixture
        .context
        .store
        .write_note("note", "Ship Friday.\n![chart](attachments/chart.png)")
        .await
        .unwrap();
    bytes
}

#[tokio::test]
async fn prepared_model_settings_and_image_policy_are_pinned_once_until_title_completion() {
    for images in [false, true] {
        let fixture = Fixture::new().await;
        let image = image_note(&fixture).await;
        fixture
            .context
            .settings
            .set_values(HashMap::from([
                ("current_llm_provider".into(), json!("anthropic")),
                ("current_llm_model".into(), json!("claude-opus-4-8")),
                ("ai_language".into(), json!("nl")),
                (
                    "auto_summary_prompt".into(),
                    json!("Captured instructions in {{ language }}."),
                ),
            ]))
            .await
            .unwrap();
        let model = BridgeModel::new(&fixture.context, Blocking::None);
        let models = BridgeModels::new(model.clone(), true, images);
        let admission = ProcessingAdmission::new(1, 0).unwrap();
        let generator = fixture.generator(models.clone(), admission.clone(), true);
        let prepared = generator
            .generate_prepared(work(), CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(admission.running.available_permits(), 0);
        assert_eq!(admission.total.available_permits(), 0);
        assert_eq!(prepared.document.markdown, "# Release\n- Ship Friday.");
        let title = prepared
            .title
            .unwrap()
            .generate(prepared.document.markdown, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(title, Some("Release planning".into()));
        assert_eq!(admission.running.available_permits(), 1);
        assert_eq!(admission.total.available_permits(), 1);
        assert_eq!(models.selections.lock().unwrap().len(), 1);
        assert_eq!(
            models.selections.lock().unwrap()[0].model_id,
            "claude-opus-4-8"
        );
        let requests = model.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        let system = hypr_template_app::EnhanceSystem {
            language: Some("nl".into()),
            prompt_override: "Captured instructions in {{ language }}.".into(),
        };
        assert_eq!(
            requests[0].system,
            hypr_template_app::render_enhance_system_bounded(&system, 256 * 1024).unwrap()
        );
        assert_eq!(requests[0].images.len(), usize::from(images));
        if images {
            assert_eq!(requests[0].images[0].bytes, image);
        }
        assert_eq!(
            requests[1].system,
            hypr_template_app::render(hypr_template_app::Template::TitleSystem(
                hypr_template_app::TitleSystem {
                    language: Some("nl".into())
                }
            ))
            .unwrap()
        );
        assert_eq!(requests[1].temperature, None);
        assert_eq!(requests[1].max_output_tokens, 128);
        assert!(requests[1].images.is_empty());
    }
}

#[tokio::test]
async fn prepared_snapshot_enforces_host_template_policy_while_legacy_fresh_recheck_is_unchanged() {
    let fixture = Fixture::new().await;
    let model = BridgeModel::new(&fixture.context, Blocking::None);
    let models = BridgeModels::new(model.clone(), true, false);
    let generator = fixture.generator(
        models.clone(),
        ProcessingAdmission::new(1, 0).unwrap(),
        false,
    );
    let prepared = generator
        .generate_prepared(work(), CancellationToken::new())
        .await
        .unwrap();
    assert!(
        !model.requests.lock().unwrap()[0]
            .system
            .contains("Later custom instructions.")
    );
    drop(prepared);
    assert_eq!(
        generator
            .generate_prepared(work(), CancellationToken::new())
            .await
            .err(),
        Some(ProcessingJobFailure::AccessDenied)
    );
    assert_eq!(models.selections.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn unused_unpolled_and_precancelled_title_runners_release_capacity_and_real_vault_lease_safely()
 {
    for mode in 0..4 {
        let mut fixture = Fixture::new().await;
        let competing = lease(&mut fixture);
        let model = BridgeModel::new(&fixture.context, Blocking::None);
        let admission = ProcessingAdmission::new(1, 0).unwrap();
        let generator = fixture.generator(
            BridgeModels::new(model.clone(), false, false),
            admission.clone(),
            true,
        );
        let prepared = generator
            .generate_prepared(work(), CancellationToken::new())
            .await
            .unwrap();
        let weak = Arc::downgrade(&fixture.context);
        let Fixture { _root, context } = fixture;
        drop(context);
        assert!(weak.upgrade().is_some());
        assert!(competing.try_lock().is_err());
        if mode == 0 {
            drop(prepared);
        } else {
            let cancel = CancellationToken::new();
            if mode == 2 {
                cancel.cancel();
            }
            let future = prepared
                .title
                .unwrap()
                .generate(prepared.document.markdown, cancel.clone());
            if mode == 1 {
                drop(future);
            } else {
                cancel.cancel();
                assert_eq!(future.await.err(), Some(ProcessingJobFailure::Interrupted));
            }
        }
        until(|| weak.upgrade().is_none() && admission.running.available_permits() == 1).await;
        assert_eq!(admission.total.available_permits(), 1);
        assert_eq!(model.requests.lock().unwrap().len(), 1);
        competing.try_lock().unwrap();
    }
}

#[tokio::test]
async fn title_startup_eof_cancellation_and_caller_drop_keep_capacity_and_real_lease_until_actual_exit()
 {
    for blocking in [Blocking::TitleStartup, Blocking::TitleEof] {
        for drop_caller in [false, true] {
            let mut fixture = Fixture::new().await;
            let competing = lease(&mut fixture);
            let model = BridgeModel::new(&fixture.context, blocking);
            let models = BridgeModels::new(model.clone(), false, false);
            let admission = ProcessingAdmission::new(1, 1).unwrap();
            let generator = fixture.generator(models.clone(), admission.clone(), true);
            let prepared = generator
                .generate_prepared(work(), CancellationToken::new())
                .await
                .unwrap();
            let cancel = CancellationToken::new();
            let title = prepared
                .title
                .unwrap()
                .generate(prepared.document.markdown, cancel.clone());
            let caller = tokio::spawn(title);
            signal(&model.started).await;
            let waiting_cancel = CancellationToken::new();
            let waiter = {
                let generator = generator.clone();
                let stopping = waiting_cancel.clone();
                tokio::spawn(async move { generator.generate(work(), stopping).await })
            };
            until(|| admission.total.available_permits() == 0).await;
            assert_eq!(models.selections.lock().unwrap().len(), 1);
            let weak = Arc::downgrade(&fixture.context);
            let Fixture { _root, context } = fixture;
            drop(context);
            if drop_caller {
                caller.abort();
            } else {
                cancel.cancel();
            }
            signal(&model.cancelled).await;
            assert!(!model.drained.load(Ordering::Acquire));
            assert!(!waiter.is_finished());
            assert_eq!(admission.running.available_permits(), 0);
            assert!(weak.upgrade().is_some());
            assert!(competing.try_lock().is_err());
            waiting_cancel.cancel();
            assert_eq!(
                waiter.await.unwrap().err(),
                Some(ProcessingJobFailure::Interrupted)
            );
            assert_eq!(admission.total.available_permits(), 1);
            assert!(weak.upgrade().is_some());
            model.drain.add_permits(1);
            if drop_caller {
                assert!(caller.await.unwrap_err().is_cancelled());
            } else {
                assert_eq!(
                    caller.await.unwrap().err(),
                    Some(ProcessingJobFailure::Interrupted)
                );
            }
            until(|| weak.upgrade().is_none() && admission.running.available_permits() == 1).await;
            assert!(model.drained.load(Ordering::Acquire));
            assert_eq!(admission.total.available_permits(), 2);
            competing.try_lock().unwrap();
        }
    }
}

#[tokio::test]
async fn cancelled_prepared_body_keeps_its_context_and_admission_until_the_actual_body_stream_drains()
 {
    let mut fixture = Fixture::new().await;
    let competing = lease(&mut fixture);
    let model = BridgeModel::new(&fixture.context, Blocking::BodyEof);
    let admission = ProcessingAdmission::new(1, 0).unwrap();
    let generator = fixture.generator(
        BridgeModels::new(model.clone(), false, false),
        admission.clone(),
        true,
    );
    let caller = tokio::spawn(async move {
        generator
            .generate_prepared(work(), CancellationToken::new())
            .await
    });
    signal(&model.started).await;
    let weak = Arc::downgrade(&fixture.context);
    let Fixture { _root, context } = fixture;
    drop(context);
    caller.abort();
    assert!(matches!(caller.await, Err(cause) if cause.is_cancelled()));
    signal(&model.cancelled).await;
    assert!(weak.upgrade().is_some());
    assert_eq!(admission.total.available_permits(), 0);
    assert!(competing.try_lock().is_err());
    model.drain.add_permits(1);
    until(|| weak.upgrade().is_none() && admission.running.available_permits() == 1).await;
    assert!(model.drained.load(Ordering::Acquire));
    assert_eq!(admission.total.available_permits(), 1);
    competing.try_lock().unwrap();
}
