use std::sync::atomic::{AtomicU8, AtomicUsize};
#[cfg(any(target_os = "windows", test))]
use std::sync::{Arc, atomic::Ordering};
#[cfg(any(target_os = "windows", test))]
use std::time::Duration;

#[cfg(any(target_os = "windows", test))]
use futures_util::StreamExt;
#[cfg(any(target_os = "windows", test))]
use tokio::sync::watch;

#[cfg(any(target_os = "windows", test))]
use super::stream::ChunkStream;

#[derive(Default)]
pub(crate) struct CaptureHealth {
    pub(super) recovering: AtomicU8,
    pub(super) generation: AtomicUsize,
}

#[cfg(any(target_os = "windows", test))]
pub(super) fn recovering_chunks(
    initial: ChunkStream,
    factory: Arc<dyn Fn() -> Result<ChunkStream, hypr_audio::Error> + Send + Sync>,
    mut refresh: watch::Receiver<u64>,
    health: Arc<CaptureHealth>,
    side: u8,
    sample_rate: u32,
    chunk_size: usize,
) -> ChunkStream {
    let (tx, rx) = tokio::sync::mpsc::channel(8);
    tokio::spawn(async move {
        let mut stream = Some(initial);
        let mut opening = None;
        let mut opening_revision = 0;
        let mut retry = tokio::time::interval(Duration::from_secs(1));
        retry.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut silence = tokio::time::interval(Duration::from_secs_f64(
            chunk_size as f64 / sample_rate as f64,
        ));
        silence.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut watch_open = true;
        loop {
            tokio::select! {
                _ = tx.closed() => break,
                changed = refresh.changed(), if watch_open => {
                    if changed.is_err() { watch_open = false; continue; }
                    stream = None;
                    health.recovering.fetch_or(side, Ordering::SeqCst);
                    health.generation.fetch_add(1, Ordering::SeqCst);
                }
                item = async { stream.as_mut().expect("active capture stream").next().await }, if stream.is_some() => {
                    match item {
                        Some(Ok(data)) => if tx.send(Ok(data)).await.is_err() { break; },
                        _ => {
                            stream = None;
                            health.recovering.fetch_or(side, Ordering::SeqCst);
                            health.generation.fetch_add(1, Ordering::SeqCst);
                            tracing::warn!(side, "audio_source_disconnected_retrying");
                        }
                    }
                }
                _ = silence.tick(), if stream.is_none() => {
                    if tx.send(Ok(vec![0.0; chunk_size])).await.is_err() { break; }
                }
                _ = retry.tick(), if stream.is_none() && opening.is_none() => {
                    let factory = factory.clone();
                    opening_revision = *refresh.borrow();
                    opening = Some(tokio::task::spawn_blocking(move || factory()));
                }
                result = async { opening.as_mut().expect("pending source open").await }, if opening.is_some() => {
                    opening = None;
                    if opening_revision != *refresh.borrow() { continue; }
                    match result {
                        Ok(Ok(reopened)) => {
                            stream = Some(reopened);
                            health.recovering.fetch_and(!side, Ordering::SeqCst);
                            health.generation.fetch_add(1, Ordering::SeqCst);
                            tracing::info!(side, "audio_source_recovered");
                        }
                        Ok(Err(error)) => tracing::debug!(side, %error, "audio_source_recovery_pending"),
                        Err(error) => tracing::warn!(side, %error, "audio_source_recovery_task_failed"),
                    }
                }
            }
        }
    });
    Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx))
}

#[cfg(target_os = "windows")]
pub(super) fn device_changes(
    follow_default_mic: bool,
) -> (watch::Receiver<u64>, watch::Receiver<u64>) {
    use hypr_device_monitor::{DeviceSwitch, DeviceSwitchMonitor};
    let (mic_tx, mic_rx) = watch::channel(0u64);
    let (speaker_tx, speaker_rx) = watch::channel(0u64);
    std::thread::spawn(move || {
        let wake_mic = mic_tx.clone();
        let wake_speaker = speaker_tx.clone();
        let _sleep = hypr_detect::SleepDetector::subscribe(Arc::new(move |event| {
            if matches!(
                event,
                hypr_detect::DetectEvent::SleepStateChanged { value: false }
            ) {
                wake_mic.send_modify(|value| *value = value.wrapping_add(1));
                wake_speaker.send_modify(|value| *value = value.wrapping_add(1));
            }
        }));
        let (event_tx, event_rx) = std::sync::mpsc::channel();
        let _monitor = DeviceSwitchMonitor::spawn_debounced(event_tx);
        while !mic_tx.is_closed() || !speaker_tx.is_closed() {
            match event_rx.recv_timeout(Duration::from_millis(100)) {
                Ok(DeviceSwitch::DefaultInputChanged) if follow_default_mic => {
                    mic_tx.send_modify(|value| *value = value.wrapping_add(1));
                }
                Ok(DeviceSwitch::DefaultOutputChanged { .. }) => {
                    speaker_tx.send_modify(|value| *value = value.wrapping_add(1));
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                _ => {}
            }
        }
    });
    (mic_rx, speaker_rx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[tokio::test]
    async fn disconnected_source_emits_silence_then_recovers_without_reopening_the_other_source() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let observed = attempts.clone();
        let health = Arc::new(CaptureHealth::default());
        let (_tx, rx) = watch::channel(0);
        let mut healthy = recovering_chunks(
            Box::pin(futures_util::stream::repeat_with(|| Ok(vec![0.5; 160]))),
            Arc::new(|| panic!("The healthy source must not be reopened")),
            rx.clone(),
            health.clone(),
            2,
            16000,
            160,
        );
        let initial: ChunkStream = Box::pin(futures_util::stream::empty());
        let mut stream = recovering_chunks(
            initial,
            Arc::new(move || {
                if observed.fetch_add(1, Ordering::SeqCst) == 0 {
                    Err(hypr_audio::Error::MicOpenFailed)
                } else {
                    Ok(Box::pin(futures_util::stream::repeat_with(|| {
                        Ok(vec![0.25; 160])
                    })))
                }
            }),
            rx,
            health.clone(),
            1,
            16000,
            160,
        );
        tokio::time::timeout(Duration::from_secs(3), async {
            let mut silence_seen = false;
            loop {
                let data = stream.next().await.unwrap().unwrap();
                assert_eq!(healthy.next().await.unwrap().unwrap()[0], 0.5);
                assert_eq!(health.recovering.load(Ordering::SeqCst) & 2, 0);
                if data[0] == 0.0 {
                    silence_seen = true;
                }
                if data[0] == 0.25 {
                    break;
                }
            }
            assert!(silence_seen);
        })
        .await
        .unwrap();
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        assert_eq!(health.recovering.load(Ordering::SeqCst), 0);
        assert!(health.generation.load(Ordering::SeqCst) >= 2);
    }
}
