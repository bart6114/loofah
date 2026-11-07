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
