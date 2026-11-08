use super::*;
use axum::Extension;

const REV: &str = "6114bd31-62da-4d90-9a0d-41b9a8872210";
struct Timeline {
    mode: AtomicUsize,
}
impl Timeline {
    fn routes(self: &Arc<Self>) -> Router<Arc<Provider>> {
        Router::new()
            .route("/api/v1/vaults/test/timeline/headers", post(headers))
            .route("/api/v1/vaults/test/timeline/facets", post(facets))
            .layer(Extension(self.clone()))
    }
}
fn header(id: &str) -> Value {
    json!({"header":{"id":id,"title":"same","created_at":"2026-01-01T00:00:00Z","folder":null,"tags":["a/b"],"author":" Raw ","has_transcript_words":true},"timestamp_ms":1767225600000_i64})
}
async fn headers(
    Extension(state): Extension<Arc<Timeline>>,
    headers: HeaderMap,
    Json(input): Json<TimelineQuery>,
) -> axum::response::Response {
    assert!(headers.contains_key("authorization"));
    assert_eq!(headers["x-loofah-api-major"], "1");
    if state.mode.load(Ordering::SeqCst) == 16 {
        let stream = futures_util::stream::once(async {
            Err::<axum::body::Bytes, std::io::Error>(std::io::Error::other("read reply lost"))
        });
        return axum::response::Response::builder()
            .header("content-type", "application/json")
            .body(axum::body::Body::from_stream(stream))
            .unwrap();
    }
    let mut page = json!({"revision":REV,"items":[header("one")],"next_cursor":null,"total_count":1,"future_count":0,"undated_count":0});
    match state.mode.load(Ordering::SeqCst) {
        1 => page["revision"] = json!("wrong"),
        2 => page["items"][0]["header"]["id"] = json!("../private"),
        3 => page["items"][0]["timestamp_ms"] = json!(TIMELINE_DATE_LIMIT + 1),
        4 => page["items"] = json!([header("one"), header("one")]),
        5 => page["total_count"] = json!(0),
        6 => page["next_cursor"] = json!("next"),
        7 => page["items"][0]["header"]["server_path"] = json!("/private"),
        8 => page["items"][0]["header"]["tags"] = json!(["wrong"]),
        9 => page["items"][0]["header"]["title"] = json!("x".repeat(TIMELINE_RESPONSE_LIMIT)),
        _ => {}
    }
    if matches!(input.view, TimelineView::Untagged {}) {
        page["items"][0]["header"]["tags"] = json!([]);
    }
    Json(page).into_response()
}
async fn facets(Extension(state): Extension<Arc<Timeline>>, headers: HeaderMap) -> Json<Value> {
    assert!(headers.contains_key("authorization"));
    let mut facets = json!({"revision":REV,"nodes":[{"path":"a","parent_path":null,"label":"a","direct_count":0,"subtree_count":1},{"path":"a/b","parent_path":"a","label":"b","direct_count":1,"subtree_count":1}],"untagged_count":0});
    match state.mode.load(Ordering::SeqCst) {
        10 => facets["nodes"][1]["parent_path"] = json!("wrong"),
        11 => facets["nodes"][1]["subtree_count"] = json!(0),
        12 => facets["nodes"][1]["label"] = json!("wrong"),
        13 => facets["nodes"][1]["path"] = json!("a"),
        14 => facets["nodes"] = json!([facets["nodes"][1].clone()]),
        15 => facets["revision"] = json!("00000000-0000-0000-0000-000000000000"),
        _ => {}
    }
    Json(facets)
}
fn query() -> TimelineQuery {
    TimelineQuery {
        view: TimelineView::Tag { path: "a/b".into() },
        limit: 200,
        cursor: None,
        expected_revision: Some(REV.into()),
    }
}
fn operations() -> Vec<RemoteOperation> {
    vec![
        RemoteOperation::TimelineHeaders {
            vault_id: "test".into(),
            input: query(),
        },
        RemoteOperation::TimelineFacets {
            vault_id: "test".into(),
            input: TimelineFacetsQuery {
                expected_revision: Some(REV.into()),
            },
        },
    ]
}
async fn execute(client: &RemoteClient, operation: RemoteOperation) -> Result<RemoteReply> {
    client
        .execute(
            operation,
            &uuid::Uuid::new_v4().to_string(),
            CancellationToken::new(),
        )
        .await
}

#[tokio::test]
async fn timeline_sdk_gates_and_validates_before_loading_credentials() {
    let state = Arc::new(Timeline {
        mode: AtomicUsize::new(0),
    });
    let fixture = Fixture::with_extra_routes(None, state.routes()).await;
    let mut client = fixture.client().await;
    let loads = fixture.credentials.loads.load(Ordering::SeqCst);
    for operation in operations() {
        assert_eq!(
            execute(&client, operation).await.unwrap_err().code,
            ErrorCode::IncompatibleVersion
        );
    }
    client.capabilities.features.push("timeline_read".into());
    let mut oversized = query();
    oversized.limit = 201;
    let mut bad_revision = query();
    bad_revision.expected_revision = Some("../private".into());
    let mut bad_cursor = query();
    bad_cursor.cursor = Some("x".repeat(TIMELINE_CURSOR_LIMIT + 1));
    for input in [oversized, bad_revision, bad_cursor] {
        assert_eq!(
            execute(
                &client,
                RemoteOperation::TimelineHeaders {
                    vault_id: "test".into(),
                    input
                }
            )
            .await
            .unwrap_err()
            .code,
            ErrorCode::Validation
        );
    }
    let mut bad = query();
    bad.view = TimelineView::Tag {
        path: "a//b".into(),
    };
    assert_eq!(
        execute(
            &client,
            RemoteOperation::TimelineHeaders {
                vault_id: "test".into(),
                input: bad
            }
        )
        .await
        .unwrap_err()
        .code,
        ErrorCode::Validation
    );
    assert_eq!(fixture.credentials.loads.load(Ordering::SeqCst), loads);
}

#[tokio::test]
async fn timeline_sdk_rejects_malformed_unbounded_cross_filter_and_mixed_revision_replies() {
    let state = Arc::new(Timeline {
        mode: AtomicUsize::new(0),
    });
    let fixture = Fixture::with_extra_routes(None, state.routes()).await;
    let mut client = fixture.client().await;
    client.capabilities.features.push("timeline_read".into());
    fixture.login(&client).await;
    for operation in operations() {
        execute(&client, operation).await.unwrap();
    }
    let mut upcoming = query();
    upcoming.view = TimelineView::Upcoming {
        after_ms: 0,
        before_ms: None,
    };
    execute(
        &client,
        RemoteOperation::TimelineHeaders {
            vault_id: "test".into(),
            input: upcoming.clone(),
        },
    )
    .await
    .unwrap();
    upcoming.view = TimelineView::Upcoming {
        after_ms: 1767225600000,
        before_ms: None,
    };
    assert!(
        execute(
            &client,
            RemoteOperation::TimelineHeaders {
                vault_id: "test".into(),
                input: upcoming
            }
        )
        .await
        .is_err()
    );
    for mode in 1..=15 {
        state.mode.store(mode, Ordering::SeqCst);
        let operation = if mode < 10 {
            operations().remove(0)
        } else {
            operations().remove(1)
        };
        assert!(execute(&client, operation).await.is_err(), "mode {mode}");
    }
}

#[tokio::test]
async fn timeline_sdk_trusted_https_roundtrips_at_50_150_500ms_without_mutation_outcomes() {
    for millis in [50, 150, 500] {
        let state = Arc::new(Timeline {
            mode: AtomicUsize::new(0),
        });
        let fixture =
            Fixture::with_extra_routes(Some(Duration::from_millis(millis)), state.routes()).await;
        let mut client = fixture.client().await;
        client.capabilities.features.push("timeline_read".into());
        fixture.login(&client).await;
        for operation in operations() {
            execute(&client, operation).await.unwrap();
        }
        assert!(client.address.as_str().starts_with("https://"));
        let cancel = CancellationToken::new();
        cancel.cancel();
        let error = client
            .execute(
                operations().remove(0),
                &uuid::Uuid::new_v4().to_string(),
                cancel,
            )
            .await
            .unwrap_err();
        assert!(error.mutation_outcome.is_none());
    }
}

#[tokio::test]
async fn lost_timeline_reply_is_a_retryable_read_without_unknown_mutation_state() {
    let state = Arc::new(Timeline {
        mode: AtomicUsize::new(16),
    });
    let fixture = Fixture::with_extra_routes(None, state.routes()).await;
    let mut client = fixture.client().await;
    client.capabilities.features.push("timeline_read".into());
    fixture.login(&client).await;
    let error = execute(&client, operations().remove(0)).await.unwrap_err();
    assert!(error.mutation_outcome.is_none());
    state.mode.store(0, Ordering::SeqCst);
    assert!(matches!(
        execute(&client, operations().remove(0)).await.unwrap(),
        RemoteReply::TimelineHeaders(_)
    ));
}
