use super::*;

fn session(root: &Path, id: &str, note: &str) {
    let meta = serde_json::json!({"id":id,"title":"Planning","started_at":null,"ended_at":null,"created_at":"2026-09-26T10:00:00Z","tags":[]});
    atomic_write(
        root,
        &format!("sessions/{id}/_meta.json"),
        &serde_json::to_vec(&meta).unwrap(),
    )
    .unwrap();
    atomic_write(root, &format!("sessions/{id}/notes.md"), note.as_bytes()).unwrap();
}
fn fixture() -> (tempfile::TempDir, Engine) {
    let temp = tempfile::tempdir().unwrap();
    for child in ["local", "remote", "state"] {
        std::fs::create_dir(temp.path().join(child)).unwrap();
    }
    let engine = Engine::open(
        temp.path().join("local"),
        temp.path().join("remote"),
        temp.path().join("state"),
    )
    .unwrap();
    (temp, engine)
}
fn note(engine: &Engine, remote: bool, id: &str) -> String {
    std::fs::read_to_string(
        if remote {
            &engine.remote
        } else {
            &engine.local
        }
        .join(format!("sessions/{id}/notes.md")),
    )
    .unwrap()
}

#[test]
fn text_round_trip_and_hashes_survive_restart() {
    let (_temp, mut engine) = fixture();
    session(&engine.remote, "s1", "cloud");
    engine.reconcile(64).unwrap();
    assert_eq!(note(&engine, false, "s1"), "cloud");
    atomic_write(&engine.local, "sessions/s1/notes.md", b"phone").unwrap();
    engine = Engine::open(&engine.local, &engine.remote, &engine.state_dir).unwrap();
    engine.reconcile(64).unwrap();
    assert_eq!(note(&engine, true, "s1"), "phone");
}

#[test]
fn session_deletion_requires_an_applied_session_tombstone() {
    let (_temp, mut engine) = fixture();
    assert!(!engine.is_session_deleted("missing").unwrap());
    session(&engine.remote, "s1", "base");
    engine.reconcile(64).unwrap();
    record_deletion(&engine.remote, Path::new("sessions/s1/notes.md")).unwrap();
    engine.reconcile(64).unwrap();
    assert!(!engine.is_session_deleted("s1").unwrap());
    record_deletion(&engine.remote, Path::new("sessions/s1")).unwrap();
    assert!(!engine.is_session_deleted("s1").unwrap());
    engine.reconcile(64).unwrap();
    assert!(engine.is_session_deleted("s1").unwrap());
    engine = Engine::open(&engine.local, &engine.remote, &engine.state_dir).unwrap();
    assert!(engine.is_session_deleted("s1").unwrap());
    record_restore(&engine.local, Path::new("sessions/s1")).unwrap();
    assert!(!engine.is_session_deleted("s1").unwrap());
    engine.reconcile(64).unwrap();
    assert!(!engine.is_session_deleted("s1").unwrap());
    assert!(engine.is_session_deleted("../invalid").is_err());
}

#[test]
fn concurrent_edits_require_resolution_and_keep_both_recovers_session() {
    let (_temp, mut engine) = fixture();
    session(&engine.remote, "s1", "base");
    engine.reconcile(64).unwrap();
    atomic_write(&engine.local, "sessions/s1/notes.md", b"phone").unwrap();
    atomic_write(&engine.remote, "sessions/s1/notes.md", b"desktop").unwrap();
    let status = engine.reconcile(64).unwrap();
    assert_eq!(status.conflicts.len(), 1);
    assert_eq!(note(&engine, false, "s1"), "phone");
    assert_eq!(note(&engine, true, "s1"), "desktop");
    engine
        .resolve(&status.conflicts[0].id, Resolution::KeepBoth)
        .unwrap();
    assert_eq!(note(&engine, false, "s1"), "desktop");
    let sessions = hypr_vault_read::discover_sessions(&engine.local)
        .unwrap()
        .sessions;
    assert_eq!(sessions.len(), 2);
    let recovered = sessions.iter().find(|(l, _)| l.id != "s1").unwrap();
    assert_eq!(note(&engine, false, &recovered.0.id), "phone");
}

#[test]
fn resolution_refuses_changes_since_review() {
    let (_temp, mut engine) = fixture();
    session(&engine.local, "s1", "phone");
    session(&engine.remote, "s1", "desktop");
    let status = engine.reconcile(64).unwrap();
    atomic_write(&engine.remote, "sessions/s1/notes.md", b"new desktop edit").unwrap();
    assert!(
        engine
            .resolve(&status.conflicts[0].id, Resolution::KeepLocal)
            .is_err()
    );
    assert_eq!(note(&engine, true, "s1"), "new desktop edit");
    assert_eq!(engine.conflicts().len(), 1);
}

#[test]
fn missing_cloud_file_never_deletes_or_overwrites_local_content() {
    let (_temp, mut engine) = fixture();
    session(&engine.remote, "s1", "base");
    engine.reconcile(64).unwrap();
    std::fs::remove_file(engine.remote.join("sessions/s1/notes.md")).unwrap();
    atomic_write(&engine.local, "sessions/s1/notes.md", b"offline edit").unwrap();
    engine.reconcile(64).unwrap();
    assert_eq!(note(&engine, false, "s1"), "offline edit");
    assert!(!engine.remote.join("sessions/s1/notes.md").exists());
}

#[test]
fn deletion_against_local_edit_recovers_new_id_and_leaves_loose_attachments() {
    let (_temp, mut engine) = fixture();
    session(&engine.remote, "s1", "base");
    engine.reconcile(64).unwrap();
    atomic_write(&engine.local, "sessions/s1/notes.md", b"offline edit").unwrap();
    atomic_write(&engine.local, "sessions/s1/private.pdf", b"not ours").unwrap();
    record_deletion(&engine.remote, Path::new("sessions/s1")).unwrap();
    std::fs::remove_dir_all(engine.remote.join("sessions/s1")).unwrap();
    let status = engine.reconcile(64).unwrap();
    assert_eq!(status.recovered_sessions.len(), 1);
    assert_eq!(
        note(&engine, false, &status.recovered_sessions[0]),
        "offline edit"
    );
    assert!(!engine.local.join("sessions/s1/_meta.json").exists());
    assert_eq!(
        std::fs::read(engine.local.join("sessions/s1/private.pdf")).unwrap(),
        b"not ours"
    );
    assert!(
        !engine
            .remote
            .join(format!(
                "sessions/{}/private.pdf",
                status.recovered_sessions[0]
            ))
            .exists()
    );
}

#[test]
fn inventory_ignores_unknown_files_and_transient_recordings() {
    let (_temp, mut engine) = fixture();
    session(&engine.local, "s1", "base");
    for name in [
        "minutes.md",
        "private.pdf",
        ".hidden",
        "audio_mic.wav",
        "audio.wav.tmp",
    ] {
        atomic_write(&engine.local, &format!("sessions/s1/{name}"), b"private").unwrap();
    }
    engine.reconcile(64).unwrap();
    for name in [
        "minutes.md",
        "private.pdf",
        ".hidden",
        "audio_mic.wav",
        "audio.wav.tmp",
    ] {
        assert!(!engine.remote.join(format!("sessions/s1/{name}")).exists());
    }
}

#[test]
fn media_is_uploaded_durably_and_downloaded_only_on_request() {
    let (_temp, mut engine) = fixture();
    session(&engine.remote, "s1", "base");
    atomic_write(&engine.remote, "sessions/s1/audio.wav", b"cloud recording").unwrap();
    engine.reconcile(64).unwrap();
    assert!(!engine.local.join("sessions/s1/audio.wav").exists());
    assert_eq!(
        std::fs::read(engine.download_media("s1", "audio.wav").unwrap()).unwrap(),
        b"cloud recording"
    );
    atomic_write(&engine.local, "sessions/s1/attachments/image.png", b"image").unwrap();
    engine.reconcile(64).unwrap();
    engine = Engine::open(&engine.local, &engine.remote, &engine.state_dir).unwrap();
    engine.reconcile(64).unwrap();
    assert_eq!(
        std::fs::read(engine.remote.join("sessions/s1/attachments/image.png")).unwrap(),
        b"image"
    );
    assert!(engine.download_media("s1", "../secret").is_err());
    assert!(engine.download_media("s1", "private.pdf").is_err());
}

#[cfg(unix)]
#[test]
fn symlinked_owned_files_are_rejected() {
    let (temp, mut engine) = fixture();
    session(&engine.remote, "s1", "base");
    std::fs::write(temp.path().join("secret"), b"private").unwrap();
    std::fs::remove_file(engine.remote.join("sessions/s1/notes.md")).unwrap();
    std::os::unix::fs::symlink(
        temp.path().join("secret"),
        engine.remote.join("sessions/s1/notes.md"),
    )
    .unwrap();
    assert!(engine.reconcile(64).is_err());
    assert!(!engine.local.join("sessions/s1/notes.md").exists());
}

#[test]
fn bounded_passes_eventually_visit_later_paths() {
    let (_temp, mut engine) = fixture();
    for id in ["a", "b", "c"] {
        session(&engine.remote, id, id);
    }
    for _ in 0..6 {
        engine.reconcile(1).unwrap();
    }
    for id in ["a", "b", "c"] {
        assert_eq!(note(&engine, false, id), id);
    }
}

#[test]
fn deletion_invalidates_conflicts_and_cached_recordings() {
    let (_temp, mut engine) = fixture();
    session(&engine.local, "s1", "phone");
    session(&engine.remote, "s1", "desktop");
    atomic_write(&engine.local, "sessions/s1/audio.wav", b"recording").unwrap();
    let status = engine.reconcile(64).unwrap();
    let conflict = status.conflicts.first().unwrap().id.clone();
    record_deletion(&engine.remote, Path::new("sessions/s1")).unwrap();
    assert!(engine.download_media("s1", "audio.wav").is_err());
    assert!(engine.resolve(&conflict, Resolution::KeepLocal).is_err());
    engine.reconcile(64).unwrap();
    assert!(
        !engine
            .conflicts()
            .iter()
            .any(|c| c.path.starts_with("sessions/s1/"))
    );
    assert!(!engine.local.join("sessions/s1/_meta.json").exists());
    assert!(!engine.local.join("sessions/s1/audio.wav").exists());
}

#[test]
fn session_deletion_recovers_an_unsynced_recording_with_unchanged_text() {
    let (_temp, mut engine) = fixture();
    session(&engine.remote, "s1", "base");
    engine.reconcile(64).unwrap();
    atomic_write(&engine.local, "sessions/s1/audio.wav", b"offline recording").unwrap();
    atomic_write(
        &engine.local,
        "sessions/s1/attachments/image.png",
        b"offline image",
    )
    .unwrap();
    atomic_write(&engine.local, "sessions/s1/private.pdf", b"not ours").unwrap();
    record_deletion(&engine.remote, Path::new("sessions/s1")).unwrap();
    std::fs::remove_dir_all(engine.remote.join("sessions/s1")).unwrap();
    let status = engine.reconcile(64).unwrap();
    assert_eq!(status.recovered_sessions.len(), 1);
    let recovered = &status.recovered_sessions[0];
    assert_eq!(note(&engine, false, recovered), "base");
    for (name, bytes) in [
        ("audio.wav", b"offline recording".as_slice()),
        ("attachments/image.png", b"offline image".as_slice()),
    ] {
        assert_eq!(
            std::fs::read(engine.local.join(format!("sessions/{recovered}/{name}"))).unwrap(),
            bytes
        );
        assert!(!engine.local.join(format!("sessions/s1/{name}")).exists());
    }
    assert!(engine.local.join("sessions/s1/private.pdf").exists());
    let next = engine.reconcile(64).unwrap();
    assert!(next.recovered_sessions.is_empty());
    assert_eq!(
        hypr_vault_read::discover_sessions(&engine.local)
            .unwrap()
            .sessions
            .len(),
        1
    );
}

#[test]
fn session_deletion_removes_unchanged_synced_media_without_recovery_after_restart() {
    let (_temp, mut engine) = fixture();
    session(&engine.remote, "s1", "base");
    atomic_write(&engine.remote, "sessions/s1/audio.wav", b"recording").unwrap();
    engine.reconcile(64).unwrap();
    engine.download_media("s1", "audio.wav").unwrap();
    engine = Engine::open(&engine.local, &engine.remote, &engine.state_dir).unwrap();
    record_deletion(&engine.remote, Path::new("sessions/s1")).unwrap();
    std::fs::remove_dir_all(engine.remote.join("sessions/s1")).unwrap();
    let status = engine.reconcile(64).unwrap();
    assert!(status.recovered_sessions.is_empty());
    assert!(!engine.local.join("sessions/s1/audio.wav").exists());
}

#[test]
fn session_deletion_recovers_media_modified_since_upload_after_restart() {
    let (_temp, mut engine) = fixture();
    session(&engine.local, "s1", "base");
    atomic_write(&engine.local, "sessions/s1/audio.wav", b"original").unwrap();
    engine.reconcile(64).unwrap();
    engine = Engine::open(&engine.local, &engine.remote, &engine.state_dir).unwrap();
    atomic_write(&engine.local, "sessions/s1/audio.wav", b"replacement").unwrap();
    record_deletion(&engine.remote, Path::new("sessions/s1")).unwrap();
    std::fs::remove_dir_all(engine.remote.join("sessions/s1")).unwrap();
    let status = engine.reconcile(64).unwrap();
    assert_eq!(status.recovered_sessions.len(), 1);
    assert_eq!(
        std::fs::read(engine.local.join(format!(
            "sessions/{}/audio.wav",
            status.recovered_sessions[0]
        )))
        .unwrap(),
        b"replacement"
    );
}

#[test]
fn provider_conflict_refresh_preserves_version_identity_and_all_three_versions() {
    let (_temp, mut engine) = fixture();
    session(&engine.local, "s1", "phone");
    session(&engine.remote, "s1", "current cloud");
    let path = "sessions/s1/notes.md";
    let id = engine
        .save_conflict(path, b"phone", b"provider version")
        .unwrap();
    engine
        .set_provider_conflict(&id, "provider token".into(), b"current cloud")
        .unwrap();
    atomic_write(&engine.local, path, b"new phone edit").unwrap();
    atomic_write(&engine.remote, path, b"new cloud edit").unwrap();
    assert!(engine.resolve(&id, Resolution::KeepRemote).is_err());
    let refreshed = engine.conflicts().pop().unwrap();
    assert_ne!(refreshed.id, id);
    assert_eq!(
        refreshed.provider_version.as_deref(),
        Some("provider token")
    );
    assert_eq!(
        refreshed.provider_current_hash,
        Some(hash(b"new cloud edit"))
    );
    assert_eq!(refreshed.remote_hash, hash(b"provider version"));
    assert_eq!(
        std::fs::read(
            engine
                .remote
                .join(format!(".loofah-sync/conflicts/{}/current", refreshed.id))
        )
        .unwrap(),
        b"new cloud edit"
    );
    engine
        .resolve(&refreshed.id, Resolution::KeepRemote)
        .unwrap();
    assert_eq!(note(&engine, true, "s1"), "provider version");
    assert_eq!(note(&engine, false, "s1"), "provider version");
}

#[test]
fn file_deletion_and_recreation_propagate_without_reviving_old_bytes() {
    let (_temp, mut engine) = fixture();
    session(&engine.remote, "s1", "base");
    atomic_write(&engine.remote, "sessions/s1/summary.md", b"old summary").unwrap();
    engine.reconcile(64).unwrap();
    record_deletion(&engine.remote, Path::new("sessions/s1/summary.md")).unwrap();
    std::fs::remove_file(engine.remote.join("sessions/s1/summary.md")).unwrap();
    engine.reconcile(64).unwrap();
    assert!(!engine.local.join("sessions/s1/summary.md").exists());
    atomic_write(&engine.local, "sessions/s1/summary.md", b"new summary").unwrap();
    record_write(&engine.local, Path::new("sessions/s1/summary.md")).unwrap();
    engine.reconcile(64).unwrap();
    assert_eq!(
        std::fs::read(engine.remote.join("sessions/s1/summary.md")).unwrap(),
        b"new summary"
    );
}

#[cfg(target_vendor = "apple")]
#[test]
fn unavailable_cloud_placeholder_never_gets_overwritten_on_first_sync() {
    let (_temp, mut engine) = fixture();
    session(&engine.local, "s1", "phone");
    session(&engine.remote, "s1", "cloud");
    std::fs::remove_file(engine.remote.join("sessions/s1/notes.md")).unwrap();
    atomic_write(
        &engine.remote,
        "sessions/s1/.notes.md.icloud",
        b"provider placeholder",
    )
    .unwrap();
    assert!(engine.reconcile(64).is_err());
    assert!(!engine.remote.join("sessions/s1/notes.md").exists());
    assert_eq!(note(&engine, false, "s1"), "phone");
}

#[cfg(target_vendor = "apple")]
#[test]
fn evicted_tombstone_blocks_reconciliation_until_downloaded() {
    let (_temp, mut engine) = fixture();
    session(&engine.local, "s1", "phone");
    enable_participation(&engine.remote).unwrap();
    let tombstone = tombstone_path("sessions/s1");
    let filename = Path::new(&tombstone).file_name().unwrap().to_str().unwrap();
    atomic_write(
        &engine.remote,
        &format!(".loofah-sync/tombstones/.{filename}.icloud"),
        b"provider placeholder",
    )
    .unwrap();
    assert!(engine.reconcile(64).is_err());
    assert!(!engine.remote.join("sessions/s1/_meta.json").exists());
}

#[test]
fn config_prompt_language_and_unknown_mobile_fields_sync_in_both_directions() {
    let (_temp, mut engine) = fixture();
    let desktop = serde_json::json!({
        "ai_language": "nl", "auto_summary_prompt": "Preserve decisions and questions.",
        "mobile": {"transcription_model": "parakeet-v3", "future_setting": {"enabled": true}},
        "future_desktop_setting": ["keep", "me"]
    });
    let desktop_bytes = serde_json::to_vec(&desktop).unwrap();
    atomic_write(&engine.remote, "config.json", &desktop_bytes).unwrap();
    engine.reconcile(64).unwrap();
    assert_eq!(
        std::fs::read(engine.local.join("config.json")).unwrap(),
        desktop_bytes
    );
    let mut phone = desktop;
    phone["ai_language"] = "en".into();
    phone["auto_summary_prompt"] = "Keep action owners explicit.".into();
    phone["mobile"]["transcription_model"] = "parakeet-v2".into();
    let phone_bytes = serde_json::to_vec(&phone).unwrap();
    atomic_write(&engine.local, "config.json", &phone_bytes).unwrap();
    engine = Engine::open(&engine.local, &engine.remote, &engine.state_dir).unwrap();
    engine.reconcile(64).unwrap();
    assert_eq!(
        std::fs::read(engine.remote.join("config.json")).unwrap(),
        phone_bytes
    );
    assert_eq!(phone["mobile"]["future_setting"]["enabled"], true);
    assert_eq!(
        phone["future_desktop_setting"],
        serde_json::json!(["keep", "me"])
    );
}

#[test]
fn concurrent_config_edits_preserve_both_versions_until_explicit_resolution() {
    let (_temp, mut engine) = fixture();
    let base = br#"{"ai_language":"nl","auto_summary_prompt":"Base"}"#;
    atomic_write(&engine.remote, "config.json", base).unwrap();
    engine.reconcile(64).unwrap();
    let local = br#"{"ai_language":"en","auto_summary_prompt":"Phone","mobile":{"future":true}}"#;
    let remote = br#"{"ai_language":"nl","auto_summary_prompt":"Desktop","custom":1}"#;
    atomic_write(&engine.local, "config.json", local).unwrap();
    atomic_write(&engine.remote, "config.json", remote).unwrap();
    let status = engine.reconcile(64).unwrap();
    assert_eq!(status.conflicts.len(), 1);
    let conflict = &status.conflicts[0];
    assert_eq!(conflict.path, "config.json");
    assert_eq!(
        std::fs::read(engine.local.join("config.json")).unwrap(),
        local
    );
    assert_eq!(
        std::fs::read(engine.remote.join("config.json")).unwrap(),
        remote
    );
    assert!(engine.resolve(&conflict.id, Resolution::KeepBoth).is_err());
    assert_eq!(
        std::fs::read(engine.local.join("config.json")).unwrap(),
        local
    );
    assert_eq!(
        std::fs::read(engine.remote.join("config.json")).unwrap(),
        remote
    );
    engine
        .resolve(&conflict.id, Resolution::KeepRemote)
        .unwrap();
    assert_eq!(
        std::fs::read(engine.local.join("config.json")).unwrap(),
        remote
    );
}

#[test]
fn session_audio_availability_uses_remote_metadata_without_downloading() {
    let (_root, mut engine) = fixture();
    session(&engine.remote, "s1", "note");
    atomic_write(
        &engine.remote,
        "sessions/s1/attachment.wav",
        b"loose attachment",
    )
    .unwrap();
    atomic_write(&engine.remote, "sessions/s1/audio.peaks.json", b"peaks").unwrap();
    atomic_write(&engine.remote, "sessions/s1/audio.wav.tmp", b"unfinished").unwrap();
    assert!(!engine.has_session_audio("s1").unwrap());
    atomic_write(&engine.remote, "sessions/s1/.audio.wav.icloud", b"").unwrap();
    assert!(engine.has_session_audio("s1").unwrap());
    assert!(!engine.local.join("sessions/s1/audio.wav").exists());
    std::fs::remove_file(engine.remote.join("sessions/s1/.audio.wav.icloud")).unwrap();
    atomic_write(&engine.remote, "sessions/s1/audio.wav", b"cloud recording").unwrap();
    assert!(engine.has_session_audio("s1").unwrap());
    assert!(!engine.local.join("sessions/s1/audio.wav").exists());
    std::fs::remove_file(engine.remote.join("sessions/s1/audio.wav")).unwrap();
    assert!(!engine.has_session_audio("s1").unwrap());
    engine
        .state
        .media_uploaded
        .insert("sessions/s1/audio.wav".into(), "known-version".into());
    assert!(engine.has_session_audio("s1").unwrap());
    assert!(engine.has_session_audio("../s1").is_err());
}
