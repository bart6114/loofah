use std::{
    collections::HashMap,
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use hypr_loofah_protocol::{DocumentTarget, SummaryJobRequest};
use serde_json::json;

use super::*;
use crate::{
    jobs::generation::{ModelChunk, ModelRequest, ModelStream},
    search::SearchService,
    settings::ConfigState,
    startup::StartupState,
};

mod prepared;

struct Fixture {
    _root: tempfile::TempDir,
    context: Arc<VaultContext>,
}
impl Fixture {
    async fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let context = Arc::new(
            VaultContext::new(
                "vault".into(),
                Arc::new(hypr_vault_write::SessionStore::new(root.path().into())),
                StartupState::new(Some(root.path())),
                Arc::new(ConfigState::load_or_default(root.path())),
                Arc::new(SearchService::default()),
            )
            .with_operation_storage(root.path().join("operations")),
        );
        let meta = serde_json::from_value(
            json!({"id":"note","title":"Release","created_at":"2026-11-07T10:00:00Z","tags":[]}),
        )
        .unwrap();
        context
            .store
            .create_note_session(&meta, "Ship Friday.")
            .await
            .unwrap();
        context
            .settings
            .set_values(HashMap::from([
                ("current_llm_provider".into(), json!("openai")),
                ("current_llm_model".into(), json!("configured")),
            ]))
            .await
            .unwrap();
        Self {
            _root: root,
            context,
        }
    }
    fn generator(
        &self,
        models: Arc<dyn SummaryModels>,
        admission: ProcessingAdmission,
        custom: bool,
    ) -> Arc<ConfiguredSummaryGenerator> {
        Arc::new(ConfiguredSummaryGenerator::new(
            &self.context,
            SummaryIdentity::LocalOwner,
            models,
            admission,
            custom,
        ))
    }
}
fn work() -> JobWork {
    JobWork {
        principal: "device".into(),
        request: SummaryJobRequest {
            title_policy: Default::default(),
            session_id: "note".into(),
            target: DocumentTarget::Summary,
            expected_revision: "revision".into(),
        },
    }
}

struct Model {
    calls: AtomicUsize,
    context: Weak<VaultContext>,
    mutate_selection: bool,
    blocked: bool,
    started: Arc<Semaphore>,
    cancelled: Arc<Semaphore>,
    drain: Arc<Semaphore>,
}
impl Model {
    fn new(context: &Arc<VaultContext>, blocked: bool) -> Arc<Self> {
        Arc::new(Self {
            calls: AtomicUsize::new(0),
            context: Arc::downgrade(context),
            mutate_selection: !blocked,
            blocked,
            started: Arc::new(Semaphore::new(0)),
            cancelled: Arc::new(Semaphore::new(0)),
            drain: Arc::new(Semaphore::new(0)),
        })
    }
}
impl TextModel for Model {
    fn stream(
        &self,
        _: ModelRequest,
        cancel: CancellationToken,
    ) -> JobFuture<'_, Result<ModelStream, ProcessingJobFailure>> {
        Box::pin(async move {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            let invalid = self.mutate_selection && call == 0;
            if invalid {
                self.context
                    .upgrade()
                    .unwrap()
                    .settings
                    .set_values(HashMap::from([
                        ("current_llm_provider".into(), json!("google_generative_ai")),
                        ("current_llm_model".into(), json!("another")),
                    ]))
                    .await
                    .unwrap();
            }
            let (blocked, started, cancelled, drain) = (
                self.blocked,
                self.started.clone(),
                self.cancelled.clone(),
                self.drain.clone(),
            );
            Ok(Box::pin(async_stream::stream! {
                started.add_permits(1);
                if blocked {
                    cancel.cancelled().await;
                    cancelled.add_permits(1);
                    drain.acquire().await.unwrap().forget();
                }
                yield Ok(ModelChunk::Text(if invalid {"Invalid opening without heading."} else {"# Release\n- Ship Friday."}.into()));
                yield Ok(ModelChunk::Finish{truncated:false});
            }) as ModelStream)
        })
    }
}
struct Models {
    model: Arc<Model>,
    selections: Mutex<Vec<LlmSelection>>,
    inject_prompt: bool,
}
impl Models {
    fn new(model: Arc<Model>) -> Arc<Self> {
        Arc::new(Self {
            model,
            selections: Mutex::new(Vec::new()),
            inject_prompt: false,
        })
    }
}
impl SummaryModels for Models {
    fn resolve(
        &self,
        selection: LlmSelection,
        _: CancellationToken,
    ) -> JobFuture<'_, Result<SummaryModel, ProcessingJobFailure>> {
        Box::pin(async move {
            self.selections.lock().unwrap().push(selection);
            if self.inject_prompt {
                self.model
                    .context
                    .upgrade()
                    .unwrap()
                    .settings
                    .set_values(HashMap::from([(
                        "auto_summary_prompt".into(),
                        json!("Custom {{ language }}"),
                    )]))
                    .await
                    .unwrap();
            }
            Ok(SummaryModel {
                model: self.model.clone(),
                include_images: false,
            })
        })
    }
}
async fn until(mut ready: impl FnMut() -> bool) {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !ready() {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}

#[test]
fn selection_uses_only_current_llm_settings_and_preserves_opaque_model_ids() {
    let config:AppConfig=serde_json::from_value(json!({"current_llm_provider":" custom ","current_llm_model":" company/model-v2 ","ai_providers":{"llm:custom":{"type":"llm","base_url":" https://company.example/v1 ","api_key":"ignored-secret"}}})).unwrap();
    let selected = LlmSelection::from_config(&config).unwrap();
    assert_eq!(selected.provider_id, "custom");
    assert_eq!(selected.model_id, "company/model-v2");
    assert_eq!(selected.base_url, "https://company.example/v1");
    assert!(LlmSelection::from_config(&AppConfig::default()).is_err());
    for model in ["", "invalid\nmodel"] {
        let mut invalid = config.clone();
        invalid.current_llm_model = Some(model.into());
        assert!(LlmSelection::from_config(&invalid).is_err());
    }
}

#[tokio::test]
async fn model_is_resolved_once_per_job_and_frozen_across_heading_retries() {
    let f = Fixture::new().await;
    let model = Model::new(&f.context, false);
    let models = Models::new(model.clone());
    let generator = f.generator(
        models.clone(),
        ProcessingAdmission::new(1, 1).unwrap(),
        true,
    );
    let result = generator
        .generate(work(), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(result.markdown, "# Release\n- Ship Friday.");
    assert_eq!(model.calls.load(Ordering::SeqCst), 2);
    assert_eq!(models.selections.lock().unwrap().len(), 1);
    assert_eq!(models.selections.lock().unwrap()[0].model_id, "configured");
    generator
        .generate(work(), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(models.selections.lock().unwrap()[1].model_id, "another");
}

#[tokio::test]
async fn admission_bounds_waiters_and_retains_capacity_after_caller_abort_until_actual_model_drain()
{
    let f = Fixture::new().await;
    let model = Model::new(&f.context, true);
    let models = Models::new(model.clone());
    let admission = ProcessingAdmission::new(1, 1).unwrap();
    let generator = f.generator(models.clone(), admission.clone(), true);
    let first = {
        let g = generator.clone();
        tokio::spawn(async move { g.generate(work(), CancellationToken::new()).await })
    };
    tokio::time::timeout(std::time::Duration::from_secs(5), model.started.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    let cancel = CancellationToken::new();
    let second = {
        let g = generator.clone();
        let c = cancel.clone();
        tokio::spawn(async move { g.generate(work(), c).await })
    };
    until(|| admission.total.available_permits() == 0).await;
    assert_eq!(
        generator
            .generate(work(), CancellationToken::new())
            .await
            .err(),
        Some(ProcessingJobFailure::ResourceLimit)
    );
    cancel.cancel();
    assert_eq!(
        second.await.unwrap().err(),
        Some(ProcessingJobFailure::Interrupted)
    );
    assert_eq!(models.selections.lock().unwrap().len(), 1);
    first.abort();
    assert!(first.await.unwrap_err().is_cancelled());
    tokio::time::timeout(std::time::Duration::from_secs(5), model.cancelled.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    assert_eq!(admission.running.available_permits(), 0);
    assert_eq!(admission.total.available_permits(), 1);
    model.drain.add_permits(1);
    until(|| {
        admission.running.available_permits() == 1 && admission.total.available_permits() == 2
    })
    .await;
}

#[tokio::test]
async fn template_policy_uses_the_rendered_snapshot_even_if_settings_change_during_model_resolution()
 {
    let f = Fixture::new().await;
    let model = Model::new(&f.context, false);
    let models = Arc::new(Models {
        model: model.clone(),
        selections: Mutex::new(Vec::new()),
        inject_prompt: true,
    });
    let generator = f.generator(models, ProcessingAdmission::new(1, 1).unwrap(), false);
    assert_eq!(
        generator
            .generate(work(), CancellationToken::new())
            .await
            .err(),
        Some(ProcessingJobFailure::AccessDenied)
    );
    assert_eq!(model.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn default_local_custom_templates_remain_available_without_login() {
    let f = Fixture::new().await;
    f.context
        .settings
        .set_values(HashMap::from([(
            "auto_summary_prompt".into(),
            json!("Summarize in {{ language }}."),
        )]))
        .await
        .unwrap();
    let model = Model::new(&f.context, false);
    let generator = f.generator(
        Models::new(model),
        ProcessingAdmission::new(1, 0).unwrap(),
        true,
    );
    assert!(
        generator
            .generate(work(), CancellationToken::new())
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn pre_cancelled_or_retired_contexts_do_not_resolve_models_or_consume_capacity() {
    let f = Fixture::new().await;
    let model = Model::new(&f.context, false);
    let models = Models::new(model);
    let admission = ProcessingAdmission::new(1, 0).unwrap();
    let generator = f.generator(models.clone(), admission.clone(), true);
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert_eq!(
        generator.generate(work(), cancel).await.err(),
        Some(ProcessingJobFailure::Interrupted)
    );
    drop(f);
    assert_eq!(
        generator
            .generate(work(), CancellationToken::new())
            .await
            .err(),
        Some(ProcessingJobFailure::SourceUnavailable)
    );
    assert!(models.selections.lock().unwrap().is_empty());
    assert_eq!(admission.running.available_permits(), 1);
}
