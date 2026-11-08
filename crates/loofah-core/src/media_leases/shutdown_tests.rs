use super::*;

fn unlocked(root: &std::path::Path) -> bool {
    let file = File::options()
        .read(true)
        .write(true)
        .open(root.join(".loofah-vault.lock"))
        .unwrap();
    file.try_lock().is_ok()
}

async fn fixture(wav: bool) -> (tempfile::TempDir, Arc<VaultContext>) {
    let (root, mut context) = super::tests::fixture().await;
    let lease = File::options()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(root.path().join(".loofah-vault.lock"))
        .unwrap();
    lease.try_lock().unwrap();
    Arc::get_mut(&mut context).unwrap().lease = Some(lease.into());
    if wav {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 16_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer =
            hound::WavWriter::create(root.path().join("sessions/note/audio.wav"), spec).unwrap();
        for _ in 0..16_000 {
            writer.write_sample(1000_i16).unwrap();
        }
        writer.finalize().unwrap();
    }
    (root, context)
}

#[tokio::test]
async fn terminal_shutdown_joins_cancelled_actual_chunk_and_waveform_work_before_releasing_the_lease()
 {
    for waveform in [false, true] {
        let (root, context) = fixture(waveform).await;
        let weak = Arc::downgrade(&context);
        let leases = MediaLeases::new(1, 1, IDLE_TIMEOUT);
        let id = uuid::Uuid::new_v4().to_string();
        leases
            .open(context.clone(), "owner", &id, "note")
            .await
            .unwrap()
            .unwrap();
        let reader = leases.reader("owner", "vault", &id).unwrap();
        let recording = reader.recording.clone();
        let block = recording.file.lock().unwrap();
        drop(context);
        let call = tokio::spawn(async move {
            if waveform {
                reader
                    .waveform(tokio_util::sync::CancellationToken::new())
                    .await
                    .map(|_| ())
            } else {
                reader.read_chunk(0, 1).await.map(|_| ())
            }
        });
        tokio::time::timeout(Duration::from_secs(5), async {
            while recording.reads_started.load(Ordering::Acquire) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        call.abort();
        assert!(call.await.unwrap_err().is_cancelled());
        let ending = leases.clone();
        let shutdown = tokio::spawn(async move { ending.shutdown().await });
        tokio::time::timeout(Duration::from_secs(5), async {
            while !leases.0.stopping.load(Ordering::Acquire) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(!shutdown.is_finished());
        assert!(!unlocked(root.path()));
        assert!(weak.upgrade().is_some());
        assert!(
            matches!(leases.reader("owner", "vault", &id), Err(ref cause) if cause.code == ErrorCode::Unavailable)
        );
        drop(block);
        drop(recording);
        tokio::time::timeout(Duration::from_secs(5), shutdown)
            .await
            .unwrap()
            .unwrap();
        assert!(weak.upgrade().is_none());
        assert!(unlocked(root.path()));
        assert_eq!(leases.0.slots.available_permits(), 1);
        assert_eq!(leases.0.index_budget.available_permits(), HASH_INDEX_BUDGET);
    }
}

#[tokio::test]
async fn terminal_shutdown_includes_admitted_open_before_hash_and_never_publishes_a_late_entry() {
    let (root, context) = fixture(false).await;
    let weak = Arc::downgrade(&context);
    let leases = MediaLeases::new(1, 1, IDLE_TIMEOUT);
    let snapshot = context.store.lock_snapshot().await;
    let id = uuid::Uuid::new_v4().to_string();
    let opening = leases.clone();
    let requested = id.clone();
    let pending =
        tokio::spawn(async move { opening.open(context, "owner", &requested, "note").await });
    tokio::time::timeout(Duration::from_secs(5), async {
        while !leases.0.state.lock().unwrap().active.contains_key(&id) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let ending = leases.clone();
    let shutdown = tokio::spawn(async move { ending.shutdown().await });
    tokio::time::timeout(Duration::from_secs(5), async {
        while !leases.0.stopping.load(Ordering::Acquire) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(!shutdown.is_finished());
    assert!(!unlocked(root.path()));
    let context = weak.upgrade().unwrap();
    assert!(
        matches!(leases.open(context, "owner", &uuid::Uuid::new_v4().to_string(), "note").await, Err(ref cause) if cause.code == ErrorCode::Unavailable)
    );
    drop(snapshot);
    assert!(pending.await.unwrap().is_err());
    tokio::time::timeout(Duration::from_secs(5), shutdown)
        .await
        .unwrap()
        .unwrap();
    assert!(leases.0.state.lock().unwrap().active.is_empty());
    assert!(leases.0.state.lock().unwrap().live.is_empty());
    assert!(weak.upgrade().is_none());
    assert!(unlocked(root.path()));
}
