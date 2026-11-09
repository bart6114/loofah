use super::*;
use hypr_loofah_core::transcription_jobs::{
    PreparedTranscription, TranscriptionEngine, TranscriptionFuture, TranscriptionInput,
};
use sha2::{Digest, Sha256};
use std::{
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

const AUDIO: &[u8] = b"Synthetic server recording fixture";

struct Engine {
    calls: AtomicUsize,
    entered: Semaphore,
    finish: Semaphore,
}
impl Engine {
    fn new(permits: usize) -> Arc<Self> {
        Arc::new(Self {
            calls: AtomicUsize::new(0),
            entered: Semaphore::new(0),
            finish: Semaphore::new(permits),
        })
    }
    async fn entered(&self) {
        tokio::time::timeout(Duration::from_secs(10), self.entered.acquire())
            .await
            .unwrap()
            .unwrap()
            .forget();
    }
}
struct Prepared(Arc<Engine>);
struct Fake(Arc<Engine>);
impl TranscriptionEngine for Fake {
    fn prepare<'a>(
        &'a self,
        request: &'a TranscriptionJobRequest,
        _: CancellationToken,
    ) -> TranscriptionFuture<'a, Result<Box<dyn PreparedTranscription>, TranscriptionFailure>> {
        Box::pin(async move {
            if request.model != "test-cpu" {
                return Err(TranscriptionFailure::EngineUnavailable);
            }
            Ok(Box::new(Prepared(self.0.clone())) as Box<dyn PreparedTranscription>)
        })
    }
}
impl PreparedTranscription for Prepared {
    fn transcribe(
        self: Box<Self>,
        input: TranscriptionInput,
        cancel: CancellationToken,
    ) -> TranscriptionFuture<'static, Result<TranscriptWithData, TranscriptionFailure>> {
        Box::pin(async move {
            let (path, owner) = input.source.into_parts();
            assert_eq!(tokio::fs::read(path).await.unwrap(), AUDIO);
            self.0.calls.fetch_add(1, Ordering::SeqCst);
            self.0.entered.add_permits(1);
            tokio::select! {
                biased;
                _ = cancel.cancelled() => return Err(TranscriptionFailure::Interrupted),
                permit = self.0.finish.acquire() => permit.unwrap().forget(),
            }
            let mut transcript = input.metadata.empty_transcript();
            transcript.words = serde_json::from_value(json!([{
                "id":"word-1","text":" Authenticated words.","start_ms":0.5,"end_ms":125.5,
                "channel":2,"metadata":{"future":{"preserve":[true,42]}}
            }]))
            .unwrap();
            transcript.speaker_hints = serde_json::from_value(json!([{
                "id":"hint-1","word_id":"word-1","type":"speaker_label","value":"Self"
            }]))
            .unwrap();
            drop(owner);
            Ok(transcript)
        })
    }
}

async fn fixture(permits: usize) -> (Fixture, Arc<Engine>) {
    let fixture = Fixture::new().await;
    let engine = Engine::new(permits);
    assert!(
        fixture
            .service
            .transcription_test_engine
            .set(Arc::new(Fake(engine.clone())))
            .is_ok()
    );
    (fixture, engine)
}
async fn seed(fixture: &Fixture, vault: &str) -> Arc<hypr_loofah_core::VaultContext> {
    let context = fixture.service.vault(vault).await.unwrap();
    let meta = serde_json::from_value(json!({
        "id":"note","title":"User title","created_at":"2026-11-08T10:00:00Z",
        "audio":{"source":"recording","layout":"mic_system"},"tags":[]
    }))
    .unwrap();
    context
        .store
        .create_note_session(&meta, "Keep canonical user notes.")
        .await
        .unwrap();
    let directory = context.store.vault_base().join("sessions/note");
    tokio::fs::write(directory.join("audio.wav"), AUDIO)
        .await
        .unwrap();
    tokio::fs::write(directory.join("unknown.user-file"), "Keep attachment")
        .await
        .unwrap();
    context
}
fn input() -> Value {
    json!({"session_id":"note","expected_recording_sha256":Sha256::digest(AUDIO).iter().map(|byte|format!("{byte:02x}")).collect::<String>(),
        "expected_transcript":"absent","model":"test-cpu","languages":["en"],"keywords":["Loofah"]})
}
fn path(vault: &str) -> String {
    format!("/api/v1/vaults/{vault}/transcription/jobs")
}
async fn terminal(
    fixture: &Fixture,
    subject: &str,
    base: &str,
    id: &str,
) -> TranscriptionJobSnapshot {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let (status, body) = fixture
                .call(subject, "GET", &format!("{base}/{id}"), None)
                .await;
            assert_eq!(status, StatusCode::OK, "{body}");
            let job: TranscriptionJobSnapshot = serde_json::from_value(body).unwrap();
            if job.state.is_terminal() {
                return job;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap()
}
async fn submit(
    fixture: &Fixture,
    subject: &str,
    base: &str,
    request: &str,
) -> TranscriptionJobSnapshot {
    let (status, body) = fixture
        .call_token(
            &fixture.token(subject),
            "POST",
            base,
            Some(input()),
            Some(request),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    serde_json::from_value(body).unwrap()
}
async fn role(fixture: &Fixture, vault: &str, member: &str, role: &str) {
    let (_, current) = fixture
        .call("alice", "GET", &format!("/api/v1/vaults/{vault}"), None)
        .await;
    let (status, body) = fixture
        .call(
            "alice",
            "PUT",
            &format!("/api/v1/vaults/{vault}/members/{member}"),
            Some(json!({"role":role,"expected_revision":current["revision"]})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

#[tokio::test]
async fn authenticated_batch_replay_binds_private_result_and_never_claims_current_canonical_bytes()
{
    let (fixture, engine) = fixture(1).await;
    let alice = fixture.login("alice").await;
    let bob = fixture.login("bob").await;
    let (_, shared) = fixture
        .call(
            "alice",
            "POST",
            "/api/v1/vaults",
            Some(json!({"name":"Batch"})),
        )
        .await;
    let vault = shared["id"].as_str().unwrap();
    role(&fixture, vault, &bob.member.id, "editor").await;
    let context = seed(&fixture, vault).await;
    let base = path(vault);
    let request = uuid::Uuid::new_v4().to_string();
    let job = submit(&fixture, "alice", &base, &request).await;
    assert_eq!(
        terminal(&fixture, "alice", &base, &job.id).await.state,
        TranscriptionJobState::Succeeded
    );
    assert_eq!(submit(&fixture, "alice", &base, &request).await.id, job.id);
    assert_eq!(engine.calls.load(Ordering::SeqCst), 1);
    let mut changed = input();
    changed["keywords"] = json!(["different"]);
    assert_eq!(
        fixture
            .call_token(
                &fixture.token("alice"),
                "POST",
                &base,
                Some(changed),
                Some(&request)
            )
            .await
            .0,
        StatusCode::CONFLICT
    );
    let (_, result) = fixture
        .call("alice", "GET", &format!("{base}/{}/result", job.id), None)
        .await;
    let result: TranscriptionJobResult = serde_json::from_value(result).unwrap();
    assert_eq!(result.job_id, job.id);
    assert_eq!(result.session_id, "note");
    assert_eq!(result.transcript.session_id, "note");
    assert_eq!(result.transcript.user_id, alice.member.id);
    assert_eq!(
        result.transcript.words[0].metadata.as_ref().unwrap()["future"]["preserve"],
        json!([true, 42])
    );
    let (_, canonical) = fixture
        .call(
            "alice",
            "GET",
            &format!("/api/v1/vaults/{vault}/sessions/note/transcripts"),
            None,
        )
        .await;
    assert_eq!(canonical, json!([result.transcript]));
    for (method, suffix) in [
        ("GET", ""),
        ("GET", "/result"),
        ("POST", "/cancel"),
        ("POST", "/resume"),
    ] {
        assert_eq!(
            fixture
                .call("bob", method, &format!("{base}/{}{suffix}", job.id), None)
                .await
                .0,
            StatusCode::NOT_FOUND
        );
    }
    let (_, page) = fixture.call("bob", "GET", &base, None).await;
    assert_eq!(page["jobs"], json!([]));
    assert_eq!(
        fixture
            .call(
                "bob",
                "GET",
                &format!("{}/{}/result", path(&alice.personal_vault_id), job.id),
                None
            )
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    let file = context
        .store
        .vault_base()
        .join("sessions/note/transcript.json");
    tokio::fs::write(&file, b"{\"transcripts\":[],\"user_future\":true}")
        .await
        .unwrap();
    let replacement = tokio::fs::read(&file).await.unwrap();
    assert_eq!(submit(&fixture, "alice", &base, &request).await.id, job.id);
    let (_, historical) = fixture
        .call("alice", "GET", &format!("{base}/{}/result", job.id), None)
        .await;
    assert_eq!(historical["job_id"], job.id);
    assert_eq!(tokio::fs::read(&file).await.unwrap(), replacement);
    let (_, current) = fixture
        .call(
            "alice",
            "GET",
            &format!("/api/v1/vaults/{vault}/sessions/note/transcripts"),
            None,
        )
        .await;
    assert_eq!(current, json!([]));
    assert_eq!(
        tokio::fs::read_to_string(
            context
                .store
                .vault_base()
                .join("sessions/note/unknown.user-file")
        )
        .await
        .unwrap(),
        "Keep attachment"
    );
    fixture.service.shutdown().await.unwrap();
}

#[tokio::test]
async fn downgrade_cancels_actual_job_but_reader_can_read_and_cancel_own_history() {
    let (fixture, engine) = fixture(0).await;
    fixture.login("alice").await;
    let bob = fixture.login("bob").await;
    let (_, shared) = fixture
        .call(
            "alice",
            "POST",
            "/api/v1/vaults",
            Some(json!({"name":"Roles"})),
        )
        .await;
    let vault = shared["id"].as_str().unwrap();
    role(&fixture, vault, &bob.member.id, "editor").await;
    let context = seed(&fixture, vault).await;
    let base = path(vault);
    let job = submit(&fixture, "bob", &base, &uuid::Uuid::new_v4().to_string()).await;
    engine.entered().await;
    role(&fixture, vault, &bob.member.id, "reader").await;
    let cancelled = terminal(&fixture, "bob", &base, &job.id).await;
    assert_eq!(cancelled.state, TranscriptionJobState::Cancelled);
    assert_eq!(cancelled.failure, Some(TranscriptionFailure::AccessDenied));
    assert_eq!(
        fixture
            .call("bob", "POST", &format!("{base}/{}/cancel", job.id), None)
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        fixture
            .call("bob", "GET", &format!("{base}/{}/result", job.id), None)
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        fixture.call("bob", "POST", &base, Some(input())).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        fixture
            .call("bob", "POST", &format!("{base}/{}/resume", job.id), None)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert!(
        !context
            .store
            .vault_base()
            .join("sessions/note/transcript.json")
            .exists()
    );
    assert_eq!(engine.calls.load(Ordering::SeqCst), 1);
    fixture.service.shutdown().await.unwrap();
}

#[tokio::test]
async fn strict_batch_targets_and_options_fail_before_any_inference() {
    let (fixture, engine) = fixture(1).await;
    let alice = fixture.login("alice").await;
    seed(&fixture, &alice.personal_vault_id).await;
    let base = path(&alice.personal_vault_id);
    for (field, value) in [
        ("session_id", json!("../foreign")),
        ("expected_transcript", json!("replace")),
        ("expected_recording_sha256", json!("wrong")),
        ("file_path", json!("/private/canonical")),
    ] {
        let mut invalid = input();
        invalid[field] = value;
        assert_eq!(
            fixture.call("alice", "POST", &base, Some(invalid)).await.0,
            StatusCode::BAD_REQUEST
        );
    }
    for suffix in [
        "?limit=0",
        "?limit=201",
        "?after=invalid",
        "?future=unknown",
    ] {
        assert_eq!(
            fixture
                .call("alice", "GET", &format!("{base}{suffix}"), None)
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
    }
    for suffix in ["not-a-uuid", "00000000-0000-0000-0000-000000000000"] {
        assert_eq!(
            fixture
                .call("alice", "GET", &format!("{base}/{suffix}"), None)
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(engine.calls.load(Ordering::SeqCst), 0);
    fixture.service.shutdown().await.unwrap();
}

#[tokio::test]
async fn concurrent_transcript_conflict_retains_exact_result_and_resume_never_repeats_inference() {
    let (fixture, engine) = fixture(0).await;
    let alice = fixture.login("alice").await;
    let context = seed(&fixture, &alice.personal_vault_id).await;
    let base = path(&alice.personal_vault_id);
    let job = submit(&fixture, "alice", &base, &uuid::Uuid::new_v4().to_string()).await;
    engine.entered().await;
    let file = context
        .store
        .vault_base()
        .join("sessions/note/transcript.json");
    let user = b"{\"transcripts\":[],\"unknown\":{\"preserve\":true}}";
    tokio::fs::write(&file, user).await.unwrap();
    engine.finish.add_permits(1);
    assert_eq!(
        terminal(&fixture, "alice", &base, &job.id).await.state,
        TranscriptionJobState::Conflict
    );
    let (_, before) = fixture
        .call("alice", "GET", &format!("{base}/{}/result", job.id), None)
        .await;
    assert_eq!(before["job_id"], job.id);
    assert_eq!(
        fixture
            .call("alice", "POST", &format!("{base}/{}/resume", job.id), None)
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        terminal(&fixture, "alice", &base, &job.id).await.state,
        TranscriptionJobState::Conflict
    );
    let (_, after) = fixture
        .call("alice", "GET", &format!("{base}/{}/result", job.id), None)
        .await;
    assert_eq!(before, after);
    assert_eq!(engine.calls.load(Ordering::SeqCst), 1);
    assert_eq!(tokio::fs::read(&file).await.unwrap(), user);
    fixture.service.shutdown().await.unwrap();
}

#[tokio::test]
async fn http_waiter_disconnect_does_not_cancel_admitted_job_or_make_replay_pay_twice() {
    let (fixture, engine) = fixture(0).await;
    let alice = fixture.login("alice").await;
    seed(&fixture, &alice.personal_vault_id).await;
    let base = path(&alice.personal_vault_id);
    let request = uuid::Uuid::new_v4().to_string();
    let (origin, task) = fixture.tcp().await;
    let response = reqwest::Client::new()
        .post(format!("{origin}{base}"))
        .bearer_auth(fixture.token("alice"))
        .header("x-loofah-api-major", "1")
        .header("x-request-id", &request)
        .json(&input())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    drop(response);
    engine.entered().await;
    let replay = submit(&fixture, "alice", &base, &request).await;
    engine.finish.add_permits(1);
    assert_eq!(
        terminal(&fixture, "alice", &base, &replay.id).await.state,
        TranscriptionJobState::Succeeded
    );
    assert_eq!(engine.calls.load(Ordering::SeqCst), 1);
    fixture.service.shutdown().await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn host_stop_signals_interruption_without_manufacturing_member_revocation() {
    use hypr_loofah_core::transcription_jobs::{TranscriptionAuthorizer, TranscriptionWork};
    let fixture = Fixture::new().await;
    let alice = fixture.login("alice").await;
    let stop = CancellationToken::new();
    let authorizer = crate::transcription_authorization::CatalogTranscriptionAuthorizer::new(
        fixture.service.catalog.clone(),
        alice.personal_vault_id,
        stop.clone(),
    );
    let work = TranscriptionWork {
        job_id: uuid::Uuid::new_v4().to_string(),
        principal: alice.member.id,
        request: serde_json::from_value(input()).unwrap(),
    };
    let access = authorizer.observe(&work).await.unwrap();
    stop.cancel();
    tokio::time::timeout(Duration::from_secs(5), access.interrupted.cancelled())
        .await
        .unwrap();
    assert!(!access.cancelled.is_cancelled());
    assert!(matches!(
        authorizer.authorize(&work).await,
        Err(TranscriptionFailure::Interrupted)
    ));
    drop(access);
    fixture.service.shutdown().await.unwrap();
}
