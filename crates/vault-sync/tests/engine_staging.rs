use libsodium_rs::crypto_sign::KeyPair;
use serde::Deserialize;
use std::{fs, time::Duration};
use uuid::Uuid;
use vault_sync::{
    crypto::VaultKey,
    engine::{Binding, Engine, now_ms},
    enrollment::{Challenge, ExpectedIdentity, Kind, hex},
    remote::{Environment, RemoteClient},
    scope::Entity,
};

#[derive(Deserialize)]
struct Session {
    session: String,
    token: String,
}
#[derive(Deserialize)]
struct Fixture {
    user: String,
    vault: Uuid,
    generation: Uuid,
    sessions: Vec<Session>,
    kit: Option<Vec<u8>>,
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires disposable staging account in LOOFAH_SYNC_ENGINE_FIXTURE"]
async fn two_replicas_create_conflict_resolve_and_restore() {
    let fixture_path = std::env::var("LOOFAH_SYNC_ENGINE_FIXTURE").unwrap();
    let mut fixture_json: serde_json::Value =
        serde_json::from_slice(&fs::read(&fixture_path).unwrap()).unwrap();
    let fixture: Fixture = serde_json::from_value(fixture_json.clone()).unwrap();
    let key = fixture
        .kit
        .as_ref()
        .map(|kit| VaultKey::from_recovery_kit(kit, fixture.vault).unwrap())
        .unwrap_or_else(|| VaultKey::generate(fixture.vault).unwrap());
    fixture_json["kit"] = serde_json::json!(key.recovery_kit().to_vec());
    fs::write(&fixture_path, serde_json::to_vec(&fixture_json).unwrap()).unwrap();
    let id = Uuid::new_v4().to_string();
    let kit = key.recovery_kit();
    let authority = hex(key.enrollment_authority().unwrap().public_key.as_bytes());
    let mut clients = Vec::new();
    for session in &fixture.sessions {
        let remote = RemoteClient::new(
            Environment::Staging,
            &session.token,
            Some(fixture.generation),
        )
        .unwrap();
        let device = Uuid::new_v4();
        let pair = KeyPair::generate().unwrap();
        let public = hex(pair.public_key.as_bytes());
        let challenge: Challenge = remote.post("/enrollment/challenge", &serde_json::json!({"kind":"enroll","device":device,"publicKey":public,"authority":authority})).await.unwrap();
        let checked = challenge
            .verify(
                ExpectedIdentity {
                    vault: fixture.vault,
                    user: &fixture.user,
                    session: &session.session,
                    device,
                    public_key: &public,
                    authority: &authority,
                    generation: fixture.generation,
                    kind: Kind::Enroll,
                },
                now_ms(),
            )
            .unwrap();
        remote.post::<serde_json::Value>("/enrollment/finish",&serde_json::json!({"challenge":checked.id(),"deviceSignature":checked.sign_device(&pair).unwrap(),"authoritySignature":checked.approve_enrollment(&key).unwrap()})).await.unwrap();
        clients.push(remote);
    }
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    let first_state = tempfile::tempdir().unwrap();
    let second_state = tempfile::tempdir().unwrap();
    fs::create_dir_all(first.path().join(format!("sessions/{id}"))).unwrap();
    fs::write(
        first.path().join(format!("sessions/{id}/_meta.json")),
        serde_json::to_vec(&serde_json::json!({"id":id,"title":"engine test","created_at":"2026-09-17","tags":[],"future":true})).unwrap(),
    )
    .unwrap();
    fs::write(
        first.path().join(format!("sessions/{id}/notes.md")),
        b"original",
    )
    .unwrap();
    fs::write(
        first.path().join(format!("sessions/{id}/audio.mp3")),
        b"recording fixture",
    )
    .unwrap();
    let store_a = hypr_vault_write::SessionStore::new(first.path().into());
    let store_b = hypr_vault_write::SessionStore::new(second.path().into());
    store_a.rebuild_index().await.unwrap();
    store_b.rebuild_index().await.unwrap();
    let binding = |local: &std::path::Path| Binding {
        account: fixture.user.clone(),
        vault: fixture.vault,
        generation: fixture.generation,
        local: local.into(),
    };
    let mut a = Engine::open(
        first_state.path(),
        binding(first.path()),
        key,
        clients.remove(0),
    )
    .unwrap();
    let mut b = Engine::open(
        second_state.path(),
        binding(second.path()),
        VaultKey::from_recovery_kit(&kit, fixture.vault).unwrap(),
        clients.remove(0),
    )
    .unwrap();
    a.tick(&store_a).await.unwrap();
    tokio::time::sleep(Duration::from_secs(11)).await;
    a.tick(&store_a).await.unwrap();
    b.tick(&store_b).await.unwrap();
    assert_eq!(
        store_b.read_note(&id).await.unwrap().as_deref(),
        Some("original")
    );
    let entity = Entity::Session(id.clone());
    let first_revision: Uuid =
        serde_json::from_value(a.history(&entity).await.unwrap()["results"][0]["id"].clone())
            .unwrap();
    fs::write(
        second.path().join(format!("sessions/{id}/private.pdf")),
        b"excluded",
    )
    .unwrap();
    store_a.write_note(&id, "edit on first").await.unwrap();
    a.changed(entity.clone());
    store_b.write_note(&id, "edit on second").await.unwrap();
    b.changed(entity.clone());
    tokio::time::sleep(Duration::from_secs(11)).await;
    a.tick(&store_a).await.unwrap();
    b.tick(&store_b).await.unwrap();
    assert_eq!(b.conflicts().len(), 1);
    assert_eq!(
        store_b.read_note(&id).await.unwrap().as_deref(),
        Some("edit on second")
    );
    let local = b.conflicts()[0].local;
    b.restore(&store_b, entity.clone(), local).await.unwrap();
    a.tick(&store_a).await.unwrap();
    assert_eq!(
        store_a.read_note(&id).await.unwrap().as_deref(),
        Some("edit on second")
    );
    a.restore(&store_a, entity.clone(), first_revision)
        .await
        .unwrap();
    b.tick(&store_b).await.unwrap();
    assert_eq!(
        store_b.read_note(&id).await.unwrap().as_deref(),
        Some("original")
    );
    assert_eq!(
        fs::read(second.path().join(format!("sessions/{id}/private.pdf"))).unwrap(),
        b"excluded"
    );
    assert_eq!(
        fs::read(second.path().join(format!("sessions/{id}/audio.mp3"))).unwrap(),
        b"recording fixture"
    );
    assert!(
        a.history(&entity).await.unwrap()["results"]
            .as_array()
            .unwrap()
            .len()
            >= 5
    );
    let preview = a.preview(&entity, first_revision).await.unwrap();
    assert_eq!(preview.text, "original");
    assert!(preview.captured_at.is_some());
    assert_eq!(preview.title.as_deref(), Some("engine test"));
    assert!(preview.created_at > 0);
    assert!(!preview.device_id.is_nil());
    let receipts = hypr_vault_write::sync_deletions::directory(first.path()).unwrap();
    assert!(!receipts.exists());
    fs::create_dir_all(&receipts).unwrap();
    fs::write(receipts.join("connection.json"), b"{}").unwrap();
    struct ReceiptCleanup(std::path::PathBuf);
    impl Drop for ReceiptCleanup {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = ReceiptCleanup(receipts);
    store_a.delete_session(&id).await.unwrap();
    assert!(
        hypr_vault_write::sync_deletions::read(first.path(), &id)
            .unwrap()
            .is_some()
    );
    a.changed(entity.clone());
    tokio::time::sleep(Duration::from_secs(11)).await;
    a.tick(&store_a).await.unwrap();
    b.tick(&store_b).await.unwrap();
    assert!(
        !second
            .path()
            .join(format!("sessions/{id}/_meta.json"))
            .exists()
    );
    assert_eq!(
        fs::read(second.path().join(format!("sessions/{id}/private.pdf"))).unwrap(),
        b"excluded"
    );
    b.restore(&store_b, entity.clone(), first_revision)
        .await
        .unwrap();
    a.tick(&store_a).await.unwrap();
    assert_eq!(
        store_a.read_note(&id).await.unwrap().as_deref(),
        Some("original")
    );
    assert!(a.conflicts().is_empty());
    assert!(b.conflicts().is_empty());
}
