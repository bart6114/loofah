use super::*;
use crate::{
    local_cli::LocalCli, search::SearchService, settings::ConfigState, startup::StartupState,
};
use hypr_loofah_protocol::LocalCliOperation;
use hypr_storage::vault::{LeaseAttempt, VaultLease};

struct Fixture {
    _temporary: tempfile::TempDir,
    root: PathBuf,
    vault: PathBuf,
    id: String,
}
impl Fixture {
    async fn new(drained: bool) -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let base = temporary.path().canonicalize().unwrap();
        let vault = base.join("vault");
        std::fs::create_dir(&vault).unwrap();
        let lease = acquire(&vault);
        let store = Arc::new(hypr_vault_write::SessionStore::new(vault.clone()));
        let meta = serde_json::from_value(serde_json::json!({
            "id":"note", "title":"Original", "created_at":"2026-11-08T10:00:00Z", "tags":[]
        }))
        .unwrap();
        store
            .create_note_session(&meta, "Original user note")
            .await
            .unwrap();
        let root = base.join("operations");
        let context = Arc::new(
            VaultContext::new(
                format!("local:{}", hypr_storage::vault::canonical_vault_key(&vault)),
                store,
                StartupState::new(Some(&vault)),
                Arc::new(ConfigState::load_or_default(&vault)),
                Arc::new(SearchService::default()),
            )
            .with_operation_storage(root.clone())
            .with_vault_lease(lease)
            .unwrap(),
        );
        let service = LocalCaptureIntents::open(context.clone()).await.unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let token = service
            .declare(LocalCaptureDeclaration {
                request_id: id.clone(),
                session_id: "note".into(),
                append: false,
                batch_requested: false,
                transcription: None,
                summary: None,
            })
            .await
            .unwrap();
        if drained {
            std::fs::write(
                vault.join("sessions/note/audio.wav"),
                b"original completed audio",
            )
            .unwrap();
            token.physical_finished(Ok(())).await.unwrap();
            token
                .persistence_finished(Ok(LocalCapturePersistence {
                    live_transcription_active: false,
                    needs_batch_repair: false,
                    new_transcript_id: None,
                }))
                .await
                .unwrap();
            tokio::time::timeout(std::time::Duration::from_secs(10), async {
                loop {
                    if service.ensure_idle("note").await.is_ok() {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            assert_eq!(
                service.status(&id).await.unwrap().state,
                LocalCaptureState::Skipped
            );
        }
        let retired = Arc::downgrade(&context);
        drop(token);
        drop(service);
        drop(context);
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while retired.upgrade().is_some() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        Self {
            _temporary: temporary,
            root,
            vault,
            id,
        }
    }
    fn receipt(&self) -> PathBuf {
        self.root
            .join("local-capture-intents")
            .join(&self.id)
            .join("intent.json")
    }
    async fn cli(&self) -> Arc<LocalCli> {
        LocalCli::open_session_owner(acquire(&self.vault), self.root.clone(), None)
            .await
            .unwrap()
    }
    fn vault_id(&self) -> String {
        format!(
            "local:{}",
            hypr_storage::vault::canonical_vault_key(&self.vault)
        )
    }
}
fn acquire(vault: &std::path::Path) -> VaultLease {
    match VaultLease::try_acquire_existing(vault).unwrap() {
        LeaseAttempt::Acquired(lease) => lease,
        LeaseAttempt::Busy => panic!("original fixture owner still alive"),
    }
}
async fn lifecycle(
    cli: &Arc<LocalCli>,
) -> Result<crate::local_cli::LocalCliOutput, ApplicationError> {
    cli.execute(
        uuid::Uuid::new_v4().to_string(),
        LocalCliOperation::SessionLifecycle {
            session_id: "note".into(),
        },
        None,
        Arc::new(|_| {}),
    )
    .await
}

#[tokio::test]
async fn standalone_local_cli_cannot_bypass_original_unknown_review_or_rewrite_its_receipt() {
    let fixture = Fixture::new(false).await;
    // Loading in the original Native service records Review; the CLI must inspect it unchanged.
    let root = fixture.root.join("local-capture-intents");
    journal::load(&root, &fixture.vault_id()).unwrap();
    let receipt = std::fs::read(fixture.receipt()).unwrap();
    let note = std::fs::read(fixture.vault.join("sessions/note/notes.md")).unwrap();
    let history_existed = root.join("history").exists();
    let cli = fixture.cli().await;
    assert!(lifecycle(&cli).await.is_err());
    assert!(
        cli.execute(
            uuid::Uuid::new_v4().to_string(),
            LocalCliOperation::DeleteSession {
                session_id: "note".into(),
                expected_revision: "a".repeat(64),
            },
            None,
            Arc::new(|_| {})
        )
        .await
        .is_err()
    );
    assert_eq!(std::fs::read(fixture.receipt()).unwrap(), receipt);
    assert_eq!(
        std::fs::read(fixture.vault.join("sessions/note/notes.md")).unwrap(),
        note
    );
    assert_eq!(root.join("history").exists(), history_existed);
}

#[tokio::test]
async fn standalone_local_cli_allows_confirmed_drained_terminal_receipt_without_loading_service() {
    let fixture = Fixture::new(true).await;
    let before = std::fs::read(fixture.receipt()).unwrap();
    assert!(lifecycle(&fixture.cli().await).await.is_ok());
    assert_eq!(std::fs::read(fixture.receipt()).unwrap(), before);
}

#[tokio::test]
async fn malformed_future_and_unexpected_local_inventory_fail_closed_without_writes() {
    let fixture = Fixture::new(true).await;
    let original = std::fs::read(fixture.receipt()).unwrap();
    let mut future: serde_json::Value = serde_json::from_slice(&original).unwrap();
    future["version"] = 999.into();
    for raw in [b"{broken".to_vec(), serde_json::to_vec(&future).unwrap()] {
        std::fs::write(fixture.receipt(), &raw).unwrap();
        assert!(lifecycle(&fixture.cli().await).await.is_err());
        assert_eq!(std::fs::read(fixture.receipt()).unwrap(), raw);
    }
    std::fs::write(fixture.receipt(), original).unwrap();
    std::fs::create_dir(fixture.root.join("local-capture-intents/unknown")).unwrap();
    assert!(lifecycle(&fixture.cli().await).await.is_err());
}

#[tokio::test]
async fn known_history_layout_is_checked_without_recursively_scanning_or_initializing_it() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let history = root.join("history");
    std::fs::create_dir(&history).unwrap();
    let check = || ensure_stored_session_idle(root.clone(), "vault".into(), "note".into(), None);
    assert!(check().await.is_err());
    let layout = serde_json::json!({"schema_version":1,"vault_id":"vault"});
    std::fs::write(
        history.join("layout.json"),
        serde_json::to_vec(&layout).unwrap(),
    )
    .unwrap();
    assert!(check().await.is_ok());
    assert_eq!(std::fs::read_dir(&history).unwrap().count(), 1);
    let future = serde_json::json!({"schema_version":2,"vault_id":"vault"});
    let raw = serde_json::to_vec(&future).unwrap();
    std::fs::write(history.join("layout.json"), &raw).unwrap();
    assert!(check().await.is_err());
    assert_eq!(std::fs::read(history.join("layout.json")).unwrap(), raw);
}

#[cfg(unix)]
#[tokio::test]
async fn linked_private_inventory_is_refused_without_touching_its_target() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let target = root.join("original");
    std::fs::create_dir(&target).unwrap();
    std::fs::write(target.join("sentinel"), b"foreign bytes").unwrap();
    let alias = root.join("linked");
    std::os::unix::fs::symlink(&target, &alias).unwrap();
    assert!(
        ensure_stored_session_idle(alias, "vault".into(), "note".into(), None)
            .await
            .is_err()
    );
    assert_eq!(
        std::fs::read(target.join("sentinel")).unwrap(),
        b"foreign bytes"
    );
}
