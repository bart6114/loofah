use super::*;
use crate::VaultRuntime;
use hypr_storage::vault::{LeaseAttempt, VaultLease};
use std::{fs::File, path::PathBuf, time::Duration};

struct Fixture {
    root: tempfile::TempDir,
    runtime: VaultRuntime,
    cli: Arc<LocalCli>,
    session: String,
}
impl Fixture {
    async fn new(physical: bool) -> Self {
        let root = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        let vault = root.path().join("vault");
        std::fs::create_dir(&vault).unwrap();
        let LeaseAttempt::Acquired(lease) = VaultLease::try_acquire_existing(&vault).unwrap()
        else {
            panic!("lease")
        };
        let runtime = VaultRuntime::open_leased(
            "local:vault".into(),
            lease.clone(),
            &root.path().join("cache"),
            &root.path().join("operations"),
            hypr_vault_write::ContentLimits::unlimited(),
        )
        .await
        .unwrap();
        tokio::time::timeout(
            Duration::from_secs(10),
            runtime.context.startup.wait_until_ready(),
        )
        .await
        .unwrap()
        .unwrap();
        if physical {
            runtime
                .context
                .enable_standalone_local_session_lifecycle(&lease)
                .unwrap();
        }
        let cli = Arc::new(LocalCli::new(runtime.context.clone()).unwrap());
        let reply = cli
            .execute(
                uuid::Uuid::new_v4().to_string(),
                LocalCliOperation::CreateSession {
                    title: "Subtitle note".into(),
                    markdown: Some("Keep my note".into()),
                    metadata: Default::default(),
                },
                None,
                Arc::new(|_| {}),
            )
            .await
            .unwrap()
            .reply;
        let LocalCliReply::SessionCreated { meta } = reply else {
            panic!("create")
        };
        Self {
            root,
            runtime,
            cli,
            session: meta.id,
        }
    }
    fn artifact(&self, text: &str) -> LocalCliArtifact {
        let path = self
            .root
            .path()
            .join(format!("{}.srt", uuid::Uuid::new_v4()));
        std::fs::write(&path, text).unwrap();
        LocalCliArtifact::new(
            File::open(&path).unwrap(),
            text.len() as u64,
            digest(text.as_bytes()),
            Some(path),
            Box::new(()),
        )
    }
    fn canonical(&self) -> PathBuf {
        self.runtime
            .context
            .store
            .vault_base()
            .join("sessions")
            .join(&self.session)
            .join("transcript.json")
    }
    async fn import(
        &self,
        request: &str,
        filename: &str,
        text: &str,
    ) -> Result<LocalCliOutput, ApplicationError> {
        self.cli
            .execute(
                request.into(),
                LocalCliOperation::ImportSubtitles {
                    session_id: self.session.clone(),
                    filename: filename.into(),
                },
                Some(self.artifact(text)),
                Arc::new(|_| {}),
            )
            .await
    }
    async fn recover(&self, request: &str) -> Result<LocalCliOutput, ApplicationError> {
        self.cli
            .execute(
                uuid::Uuid::new_v4().to_string(),
                LocalCliOperation::RecoverSubtitles {
                    request_id: request.into(),
                },
                None,
                Arc::new(|_| {}),
            )
            .await
    }
    async fn stop(self) {
        drop(self.cli);
        self.runtime.shutdown().await.unwrap();
    }
}
const SRT: &str = "1\n00:00:01,000 --> 00:00:02,500\nHello there\n";

#[tokio::test]
async fn srt_and_vtt_import_without_audio_or_model_append_and_replay_exact_history() {
    let f = Fixture::new(true).await;
    for (filename, text) in [
        ("words.srt", SRT),
        (
            "words.vtt",
            "WEBVTT\n\nAlice\n00:00:01.000 --> 00:00:02.500\nHello there\n",
        ),
    ] {
        let request = uuid::Uuid::new_v4().to_string();
        let result = f.import(&request, filename, text).await.unwrap();
        let LocalCliReply::SubtitlesImported {
            transcript_id: Some(id),
            word_count,
            revision: Some(revision),
            ..
        } = result.reply
        else {
            panic!("subtitle reply")
        };
        assert_eq!(word_count, 1);
        let raw = std::fs::read(f.canonical()).unwrap();
        assert_eq!(digest(&raw), revision);
        let value: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        let last = value["transcripts"].as_array().unwrap().last().unwrap();
        assert_eq!(last["id"], id);
        assert_eq!(last["words"][0]["start_ms"], 1000.0);
        assert_eq!(last["words"][0]["end_ms"], 2500.0);
        assert_eq!(last["words"][0]["channel"], 2.0);
        assert_eq!(last["user_id"], uuid::Uuid::nil().to_string());
        assert!(last["memo_md"].as_str().unwrap().contains("Keep my note"));
        f.import(&request, filename, text).await.unwrap();
        assert_eq!(std::fs::read(f.canonical()).unwrap(), raw);
        assert_eq!(
            f.import(
                &request,
                filename,
                "1\n00:00:01,000 --> 00:00:02,500\nChanged\n"
            )
            .await
            .err()
            .unwrap()
            .code,
            ErrorCode::Conflict
        );
        if filename.ends_with("vtt") {
            std::fs::remove_file(f.canonical()).unwrap();
            f.recover(&request).await.unwrap();
            assert!(!f.canonical().exists());
        }
    }
    f.stop().await;
}

#[tokio::test]
async fn subtitle_mutation_requires_original_physical_host_and_exclusive_lifecycle() {
    let f = Fixture::new(false).await;
    assert_eq!(
        f.import(&uuid::Uuid::new_v4().to_string(), "words.srt", SRT)
            .await
            .err()
            .unwrap()
            .code,
        ErrorCode::Unavailable
    );
    assert!(!f.canonical().exists());
    f.stop().await;
    let f = Fixture::new(true).await;
    let held = f
        .runtime
        .context
        .admit_session_work(&f.session)
        .await
        .unwrap();
    assert!(
        f.import(&uuid::Uuid::new_v4().to_string(), "words.srt", SRT)
            .await
            .is_err()
    );
    assert!(!f.canonical().exists());
    drop(held);
    f.stop().await;
}

#[test]
fn retained_parsing_reuses_original_ids_and_rejects_changed_declarations_or_source() {
    let root = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let intent = Intent {
        version: 1,
        vault_id: "vault".into(),
        request_id: uuid::Uuid::new_v4().to_string(),
        session_id: "session".into(),
        filename: "words.srt".into(),
        source_sha256: digest(SRT.as_bytes()),
        source_bytes: SRT.len() as u64,
        transcript_revision: digest(b"absent"),
        transcript_present: false,
        session_created_at: "2026-11-08T12:00:00Z".into(),
        transcript_id: uuid::Uuid::new_v4().to_string(),
        created_at: "2026-11-08T12:00:00Z".into(),
        started_at: 1000.0,
        memo: String::new(),
    };
    let path = root.path().join("original.srt");
    std::fs::write(&path, SRT).unwrap();
    let artifact = LocalCliArtifact::new(
        File::open(path).unwrap(),
        SRT.len() as u64,
        digest(SRT.as_bytes()),
        None,
        Box::new(()),
    );
    let directory = root.path().join("request");
    let first = journal::prepare(&directory, &intent, Some(artifact), &|_| {}).unwrap();
    let again = journal::prepare(&directory, &intent, None, &|_| {}).unwrap();
    assert_eq!(first.transcript, again.transcript);
    let mut changed = intent.clone();
    changed.transcript_present = true;
    assert!(journal::prepare(&directory, &changed, None, &|_| {}).is_err());
    std::fs::remove_file(directory.join("parsed.json")).unwrap();
    std::fs::write(directory.join("source.srt"), b"changed").unwrap();
    assert!(journal::prepare(&directory, &intent, None, &|_| {}).is_err());
}

#[tokio::test]
async fn retained_subtitle_result_recovers_without_caller_file_and_never_rebinds_changed_prefix() {
    for changed in [false, true] {
        let f = Fixture::new(true).await;
        let request = uuid::Uuid::new_v4().to_string();
        let source = subtitle_source(&f.runtime.context, &f.session, digest(SRT.as_bytes()))
            .await
            .unwrap();
        let intent = Intent {
            version: 1,
            vault_id: f.runtime.context.events.vault_id(),
            request_id: request.clone(),
            session_id: f.session.clone(),
            filename: "words.srt".into(),
            source_sha256: digest(SRT.as_bytes()),
            source_bytes: SRT.len() as u64,
            transcript_revision: source.transcript_revision().into(),
            transcript_present: source.transcript_present(),
            session_created_at: source.session_created_at().into(),
            transcript_id: uuid::Uuid::new_v4().to_string(),
            created_at: "2026-11-08T12:00:00Z".into(),
            started_at: 1000.0,
            memo: String::new(),
        };
        let directory = f
            .runtime
            .context
            .operations
            .as_ref()
            .unwrap()
            .root
            .join("local-subtitles")
            .join(&request);
        let parsed = journal::prepare(&directory, &intent, Some(f.artifact(SRT)), &|_| {}).unwrap();
        if changed {
            std::fs::write(f.canonical(), b"{\"transcripts\":[],\"future\":42}").unwrap();
        }
        let result = f.recover(&request).await;
        if changed {
            let cause = result.err().unwrap();
            assert_eq!(cause.code, ErrorCode::Conflict);
            assert_eq!(cause.request_id.as_deref(), Some(request.as_str()));
            assert_eq!(
                std::fs::read(f.canonical()).unwrap(),
                b"{\"transcripts\":[],\"future\":42}"
            );
        } else {
            let LocalCliReply::SubtitlesImported {
                transcript_id: Some(id),
                ..
            } = result.unwrap().reply
            else {
                panic!("recovery")
            };
            assert_eq!(id, parsed.transcript.id);
        }
        f.stop().await;
    }
}

#[tokio::test]
async fn empty_subtitles_are_an_explicit_noop_with_recoverable_history() {
    let f = Fixture::new(true).await;
    let request = uuid::Uuid::new_v4().to_string();
    let reply = f
        .import(&request, "empty.vtt", "WEBVTT\n\n")
        .await
        .unwrap()
        .reply;
    assert!(matches!(
        reply,
        LocalCliReply::SubtitlesImported {
            transcript_id: None,
            word_count: 0,
            revision: None,
            ..
        }
    ));
    assert!(!f.canonical().exists());
    assert!(matches!(
        f.recover(&request).await.unwrap().reply,
        LocalCliReply::SubtitlesImported {
            transcript_id: None,
            word_count: 0,
            revision: None,
            ..
        }
    ));
    assert!(!f.canonical().exists());
    f.stop().await;
}

#[tokio::test]
async fn subtitle_inventory_is_read_only_and_retains_original_history_after_canonical_deletion() {
    let f = Fixture::new(true).await;
    let root = f
        .runtime
        .context
        .operations
        .as_ref()
        .unwrap()
        .root
        .join("local-subtitles");
    assert!(!root.exists());
    assert!(f.cli.subtitle_statuses(None).await.unwrap().is_empty());
    assert!(!root.exists());
    let request = uuid::Uuid::new_v4().to_string();
    f.import(&request, "words.srt", SRT).await.unwrap();
    std::fs::remove_file(f.canonical()).unwrap();
    let statuses = f.cli.subtitle_statuses(Some(&f.session)).await.unwrap();
    assert_eq!(statuses.len(), 1);
    assert_eq!(statuses[0].request_id, request);
    assert_eq!(statuses[0].state, LocalMediaState::Succeeded);
    assert_eq!(statuses[0].word_count, Some(1));
    assert!(!f.canonical().exists());
    let path = root.join(&request).join("intent.json");
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    value["version"] = 2.into();
    std::fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
    assert!(f.cli.subtitle_statuses(None).await.is_err());
    assert!(!f.canonical().exists());
    f.stop().await;
}
