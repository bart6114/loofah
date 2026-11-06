use super::*;

pub(super) fn wav() -> Vec<u8> {
    let samples: Vec<u8> = (0..16_000)
        .flat_map(|index| ((index % 1000 - 500) as i16 * 50).to_le_bytes())
        .collect();
    let size = samples.len() as u32;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + size).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&16_000_u32.to_le_bytes());
    bytes.extend_from_slice(&32_000_u32.to_le_bytes());
    bytes.extend_from_slice(&2_u16.to_le_bytes());
    bytes.extend_from_slice(&16_u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&size.to_le_bytes());
    bytes.extend_from_slice(&samples);
    bytes
}

#[tokio::test]
async fn waveform_reads_are_owner_and_vault_scoped_and_do_not_publish_stale_sidecars() {
    let fixture = Fixture::new().await;
    let alice = fixture.login("alice").await;
    fixture.login("bob").await;
    let vault = &alice.personal_vault_id;
    let context = fixture.service.vault(vault).await.unwrap();
    let meta = serde_json::from_value(
        json!({"id":"note","title":"Audio","created_at":"2026-11-06T10:00:00Z","tags":[]}),
    )
    .unwrap();
    context
        .store
        .create_note_session(&meta, "Keep this note")
        .await
        .unwrap();
    let directory = context.store.vault_base().join("sessions/note");
    let bytes = wav();
    std::fs::write(directory.join("audio.wav"), &bytes).unwrap();
    std::fs::write(directory.join("audio.peaks.json"), "Old cache canary").unwrap();
    std::fs::write(directory.join("unknown.md"), "User attachment").unwrap();
    std::fs::write(directory.join(".hidden"), "Private attachment").unwrap();
    let names = [
        "_meta.json",
        "notes.md",
        "audio.wav",
        "audio.peaks.json",
        "unknown.md",
        ".hidden",
    ];
    let before: Vec<_> = names
        .iter()
        .map(|name| std::fs::read(directory.join(name)).unwrap())
        .collect();
    let lease = uuid::Uuid::new_v4();
    let open = format!("/api/v1/vaults/{vault}/sessions/note/audio/leases/{lease}");
    let path = format!("/api/v1/vaults/{vault}/audio/leases/{lease}/peaks");
    let (status, info) = fixture.call("alice", "PUT", &open, None).await;
    assert_eq!(status, StatusCode::OK);
    let (status, value) = fixture.call("alice", "GET", &path, None).await;
    assert_eq!(status, StatusCode::OK);
    let waveform: RecordingWaveform = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(waveform.recording_sha256, info["metadata"]["sha256"]);
    assert_eq!(waveform.peaks.duration_sec, 1.0);
    assert_eq!(waveform.peaks.channels.len(), 1);
    assert_eq!(waveform.peaks.channels[0].len(), 50);
    assert!(
        waveform.peaks.channels[0]
            .iter()
            .all(|peak| peak.is_finite() && *peak > 0.0 && *peak < 1.0)
    );
    assert_eq!(
        fixture.call("bob", "GET", &path, None).await.0,
        StatusCode::NOT_FOUND
    );
    let stale = fixture
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri(&path)
                .header(
                    "authorization",
                    format!("Bearer {}", fixture.token("alice")),
                )
                .header("x-loofah-api-major", "1")
                .header("if-match", "\"stale\"")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(stale.status(), StatusCode::PRECONDITION_FAILED);
    assert_eq!(stale.headers()["cache-control"], "no-store");
    assert_eq!(fixture.call("alice", "GET", &path, None).await.1, value);
    for (index, name) in names.iter().enumerate() {
        assert_eq!(std::fs::read(directory.join(name)).unwrap(), before[index]);
    }
    let close = format!("/api/v1/vaults/{vault}/audio/leases/{lease}");
    assert_eq!(
        fixture.call("alice", "DELETE", &close, None).await.0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        fixture.call("alice", "GET", &path, None).await.0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn waveform_authorization_monitor_allows_prompt_revocation_and_cancels_on_expiry_and_shutdown()
 {
    let fixture = Fixture::new().await;
    fixture.login("alice").await;
    let bob = fixture.login("bob").await;
    let (_, shared) = fixture
        .call(
            "alice",
            "POST",
            "/api/v1/vaults",
            Some(json!({"name":"Audio"})),
        )
        .await;
    let vault = shared["id"].as_str().unwrap().to_owned();
    let (status, shared) = fixture
        .call(
            "alice",
            "PUT",
            &format!("/api/v1/vaults/{vault}/members/{}", bob.member.id),
            Some(json!({"role":"reader","expected_revision":shared["revision"]})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let identity = fixture
        .service
        .auth
        .authenticate(&fixture.token("bob"))
        .await
        .unwrap();
    let monitor = crate::recording::WaveformAccess::watch(
        fixture.service.clone(),
        identity.clone(),
        vault.clone(),
    );
    let remove = format!(
        "/api/v1/vaults/{vault}/members/{}?expected_revision={}",
        bob.member.id,
        shared["revision"].as_str().unwrap()
    );
    let removal = fixture.call("alice", "DELETE", &remove, None);
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(2), removal)
            .await
            .unwrap()
            .0,
        StatusCode::OK
    );
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        monitor.cancel.cancelled(),
    )
    .await
    .unwrap();
    drop(monitor);
    let mut expired = fixture
        .service
        .auth
        .authenticate(&fixture.token("alice"))
        .await
        .unwrap();
    expired.expires_at = now();
    let expiry =
        crate::recording::WaveformAccess::watch(fixture.service.clone(), expired, vault.clone());
    tokio::time::timeout(std::time::Duration::from_secs(2), expiry.cancel.cancelled())
        .await
        .unwrap();
    let owner = fixture
        .service
        .auth
        .authenticate(&fixture.token("alice"))
        .await
        .unwrap();
    let shutdown = crate::recording::WaveformAccess::watch(fixture.service.clone(), owner, vault);
    fixture.service.stop.cancel();
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        shutdown.cancel.cancelled(),
    )
    .await
    .unwrap();
}
