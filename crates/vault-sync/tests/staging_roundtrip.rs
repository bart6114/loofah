use std::collections::BTreeMap;
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use libsodium_rs::crypto_sign::KeyPair;
use serde::Deserialize;
use uuid::Uuid;
use vault_sync::{
    crypto::VaultKey,
    enrollment::{Challenge, ExpectedIdentity, Kind, hex},
    remote::{Environment, Operation, RemoteClient, Revision},
    scope::Entity,
    snapshot::{FileDigest, Snapshot},
    spool::UploadSpool,
};
use zeroize::Zeroizing;

#[derive(Deserialize)]
struct Fixture {
    token: String,
    user: String,
    session: String,
    vault: Uuid,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.token.zeroize();
    }
}

fn revision(
    key: &VaultKey,
    spool: &UploadSpool,
    expected: Option<Uuid>,
    operation: Operation,
) -> Revision {
    Revision {
        id: spool.revision(),
        entity: key
            .entity_identity(&spool.manifest().manifest.entity)
            .unwrap(),
        expected,
        manifest: spool.revision(),
        operation,
        objects: spool.manifest().membership().into_keys().collect(),
        conflicts: vec![],
    }
}

#[tokio::test]
#[ignore = "requires an administrator-created disposable staging fixture in LOOFAH_SYNC_STAGING_FIXTURE"]
async fn enroll_upload_edit_and_restore_against_staging() {
    let bytes = Zeroizing::new(
        fs::read(
            std::env::var_os("LOOFAH_SYNC_STAGING_FIXTURE").expect("set staging fixture path"),
        )
        .unwrap(),
    );
    let fixture: Fixture = serde_json::from_slice(&bytes).unwrap();
    let authenticated = RemoteClient::new(Environment::Staging, &fixture.token, None).unwrap();
    let account: serde_json::Value = authenticated.get("/account").await.unwrap();
    assert_eq!(account["account"]["vault_id"], fixture.vault.to_string());
    assert_eq!(account["identity"]["user"], fixture.user);
    assert_eq!(account["identity"]["session"], fixture.session);
    assert!(account["account"]["enrollment_authority"].is_null());
    let generation =
        Uuid::parse_str(account["account"]["recovery_generation"].as_str().unwrap()).unwrap();
    let key = VaultKey::generate(fixture.vault).unwrap();
    let device = KeyPair::generate().unwrap();
    let device_id = Uuid::new_v4();
    let public_key = hex(device.public_key.as_bytes());
    let authority = hex(key.enrollment_authority().unwrap().public_key.as_bytes());
    let challenge: Challenge = authenticated.post("/enrollment/challenge", &serde_json::json!({
        "kind": "enroll", "device": device_id, "publicKey": public_key, "authority": authority
    })).await.unwrap();
    let verified = challenge
        .verify(
            ExpectedIdentity {
                vault: fixture.vault,
                user: &fixture.user,
                session: &fixture.session,
                device: device_id,
                public_key: &public_key,
                authority: &authority,
                generation,
                kind: Kind::Enroll,
            },
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64,
        )
        .unwrap();
    let proof = serde_json::json!({
        "challenge": verified.id(), "deviceSignature": verified.sign_device(&device).unwrap(),
        "authoritySignature": verified.approve_enrollment(&key).unwrap()
    });
    let principal: serde_json::Value = authenticated
        .post("/enrollment/finish", &proof)
        .await
        .unwrap();
    assert_eq!(principal["device"], device_id.to_string());
    let repeated: serde_json::Value = authenticated
        .post("/enrollment/finish", &proof)
        .await
        .unwrap();
    assert_eq!(principal, repeated);
    let remote = RemoteClient::new(Environment::Staging, &fixture.token, Some(generation)).unwrap();
    let vault = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let session = vault.path().join("sessions/one");
    fs::create_dir_all(&session).unwrap();
    fs::write(session.join("_meta.json"), br#"{"id":"one","title":"staging fixture","created_at":"2026-09-16","tags":[],"future":{"preserve":true}}"#).unwrap();
    fs::write(session.join("notes.md"), b"original note").unwrap();
    fs::File::create(session.join("audio.mp3"))
        .unwrap()
        .set_len(8 * 1024 * 1024 + 17)
        .unwrap();
    fs::write(session.join("private.pdf"), b"excluded").unwrap();
    let entity = Entity::Session("one".into());
    let first = Snapshot::capture(vault.path(), entity.clone(), state.path()).unwrap();
    let first = UploadSpool::persist(
        key.prepare_version(&first, None, state.path()).unwrap(),
        state.path(),
    )
    .unwrap();
    remote.upload(&first, false).await.unwrap();
    let initial = revision(&key, &first, None, Operation::Checkpoint);
    remote.commit(&initial).await.unwrap();
    remote.commit(&initial).await.unwrap();

    fs::write(session.join("notes.md"), b"edited note").unwrap();
    let second = Snapshot::capture(vault.path(), entity, state.path()).unwrap();
    let second = UploadSpool::persist(
        key.prepare_version(&second, Some(first.manifest()), state.path())
            .unwrap(),
        state.path(),
    )
    .unwrap();
    assert_eq!(second.pending_objects().len(), 1);
    assert_eq!(
        first.manifest().objects["audio.mp3"].object,
        second.manifest().objects["audio.mp3"].object
    );
    remote.upload(&second, false).await.unwrap();
    remote
        .commit(&revision(
            &key,
            &second,
            Some(first.revision()),
            Operation::Checkpoint,
        ))
        .await
        .unwrap();

    let stored: serde_json::Value = remote
        .get(&format!("/sync/revisions/{}", first.revision()))
        .await
        .unwrap();
    let mut membership = BTreeMap::new();
    let mut downloaded = BTreeMap::new();
    for object in stored["objects"].as_array().unwrap() {
        let id = Uuid::parse_str(object["id"].as_str().unwrap()).unwrap();
        let digest = FileDigest {
            bytes: object["bytes"].as_u64().unwrap(),
            sha256: object["digest"].as_str().unwrap().into(),
        };
        let file = remote.download(id, &digest, state.path()).await.unwrap();
        if id != first.revision() {
            membership.insert(id, digest);
        }
        downloaded.insert(id, file);
    }
    let ciphertext = fs::read(&downloaded[&first.revision()]).unwrap();
    let selected = key
        .restore_snapshot(
            first.revision(),
            &ciphertext,
            &membership,
            state.path(),
            |id| Ok(fs::File::open(&downloaded[&id])?),
        )
        .unwrap();
    assert_eq!(
        fs::read(selected.directory().join("notes.md")).unwrap(),
        b"original note"
    );
    assert_eq!(selected.manifest(), &first.manifest().manifest);
    let restored = UploadSpool::persist(
        key.prepare_version(&selected, Some(first.manifest()), state.path())
            .unwrap(),
        state.path(),
    )
    .unwrap();
    assert!(restored.pending_objects().is_empty());
    remote.upload(&restored, true).await.unwrap();
    remote
        .commit(&revision(
            &key,
            &restored,
            Some(second.revision()),
            Operation::Restore,
        ))
        .await
        .unwrap();
    let store = hypr_vault_write::SessionStore::new(vault.path().into());
    store.rebuild_index().await.unwrap();
    vault_sync::replica::LocalReplica::open(vault.path(), state.path())
        .unwrap()
        .apply_to_store(&store, second.manifest().manifest.clone(), selected)
        .await
        .unwrap();
    assert_eq!(
        store.read_note("one").await.unwrap().as_deref(),
        Some("original note")
    );
    let history: serde_json::Value = remote
        .get(&format!("/sync/history/{}", initial.entity))
        .await
        .unwrap();
    let versions = history["results"].as_array().unwrap();
    assert_eq!(versions.len(), 3);
    assert_eq!(
        versions.iter().find(|row| row["current"] == 1).unwrap()["id"],
        restored.revision().to_string()
    );
    assert!(
        versions
            .iter()
            .any(|row| row["id"] == second.revision().to_string())
    );
    assert_eq!(fs::read(session.join("private.pdf")).unwrap(), b"excluded");
}
