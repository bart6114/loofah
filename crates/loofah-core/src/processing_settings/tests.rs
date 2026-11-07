use super::*;
use crate::VaultRuntime;
use serde_json::json;

async fn runtime(
    vault: &std::path::Path,
    cache: &std::path::Path,
    operations: &std::path::Path,
) -> VaultRuntime {
    let runtime = VaultRuntime::open_with_changes(
        "settings-vault".into(),
        vault,
        cache,
        operations,
        tokio::sync::mpsc::unbounded_channel().1,
    )
    .await
    .unwrap();
    runtime
        .context
        .configure_processing_policy(ProcessingPolicy {
            providers: vec![ProcessingProviderPolicy {
                id: "openai".into(),
                models: vec!["approved-model".into()],
                base_url: "https://api.openai.com/v1".into(),
                allow_custom_model: false,
                allow_custom_endpoint: false,
                requires_key: true,
            }],
            allow_custom_templates: false,
        })
        .unwrap();
    runtime
}

fn selection() -> SummaryModelSelection {
    SummaryModelSelection {
        provider_id: "openai".into(),
        model_id: "approved-model".into(),
        base_url: String::new(),
    }
}

#[tokio::test]
async fn checked_projection_preserves_current_device_and_unknown_fields() {
    let vault = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let operations = tempfile::tempdir().unwrap();
    let runtime = runtime(vault.path(), cache.path(), operations.path()).await;
    let before = runtime.context.read_processing_settings().await.unwrap();
    // A canonical-file edit after startup must not disappear into a cached merge.
    std::fs::write(vault.path().join("config.json"),json!({"theme":"dark","future":{"keep":42},"ai_providers":{"llm:openai":{"type":"llm","base_url":"","future":true},"stt:custom":{"type":"stt","base_url":"private"}}}).to_string()).unwrap();
    let mut settings = before.settings.clone();
    settings.summary_model = Some(selection());
    settings.ai_language = "nl".into();
    settings.auto_apply_high_confidence_tags = false;
    let result = runtime
        .context
        .save_processing_settings(SaveProcessingSettingsRequest {
            settings: settings.clone(),
            expected_revision: before.revision.clone(),
        })
        .await
        .unwrap();
    let ProcessingSettingsSaveOutcome::Saved(saved) = result else {
        panic!("unrelated changes must not conflict")
    };
    assert_eq!(saved.settings, settings);
    assert_ne!(saved.revision, before.revision);
    let config = runtime.context.settings.snapshot();
    assert_eq!(config.theme, "dark");
    assert_eq!(config.extra["future"], json!({"keep":42}));
    assert_eq!(config.ai_providers["llm:openai"].extra["future"], true);
    assert_eq!(config.ai_providers["stt:custom"].base_url, "private");
    assert!(
        !serde_json::to_value(&saved)
            .unwrap()
            .to_string()
            .contains("private")
    );
    assert_eq!(
        runtime.context.read_processing_settings().await.unwrap(),
        saved
    );
    runtime.shutdown().await.unwrap();
    assert_eq!(
        snapshot(&crate::settings::ConfigState::load_or_default(vault.path()).snapshot()),
        saved
    );
}

#[tokio::test]
async fn simultaneous_writes_and_external_processing_edits_conflict_without_overwrite() {
    let vault = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let operations = tempfile::tempdir().unwrap();
    let runtime = runtime(vault.path(), cache.path(), operations.path()).await;
    let before = runtime.context.read_processing_settings().await.unwrap();
    let mut first = SaveProcessingSettingsRequest {
        settings: before.settings.clone(),
        expected_revision: before.revision.clone(),
    };
    first.settings.ai_language = "nl".into();
    let mut second = first.clone();
    second.settings.ai_language = "fr".into();
    let (one, two) = tokio::join!(
        runtime.context.save_processing_settings(first),
        runtime.context.save_processing_settings(second)
    );
    let outcomes = [one.unwrap(), two.unwrap()];
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| matches!(outcome, ProcessingSettingsSaveOutcome::Saved(_)))
            .count(),
        1
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| matches!(outcome, ProcessingSettingsSaveOutcome::Conflict(_)))
            .count(),
        1
    );
    let current = runtime.context.read_processing_settings().await.unwrap();
    std::fs::write(
        vault.path().join("config.json"),
        json!({"ai_language":"de"}).to_string(),
    )
    .unwrap();
    let result = runtime
        .context
        .save_processing_settings(SaveProcessingSettingsRequest {
            settings: current.settings,
            expected_revision: current.revision,
        })
        .await
        .unwrap();
    assert!(
        matches!(result,ProcessingSettingsSaveOutcome::Conflict(value) if value.settings.ai_language=="de")
    );
    runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn processing_policy_rejects_unapproved_models_endpoints_prompts_and_unknown_fields() {
    let vault = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let operations = tempfile::tempdir().unwrap();
    let runtime = runtime(vault.path(), cache.path(), operations.path()).await;
    let initial = runtime.context.read_processing_settings().await.unwrap();
    for (model, endpoint, prompt, code) in [
        ("unapproved", "", "", ErrorCode::Validation),
        (
            "approved-model",
            "https://another-company.example/v1",
            "",
            ErrorCode::Validation,
        ),
        (
            "approved-model",
            "",
            "{{ private_template }}",
            ErrorCode::Validation,
        ),
        ("bad\nmodel", "", "", ErrorCode::Validation),
    ] {
        let mut settings = initial.settings.clone();
        settings.summary_model = Some(selection());
        let selected = settings.summary_model.as_mut().unwrap();
        selected.model_id = model.into();
        selected.base_url = endpoint.into();
        settings.summary_prompt = prompt.into();
        assert_eq!(
            runtime
                .context
                .save_processing_settings(SaveProcessingSettingsRequest {
                    settings,
                    expected_revision: initial.revision.clone()
                })
                .await
                .unwrap_err()
                .code,
            code
        );
        assert_eq!(
            runtime.context.read_processing_settings().await.unwrap(),
            initial
        );
    }
    assert!(
        serde_json::from_value::<SaveProcessingSettingsRequest>(
            json!({"settings":initial.settings,"expected_revision":initial.revision,"theme":"dark"})
        )
        .is_err()
    );
    runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn settings_events_are_vault_scoped_and_ignore_device_only_writes() {
    let vault = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let operations = tempfile::tempdir().unwrap();
    let runtime = runtime(vault.path(), cache.path(), operations.path()).await;
    let cursor = runtime
        .context
        .events
        .publish(hypr_loofah_protocol::Change::SearchReady)
        .cursor;
    let mut events = runtime.context.events.subscribe(Some(&cursor));
    runtime
        .context
        .settings
        .set_values(std::collections::HashMap::from([(
            "theme".into(),
            json!("dark"),
        )]))
        .await
        .unwrap();
    // A subsequent processing write coalesces with any pending device-only update.
    runtime
        .context
        .settings
        .set_values(std::collections::HashMap::from([(
            "ai_language".into(),
            json!("nl"),
        )]))
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let event = events.recv().await.unwrap();
            if event.change == hypr_loofah_protocol::Change::ProcessingSettingsChanged {
                assert_eq!(event.vault_id, "settings-vault");
                break;
            }
        }
    })
    .await
    .unwrap();
    runtime.shutdown().await.unwrap();
}

#[test]
fn local_policy_preserves_custom_providers_and_subscription_without_account_setup() {
    let policy = local_processing_policy();
    let mut settings = snapshot(&AppConfig::default()).settings;
    for (id, model, base_url) in [
        ("ollama", "my-local-model", ""),
        ("custom", "private-model", "http://192.168.1.4:9000/v1"),
        ("chatgpt_subscription", "account-model", ""),
    ] {
        settings.summary_model = Some(SummaryModelSelection {
            provider_id: id.into(),
            model_id: model.into(),
            base_url: base_url.into(),
        });
        validate_settings(&settings, &policy).unwrap();
    }
    settings.summary_model.as_mut().unwrap().base_url = "https://redirect.example".into();
    assert_eq!(
        validate_settings(&settings, &policy).unwrap_err().code,
        ErrorCode::Validation
    );
}
