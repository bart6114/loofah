use super::live_worker::{Job, Work};
use hypr_audio_chunking::{AudioChunk, LiveSpeechChunker};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::{OwnedSemaphorePermit, mpsc};

pub(crate) const MAX_MESSAGE_SAMPLES: usize = 10 * 16000;
pub(crate) const AUDIO_BUDGET: usize = 30 * 16000;

struct Channel {
    vad: LiveSpeechChunker,
    cursor: usize,
    held: VecDeque<(usize, OwnedSemaphorePermit)>,
}

pub(crate) async fn send(tx: &mpsc::Sender<Job>, job: impl FnOnce() -> Job) -> Result<(), String> {
    let permit = tx.reserve().await.map_err(|_| "inference worker closed")?;
    permit.send(job());
    Ok(())
}

async fn chunks(
    tx: &mpsc::Sender<Job>,
    channel: &mut Channel,
    index: usize,
    chunks: Vec<AudioChunk>,
    base_frame: usize,
) -> Result<(), String> {
    for mut chunk in chunks {
        let permit = tx.reserve().await.map_err(|_| "inference worker closed")?;
        let mut permits = Vec::new();
        while channel
            .held
            .front()
            .is_some_and(|(end, _)| *end <= chunk.sample_end)
        {
            permits.push(channel.held.pop_front().unwrap().1);
        }
        chunk.sample_start += base_frame;
        chunk.sample_end += base_frame;
        permit.send(Job::Audio(Work {
            channel: index,
            chunk,
            permits,
            queued: Instant::now(),
        }));
    }
    while channel
        .held
        .front()
        .is_some_and(|(end, _)| *end <= channel.vad.retained_start())
    {
        channel.held.pop_front();
    }
    Ok(())
}

pub(crate) struct EvidenceWindow {
    pub channels: Vec<Vec<f32>>,
    pub end: usize,
    pub cutoff: usize,
}

pub(crate) type Evidence = Arc<Mutex<EvidenceWindow>>;

pub(crate) async fn evidence(
    tx: &mpsc::Sender<Job>,
    observations: &Evidence,
) -> Result<(), String> {
    let permit = tx.reserve().await.map_err(|_| "inference worker closed")?;
    let mut window = observations.lock().unwrap();
    let channels = &mut window.channels;
    let best = channels
        .iter()
        .enumerate()
        .max_by_key(|(_, samples)| samples.len())
        .map(|(i, _)| i)
        .unwrap();
    if channels[best].is_empty() {
        return Ok(());
    }
    let samples = std::mem::take(&mut channels[best]);
    for channel in channels.iter_mut() {
        channel.clear();
    }
    window.cutoff = window.end;
    permit.send(Job::Evidence(samples));
    Ok(())
}

pub(crate) struct Pipeline {
    channels: Vec<Channel>,
    tx: mpsc::Sender<Job>,
    started: Option<tokio::sync::oneshot::Sender<()>>,
    observations: Evidence,
    base_frame: usize,
}

impl Pipeline {
    pub fn new(
        tx: mpsc::Sender<Job>,
        count: usize,
        redemption: Duration,
        started: tokio::sync::oneshot::Sender<()>,
        observations: Evidence,
        base_frame: usize,
    ) -> Result<Self, String> {
        let channels = (0..count)
            .map(|_| {
                LiveSpeechChunker::new(redemption, MAX_MESSAGE_SAMPLES).map(|vad| Channel {
                    vad,
                    cursor: 0,
                    held: VecDeque::new(),
                })
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        Ok(Self {
            channels,
            tx,
            started: Some(started),
            observations,
            base_frame,
        })
    }
    pub fn consumed_frames(&self) -> usize {
        self.channels
            .iter()
            .map(|channel| channel.cursor)
            .max()
            .unwrap_or(0)
    }
    pub async fn push(
        &mut self,
        audio: Vec<Vec<f32>>,
        mut permits: Vec<OwnedSemaphorePermit>,
    ) -> Result<(), String> {
        let frames = audio.iter().map(Vec::len).max().unwrap_or(0);
        for offset in (0..frames).step_by(512) {
            for (index, samples) in audio.iter().enumerate() {
                let end = (offset + 512).min(samples.len());
                if offset >= end {
                    continue;
                }
                let frame = &samples[offset..end];
                let channel = &mut self.channels[index];
                channel.cursor += frame.len();
                channel
                    .held
                    .push_back((channel.cursor, permits[index].split(frame.len()).unwrap()));
                let ready = channel.vad.push(frame).map_err(|e| e.to_string())?;
                if !channel.vad.speech_observation().is_empty() {
                    if let Some(started) = self.started.take() {
                        let _ = started.send(());
                    }
                    let mut window = self.observations.lock().unwrap();
                    if channel.cursor > window.cutoff {
                        window.channels[index].extend_from_slice(channel.vad.speech_observation());
                        window.end = window.end.max(channel.cursor);
                    }
                }
                chunks(&self.tx, channel, index, ready, self.base_frame).await?;
            }
            if self
                .observations
                .lock()
                .unwrap()
                .channels
                .iter()
                .any(|samples| samples.len() >= 5 * 16000)
            {
                evidence(&self.tx, &self.observations).await?;
            }
            tokio::task::yield_now().await;
        }
        Ok(())
    }
    pub async fn finish(&mut self, finalized: bool) -> Result<(), String> {
        evidence(&self.tx, &self.observations).await?;
        for (index, channel) in self.channels.iter_mut().enumerate() {
            let ready = channel.vad.finish().map_err(|e| e.to_string())?;
            chunks(&self.tx, channel, index, ready, self.base_frame).await?;
            channel.held.clear();
        }
        let duration = self.channels.iter().map(|c| c.cursor).max().unwrap_or(0) as f64 / 16000.0;
        send(&self.tx, || Job::Finish {
            duration,
            finalized,
        })
        .await?;
        Ok(())
    }
}
