use super::*;
use futures_util::StreamExt;
use hypr_loofah_core::capture_live::*;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use tokio::sync::{Notify, mpsc, watch};
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct Engine {
    calls: Arc<AtomicUsize>,
}
struct Control {
    spec: CaptureLiveEngineSpec,
    next: AtomicU64,
    count: AtomicU64,
    captions: std::sync::Mutex<Option<mpsc::Sender<CaptureLiveCaption>>>,
    finish: AtomicBool,
    wake: Notify,
    stop: CancellationToken,
}
impl CaptureLiveControl for Control {
    fn try_accept(
        &self,
        first: u64,
        pcm: &Arc<[f32]>,
    ) -> Result<CaptureLiveIngress, CaptureLiveFailure> {
        if self.stop.is_cancelled() || self.finish.load(Ordering::Acquire) {
            return Err(CaptureLiveFailure::Interrupted);
        }
        if first != self.next.load(Ordering::Acquire) {
            return Err(CaptureLiveFailure::SourceChanged);
        }
        let end = first + (pcm.len() / self.spec.format.channels.len()) as u64;
        for (physical, role) in self.spec.format.channels.iter().enumerate() {
            let sequence = self.count.fetch_add(1, Ordering::AcqRel);
            let word=serde_json::from_value(json!({"id":format!("word{sequence}"),"text":" hello","start_ms":first*1000/16000,"end_ms":end*1000/16000,"channel":if self.spec.format.channels.len()==1 {2}else{physical},"state":"final"})).unwrap();
            self.captions
                .lock()
                .unwrap()
                .as_ref()
                .ok_or(CaptureLiveFailure::Interrupted)?
                .try_send(CaptureLiveCaption {
                    generation_id: self.spec.generation_id.clone(),
                    sequence,
                    physical_channel: physical as u8,
                    role: *role,
                    new_words: vec![word],
                    partials: vec![],
                    replaced_ids: vec![],
                    languages: vec!["en".into()],
                })
                .map_err(|_| CaptureLiveFailure::Backlog)?;
        }
        self.next.store(end, Ordering::Release);
        Ok(CaptureLiveIngress::Accepted)
    }
    fn finish(&self, end: u64) -> Result<(), CaptureLiveFailure> {
        if end != self.next.load(Ordering::Acquire) {
            return Err(CaptureLiveFailure::SourceChanged);
        }
        self.finish.store(true, Ordering::Release);
        self.wake.notify_one();
        Ok(())
    }
    fn cancel(&self) {
        self.stop.cancel();
    }
}
impl CaptureLiveEngine for Engine {
    fn start(
        &self,
        spec: CaptureLiveEngineSpec,
        owner: Box<dyn Send>,
    ) -> hypr_loofah_core::transcription_jobs::TranscriptionFuture<
        '_,
        Result<CaptureLiveSession, CaptureLiveFailure>,
    > {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let (sender, captions) = mpsc::channel(16);
            let (done, completion) = watch::channel(None);
            let control = Arc::new(Control {
                spec,
                next: 0.into(),
                count: 0.into(),
                captions: std::sync::Mutex::new(Some(sender)),
                finish: false.into(),
                wake: Notify::new(),
                stop: CancellationToken::new(),
            });
            let worker = control.clone();
            tokio::spawn(async move {
                while !worker.finish.load(Ordering::Acquire) && !worker.stop.is_cancelled() {
                    tokio::select! {_=worker.stop.cancelled()=>{},_=worker.wake.notified()=>{}}
                }
                let interrupted = worker.stop.is_cancelled();
                let frames = worker.next.load(Ordering::Acquire);
                worker.captions.lock().unwrap().take();
                drop(owner);
                done.send_replace(Some(Arc::new(CaptureLiveEngineReceipt {
                    generation_id: worker.spec.generation_id.clone(),
                    admitted_end_frame: frames,
                    input_consumed_end_frame: frames,
                    completed_through_frame: (!interrupted).then_some(frames),
                    caption_count: worker.count.load(Ordering::Acquire),
                    failure: interrupted.then_some(CaptureLiveFailure::Interrupted),
                    native_joined: true,
                    owners_released: true,
                })));
            });
            Ok(CaptureLiveSession {
                control,
                captions,
                completion,
            })
        })
    }
}

async fn setup() -> (Fixture, Me, CaptureIngestRequest, Arc<Engine>) {
    let fixture = Fixture::new().await;
    let engine = Arc::new(Engine::default());
    assert!(
        fixture
            .service
            .capture_live_test_engine
            .set(engine.clone())
            .is_ok()
    );
    let alice = fixture.login("alice").await;
    let (_, shared) = fixture
        .call(
            "alice",
            "POST",
            "/api/v1/vaults",
            Some(json!({"name":"Live"})),
        )
        .await;
    let shared = shared["id"].as_str().unwrap().to_owned();
    let context = fixture.service.vault(&shared).await.unwrap();
    context
        .store
        .create_note_session(
            &serde_json::from_value(
                json!({"id":"note","title":"Live","created_at":"2026-11-08T00:00:00Z","tags":[]}),
            )
            .unwrap(),
            "Keep note",
        )
        .await
        .unwrap();
    let mut input = super::capture::input(&fixture, &alice);
    input.identity.scope.vault_id = shared;
    input.identity.target = CaptureTarget::ExistingSession {
        session_id: "note".into(),
        expected_audio_sha256: None,
    };
    assert_eq!(
        super::capture::begin(&fixture, "alice", &input).await.0,
        StatusCode::OK
    );
    let descriptor = super::capture::descriptor(&input, 0, 16000, None);
    assert_eq!(
        super::capture::append(
            &fixture,
            "alice",
            &input,
            &descriptor,
            super::capture::pcm(16000)
        )
        .await
        .0,
        StatusCode::OK
    );
    (fixture, alice, input, engine)
}
fn request(input: &CaptureIngestRequest) -> Value {
    json!({"capture_fingerprint":capture_fingerprint(&input.identity,&input.format),"expected_transcript":"absent","model":"test-live","languages":["en"],"keywords":[]})
}
fn path(input: &CaptureIngestRequest) -> String {
    format!(
        "/api/v1/vaults/{}/captures/{}/live",
        input.identity.scope.vault_id, input.identity.capture_id
    )
}
async fn start(fixture: &Fixture, input: &CaptureIngestRequest) -> (StatusCode, Value) {
    fixture
        .call_token(
            &fixture.token("alice"),
            "POST",
            &path(input),
            Some(request(input)),
            Some(&input.identity.request_id),
        )
        .await
}
async fn wait_state(
    fixture: &Fixture,
    input: &CaptureIngestRequest,
    predicate: impl Fn(&Value) -> bool,
) -> Value {
    let mut last = Value::Null;
    let result = tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            let (status, value) = fixture
                .call(
                    "alice",
                    "GET",
                    &format!("{}/{}", path(input), input.identity.request_id),
                    None,
                )
                .await;
            assert_eq!(status, StatusCode::OK, "{value}");
            last = value.clone();
            if predicate(&value) {
                return value;
            }
            tokio::task::yield_now().await;
        }
    })
    .await;
    result.unwrap_or_else(|_| panic!("Timed out waiting for live state; last snapshot: {last}"))
}

#[tokio::test]
async fn live_http_exact_replay_captions_precede_seal_and_canonical_publication_requires_recording()
{
    let (fixture, alice, input, engine) = setup().await;
    let started = start(&fixture, &input).await;
    assert_eq!(started.0, StatusCode::OK);
    assert_eq!(started.1["generation_id"], input.identity.request_id);
    let snapshot = wait_state(&fixture, &input, |value| {
        value["next_caption_sequence"].as_u64().unwrap() >= 2
    })
    .await;
    assert!(snapshot["completed_through_frame"].is_null());
    assert_eq!(snapshot["admitted_end_frame"], 16000);
    let context = fixture
        .service
        .vault(&input.identity.scope.vault_id)
        .await
        .unwrap();
    let directory = context.store.vault_base().join("sessions/note");
    std::fs::write(directory.join("private-user.mp3"), b"never touch").unwrap();
    assert!(!directory.join("transcript.json").exists());
    assert_eq!(start(&fixture, &input).await.0, StatusCode::OK);
    assert_eq!(engine.calls.load(Ordering::SeqCst), 1);
    let mut changed = request(&input);
    changed["keywords"] = json!(["changed"]);
    assert_eq!(
        fixture
            .call_token(
                &fixture.token("alice"),
                "POST",
                &path(&input),
                Some(changed),
                Some(&input.identity.request_id)
            )
            .await
            .0,
        StatusCode::CONFLICT
    );
    let pages = format!("{}/{}/captions", path(&input), input.identity.request_id);
    let (status, page) = fixture.call("alice", "GET", &pages, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["captions"].as_array().unwrap().len(), 2);
    assert_eq!(page["captions"][0]["role"], "microphone");
    assert_eq!(page["captions"][1]["role"], "system_audio");
    let descriptor = super::capture::descriptor(&input, 0, 16000, None);
    let seal = CaptureSeal {
        capture_fingerprint: descriptor.capture_fingerprint,
        chunk_count: 1,
        frames: 16000,
        pcm_bytes: 128000,
        chain_digest: descriptor.digest,
        source_sha256: super::capture::hash(super::capture::pcm(16000)),
    };
    let raw = format!(
        "/api/v1/vaults/{}/captures/{}",
        input.identity.scope.vault_id, input.identity.capture_id
    );
    assert_eq!(
        fixture
            .call(
                "alice",
                "POST",
                &format!("{raw}/seal"),
                Some(serde_json::to_value(&seal).unwrap())
            )
            .await
            .0,
        StatusCode::OK
    );
    wait_state(&fixture, &input, |value| {
        value["state"] == "awaiting_recording"
    })
    .await;
    assert!(!directory.join("transcript.json").exists());
    assert_eq!(
        fixture
            .call(
                "alice",
                "POST",
                &format!("{raw}/publish"),
                Some(serde_json::to_value(&seal).unwrap())
            )
            .await
            .0,
        StatusCode::OK
    );
    let completed = wait_state(&fixture, &input, |value| value["state"] == "completed").await;
    assert_eq!(completed["completed_through_frame"], 16000);
    assert!(completed["publication"].is_object());
    let (status, result) = fixture
        .call(
            "alice",
            "GET",
            &format!("{}/{}/result", path(&input), input.identity.request_id),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(result["job_id"], input.identity.request_id);
    assert_eq!(result["transcript"]["words"].as_array().unwrap().len(), 2);
    assert_eq!(result["transcript"]["user_id"], alice.member.id);
    assert_eq!(
        std::fs::read(directory.join("private-user.mp3")).unwrap(),
        b"never touch"
    );
    let canonical = context.session_transcripts("note").await.unwrap();
    assert_eq!(canonical.len(), 1);
    assert_eq!(
        canonical[0].id,
        result["transcript"]["id"].as_str().unwrap()
    );
    assert_eq!(start(&fixture, &input).await.0, StatusCode::OK);
    assert_eq!(engine.calls.load(Ordering::SeqCst), 1);
    fixture.service.shutdown().await.unwrap();
}

#[tokio::test]
async fn live_http_private_roles_strict_targets_and_hidden_capability_do_not_start_more_work() {
    let (fixture, _alice, input, engine) = setup().await;
    let bob = fixture.login("bob").await;
    assert_eq!(start(&fixture, &input).await.0, StatusCode::OK);
    wait_state(&fixture, &input, |value| {
        value["next_caption_sequence"].as_u64().unwrap() >= 2
    })
    .await;
    let (_, vault) = fixture
        .call(
            "alice",
            "GET",
            &format!("/api/v1/vaults/{}", input.identity.scope.vault_id),
            None,
        )
        .await;
    assert_eq!(
        fixture
            .call(
                "alice",
                "PUT",
                &format!(
                    "/api/v1/vaults/{}/members/{}",
                    input.identity.scope.vault_id, bob.member.id
                ),
                Some(json!({"role":"reader","expected_revision":vault["revision"]}))
            )
            .await
            .0,
        StatusCode::OK
    );
    let generation = format!("{}/{}", path(&input), input.identity.request_id);
    for tail in ["", "/captions", "/result"] {
        assert_eq!(
            fixture
                .call("bob", "GET", &format!("{generation}{tail}"), None)
                .await
                .0,
            StatusCode::NOT_FOUND
        );
    }
    assert_eq!(
        fixture
            .call("bob", "POST", &format!("{generation}/cancel"), None)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    for query in [
        "limit=0",
        "limit=129",
        "limit=1&after=9007199254740992",
        "limit=2&future=1",
    ] {
        assert_eq!(
            fixture
                .call(
                    "alice",
                    "GET",
                    &format!("{generation}/captions?{query}"),
                    None
                )
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
    }
    let mut invalid = request(&input);
    invalid["path"] = json!("/private/audio.wav");
    assert_eq!(
        fixture
            .call_token(
                &fixture.token("alice"),
                "POST",
                &path(&input),
                Some(invalid),
                Some(&input.identity.request_id)
            )
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    let (_, capabilities) = fixture
        .call("alice", "GET", "/api/v1/capabilities", None)
        .await;
    assert!(
        !capabilities["features"]
            .as_array()
            .unwrap()
            .contains(&json!("capture_live_transcription"))
    );
    assert_eq!(
        fixture
            .call("alice", "POST", &format!("{generation}/cancel"), None)
            .await
            .0,
        StatusCode::OK
    );
    wait_state(&fixture, &input, |value| value["state"] == "cancelled").await;
    assert_eq!(engine.calls.load(Ordering::SeqCst), 1);
    fixture.service.shutdown().await.unwrap();
}

#[tokio::test]
async fn live_sse_real_network_disconnect_and_generation_cursor_replay_never_cancel_inference() {
    let (fixture, _alice, input, engine) = setup().await;
    assert_eq!(start(&fixture, &input).await.0, StatusCode::OK);
    wait_state(&fixture, &input, |value| {
        value["next_caption_sequence"].as_u64().unwrap() >= 2
    })
    .await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let stop = CancellationToken::new();
    let done = stop.clone();
    let app = fixture.app.clone();
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(done.cancelled_owned())
            .await
            .unwrap();
    });
    let client = reqwest::Client::new();
    let url = format!(
        "http://{address}{}/{}/events",
        path(&input),
        input.identity.request_id
    );
    let connect = |cursor: String| {
        client
            .get(&url)
            .bearer_auth(fixture.token("alice"))
            .header("x-loofah-api-major", "1")
            .header("last-event-id", cursor)
    };
    let response = connect(format!("{}:initial", input.identity.request_id))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let text = first_page(response).await;
    assert!(text.contains("event: page"));
    assert!(text.contains(&format!("{}:1", input.identity.request_id)));
    let generation = format!("{}/{}", path(&input), input.identity.request_id);
    assert_eq!(
        fixture.call("alice", "GET", &generation, None).await.1["state"],
        "running"
    );
    assert_eq!(engine.calls.load(Ordering::SeqCst), 1);
    let response = connect(format!("{}:1", input.identity.request_id))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let text = first_page(response).await;
    assert!(text.contains("\"captions\":[]"));
    assert_eq!(
        connect(format!("{}:1", uuid::Uuid::new_v4()))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        fixture
            .call("alice", "POST", &format!("{generation}/cancel"), None)
            .await
            .0,
        StatusCode::OK
    );
    wait_state(&fixture, &input, |value| value["state"] == "cancelled").await;
    stop.cancel();
    server.await.unwrap();
    fixture.service.shutdown().await.unwrap();
}

async fn first_page(response: reqwest::Response) -> String {
    tokio::time::timeout(std::time::Duration::from_secs(10), async move {
        let mut stream = response.bytes_stream();
        let mut bytes = Vec::new();
        while let Some(chunk) = stream.next().await {
            bytes.extend_from_slice(&chunk.unwrap());
            assert!(bytes.len() <= CAPTURE_LIVE_PAGE_LIMIT);
            while let Some(end) = bytes.windows(2).position(|value| value == b"\n\n") {
                let text = String::from_utf8(bytes.drain(..end + 2).collect()).unwrap();
                if text.contains("event: page") {
                    return text;
                }
            }
        }
        panic!("Live stream closed without its first page")
    })
    .await
    .unwrap()
}
