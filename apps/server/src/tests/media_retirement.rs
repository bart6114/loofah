use super::*;

fn decision(status: &Value, disposition: &str) -> Value {
    json!({
        "retirement_id": uuid::Uuid::new_v4().to_string(),
        "expected_revision": status["revision"],
        "expected_declaration_sha256": status["declaration_sha256"],
        "expected_source": status["source"],
        "disposition": disposition,
    })
}

async fn retire(
    fixture: &Fixture,
    subject: &str,
    path: &str,
    input: &Value,
) -> (StatusCode, Value) {
    fixture
        .call_token(
            &fixture.token(subject),
            "POST",
            path,
            Some(input.clone()),
            input["retirement_id"].as_str(),
        )
        .await
}

async fn wait_succeeded(fixture: &Fixture, path: &str) -> Value {
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            let (code, status) = fixture.call("alice", "GET", path, None).await;
            assert_eq!(code, StatusCode::OK, "{status}");
            if status["phase"] != "running" {
                assert_eq!(status["phase"], "succeeded", "{status}");
                return status;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn media_retirement_http_keeps_results_and_original_identity_after_restart() {
    let fixture = Fixture::new().await;
    let alice = fixture.login("alice").await;
    let vault = alice.personal_vault_id;
    let request = uuid::Uuid::new_v4().to_string();
    let admission = media_jobs::declare_subtitle(&fixture, &vault, &request).await;
    let path = format!("/api/v1/vaults/{vault}/media/requests/{request}");
    let retirement_path = format!("{path}/retirement");
    let (code, uploaded) = media_jobs::upload_bytes(
        &fixture,
        "alice",
        &vault,
        &request,
        admission["revision"].as_str().unwrap(),
        media_jobs::SUBTITLE.as_bytes().to_vec(),
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{uploaded}");
    let terminal = wait_succeeded(&fixture, &path).await;
    let (code, result) = fixture
        .call("alice", "GET", &format!("{path}/result"), None)
        .await;
    assert_eq!(code, StatusCode::OK, "{result}");
    assert!(!result.is_null());

    let directory = fixture
        .service
        .vault(&vault)
        .await
        .unwrap()
        .store
        .vault_base()
        .join("sessions/note");
    std::fs::write(directory.join("notes.md"), b"Keep my original note").unwrap();
    std::fs::write(directory.join("unknown.bin"), b"Keep this attachment").unwrap();
    std::fs::write(directory.join(".private"), b"Keep this hidden file").unwrap();
    let canonical: Vec<_> = [
        "_meta.json",
        "transcript.json",
        "notes.md",
        "unknown.bin",
        ".private",
    ]
    .into_iter()
    .map(|name| (name, std::fs::read(directory.join(name)).unwrap()))
    .collect();

    let response = fixture
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("{path}/source"))
                .header(
                    "authorization",
                    format!("Bearer {}", fixture.token("alice")),
                )
                .header("x-loofah-api-major", "1")
                .header(
                    "x-loofah-media-revision",
                    admission["revision"].as_str().unwrap(),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()["x-loofah-media-sha256"]
            .to_str()
            .unwrap(),
        admission["declaration"]["operation"]["source"]["sha256"]
            .as_str()
            .unwrap()
    );
    assert_eq!(
        to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap()
            .as_ref(),
        media_jobs::SUBTITLE.as_bytes()
    );

    let (code, eligible) = fixture.call("alice", "GET", &retirement_path, None).await;
    assert_eq!(code, StatusCode::OK, "{eligible}");
    assert_eq!(eligible["state"], "eligible");
    assert_eq!(eligible["quota_released"], false);
    assert!(
        eligible["allowed_dispositions"]
            .as_array()
            .unwrap()
            .contains(&json!("exported"))
    );
    let input = decision(&eligible, "exported");
    let mut stale = input.clone();
    stale["expected_revision"] = json!(uuid::Uuid::new_v4().to_string());
    assert_eq!(
        retire(&fixture, "alice", &retirement_path, &stale).await.0,
        StatusCode::CONFLICT
    );
    let (code, retired) = retire(&fixture, "alice", &retirement_path, &input).await;
    assert_eq!(code, StatusCode::OK, "{retired}");
    assert_eq!(retired["state"], "retired");
    assert_eq!(retired["quota_released"], true);
    assert_eq!(retired["retirement"], input);
    assert_eq!(
        retire(&fixture, "alice", &retirement_path, &input).await,
        (StatusCode::OK, retired.clone())
    );
    assert_eq!(
        fixture.call("alice", "GET", &path, None).await,
        (StatusCode::OK, terminal.clone())
    );
    assert_eq!(
        fixture
            .call("alice", "GET", &format!("{path}/result"), None)
            .await,
        (StatusCode::OK, result.clone())
    );
    assert_eq!(
        fixture
            .call("alice", "POST", &format!("{path}/recover"), None)
            .await
            .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        fixture
            .call(
                "alice",
                "GET",
                &format!("/api/v1/vaults/{vault}/media/requests"),
                None
            )
            .await,
        (StatusCode::OK, json!([]))
    );

    let fixture = fixture.reopen().await;
    assert_eq!(
        fixture.call("alice", "GET", &retirement_path, None).await,
        (StatusCode::OK, retired.clone())
    );
    assert_eq!(
        retire(&fixture, "alice", &retirement_path, &input).await,
        (StatusCode::OK, retired)
    );
    assert_eq!(
        fixture.call("alice", "GET", &path, None).await,
        (StatusCode::OK, terminal)
    );
    assert_eq!(
        fixture
            .call("alice", "GET", &format!("{path}/result"), None)
            .await,
        (StatusCode::OK, result)
    );
    let declaration = admission["declaration"].clone();
    let declare_path = format!("/api/v1/vaults/{vault}/media/requests");
    let (code, replay) = fixture
        .call_token(
            &fixture.token("alice"),
            "POST",
            &declare_path,
            Some(declaration.clone()),
            Some(&request),
        )
        .await;
    assert_eq!(code, StatusCode::OK, "{replay}");
    assert_eq!(replay["revision"], admission["revision"]);
    let mut changed = declaration;
    changed["operation"]["filename"] = json!("changed.srt");
    assert_eq!(
        fixture
            .call_token(
                &fixture.token("alice"),
                "POST",
                &declare_path,
                Some(changed),
                Some(&request)
            )
            .await
            .0,
        StatusCode::CONFLICT
    );
    for (name, bytes) in canonical {
        assert_eq!(
            std::fs::read(directory.join(name)).unwrap(),
            bytes,
            "{name}"
        );
    }
    fixture.service.shutdown().await.unwrap();
}

#[tokio::test]
async fn media_retirement_http_never_dispatched_discard_does_not_create_content() {
    let fixture = Fixture::new().await;
    let alice = fixture.login("alice").await;
    let vault = alice.personal_vault_id;
    let request = uuid::Uuid::new_v4().to_string();
    media_jobs::declare_subtitle(&fixture, &vault, &request).await;
    let path = format!("/api/v1/vaults/{vault}/media/requests/{request}");
    let retirement_path = format!("{path}/retirement");
    let (code, status) = fixture.call("alice", "GET", &retirement_path, None).await;
    assert_eq!(code, StatusCode::OK, "{status}");
    assert_eq!(status["state"], "eligible");
    assert!(
        !status["allowed_dispositions"]
            .as_array()
            .unwrap()
            .contains(&json!("exported"))
    );
    let input = decision(&status, "discard");
    let (code, retired) = retire(&fixture, "alice", &retirement_path, &input).await;
    assert_eq!(code, StatusCode::OK, "{retired}");
    assert_eq!(retired["state"], "retired");
    assert_eq!(retired["quota_released"], true);
    let context = fixture.service.vault(&vault).await.unwrap();
    assert!(
        !context
            .store
            .vault_base()
            .join("sessions/note/transcript.json")
            .exists()
    );
    assert!(
        !context
            .store
            .vault_base()
            .join("sessions/note/audio.wav")
            .exists()
    );
    assert_eq!(
        fixture
            .call("alice", "POST", &format!("{path}/recover"), None)
            .await
            .0,
        StatusCode::CONFLICT
    );
    fixture.service.shutdown().await.unwrap();
}

#[tokio::test]
async fn media_retirement_http_rejects_reader_foreign_member_and_changed_decisions() {
    let fixture = Fixture::new().await;
    let alice = fixture.login("alice").await;
    let bob = fixture.login("bob").await;
    let (_, shared) = fixture
        .call(
            "alice",
            "POST",
            "/api/v1/vaults",
            Some(json!({"name":"Retirement authorization"})),
        )
        .await;
    let vault = shared["id"].as_str().unwrap();
    let member = format!("/api/v1/vaults/{vault}/members/{}", bob.member.id);
    let (code, editor) = fixture
        .call(
            "alice",
            "PUT",
            &member,
            Some(json!({"role":"editor","expected_revision":shared["revision"]})),
        )
        .await;
    assert_eq!(code, StatusCode::OK);
    let request = uuid::Uuid::new_v4().to_string();
    media_jobs::declare_subtitle(&fixture, vault, &request).await;
    let path = format!("/api/v1/vaults/{vault}/media/requests/{request}/retirement");
    let (code, status) = fixture.call("alice", "GET", &path, None).await;
    assert_eq!(code, StatusCode::OK, "{status}");
    let input = decision(&status, "discard");
    assert_eq!(
        fixture.call("bob", "GET", &path, None).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        retire(&fixture, "bob", &path, &input).await.0,
        StatusCode::NOT_FOUND
    );
    let (code, _) = fixture
        .call(
            "alice",
            "PUT",
            &member,
            Some(json!({"role":"reader","expected_revision":editor["revision"]})),
        )
        .await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(
        fixture.call("bob", "GET", &path, None).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        retire(&fixture, "bob", &path, &input).await.0,
        StatusCode::FORBIDDEN
    );
    for other_vault in [alice.personal_vault_id, bob.personal_vault_id] {
        let foreign = format!("/api/v1/vaults/{other_vault}/media/requests/{request}/retirement");
        assert_eq!(
            retire(&fixture, "alice", &foreign, &input).await.0,
            StatusCode::NOT_FOUND
        );
    }
    let (code, rejected) = fixture
        .call_token(
            &fixture.token("alice"),
            "POST",
            &path,
            Some(input.clone()),
            Some(&uuid::Uuid::new_v4().to_string()),
        )
        .await;
    assert_eq!(code, StatusCode::BAD_REQUEST, "{rejected}");
    assert_eq!(rejected["mutation_outcome"], "not_committed");
    let mut unknown = input.clone();
    unknown["ignore_revision"] = json!(true);
    assert_eq!(
        retire(&fixture, "alice", &path, &unknown).await.0,
        StatusCode::BAD_REQUEST
    );
    let mut wrong_source = input.clone();
    wrong_source["expected_source"]["sha256"] = json!("0".repeat(64));
    assert_eq!(
        retire(&fixture, "alice", &path, &wrong_source).await.0,
        StatusCode::CONFLICT
    );
    let (code, retired) = retire(&fixture, "alice", &path, &input).await;
    assert_eq!(code, StatusCode::OK, "{retired}");
    let mut replacement = input;
    replacement["retirement_id"] = json!(uuid::Uuid::new_v4().to_string());
    assert_eq!(
        retire(&fixture, "alice", &path, &replacement).await.0,
        StatusCode::CONFLICT
    );
    fixture.service.shutdown().await.unwrap();
}
