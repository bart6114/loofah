use std::time::Duration;

use hypr_audio_chunking::{AudioChunk, Chunker, SpeechChunker, SpeechChunkingConfig};

pub const TARGET_SAMPLE_RATE: u32 = 16_000;

const DEFAULT_SPEECH_REDEMPTION_TIME: Duration = Duration::from_millis(150);
const MAX_CHUNK_SAMPLES: usize = TARGET_SAMPLE_RATE as usize * 25;

pub fn chunk_channel_audio<E>(samples: &[f32]) -> Result<Vec<AudioChunk>, E>
where
    E: From<hypr_audio_chunking::Error>,
{
    let mut chunker =
        SpeechChunker::new(SpeechChunkingConfig::speech(DEFAULT_SPEECH_REDEMPTION_TIME))?;
    Ok(chunk_channel_audio_with(samples, &mut chunker)?)
}

fn chunk_channel_audio_with<C>(
    samples: &[f32],
    chunker: &mut C,
) -> Result<Vec<AudioChunk>, C::Error>
where
    C: Chunker,
{
    let chunks = chunker.chunk(samples, TARGET_SAMPLE_RATE)?;
    let mut normalized = Vec::new();

    for chunk in chunks {
        let ranges = hypr_audio_chunking::split_audio_range(
            chunk.sample_start..chunk.sample_end,
            MAX_CHUNK_SAMPLES,
            TARGET_SAMPLE_RATE,
            |range| {
                Ok::<_, C::Error>(
                    samples
                        .get(range.clone())
                        .unwrap_or_else(|| {
                            &chunk.samples
                                [range.start - chunk.sample_start..range.end - chunk.sample_start]
                        })
                        .to_vec(),
                )
            },
        )?;
        for range in ranges {
            let audio = samples.get(range.clone()).unwrap_or_else(|| {
                &chunk.samples[range.start - chunk.sample_start..range.end - chunk.sample_start]
            });
            normalized.push(AudioChunk {
                samples: audio.to_vec(),
                sample_start: range.start,
                sample_end: range.end,
            });
        }
    }

    tracing::info!(
        chunk_count = normalized.len(),
        chunk_durations_ms = ?normalized
            .iter()
            .map(|chunk| (chunk.sample_end - chunk.sample_start) * 1000 / TARGET_SAMPLE_RATE as usize)
            .collect::<Vec<_>>(),
        "audio_chunking_complete"
    );

    Ok(normalized)
}

pub fn split_resampled_channels(samples: &[f32], channel_count: usize) -> Vec<Vec<f32>> {
    if channel_count <= 1 {
        return vec![samples.to_vec()];
    }

    hypr_audio_utils::deinterleave(samples, channel_count)
}

pub fn channel_duration_sec(samples: &[f32]) -> f64 {
    samples.len() as f64 / TARGET_SAMPLE_RATE as f64
}

#[cfg(test)]
mod tests {
    use hypr_audio_chunking::{AudioChunk, Chunker};

    use super::*;
    use crate::{initial_resolved_until, next_resolved_until};

    #[test]
    #[ignore = "requires LOOFAH_PCM_VAD_QA_AUDIO; compares complete long-recording VAD ranges"]
    fn file_vad_matches_complete_recording() {
        let pcm = hypr_audio_utils::PcmDescriptor::open(
            std::env::var("LOOFAH_PCM_VAD_QA_AUDIO").unwrap(),
        )
        .unwrap();
        let samples = pcm.reader().unwrap().channel(0, 0..pcm.frames).unwrap();
        let start = std::time::Instant::now();
        let legacy = chunk_channel_audio::<hypr_audio_chunking::Error>(&samples).unwrap();
        let legacy_ms = start.elapsed().as_millis();
        let expected: Vec<_> = legacy
            .iter()
            .map(|chunk| (chunk.sample_start, chunk.sample_end))
            .collect();
        drop(legacy);
        drop(samples);
        let start = std::time::Instant::now();
        let ranges = chunk_pcm_channel::<Box<dyn std::error::Error>>(&pcm, 0, || false).unwrap();
        let file_ms = start.elapsed().as_millis();
        assert_eq!(
            ranges
                .iter()
                .map(|chunk| (chunk.sample_start, chunk.sample_end))
                .collect::<Vec<_>>(),
            expected
        );
        assert!(ranges.iter().all(|chunk| chunk.samples.is_empty()));
        println!(
            "duration={} ranges={} legacy_vad_ms={legacy_ms} file_vad_ms={file_ms}",
            pcm.duration(),
            ranges.len()
        );
    }

    #[test]
    fn file_and_memory_splits_agree_for_pauses_and_unequal_channels() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let mut writer = hound::WavWriter::create(
            file.path(),
            hound::WavSpec {
                channels: 2,
                sample_rate: TARGET_SAMPLE_RATE,
                bits_per_sample: 32,
                sample_format: hound::SampleFormat::Float,
            },
        )
        .unwrap();
        let mut channels = [Vec::new(), Vec::new()];
        for index in 0..40 * 16000 {
            for (channel, samples) in channels.iter_mut().enumerate() {
                let quiet = if channel == 0 {
                    (22 * 16000..23 * 16000).contains(&index)
                } else {
                    (23 * 16000..23 * 16000 + 3200).contains(&index)
                };
                let sample = if quiet {
                    0.01 + index as f32 * 1e-9
                } else {
                    1.0 + index as f32 / 640000.0
                };
                let sample = if channel == 0 { sample } else { -sample };
                writer.write_sample(sample).unwrap();
                samples.push(sample);
            }
        }
        writer.finalize().unwrap();
        let pcm = hypr_audio_utils::PcmDescriptor::open(file.path()).unwrap();
        let mut cuts = Vec::new();
        for (channel, samples) in channels.iter().enumerate() {
            let selected = if channel == 0 {
                123..samples.len() - 17
            } else {
                5000..30 * 16000
            };
            let memory = hypr_audio_chunking::split_audio_range(
                selected.clone(),
                MAX_CHUNK_SAMPLES,
                TARGET_SAMPLE_RATE,
                |range| Ok::<_, std::io::Error>(samples[range].to_vec()),
            )
            .unwrap();
            let mut reader = pcm.reader().unwrap();
            let disk = hypr_audio_chunking::split_audio_range(
                selected.clone(),
                MAX_CHUNK_SAMPLES,
                TARGET_SAMPLE_RATE,
                |range| {
                    assert!(range.len() <= 4 * TARGET_SAMPLE_RATE as usize);
                    reader.channel(channel, range)
                },
            )
            .unwrap();
            assert_eq!(memory, disk);
            assert_eq!(disk.len(), 2);
            let recovered: Vec<_> = disk
                .iter()
                .flat_map(|range| reader.channel(channel, range.clone()).unwrap())
                .collect();
            assert_eq!(recovered, samples[selected]);
            cuts.push(disk[0].end);
        }
        assert_ne!(cuts[0], cuts[1]);
    }

    struct FakeChunker {
        chunks: Vec<AudioChunk>,
    }

    impl Chunker for FakeChunker {
        type Error = std::convert::Infallible;

        fn chunk(
            &mut self,
            _samples: &[f32],
            _sample_rate: u32,
        ) -> Result<Vec<AudioChunk>, Self::Error> {
            Ok(self.chunks.clone())
        }
    }

    #[test]
    fn empty_audio_marks_channel_complete() {
        let chunks = chunk_channel_audio::<hypr_audio_chunking::Error>(&[]).unwrap();

        assert!(chunks.is_empty());
        assert_eq!(initial_resolved_until(&chunks, 40.0), 40.0);
    }

    #[test]
    fn empty_chunk_lists_mark_channel_complete() {
        let mut chunker = FakeChunker { chunks: Vec::new() };
        let chunks = chunk_channel_audio_with(&[], &mut chunker).unwrap();

        assert!(chunks.is_empty());
        assert_eq!(initial_resolved_until(&chunks, 40.0), 40.0);
    }

    #[test]
    fn leading_silence_uses_sample_offsets() {
        let mut chunker = FakeChunker {
            chunks: vec![AudioChunk {
                samples: vec![0.0; TARGET_SAMPLE_RATE as usize * 3],
                sample_start: TARGET_SAMPLE_RATE as usize * 12,
                sample_end: TARGET_SAMPLE_RATE as usize * 15,
            }],
        };
        let chunks =
            chunk_channel_audio_with(&vec![0.0; TARGET_SAMPLE_RATE as usize * 15], &mut chunker)
                .unwrap();

        assert_eq!(initial_resolved_until(&chunks, 40.0), 12.0);
    }

    #[test]
    fn oversized_chunks_are_split_at_generic_limit() {
        let oversized = MAX_CHUNK_SAMPLES + TARGET_SAMPLE_RATE as usize;
        let mut chunker = FakeChunker {
            chunks: vec![AudioChunk {
                samples: vec![1.0; oversized],
                sample_start: TARGET_SAMPLE_RATE as usize * 2,
                sample_end: TARGET_SAMPLE_RATE as usize * 2 + oversized,
            }],
        };

        let chunks = chunk_channel_audio_with(&vec![0.0; oversized], &mut chunker).unwrap();

        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].sample_start, TARGET_SAMPLE_RATE as usize * 2);
        assert_eq!(
            chunks[0].sample_end,
            TARGET_SAMPLE_RATE as usize * 2 + MAX_CHUNK_SAMPLES
        );
        assert_eq!(chunks[1].sample_start, chunks[0].sample_end);
        assert_eq!(chunks[1].samples.len(), TARGET_SAMPLE_RATE as usize);
    }

    #[test]
    fn resolved_progress_uses_sample_offsets() {
        let mut chunker = FakeChunker {
            chunks: vec![
                AudioChunk {
                    samples: vec![0.0; TARGET_SAMPLE_RATE as usize * 2],
                    sample_start: TARGET_SAMPLE_RATE as usize * 4,
                    sample_end: TARGET_SAMPLE_RATE as usize * 6,
                },
                AudioChunk {
                    samples: vec![0.0; TARGET_SAMPLE_RATE as usize * 3],
                    sample_start: TARGET_SAMPLE_RATE as usize * 8,
                    sample_end: TARGET_SAMPLE_RATE as usize * 11,
                },
            ],
        };

        let chunks =
            chunk_channel_audio_with(&vec![0.0; TARGET_SAMPLE_RATE as usize * 11], &mut chunker)
                .unwrap();

        assert_eq!(initial_resolved_until(&chunks, 20.0), 4.0);
        assert_eq!(next_resolved_until(&chunks, 0, 20.0), 8.0);
        assert_eq!(next_resolved_until(&chunks, 1, 20.0), 20.0);
    }
}

pub fn chunk_pcm_channel<E>(
    pcm: &hypr_audio_utils::PcmDescriptor,
    channel: usize,
    cancelled: impl Fn() -> bool,
) -> Result<Vec<AudioChunk>, E>
where
    E: From<hypr_audio_chunking::Error> + From<hypr_audio_utils::Error>,
{
    let mut reader = pcm.reader()?;
    let mut chunker =
        hypr_audio_chunking::LiveSpeechChunker::ranges(DEFAULT_SPEECH_REDEMPTION_TIME)?;
    let mut ranges = Vec::new();
    for start in (0..pcm.frames).step_by(hypr_audio_utils::PCM_BLOCK_FRAMES) {
        if cancelled() {
            return Err(hypr_audio_utils::Error::Io(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "Transcription cancelled",
            ))
            .into());
        }
        let samples = reader.channel(
            channel,
            start..(start + hypr_audio_utils::PCM_BLOCK_FRAMES).min(pcm.frames),
        )?;
        ranges.extend(chunker.push(&samples)?);
    }
    ranges.extend(chunker.finish()?);
    let mut normalized = Vec::new();
    for chunk in ranges {
        for range in hypr_audio_chunking::split_audio_range(
            chunk.sample_start..chunk.sample_end,
            MAX_CHUNK_SAMPLES,
            TARGET_SAMPLE_RATE,
            |window| reader.channel(channel, window),
        )? {
            normalized.push(AudioChunk {
                sample_start: range.start,
                sample_end: range.end,
                samples: Vec::new(),
            });
        }
    }
    Ok(normalized)
}
