use super::*;
use serde_json::json;

fn config() -> ProcessingConfig {
    serde_json::from_value(json!({"providers":[{"id":"openai","models":["configured"],"api_key_env":"LOOFAH_TEST_API_KEY"}]})).unwrap()
}
fn selection(provider: &str, model: &str, base: &str) -> LlmSelection {
    LlmSelection {
        provider_id: provider.into(),
        model_id: model.into(),
        base_url: base.into(),
    }
}

#[tokio::test]
async fn operator_models_restrict_selection_and_endpoints_before_constructing_a_request() {
    let host = ProcessingHost::load_with(&config(), |_| Some("host-secret".into())).unwrap();
    assert!(
        host.models
            .resolve(
                selection("openai", "configured", ""),
                CancellationToken::new()
            )
            .await
            .is_ok()
    );
    assert!(
        host.models
            .resolve(
                selection("openai", "configured", "https://api.openai.com/v1/"),
                CancellationToken::new()
            )
            .await
            .is_ok()
    );
    for selected in [
        selection("unknown", "configured", ""),
        selection("openai", "unapproved", ""),
        selection("openai", "configured", "https://unapproved.example/v1"),
        selection(
            "openai",
            "configured",
            "http://169.254.169.254/latest/meta-data",
        ),
    ] {
        assert!(
            host.models
                .resolve(selected, CancellationToken::new())
                .await
                .is_err()
        );
    }
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(matches!(
        host.models
            .resolve(selection("openai", "configured", ""), cancel)
            .await,
        Err(ProcessingJobFailure::Interrupted)
    ));
}

#[test]
fn provider_secrets_are_host_only_validated_and_never_serialized_into_config() {
    let config = config();
    for secret in [None, Some(""), Some("invalid\r\nheader")] {
        let result = ProcessingHost::load_with(&config, |_| secret.map(str::to_owned));
        assert!(result.is_err());
        let message = result.err().unwrap().to_string();
        assert!(!message.contains("invalid\r\nheader"));
    }
    let _host = ProcessingHost::load_with(&config, |name| {
        assert_eq!(name, "LOOFAH_TEST_API_KEY");
        Some("host-secret".into())
    })
    .unwrap();
    let serialized = serde_json::to_string(&config).unwrap();
    assert!(!serialized.contains("host-secret"));
    assert!(serialized.contains("LOOFAH_TEST_API_KEY"));
}

#[test]
fn policy_validation_checks_all_models_env_references_limits_and_insecure_endpoints() {
    for patch in [
        json!({"concurrent_jobs":0}),
        json!({"concurrent_jobs":17}),
        json!({"waiting_jobs":1025}),
        json!({"providers":[]}),
        json!({"providers":[{"id":"chatgpt_subscription","models":["configured"]}]}),
        json!({"providers":[{"id":"openai","models":["configured"],"api_key_env":"INVALID-NAME"}]}),
        json!({"providers":[{"id":"openai","models":["configured","configured"],"api_key_env":"KEY"}]}),
        json!({"providers":[{"id":"google_generative_ai","models":["models/valid","models/../escape"],"api_key_env":"KEY"}]}),
        json!({"providers":[{"id":"ollama","base_url":"http://model.company.internal/v1","models":["configured"]}]}),
    ] {
        let mut value = serde_json::to_value(config()).unwrap();
        for (key, value_patch) in patch.as_object().unwrap() {
            value[key] = value_patch.clone();
        }
        assert!(
            serde_json::from_value::<ProcessingConfig>(value)
                .unwrap()
                .validate()
                .is_err()
        );
    }
    let config:ProcessingConfig=serde_json::from_value(json!({"providers":[{"id":"ollama","base_url":"http://model.company.internal/v1","models":["configured"],"allow_insecure_http":true}]})).unwrap();
    config.validate().unwrap();
    assert!(
        ProcessingHost::load_with(&config, |_| panic!("anonymous provider read credentials"))
            .is_ok()
    );
}

fn identity(subject: &str) -> crate::auth::Identity {
    crate::auth::Identity {
        issuer: "https://company.example".into(),
        subject: subject.into(),
        name: subject.into(),
        expires_at: 0,
    }
}

#[tokio::test]
async fn durable_job_authorization_uses_current_member_roles_without_retaining_an_oauth_session() {
    let root = tempfile::tempdir().unwrap();
    let catalog = Catalog::open(root.path()).await.unwrap();
    let alice = catalog.provision(&identity("alice")).await.unwrap();
    let bob = catalog.provision(&identity("bob")).await.unwrap();
    let authorizer = MemberAuthorizer {
        catalog: catalog.clone(),
        vault_id: alice.personal_vault_id.clone(),
    };
    let mut work = JobWork {
        principal: alice.member.id.clone(),
        request: hypr_loofah_protocol::SummaryJobRequest {
            title_policy: Default::default(),
            session_id: "note".into(),
            target: hypr_loofah_protocol::DocumentTarget::Summary,
            expected_revision: "revision".into(),
        },
    };
    assert!(authorizer.authorize(&work).await.is_ok());
    work.principal = bob.member.id;
    assert!(matches!(
        authorizer.authorize(&work).await,
        Err(ProcessingJobFailure::AccessDenied)
    ));
    work.principal = "unknown".into();
    assert!(matches!(
        authorizer.authorize(&work).await,
        Err(ProcessingJobFailure::AccessDenied)
    ));
}

#[tokio::test]
async fn fresh_model_admissions_use_rotated_vault_keys_while_admitted_models_keep_original_keys() {
    use axum::{Router, http::HeaderMap};
    use futures_util::StreamExt;
    use hypr_loofah_core::jobs::generation::ModelRequest;
    use hypr_loofah_protocol::{ProviderCredentialSaveOutcome, SaveProviderCredentialRequest};
    let recorded = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let captured = recorded.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/v1", listener.local_addr().unwrap());
    let app = Router::new().route("/v1/chat/completions", axum::routing::post(move |headers: HeaderMap| {
        let captured = captured.clone();
        async move {
            captured.lock().unwrap().push(headers["authorization"].to_str().unwrap().to_owned());
            ([ ("content-type", "text/event-stream") ], "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"body\"},\"finish_reason\":null}]}\n\ndata: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n")
        }
    }));
    let stop = CancellationToken::new();
    let stopped = stop.clone();
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(stopped.cancelled_owned())
            .await
            .unwrap()
    });
    let config: ProcessingConfig = serde_json::from_value(json!({"providers":[{"id":"custom","base_url":endpoint,"models":["approved"],"api_key_env":"TEST_KEY"}]})).unwrap();
    let host = ProcessingHost::load_with(&config, |_| Some("operator-key".into())).unwrap();
    let root = tempfile::tempdir().unwrap();
    let runtime = VaultRuntime::open_with_changes(
        "credential-models".into(),
        &root.path().join("vault"),
        &root.path().join("cache"),
        &root.path().join("operations"),
        tokio::sync::mpsc::unbounded_channel().1,
    )
    .await
    .unwrap();
    let credentials = runtime
        .context
        .configure_provider_credentials(vec![
            hypr_loofah_core::provider_credentials::ProviderCredentialPolicy {
                provider_id: "custom".into(),
                endpoint,
                operator_key: Some("operator-key".into()),
            },
        ])
        .await
        .unwrap();
    let models = VaultModels {
        operator: host.models.clone(),
        credentials: credentials.clone(),
    };
    let original = models
        .resolve(
            selection("custom", "approved", ""),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let initial = credentials.status("custom".into()).await.unwrap();
    let saved = credentials
        .save(
            "custom".into(),
            SaveProviderCredentialRequest {
                expected_revision: initial.revision,
                api_key: Some("vault-key".into()),
            },
        )
        .await
        .unwrap();
    let ProviderCredentialSaveOutcome::Saved(saved) = saved else {
        panic!("expected save")
    };
    let updated = models
        .resolve(
            selection("custom", "approved", ""),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    credentials
        .save(
            "custom".into(),
            SaveProviderCredentialRequest {
                expected_revision: saved.revision,
                api_key: None,
            },
        )
        .await
        .unwrap();
    let fallback = models
        .resolve(
            selection("custom", "approved", ""),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    for model in [original, updated, fallback] {
        let stream = model
            .model
            .stream(
                ModelRequest {
                    system: "System".into(),
                    prompt: "Prompt".into(),
                    images: vec![],
                    max_output_tokens: 64,
                    max_retries: 0,
                    temperature: None,
                },
                CancellationToken::new(),
            )
            .await
            .unwrap();
        let chunks: Vec<_> = stream.collect().await;
        assert!(chunks.iter().all(|chunk| chunk.is_ok()));
    }
    assert_eq!(
        *recorded.lock().unwrap(),
        [
            "Bearer operator-key",
            "Bearer vault-key",
            "Bearer operator-key"
        ]
    );
    drop(models);
    drop(credentials);
    runtime.shutdown().await.unwrap();
    stop.cancel();
    server.await.unwrap();
}
