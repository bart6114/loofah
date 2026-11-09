mod joiner;
mod recovery;
mod stream;

use hypr_audio::{CaptureConfig, CaptureIntegrity, CaptureStream, Error};
use std::sync::Arc;
use stream::{CaptureSide, setup_mic_stream_owned, setup_speaker_stream_owned};

pub(crate) fn open_capture(config: CaptureConfig) -> Result<CaptureStream, Error> {
    open_capture_owned(config, None)
}
pub(crate) fn open_capture_owned(
    config: CaptureConfig,
    owner: Option<Arc<dyn Send + Sync>>,
) -> Result<CaptureStream, Error> {
    let integrity = Arc::new(CaptureIntegrity::default());
    let mut health = recovery::CaptureHealth::new(integrity.clone());
    health.owner = owner;
    let health = Arc::new(health);
    let mut mic_stream = setup_mic_stream_owned(
        config.sample_rate,
        config.chunk_size,
        config.mic_device.clone(),
        integrity.clone(),
        health.owner.clone(),
    )?;
    std::thread::sleep(std::time::Duration::from_millis(50));
    let speaker_stream = match setup_speaker_stream_owned(
        config.sample_rate,
        config.chunk_size,
        integrity.clone(),
        health.owner.clone(),
    ) {
        Ok(stream) => stream,
        Err(error) => {
            // An open failure must not leave the microphone running after returning Err.
            let _ = mic_stream.as_mut().request_stop();
            drop(mic_stream);
            return Err(error);
        }
    };

    #[cfg(target_os = "windows")]
    let (mic_stream, speaker_stream) = {
        let (mic_changes, speaker_changes) = recovery::device_changes(config.mic_device.is_none());
        let mic_config = config.clone();
        let speaker_config = config.clone();
        let mic_integrity = integrity.clone();
        let mic_owner = health.owner.clone();
        let speaker_owner = health.owner.clone();
        let mic_stream = recovery::recovering_chunks(
            mic_stream,
            Arc::new(move || {
                setup_mic_stream_owned(
                    mic_config.sample_rate,
                    mic_config.chunk_size,
                    mic_config.mic_device.clone(),
                    mic_integrity.clone(),
                    mic_owner.clone(),
                )
            }),
            mic_changes,
            health.clone(),
            1,
            config.sample_rate,
            config.chunk_size,
        );
        let speaker_stream = recovery::recovering_chunks(
            speaker_stream,
            Arc::new(move || {
                setup_speaker_stream_owned(
                    speaker_config.sample_rate,
                    speaker_config.chunk_size,
                    integrity.clone(),
                    speaker_owner.clone(),
                )
            }),
            speaker_changes,
            health.clone(),
            2,
            config.sample_rate,
            config.chunk_size,
        );
        (mic_stream, speaker_stream)
    };

    Ok(stream::open_dual(
        config.sample_rate,
        mic_stream,
        speaker_stream,
        config.enable_aec,
        Some(health),
    ))
}

pub(crate) fn open_speaker_capture(
    sample_rate: u32,
    chunk_size: usize,
) -> Result<CaptureStream, Error> {
    open_speaker_capture_owned(sample_rate, chunk_size, None)
}
pub(crate) fn open_speaker_capture_owned(
    sample_rate: u32,
    chunk_size: usize,
    owner: Option<Arc<dyn Send + Sync>>,
) -> Result<CaptureStream, Error> {
    let integrity = Arc::new(CaptureIntegrity::default());
    let mut health = recovery::CaptureHealth::new(integrity.clone());
    health.owner = owner;
    let health = Arc::new(health);
    let speaker_stream = setup_speaker_stream_owned(
        sample_rate,
        chunk_size,
        integrity.clone(),
        health.owner.clone(),
    )?;
    #[cfg(target_os = "windows")]
    let speaker_stream = {
        let (_, changes) = recovery::device_changes(false);
        let owner = health.owner.clone();
        recovery::recovering_chunks(
            speaker_stream,
            Arc::new(move || {
                setup_speaker_stream_owned(
                    sample_rate,
                    chunk_size,
                    integrity.clone(),
                    owner.clone(),
                )
            }),
            changes,
            health.clone(),
            2,
            sample_rate,
            chunk_size,
        )
    };
    Ok(stream::open_single(
        speaker_stream,
        CaptureSide::Speaker,
        Some(health),
    ))
}

pub(crate) fn open_mic_capture(
    device: Option<String>,
    sample_rate: u32,
    chunk_size: usize,
) -> Result<CaptureStream, Error> {
    open_mic_capture_owned(device, sample_rate, chunk_size, None)
}
pub(crate) fn open_mic_capture_owned(
    device: Option<String>,
    sample_rate: u32,
    chunk_size: usize,
    owner: Option<Arc<dyn Send + Sync>>,
) -> Result<CaptureStream, Error> {
    let integrity = Arc::new(CaptureIntegrity::default());
    let mut health = recovery::CaptureHealth::new(integrity.clone());
    health.owner = owner;
    let health = Arc::new(health);
    let mic_stream = setup_mic_stream_owned(
        sample_rate,
        chunk_size,
        device.clone(),
        integrity.clone(),
        health.owner.clone(),
    )?;
    #[cfg(target_os = "windows")]
    let mic_stream = {
        let (changes, _) = recovery::device_changes(device.is_none());
        let owner = health.owner.clone();
        recovery::recovering_chunks(
            mic_stream,
            Arc::new(move || {
                setup_mic_stream_owned(
                    sample_rate,
                    chunk_size,
                    device.clone(),
                    integrity.clone(),
                    owner.clone(),
                )
            }),
            changes,
            health.clone(),
            1,
            sample_rate,
            chunk_size,
        )
    };
    Ok(stream::open_single(
        mic_stream,
        CaptureSide::Mic,
        Some(health),
    ))
}
