use std::fs::{self, File};

use vault_sync::{crypto::VaultKey, replica::LocalReplica, scope::Entity, snapshot::Snapshot};

#[test]
#[ignore = "requires LOOFAH_SYNC_FIXTURE_ROOT on a volume with at least 50 GB free"]
fn encrypted_7_6_gb_9000_file_restore_preserves_excluded_content() {
    let root = std::env::var_os("LOOFAH_SYNC_FIXTURE_ROOT").expect("set LOOFAH_SYNC_FIXTURE_ROOT");
    assert!(fs2::available_space(&root).unwrap() > 50_000_000_000);
    let fixture = tempfile::Builder::new()
        .prefix("loofah-sync-acceptance-")
        .tempdir_in(root)
        .unwrap();
    let source = fixture.path().join("source");
    let target = fixture.path().join("target");
    let spool = fixture.path().join("spool");
    for vault in [&source, &target] {
        let session = vault.join("sessions/one");
        fs::create_dir_all(&session).unwrap();
        fs::write(session.join("_meta.json"), br#"{"id":"one","title":"fixture","created_at":"2026-09-16","tags":[],"future":{"preserve":true}}"#).unwrap();
        fs::write(session.join("notes.md"), b"before").unwrap();
        fs::write(session.join("private.pdf"), b"excluded attachment").unwrap();
        fs::write(vault.join("config.json"), b"excluded settings").unwrap();
    }
    let source_session = source.join("sessions/one");
    fs::write(source_session.join("notes.md"), b"restored note").unwrap();
    fs::create_dir_all(source_session.join("attachments")).unwrap();
    File::create(source_session.join("audio.mp3"))
        .unwrap()
        .set_len(7_600_000_000)
        .unwrap();
    for index in 0..8997 {
        fs::write(
            source_session.join(format!("attachments/{index:05}.txt")),
            format!("attachment {index}\n"),
        )
        .unwrap();
    }
    fs::write(
        source_session.join("attachments/.private"),
        b"excluded dotfile",
    )
    .unwrap();
    fs::create_dir_all(&spool).unwrap();
    let entity = Entity::Session("one".into());
    let before = Snapshot::capture(&target, entity.clone(), &spool).unwrap();
    let snapshot = Snapshot::capture(&source, entity.clone(), &spool).unwrap();
    assert_eq!(snapshot.manifest().files.len(), 9000);
    let key = VaultKey::generate(uuid::Uuid::new_v4()).unwrap();
    let encrypted = key.prepare_version(&snapshot, None, &spool).unwrap();
    let restored = key
        .restore_snapshot(
            encrypted.revision,
            &encrypted.encrypted_manifest,
            &encrypted.manifest.membership(),
            &spool,
            |object| Ok(File::open(&encrypted.new_objects[&object])?),
        )
        .unwrap();
    LocalReplica::open(&target, &spool)
        .unwrap()
        .apply(before.manifest(), &restored)
        .unwrap();
    let installed = Snapshot::capture(&target, entity, &spool).unwrap();
    assert_eq!(installed.manifest(), snapshot.manifest());
    assert_eq!(
        fs::read(target.join("sessions/one/private.pdf")).unwrap(),
        b"excluded attachment"
    );
    assert_eq!(
        fs::read(target.join("config.json")).unwrap(),
        b"excluded settings"
    );
    assert!(!target.join("sessions/one/attachments/.private").exists());
}
