use super::*;
use crate::capture_spool::*;
use std::io::{Cursor, Read};

async fn fixture(
    root: &std::path::Path,
    channels: Vec<CaptureChannel>,
) -> (CaptureWriter, Vec<u8>) {
    let spool = CaptureSpool::open(root.join("spool"), CaptureSpoolLimits::default())
        .await
        .unwrap();
    let writer = spool
        .begin_with_observation(
            CaptureIdentity {
                scope: CaptureScope {
                    server: "https://capture.example/".into(),
                    member_id: "original".into(),
                    vault_id: "original-vault".into(),
                },
                capture_id: uuid::Uuid::new_v4().to_string(),
                request_id: uuid::Uuid::new_v4().to_string(),
                target: CaptureTarget::PendingSession {
                    create_request_id: uuid::Uuid::new_v4().to_string(),
                },
            },
            CaptureFormat {
                channels: channels.clone(),
                ..CaptureFormat::stereo()
            },
            1_794_000_000_001,
        )
        .await
        .unwrap();
    let mut source = Vec::new();
    for sequence in 0..3 {
        let bytes: Vec<u8> = (0..channels.len() * 160)
            .flat_map(|sample| {
                [0.25f32, -0.5, -0.0, f32::MIN_POSITIVE][(sample + sequence as usize) % 4]
                    .to_le_bytes()
            })
            .collect();
        writer
            .append_chunk(sequence, 160, bytes.clone())
            .await
            .unwrap();
        source.extend(bytes);
    }
    (writer, source)
}

#[tokio::test]
async fn sealed_and_interrupted_exports_preserve_float_bits_channels_and_original_capture() {
    for sealed in [false, true] {
        for channels in [
            vec![CaptureChannel::Microphone],
            vec![CaptureChannel::SystemAudio],
            vec![CaptureChannel::Microphone, CaptureChannel::SystemAudio],
        ] {
            let root = tempfile::tempdir().unwrap();
            let root = root.path().canonicalize().unwrap();
            let (writer, source) = fixture(&root, channels.clone()).await;
            if sealed {
                writer.seal().await.unwrap();
            }
            let before = writer.snapshot().await.unwrap();
            let output = root.join("recovered.wav");
            let receipt = export_wave(writer.clone(), output.clone()).await.unwrap();
            let encoded = std::fs::read(&output).unwrap();
            assert_eq!(receipt.sealed, sealed);
            assert_eq!(receipt.frames, 480);
            assert_eq!(receipt.pcm_bytes, source.len() as u64);
            assert_eq!(receipt.file_bytes, encoded.len() as u64);
            assert_eq!(receipt.sha256, hex(Sha256::new().chain_update(&encoded)));
            let mut reader = hound::WavReader::new(Cursor::new(&encoded)).unwrap();
            assert_eq!(reader.spec().channels as usize, channels.len());
            assert_eq!(reader.spec().sample_rate, 16000);
            assert_eq!(reader.spec().sample_format, hound::SampleFormat::Float);
            let decoded: Vec<u8> = reader
                .samples::<f32>()
                .flat_map(|sample| sample.unwrap().to_le_bytes())
                .collect();
            assert_eq!(decoded, source);
            assert_eq!(&encoded[encoded.len() - source.len()..], source);
            assert_eq!(writer.snapshot().await.unwrap(), before);
            assert!(root.read_dir().unwrap().all(|entry| {
                !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".loofah-audio-")
            }));
        }
    }
}

#[tokio::test]
async fn existing_destination_and_corrupt_source_never_publish_or_remove_original_audio() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let (writer, _) = fixture(&root, vec![CaptureChannel::Microphone]).await;
    let output = root.join("user-file.wav");
    std::fs::write(&output, b"user-owned").unwrap();
    assert!(export_wave(writer.clone(), output.clone()).await.is_err());
    assert_eq!(std::fs::read(output).unwrap(), b"user-owned");
    let scope = root
        .join("spool")
        .read_dir()
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.is_dir())
        .unwrap();
    let capture = scope.read_dir().unwrap().next().unwrap().unwrap().path();
    let chunk = capture
        .join("chunks")
        .read_dir()
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let original = std::fs::read(&chunk).unwrap();
    let mut damaged = original.clone();
    *damaged.last_mut().unwrap() ^= 1;
    std::fs::write(&chunk, &damaged).unwrap();
    let output = root.join("invalid.wav");
    assert!(export_wave(writer.clone(), output.clone()).await.is_err());
    assert!(!output.exists());
    assert_eq!(std::fs::read(&chunk).unwrap(), damaged);
    assert!(root.read_dir().unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".loofah-audio-")
    }));
}

#[tokio::test]
async fn dropped_caller_keeps_owned_export_through_actual_file_publication() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let (writer, source) = fixture(&root, vec![CaptureChannel::Microphone]).await;
    let original = writer.snapshot().await.unwrap();
    let output = root.join("recovered.wav");
    let (entered, ready) = tokio::sync::oneshot::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let task = tokio::spawn(run(
        writer.clone(),
        output.clone(),
        Some(ExportPause {
            entered,
            release: wait,
        }),
    ));
    ready.await.unwrap();
    task.abort();
    let _ = task.await;
    assert!(!output.exists());
    release.send(()).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !output.exists() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let mut file = std::fs::File::open(&output).unwrap();
    let mut encoded = Vec::new();
    file.read_to_end(&mut encoded).unwrap();
    assert_eq!(&encoded[encoded.len() - source.len()..], source);
    assert_eq!(writer.snapshot().await.unwrap(), original);
}
