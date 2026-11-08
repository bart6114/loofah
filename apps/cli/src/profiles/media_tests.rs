use super::*;
use axum::{
    Json, Router,
    body::Bytes,
    extract::{Path as ApiPath, State},
    http::HeaderMap,
    response::IntoResponse,
    routing::{get, post, put},
};
use serde_json::{Value, json};
use std::sync::{
    Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
struct Credentials(AtomicUsize);
impl CredentialStore for Credentials {
    fn load<'a>(&'a self, _: &'a str) -> hypr_loofah_client::CredentialFuture<'a, Option<String>> {
        Box::pin(async {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(Some(r#"{"schema":1,"refresh_token":"original"}"#.into()))
        })
    }
    fn save<'a>(
        &'a self,
        _: &'a str,
        _: Option<&'a str>,
    ) -> hypr_loofah_client::CredentialFuture<'a, ()> {
        Box::pin(async { Ok(()) })
    }
}
struct Api {
    origin: String,
    enabled: bool,
    declarations: Mutex<Vec<MediaDeclaration>>,
    uploads: Mutex<Vec<Vec<u8>>>,
    phase: Mutex<MediaPhase>,
    malformed: AtomicBool,
    recoveries: AtomicUsize,
}
impl Api {
    fn status(&self) -> MediaRequestStatus {
        let declaration = self.declarations.lock().unwrap().last().unwrap().clone();
        let transcription = matches!(declaration.operation, MediaOperation::Transcribe { .. });
        let request = declaration.request_id.clone();
        MediaRequestStatus {
            declaration,
            revision: "3ee6ab27-391d-47cb-ad40-5c61b57f4181".into(),
            phase: *self.phase.lock().unwrap(),
            media: Some(LocalMediaStatus {
                request_id: request,
                session_id: "original".into(),
                kind: if transcription {
                    LocalMediaKind::Transcribe
                } else {
                    LocalMediaKind::ImportAudio
                },
                state: LocalMediaState::Succeeded,
                transcript_id: transcription.then(|| "original-transcript".into()),
                word_count: transcription.then_some(5),
                error: None,
                imported: (!transcription).then(|| source_snapshot().metadata.meta),
            }),
        }
    }
}
struct Fixture {
    root: tempfile::TempDir,
    session: RemoteSession,
    api: Arc<Api>,
    credentials: Arc<Credentials>,
    proxy: tokio::task::JoinHandle<()>,
    server: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.proxy.abort();
        self.server.abort();
    }
}
impl Fixture {
    async fn new(enabled: bool) -> Self {
        use tokio_rustls::rustls::{
            self,
            pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject},
        };
        let cert = CertificateDer::from_pem_slice(include_bytes!(
            "../../../../../crates/loofah-client/tests/fixtures/localhost.pem"
        ))
        .unwrap();
        let key = PrivateKeyDer::from_pem_slice(include_bytes!(
            "../../../../../crates/loofah-client/tests/fixtures/localhost.key"
        ))
        .unwrap();
        let config = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![cert], key)
        .unwrap();
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
        let upstream = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = upstream.local_addr().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("https://{}", listener.local_addr().unwrap());
        let proxy = tokio::spawn(async move {
            let mut tasks = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    socket=listener.accept()=>{let (socket,_)=socket.unwrap();let acceptor=acceptor.clone();tasks.spawn(async move {let Ok(mut tls)=acceptor.accept(socket).await else{return;};let mut upstream=tokio::net::TcpStream::connect(addr).await.unwrap();let _=tokio::io::copy_bidirectional(&mut tls,&mut upstream).await;});},
                    _=tasks.join_next(),if !tasks.is_empty()=>{}
                }
            }
        });
        let api = Arc::new(Api {
            origin: origin.clone(),
            enabled,
            declarations: Default::default(),
            uploads: Default::default(),
            phase: Mutex::new(MediaPhase::Receiving),
            malformed: AtomicBool::new(false),
            recoveries: AtomicUsize::new(0),
        });
        let router=Router::new()
            .route("/.well-known/openid-configuration",get(|State(a):State<Arc<Api>>|async move{Json(json!({"issuer":a.origin,"authorization_endpoint":format!("{}/authorize",a.origin),"token_endpoint":format!("{}/token",a.origin),"code_challenge_methods_supported":["S256"]}))}))
            .route("/api/v1/auth/config",get(|State(a):State<Arc<Api>>|async move{Json(json!({"issuer":a.origin,"authorization_endpoint":format!("{}/authorize",a.origin),"token_endpoint":format!("{}/token",a.origin),"client_id":"cli","audience":"api","scopes":["loofah:access"]}))}))
            .route("/api/v1/capabilities",get(|State(a):State<Arc<Api>>|async move{let mut features=vec!["metadata","transcription_policy"];if a.enabled{features.push("media_jobs"); features.push("media_initial_metadata");}Json(json!({"api_major":1,"application_version":"test","features":features}))}))
            .route("/token",post(||async{Json(json!({"token_type":"Bearer","access_token":"original-access","refresh_token":"original","expires_in":3600}))}))
            .route("/api/v1/me",get(||async{Json(json!({"member":{"id":"alice","name":"Alice"},"personal_vault_id":"vault"}))}))
            .route("/api/v1/vaults/vault/transcription/policy",get(||async{Json(json!({"model":"server-cpu","availability":"ready","batch_enabled":true,"live_enabled":false}))}))
            .route("/api/v1/vaults/vault/media/source/original",get(||async{Json(source_snapshot())}))
            .route("/api/v1/vaults/vault/sessions/original/metadata",get(||async{Json(source_snapshot().metadata)}))
            .route("/api/v1/vaults/vault/media/requests",post(declare))
            .route("/api/v1/vaults/vault/media/requests/{id}",get(|State(a):State<Arc<Api>>,ApiPath(id):ApiPath<String>|async move {let status=a.status();assert_eq!(status.declaration.request_id,id);Json(status)}))
            .route("/api/v1/vaults/vault/media/requests/{id}/recover",post(|State(a):State<Arc<Api>>,ApiPath(id):ApiPath<String>|async move {a.recoveries.fetch_add(1,Ordering::SeqCst);let status=a.status();assert_eq!(status.declaration.request_id,id);Json(status)}))
            .route("/api/v1/vaults/vault/media/requests/{id}/source",put(upload)).with_state(api.clone());
        let server = tokio::spawn(async move {
            axum::serve(upstream, router).await.unwrap();
        });
        let credentials = Arc::new(Credentials(AtomicUsize::new(0)));
        let client = RemoteClient::discover_fixture(
            &origin,
            credentials.clone(),
            include_bytes!("../../../../../crates/loofah-client/tests/fixtures/localhost-ca.pem"),
        )
        .await
        .unwrap();
        let root = tempfile::tempdir().unwrap();
        let path = root.path().canonicalize().unwrap();
        let profiles = ConnectionProfiles::new(path.join("profiles")).unwrap();
        let lease = profiles.create("remote", &origin).await.unwrap();
        let mut profile = lease.profile().await;
        profile.member_id = Some("alice".into());
        profile.vault_id = Some("vault".into());
        profile.credential_accounts = vec![client.credential_account().into()];
        lease.save(profile).await.unwrap();
        let generation = lease.generation().await.unwrap();
        let session = RemoteSession {
            client,
            lease,
            generation,
            member: "alice".into(),
            vault: "vault".into(),
            request_id: uuid::Uuid::new_v4().to_string(),
            media_receipts: path.join("media-submissions"),
        };
        Self {
            root,
            session,
            api,
            credentials,
            proxy,
            server,
        }
    }
    fn file(&self) -> PathBuf {
        let path = self.root.path().join("source.wav");
        std::fs::write(&path, b"original source bytes").unwrap();
        path
    }
}
fn source_snapshot() -> MediaSessionSnapshot {
    serde_json::from_value(json!({"metadata":{"revision":"a".repeat(64),"meta":{"id":"original","title":"Saved title","created_at":"2026-11-08T10:00:00Z","tags":[]}},"transcript_revision":"b".repeat(64),"transcript_present":true,
    "recording":{"session_id":"original","session_created_at":"2026-11-08T10:00:00Z","recording_selection":"audio.mp3","recording_sha256":"c".repeat(64),"transcript_revision":"b".repeat(64),"transcript_present":true,"audio":{"source":"import","layout":"mixed"}}})).unwrap()
}
async fn declare(
    State(api): State<Arc<Api>>,
    Json(input): Json<MediaDeclaration>,
) -> Json<MediaAdmission> {
    validate_media_declaration(&input).unwrap();
    let transcription = matches!(input.operation, MediaOperation::Transcribe { .. });
    api.declarations.lock().unwrap().push(input);
    *api.phase.lock().unwrap() = if transcription {
        MediaPhase::Succeeded
    } else {
        MediaPhase::Receiving
    };
    let status = api.status();
    Json(MediaAdmission {
        declaration: status.declaration.clone(),
        revision: status.revision.clone(),
        status,
    })
}
async fn upload(
    State(api): State<Arc<Api>>,
    ApiPath(id): ApiPath<String>,
    headers: HeaderMap,
    bytes: Bytes,
) -> axum::response::Response {
    assert_eq!(api.status().declaration.request_id, id);
    assert_eq!(
        headers["x-loofah-media-revision"],
        "3ee6ab27-391d-47cb-ad40-5c61b57f4181"
    );
    api.uploads.lock().unwrap().push(bytes.to_vec());
    *api.phase.lock().unwrap() = MediaPhase::Succeeded;
    if api.malformed.load(Ordering::SeqCst) {
        return "lost original response".into_response();
    }
    Json(api.status()).into_response()
}
#[tokio::test]
async fn remote_media_capability_rejects_before_credentials_source_or_receipts() {
    let fixture = Fixture::new(false).await;
    let before = fixture.credentials.0.load(Ordering::SeqCst);
    let cause = fixture
        .session
        .import_media(
            fixture.root.path().join("missing.wav"),
            None,
            None,
            false,
            Default::default(),
            true,
        )
        .await
        .unwrap_err();
    assert_eq!(cause.code(), "incompatible_version");
    assert_eq!(fixture.credentials.0.load(Ordering::SeqCst), before);
    assert!(!fixture.session.media_receipts.exists());
    assert!(fixture.api.declarations.lock().unwrap().is_empty());
}
#[tokio::test]
async fn remote_import_freezes_atomic_metadata_and_unknown_reply_only_explicitly_recovers() {
    let fixture = Fixture::new(true).await;
    fixture.api.malformed.store(true, Ordering::SeqCst);
    let file = fixture.file();
    let cause = fixture
        .session
        .import_media(
            file,
            Some("Original title".into()),
            None,
            false,
            NewSessionOptions {
                created_at: Some("2026-11-08T10:00:00Z".into()),
                started_at: Some("2026-11-08T09:00:00Z".into()),
                author: Some("agent".into()),
                skill: Some("notes".into()),
                ..Default::default()
            },
            true,
        )
        .await
        .unwrap_err();
    let value: Value = serde_json::from_str(&cause.to_json()).unwrap();
    assert_eq!(value["error"]["request_id"], fixture.session.request_id);
    assert_eq!(value["error"]["mutation_outcome"], "unknown");
    assert_eq!(fixture.api.declarations.lock().unwrap().len(), 1);
    assert_eq!(
        fixture.api.uploads.lock().unwrap().as_slice(),
        &[b"original source bytes".to_vec()]
    );
    let original = fixture.api.declarations.lock().unwrap()[0].clone();
    let MediaOperation::ImportAudio {
        initial: Some(initial),
        ..
    } = &original.operation
    else {
        panic!()
    };
    assert_eq!(initial.title.as_deref(), Some("Original title"));
    assert_eq!(initial.metadata.author.as_deref(), Some("agent"));
    assert!(original.transcription.is_none());
    fixture
        .session
        .recover_media(&fixture.session.request_id, None, true)
        .await
        .unwrap();
    assert_eq!(fixture.api.recoveries.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.api.declarations.lock().unwrap().len(), 1);
    assert_eq!(fixture.api.uploads.lock().unwrap().len(), 1);
}
#[tokio::test]
async fn remote_transcribe_freezes_server_alias_and_original_recording_transcript_baseline() {
    let fixture = Fixture::new(true).await;
    fixture
        .session
        .transcribe_media("original", true)
        .await
        .unwrap();
    let inputs = fixture.api.declarations.lock().unwrap();
    assert_eq!(inputs.len(), 1);
    assert_eq!(
        inputs[0].transcription.as_ref().unwrap().model,
        "server-cpu"
    );
    let MediaOperation::Transcribe { expected } = &inputs[0].operation else {
        panic!()
    };
    assert_eq!(expected, &source_snapshot().recording.unwrap());
    assert!(fixture.api.uploads.lock().unwrap().is_empty());
}

#[tokio::test]
async fn partial_remote_import_reports_original_receipt_with_nonzero_exit() {
    let fixture = Fixture::new(true).await;
    let declaration = MediaDeclaration {
        request_id: fixture.session.request_id.clone(),
        operation: MediaOperation::ImportAudio {
            session_id: None,
            expected: None,
            filename: "a.wav".into(),
            content_type: None,
            created_at: None,
            initial: None,
            transcribe: true,
            source: MediaUploadSource {
                sha256: "a".repeat(64),
                size_bytes: 1,
            },
        },
        transcription: Some(MediaTranscriptionOptions {
            model: "server-cpu".into(),
            languages: vec![],
        }),
        summary: None,
    };
    fixture.api.declarations.lock().unwrap().push(declaration);
    let mut status = fixture.api.status();
    status.phase = MediaPhase::Failed;
    let media = status.media.as_mut().unwrap();
    media.state = LocalMediaState::Failed;
    media.error = Some(failure(
        ErrorCode::Unavailable,
        "Transcription model unavailable after import",
    ));
    assert_eq!(
        fixture
            .session
            .emit_media(status, "import", true)
            .await
            .unwrap(),
        1
    );
    assert!(fixture.api.uploads.lock().unwrap().is_empty());
}
#[test]
fn remote_media_recovery_keeps_explicit_source_and_original_id() {
    use clap::Parser;
    let args = crate::Args::try_parse_from([
        "loof",
        "--profile",
        "company",
        "sessions",
        "recover",
        "3ee6ab27-391d-47cb-ad40-5c61b57f4181",
        "--source",
        "original.wav",
    ])
    .unwrap();
    assert!(
        matches!(args.command,crate::cli::Command::Sessions {command:crate::cli::MeetingCommand::Recover {request_id,source:Some(source)}} if request_id=="3ee6ab27-391d-47cb-ad40-5c61b57f4181" && source==PathBuf::from("original.wav"))
    );
}

#[tokio::test]
async fn explicit_remote_source_recovery_rejects_changed_bytes_without_fresh_declaration() {
    let fixture = Fixture::new(true).await;
    let authority = fixture.session.media().await.unwrap();
    let (filename, source, _file) = open_source(fixture.file(), authority.clone())
        .await
        .unwrap();
    let declaration = MediaDeclaration {
        request_id: fixture.session.request_id.clone(),
        operation: MediaOperation::ImportAudio {
            session_id: None,
            expected: None,
            filename,
            content_type: None,
            created_at: None,
            initial: None,
            transcribe: false,
            source,
        },
        transcription: None,
        summary: None,
    };
    let receipts = MediaSubmissions::new(fixture.session.media_receipts.clone()).unwrap();
    let (saved, _) = receipts
        .prepare(authority.scope().clone(), declaration)
        .await
        .unwrap();
    let admission = authority.declare(&saved).await.unwrap();
    receipts
        .observe_admission(&saved, &admission)
        .await
        .unwrap();
    let changed = fixture.root.path().join("changed.wav");
    std::fs::write(&changed, b"different bytes").unwrap();
    let cause = fixture
        .session
        .recover_media(&fixture.session.request_id, Some(changed), true)
        .await
        .unwrap_err();
    assert!(
        cause
            .to_string()
            .contains("differs from the original saved SHA")
    );
    assert_eq!(fixture.api.declarations.lock().unwrap().len(), 1);
    assert!(fixture.api.uploads.lock().unwrap().is_empty());
    assert_eq!(*fixture.api.phase.lock().unwrap(), MediaPhase::Receiving);
}
