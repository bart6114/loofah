use std::sync::Mutex;

use super::*;

struct Secrets {
    reads: Mutex<Vec<String>>,
    value: Option<String>,
    failure: Option<ProcessingJobFailure>,
}
impl ProviderSecrets for Secrets {
    fn read(
        &self,
        provider_id: String,
        _: CancellationToken,
    ) -> JobFuture<'_, Result<Option<String>, ProcessingJobFailure>> {
        Box::pin(async move {
            self.reads.lock().unwrap().push(provider_id);
            if let Some(failure) = self.failure {
                return Err(failure);
            }
            Ok(self.value.clone())
        })
    }
}
fn selection(provider: &str, base_url: &str) -> LlmSelection {
    LlmSelection {
        provider_id: provider.into(),
        model_id: "configured".into(),
        base_url: base_url.into(),
    }
}

#[tokio::test]
async fn local_provider_resolution_keeps_anonymous_models_and_keychain_owned_keys_separate() {
    let secrets = Arc::new(Secrets {
        reads: Mutex::new(Vec::new()),
        value: None,
        failure: None,
    });
    let models = LocalHttpModels::new(secrets.clone());
    assert!(
        models
            .resolve(selection("ollama", ""), CancellationToken::new())
            .await
            .is_ok()
    );
    assert!(secrets.reads.lock().unwrap().is_empty());
    assert!(
        models
            .resolve(selection("lmstudio", ""), CancellationToken::new())
            .await
            .is_ok()
    );
    assert_eq!(secrets.reads.lock().unwrap().as_slice(), ["lmstudio"]);
    assert!(matches!(
        models
            .resolve(selection("openai", ""), CancellationToken::new())
            .await,
        Err(ProcessingJobFailure::ProviderUnavailable)
    ));
}

#[tokio::test]
async fn configured_keys_are_read_only_from_the_injected_host_and_failures_are_preserved() {
    for failure in [
        None,
        Some(ProcessingJobFailure::ProviderUnavailable),
        Some(ProcessingJobFailure::Interrupted),
    ] {
        let secrets = Arc::new(Secrets {
            reads: Mutex::new(Vec::new()),
            value: Some(" host-secret ".into()),
            failure,
        });
        let models = LocalHttpModels::new(secrets.clone());
        let result = models
            .resolve(selection("openai", ""), CancellationToken::new())
            .await;
        match failure {
            Some(failure) => assert_eq!(result.err(), Some(failure)),
            None => assert!(result.is_ok()),
        }
        assert_eq!(secrets.reads.lock().unwrap().as_slice(), ["openai"]);
    }
}

#[tokio::test]
async fn subscription_unknown_missing_endpoint_and_precancelled_selection_never_read_keys() {
    let secrets = Arc::new(Secrets {
        reads: Mutex::new(Vec::new()),
        value: None,
        failure: None,
    });
    let models = LocalHttpModels::new(secrets.clone());
    for provider in ["chatgpt_subscription", "unknown", "custom", "azure_ai"] {
        assert!(
            models
                .resolve(selection(provider, ""), CancellationToken::new())
                .await
                .is_err()
        );
    }
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(matches!(
        models.resolve(selection("openai", ""), cancel).await,
        Err(ProcessingJobFailure::Interrupted)
    ));
    assert!(secrets.reads.lock().unwrap().is_empty());
}

#[tokio::test]
async fn explicit_local_provider_endpoints_are_preserved_without_new_login_requirements() {
    let secrets = Arc::new(Secrets {
        reads: Mutex::new(Vec::new()),
        value: Some("secret".into()),
        failure: None,
    });
    let models = LocalHttpModels::new(secrets);
    assert!(
        models
            .resolve(
                selection("custom", "http://model.company.internal/v1"),
                CancellationToken::new()
            )
            .await
            .is_ok()
    );
    assert!(
        models
            .resolve(
                selection("anthropic", "https://model.company.example/v1"),
                CancellationToken::new()
            )
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn resolved_local_models_gate_image_collection_before_source_reads() {
    use crate::{
        VaultContext,
        jobs::{
            JobWork,
            generation::SummarySource,
            source::{SummaryIdentity, VaultSummarySource},
        },
        search::SearchService,
        settings::ConfigState,
        startup::StartupState,
    };
    use hypr_loofah_protocol::{DocumentTarget, SummaryJobRequest};
    use hypr_vault_write::SessionStore;
    let root = tempfile::tempdir().unwrap();
    let context = Arc::new(VaultContext::new(
        "vault".into(),
        Arc::new(SessionStore::new(root.path().into())),
        StartupState::new(Some(root.path())),
        Arc::new(ConfigState::load_or_default(root.path())),
        Arc::new(SearchService::default()),
    ));
    let references = "![chart](attachments/chart.png)\n".repeat(2001);
    context.store.create_note_session(&serde_json::from_value(serde_json::json!({"id":"note","title":"Note","created_at":"2026-11-07T10:00:00Z","tags":[]})).unwrap(), &format!("Source text\n{references}")).await.unwrap();
    let directory = root.path().join("sessions/note/attachments");
    std::fs::create_dir(&directory).unwrap();
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(2, 2, image::Rgb([10, 20, 30])))
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
    let bytes = bytes.into_inner();
    std::fs::write(directory.join("chart.png"), &bytes).unwrap();
    let models = LocalHttpModels::new(Arc::new(Secrets {
        reads: Mutex::new(Vec::new()),
        value: None,
        failure: None,
    }));
    let work = || JobWork {
        principal: "desktop".into(),
        request: SummaryJobRequest {
            session_id: "note".into(),
            target: DocumentTarget::Summary,
            expected_revision: "unused".into(),
            title_policy: Default::default(),
            only_if_empty: false,
        },
    };
    for name in ["llama-3.1-8b", "unknown-model", "llava:latest"] {
        let mut input = selection("ollama", "");
        input.model_id = name.into();
        let resolved = models
            .resolve(input, CancellationToken::new())
            .await
            .unwrap();
        let source = VaultSummarySource::new(
            &context,
            SummaryIdentity::LocalOwner,
            resolved.include_images,
        );
        let prepared = source.prepare(work(), CancellationToken::new()).await;
        if name == "llava:latest" {
            // The vision path reaches reference collection; text-only paths skip it entirely.
            assert!(matches!(prepared, Err(ProcessingJobFailure::ResourceLimit)));
            context
                .store
                .write_note("note", "Source text\n![chart](attachments/chart.png)")
                .await
                .unwrap();
            let prepared = source
                .prepare(work(), CancellationToken::new())
                .await
                .unwrap();
            assert_eq!(prepared.images.len(), 1);
            assert_eq!(prepared.images[0].bytes, bytes);
        } else {
            assert!(prepared.unwrap().images.is_empty());
        }
    }
    assert_eq!(std::fs::read(directory.join("chart.png")).unwrap(), bytes);
}
