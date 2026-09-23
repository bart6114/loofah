use std::process::Command;

#[test]
fn import_persists_mixed_layout_even_with_recording_timestamps() {
    let root = tempfile::tempdir().unwrap();
    let vault = root.path().join("vault");
    let session = vault.join("sessions/existing");
    std::fs::create_dir_all(&session).unwrap();
    std::fs::write(
        session.join("_meta.json"),
        serde_json::json!({
            "id":"existing", "title":"Synthetic", "created_at":"2026-01-01T00:00:00Z",
            "started_at":"2026-01-01T00:00:00Z", "tags":[],
            "audio":{"source":"recording", "layout":"mic_system"}
        })
        .to_string(),
    )
    .unwrap();
    let source = root.path().join("synthetic.wav");
    let mut writer = hound::WavWriter::create(
        &source,
        hound::WavSpec {
            channels: 2,
            sample_rate: 16000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        },
    )
    .unwrap();
    for i in 0..16000 {
        let sample = ((i as f32 * 0.08).sin() * 2000.0) as i16;
        writer.write_sample(sample).unwrap();
        writer.write_sample(sample / 2).unwrap();
    }
    writer.finalize().unwrap();
    let original = std::fs::read(&source).unwrap();
    for args in [
        vec!["--into", "existing"],
        vec!["--started-at", "2026-01-01T00:00:00Z"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_loof"))
            .arg("--vault-path")
            .arg(&vault)
            .arg("--json")
            .arg("import")
            .arg(&source)
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let response: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        let id = response["data"]["id"].as_str().unwrap();
        let directory = vault.join("sessions").join(id);
        let meta: serde_json::Value =
            serde_json::from_slice(&std::fs::read(directory.join("_meta.json")).unwrap()).unwrap();
        assert_eq!(
            meta["audio"],
            serde_json::json!({"source":"import", "layout":"mixed"})
        );
        assert!(meta.get("audio_import_pending").is_none());
        assert!(directory.join("audio.mp3").exists());
    }
    assert_eq!(std::fs::read(source).unwrap(), original);
}
