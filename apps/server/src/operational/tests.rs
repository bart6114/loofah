use super::*;
use crate::tests::Fixture;
use axum::{
    body::{Body, to_bytes},
    http::Request,
};
use serde_json::{Value, json};
use tower::ServiceExt;

async fn sampled(fixture: &Fixture, vault: &str) -> Arc<VaultMetrics> {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(metrics) = fixture.service.observability.entry(vault) {
                let ready = metrics.observation.lock().unwrap().readiness == Readiness::Ready;
                if ready {
                    return metrics;
                }
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap()
}
#[tokio::test]
async fn operational_status_requires_owner_and_exposes_no_source_identifiers() {
    let fixture = Fixture::new().await;
    fixture.login("alice").await;
    let bob = fixture.login("bob").await;
    let (_, shared) = fixture
        .call(
            "alice",
            "POST",
            "/api/v1/vaults",
            Some(json!({"name":"PRIVATE VAULT CANARY"})),
        )
        .await;
    let vault = shared["id"].as_str().unwrap();
    fixture
        .call(
            "alice",
            "PUT",
            &format!("/api/v1/vaults/{vault}/members/{}", bob.member.id),
            Some(json!({"role":"editor","expected_revision":shared["revision"]})),
        )
        .await;
    let metrics = sampled(&fixture, vault).await;
    let path = format!("/api/v1/vaults/{vault}/operations");
    assert_eq!(
        fixture.call("bob", "GET", &path, None).await.0,
        StatusCode::FORBIDDEN
    );
    let (code, value) = fixture.call("alice", "GET", &path, None).await;
    assert_eq!(code, StatusCode::OK);
    let encoded = value.to_string();
    for forbidden in [
        vault,
        bob.member.id.as_str(),
        "PRIVATE VAULT CANARY",
        "sessions/",
        "vault_path",
        "api_key",
        "prompt",
        "member_id",
    ] {
        assert!(!encoded.contains(forbidden), "{forbidden}: {encoded}");
    }
    assert_eq!(value["observation"]["readiness"], "ready");
    let request = RequestObservation {
        metrics: fixture.service.observability.clone(),
        vault: vault.into(),
        mutation: true,
    };
    let result: Result<()> = REQUEST
        .scope(
            Some(request),
            crate::routes::admitted_write(&fixture.service, async {
                let mut error = ApiError::new(ErrorCode::Unavailable, "SECRET STORAGE PATH CANARY");
                error.0.mutation_outcome = Some(hypr_loofah_protocol::MutationOutcome::Unknown);
                Err(error)
            }),
        )
        .await;
    assert!(result.is_err());
    assert_eq!(metrics.write_failures.load(Ordering::Relaxed), 1);
    let (_, value) = fixture.call("alice", "GET", &path, None).await;
    assert_eq!(value["failed_writes"], 1);
    assert_eq!(value["unknown_write_outcomes"], 1);
    assert!(!value.to_string().contains("CANARY"));
    fixture.service.shutdown().await.unwrap();
}
#[tokio::test]
async fn event_reconnect_and_resync_are_counted_without_cursor_labels() {
    let fixture = Fixture::new().await;
    let me = fixture.login("alice").await;
    let metrics = sampled(&fixture, &me.personal_vault_id).await;
    let request = Request::builder()
        .uri(format!("/api/v1/vaults/{}/events", me.personal_vault_id))
        .header(
            "authorization",
            format!("Bearer {}", fixture.token("alice")),
        )
        .header("x-loofah-api-major", "1")
        .header("last-event-id", "PRIVATE-CURSOR-CANARY:1")
        .body(Body::empty())
        .unwrap();
    let response = fixture.app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body();
    use http_body_util::BodyExt;
    let _ = body.frame().await.unwrap().unwrap();
    drop(body);
    let value = serde_json::to_value(metrics.snapshot(StoragePressure::Normal)).unwrap();
    assert_eq!(value["event_subscriptions"], 1);
    assert_eq!(value["event_reconnects"], 1);
    assert_eq!(value["event_resyncs"], 1);
    assert!(!value.to_string().contains("CANARY"));
    fixture.service.shutdown().await.unwrap();
}
#[tokio::test]
async fn readiness_is_separate_from_liveness_and_catalog_changes_invalidate_cache() {
    let fixture = Fixture::new().await;
    let me = fixture.login("alice").await;
    sampled(&fixture, &me.personal_vault_id).await;
    async fn probe(fixture: &Fixture, path: &str) -> StatusCode {
        let response = fixture
            .app
            .clone()
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        assert!(to_bytes(response.into_body(), 32).await.unwrap().is_empty());
        status
    }
    tokio::time::timeout(Duration::from_secs(5), async {
        while probe(&fixture, "/ready").await != StatusCode::NO_CONTENT {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(probe(&fixture, "/health").await, StatusCode::NO_CONTENT);
    fixture
        .service
        .observability
        .ready
        .store(false, Ordering::Release);
    assert_eq!(
        probe(&fixture, "/ready").await,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(probe(&fixture, "/health").await, StatusCode::NO_CONTENT);
    fixture
        .service
        .observability
        .ready
        .store(true, Ordering::Release);
    fixture
        .service
        .observability
        .catalog_generation
        .store(u64::MAX, Ordering::Release);
    assert_eq!(
        probe(&fixture, "/ready").await,
        StatusCode::SERVICE_UNAVAILABLE
    );
    fixture.service.stop.cancel();
    assert_eq!(
        probe(&fixture, "/ready").await,
        StatusCode::SERVICE_UNAVAILABLE
    );
    fixture.service.shutdown().await.unwrap();
}
#[test]
fn samples_never_treat_missing_or_stale_observations_as_ready() {
    let metrics = Observability::default();
    assert!(!metrics.is_ready(0));
    assert!(!metrics.synchronize(&vec!["not-exposed".into(); MAX_VAULTS + 1]));
    let sample = VaultMetrics::default().snapshot(StoragePressure::Unavailable);
    let value: Value = serde_json::to_value(sample).unwrap();
    assert_eq!(value["sample_stale"], true);
    assert!(value["observation"]["durable"].is_null());
}

#[tokio::test]
async fn future_runtime_schema_keeps_liveness_but_fails_readiness_without_leaking_errors() {
    let fixture = Fixture::new().await;
    let me = fixture.login("alice").await;
    sampled(&fixture, &me.personal_vault_id).await;
    fixture.service.shutdown().await.unwrap();
    let root = tempfile::tempdir().unwrap();
    let mut config = fixture.service.config.clone();
    config.storage_root = root.path().join("storage");
    config.cache_root = root.path().join("cache");
    std::fs::create_dir_all(
        config
            .storage_root
            .join("vaults")
            .join(&me.personal_vault_id),
    )
    .unwrap();
    std::fs::copy(
        fixture.service.config.storage_root.join("catalog.json"),
        config.storage_root.join("catalog.json"),
    )
    .unwrap();
    let pending = config
        .storage_root
        .join("runtime")
        .join(&me.personal_vault_id)
        .join("creates/document-saves");
    std::fs::create_dir_all(&pending).unwrap();
    let path = pending.join(format!("{}.json", "a".repeat(64)));
    let bytes = br#"{"schema_version":999,"private":"PRIVATE-SOURCE-CANARY"}"#;
    std::fs::write(&path, bytes).unwrap();
    let service = Service::open(config).await.unwrap();
    let app = crate::routes::router(service.clone());
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let failed = service
                .observability
                .entry(&me.personal_vault_id)
                .is_some_and(|metrics| {
                    metrics.observation.lock().unwrap().readiness == Readiness::Failed
                });
            if failed {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    for (path, expected) in [
        ("/health", StatusCode::NO_CONTENT),
        ("/ready", StatusCode::SERVICE_UNAVAILABLE),
    ] {
        assert_eq!(
            app.clone()
                .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
                .await
                .unwrap()
                .status(),
            expected
        );
    }
    let response = app
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/api/v1/vaults/{}/operations",
                    me.personal_vault_id
                ))
                .header(
                    "authorization",
                    format!("Bearer {}", fixture.token("alice")),
                )
                .header("x-loofah-api-major", "1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
    let value: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(value["observation"]["readiness"], "failed");
    assert!(!String::from_utf8_lossy(&body).contains("CANARY"));
    assert_eq!(std::fs::read(path).unwrap(), bytes);
    // Shutdown reports the original failed runtime while still draining its owners.
    let _ = service.shutdown().await;
}
