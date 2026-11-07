mod joiner;
mod recovery;
mod stream;

use hypr_audio::{CaptureConfig, CaptureIntegrity, CaptureStream, Error};
use std::sync::Arc;
use stream::{CaptureSide, setup_mic_stream, setup_speaker_stream};

pub(crate) fn open_capture(config: CaptureConfig) -> Result<CaptureStream, Error> {
    let integrity = Arc::new(CaptureIntegrity::default());
    let health = Arc::new(recovery::CaptureHealth::new(integrity.clone()));
    let mic_stream = setup_mic_stream(
        config.sample_rate,
        config.chunk_size,
        config.mic_device.clone(),
        integrity.clone(),
    )?;
    std::thread::sleep(std::time::Duration::from_millis(50));
    let speaker_stream =
        setup_speaker_stream(config.sample_rate, config.chunk_size, integrity.clone())?;

    #[cfg(target_os = "windows")]
    let (mic_stream, speaker_stream) = {
        let (mic_changes, speaker_changes) = recovery::device_changes(config.mic_device.is_none());
        let mic_config = config.clone();
        let speaker_config = config.clone();
        let mic_integrity = integrity.clone();
        let mic_stream = recovery::recovering_chunks(
            mic_stream,
            Arc::new(move || {
                setup_mic_stream(
                    mic_config.sample_rate,
                    mic_config.chunk_size,
                    mic_config.mic_device.clone(),
                    mic_integrity.clone(),
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
                setup_speaker_stream(
                    speaker_config.sample_rate,
                    speaker_config.chunk_size,
                    integrity.clone(),
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
    let integrity = Arc::new(CaptureIntegrity::default());
    let health = Arc::new(recovery::CaptureHealth::new(integrity.clone()));
    let speaker_stream = setup_speaker_stream(sample_rate, chunk_size, integrity.clone())?;
    #[cfg(target_os = "windows")]
    let speaker_stream = {
        let (_, changes) = recovery::device_changes(false);
        recovery::recovering_chunks(
            speaker_stream,
            Arc::new(move || setup_speaker_stream(sample_rate, chunk_size, integrity.clone())),
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
    let integrity = Arc::new(CaptureIntegrity::default());
    let health = Arc::new(recovery::CaptureHealth::new(integrity.clone()));
    let mic_stream = setup_mic_stream(sample_rate, chunk_size, device.clone(), integrity.clone())?;
    #[cfg(target_os = "windows")]
    let mic_stream = {
        let (changes, _) = recovery::device_changes(device.is_none());
        recovery::recovering_chunks(
            mic_stream,
            Arc::new(move || {
                setup_mic_stream(sample_rate, chunk_size, device.clone(), integrity.clone())
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
