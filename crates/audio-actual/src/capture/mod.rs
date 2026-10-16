mod joiner;
mod recovery;
mod stream;

use hypr_audio::{CaptureConfig, CaptureStream, Error};
use stream::{CaptureSide, setup_mic_stream, setup_speaker_stream};

pub(crate) fn open_capture(config: CaptureConfig) -> Result<CaptureStream, Error> {
    let mic_stream = setup_mic_stream(
        config.sample_rate,
        config.chunk_size,
        config.mic_device.clone(),
    )?;

    std::thread::sleep(std::time::Duration::from_millis(50));

    let speaker_stream = setup_speaker_stream(config.sample_rate, config.chunk_size)?;

    #[cfg(not(target_os = "windows"))]
    let health = None;
    #[cfg(target_os = "windows")]
    let (mic_stream, speaker_stream, health) = {
        use std::sync::Arc;
        let health = Arc::new(recovery::CaptureHealth::default());
        let (mic_changes, speaker_changes) = recovery::device_changes(config.mic_device.is_none());
        let mic_config = config.clone();
        let speaker_config = config.clone();
        let mic_stream = recovery::recovering_chunks(
            mic_stream,
            Arc::new(move || {
                setup_mic_stream(
                    mic_config.sample_rate,
                    mic_config.chunk_size,
                    mic_config.mic_device.clone(),
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
                setup_speaker_stream(speaker_config.sample_rate, speaker_config.chunk_size)
            }),
            speaker_changes,
            health.clone(),
            2,
            config.sample_rate,
            config.chunk_size,
        );
        (mic_stream, speaker_stream, Some(health))
    };

    Ok(stream::open_dual(
        config.sample_rate,
        mic_stream,
        speaker_stream,
        config.enable_aec,
        health,
    ))
}

pub(crate) fn open_speaker_capture(
    sample_rate: u32,
    chunk_size: usize,
) -> Result<CaptureStream, Error> {
    let speaker_stream = setup_speaker_stream(sample_rate, chunk_size)?;
    #[cfg(not(target_os = "windows"))]
    let health = None;
    #[cfg(target_os = "windows")]
    let (speaker_stream, health) = {
        use std::sync::Arc;
        let health = Arc::new(recovery::CaptureHealth::default());
        let (_, changes) = recovery::device_changes(false);
        let stream = recovery::recovering_chunks(
            speaker_stream,
            Arc::new(move || setup_speaker_stream(sample_rate, chunk_size)),
            changes,
            health.clone(),
            2,
            sample_rate,
            chunk_size,
        );
        (stream, Some(health))
    };
    Ok(stream::open_single(
        speaker_stream,
        CaptureSide::Speaker,
        health,
    ))
}

pub(crate) fn open_mic_capture(
    device: Option<String>,
    sample_rate: u32,
    chunk_size: usize,
) -> Result<CaptureStream, Error> {
    let mic_stream = setup_mic_stream(sample_rate, chunk_size, device.clone())?;
    #[cfg(not(target_os = "windows"))]
    let health = None;
    #[cfg(target_os = "windows")]
    let (mic_stream, health) = {
        use std::sync::Arc;
        let health = Arc::new(recovery::CaptureHealth::default());
        let (changes, _) = recovery::device_changes(device.is_none());
        let stream = recovery::recovering_chunks(
            mic_stream,
            Arc::new(move || setup_mic_stream(sample_rate, chunk_size, device.clone())),
            changes,
            health.clone(),
            1,
            sample_rate,
            chunk_size,
        );
        (stream, Some(health))
    };
    Ok(stream::open_single(mic_stream, CaptureSide::Mic, health))
}
