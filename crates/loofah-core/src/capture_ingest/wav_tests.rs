use super::*;
use std::{path::Path, sync::Arc};

use crate::{search::SearchService, settings::ConfigState, startup::StartupState};
use hypr_capture_store::capture_spool::{
    CaptureChannel, CaptureFormat, CaptureIdentity, CaptureScope, CaptureSpool, CaptureSpoolLimits,
    CaptureTarget,
};

fn context(root: &Path) -> Arc<crate::VaultContext> {
    Arc::new(crate::VaultContext::new(
        "vault".into(),
        Arc::new(hypr_vault_write::SessionStore::new(root.into())),
        StartupState::new(Some(root)),
        Arc::new(ConfigState::load_or_default(root)),
        Arc::new(SearchService::default()),
    ))
}
async fn fixture(
    root: &Path,
    channels: Vec<CaptureChannel>,
) -> (CaptureWriter, CaptureSeal, Vec<u8>) {
    let spool = CaptureSpool::open(root.join("captures"), CaptureSpoolLimits::default())
        .await
        .unwrap();
    let identity = CaptureIdentity {
        scope: CaptureScope {
            server: "https://capture.example/".into(),
            member_id: "member".into(),
            vault_id: "vault".into(),
        },
        capture_id: uuid::Uuid::new_v4().to_string(),
        request_id: uuid::Uuid::new_v4().to_string(),
        target: CaptureTarget::ExistingSession {
            session_id: "note".into(),
            expected_audio_sha256: None,
        },
    };
    let mut format = CaptureFormat::stereo();
    format.channels = channels;
    let channels = format.channels.len();
    let writer = spool
        .begin_with_observation(identity, format, 1_794_000_000_001)
        .await
        .unwrap();
    let values = [
        0.0f32,
        -0.0,
        0.25,
        -0.5,
        f32::MIN_POSITIVE,
        1.0,
        -1.0,
        0.001,
    ];
    let mut all = Vec::new();
    for sequence in 0..3 {
        let bytes: Vec<_> = (0..4 * channels)
            .flat_map(|index| values[(index + sequence as usize) % values.len()].to_le_bytes())
            .collect();
        writer
            .append_chunk(sequence, 4, bytes.clone())
            .await
            .unwrap();
        all.extend(bytes);
    }
    let seal = writer.seal().await.unwrap();
    (writer, seal, all)
}

#[tokio::test]
async fn verified_streaming_wav_preserves_exact_float_bits_and_channel_order() {
    for channels in [
        vec![CaptureChannel::Microphone],
        vec![CaptureChannel::SystemAudio],
        vec![CaptureChannel::Microphone, CaptureChannel::SystemAudio],
    ] {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let (writer, seal, raw) = fixture(&root, channels.clone()).await;
        let verified = verify(&writer, &seal).await.unwrap();
        let (mut reader, producer) = verified.reader(context(&root), writer);
        let encoded = tokio::task::spawn_blocking(move || {
            let mut bytes = Vec::new();
            reader.read_to_end(&mut bytes).unwrap();
            bytes
        })
        .await
        .unwrap();
        producer.await.unwrap().unwrap();
        assert_eq!(encoded.len() as u64, verified.artifact_size_bytes);
        assert_eq!(hash(&encoded), verified.artifact_sha256);
        let mut decoded = hound::WavReader::new(Cursor::new(&encoded)).unwrap();
        assert_eq!(decoded.spec().sample_format, hound::SampleFormat::Float);
        assert_eq!(decoded.spec().channels as usize, channels.len());
        assert_eq!(decoded.spec().sample_rate, 16_000);
        assert_eq!(decoded.duration(), 12);
        let decoded: Vec<_> = decoded
            .samples::<f32>()
            .map(|sample| sample.unwrap().to_le_bytes())
            .flatten()
            .collect();
        assert_eq!(decoded, raw);
        assert_eq!(&encoded[encoded.len() - raw.len()..], raw);
    }
}

#[tokio::test]
async fn wav_source_changed_after_verification_fails_the_reader_instead_of_reporting_eof() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let (writer, seal, _) = fixture(&root, vec![CaptureChannel::Microphone]).await;
    let verified = verify(&writer, &seal).await.unwrap();
    let scope = root
        .join("captures")
        .read_dir()
        .unwrap()
        .filter_map(Result::ok)
        .find(|entry| entry.file_type().unwrap().is_dir())
        .unwrap()
        .path();
    let capture = scope.read_dir().unwrap().next().unwrap().unwrap().path();
    let chunk = capture
        .join("chunks")
        .read_dir()
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let mut bytes = std::fs::read(&chunk).unwrap();
    *bytes.last_mut().unwrap() ^= 1;
    std::fs::write(chunk, bytes).unwrap();
    let (mut reader, producer) = verified.reader(context(&root), writer);
    assert!(
        tokio::task::spawn_blocking(move || {
            let mut bytes = Vec::new();
            reader.read_to_end(&mut bytes)
        })
        .await
        .unwrap()
        .is_err()
    );
    assert!(producer.await.unwrap().is_err());
}

#[tokio::test]
async fn bounded_reader_retains_context_until_its_consumer_is_dropped() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let (writer, seal, _) = fixture(&root, vec![CaptureChannel::Microphone]).await;
    let verified = verify(&writer, &seal).await.unwrap();
    let context = context(&root);
    let weak = Arc::downgrade(&context);
    let (reader, producer) = verified.reader(context.clone(), writer);
    drop(context);
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    assert!(weak.upgrade().is_some());
    assert!(!producer.is_finished());
    drop(reader);
    tokio::time::timeout(std::time::Duration::from_secs(5), producer)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(weak.upgrade().is_none());
}

#[tokio::test]
async fn two_hour_wav_header_is_bounded_and_its_frame_count_is_not_rounded() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let (writer, _, _) = fixture(
        &root,
        vec![CaptureChannel::Microphone, CaptureChannel::SystemAudio],
    )
    .await;
    let mut snapshot = writer.snapshot().await.unwrap();
    snapshot.frames = 16_000 * 7200;
    snapshot.pcm_bytes = snapshot.frames * 8;
    let header = header(&snapshot).unwrap();
    assert_eq!(header.len(), 68);
    let decoded = hound::WavReader::new(Cursor::new(header)).unwrap();
    assert_eq!(u64::from(decoded.duration()), snapshot.frames);
    snapshot.frames = u64::from(u32::MAX);
    snapshot.pcm_bytes = snapshot.frames * 8;
    assert_eq!(
        super::header(&snapshot).unwrap_err().code,
        ErrorCode::ResourceLimit
    );
}
