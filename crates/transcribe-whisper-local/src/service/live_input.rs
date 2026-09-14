use super::{
    live_worker::{Job, Work},
    message::{AudioExtract, IncomingMessage, process_incoming_message},
};
use axum::extract::ws::{Message, WebSocket};
use futures_util::{StreamExt, stream::SplitStream};
use hypr_audio_chunking::{AudioChunk, LiveSpeechChunker};
use owhisper_interface::ControlMessage;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc};

pub(super) const MAX_MESSAGE_SAMPLES: usize = 10 * 16000;
pub(super) const AUDIO_BUDGET: usize = 30 * 16000;

struct Channel {
    vad: LiveSpeechChunker,
    cursor: usize,
    held: VecDeque<(usize, OwnedSemaphorePermit)>,
}

async fn send(tx: &mpsc::Sender<Job>, job: impl FnOnce() -> Job) -> Result<(), String> {
    let permit = tx.reserve().await.map_err(|_| "inference worker closed")?;
    permit.send(job());
    Ok(())
}

async fn chunks(
    tx: &mpsc::Sender<Job>,
    channel: &mut Channel,
    index: usize,
    chunks: Vec<AudioChunk>,
) -> Result<(), String> {
    for chunk in chunks {
        let permit = tx.reserve().await.map_err(|_| "inference worker closed")?;
        let mut permits = Vec::new();
        while channel
            .held
            .front()
            .is_some_and(|(end, _)| *end <= chunk.sample_end)
        {
            permits.push(channel.held.pop_front().unwrap().1);
        }
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

struct EvidenceWindow {
    channels: Vec<Vec<f32>>,
    end: usize,
    cutoff: usize,
}

type Evidence = Arc<Mutex<EvidenceWindow>>;

async fn evidence(tx: &mpsc::Sender<Job>, observations: &Evidence) -> Result<(), String> {
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

async fn run_input(
    mut socket: SplitStream<WebSocket>,
    tx: mpsc::Sender<Job>,
    count: usize,
    redemption: Duration,
    started: tokio::sync::oneshot::Sender<()>,
    observations: Evidence,
) -> Result<(), String> {
    let mut channels = (0..count)
        .map(|_| {
            LiveSpeechChunker::new(redemption, MAX_MESSAGE_SAMPLES).map(|vad| Channel {
                vad,
                cursor: 0,
                held: VecDeque::new(),
            })
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    let budgets: Vec<_> = (0..count)
        .map(|_| Arc::new(Semaphore::new(AUDIO_BUDGET)))
        .collect();
    let mut started = Some(started);
    let mut finalized = false;
    loop {
        let receive = async {
            let mut permits = Vec::new();
            for budget in &budgets {
                permits.push(
                    budget
                        .clone()
                        .acquire_many_owned(MAX_MESSAGE_SAMPLES as u32)
                        .await
                        .map_err(|e| e.to_string())?,
                );
            }
            Ok::<_, String>((socket.next().await, permits))
        };
        let (incoming, mut permits) = receive.await?;
        let Some(incoming) = incoming else {
            return Err("websocket disconnected".into());
        };
        let message = incoming.map_err(|e| e.to_string())?;
        if matches!(message, Message::Close(_)) {
            return Err("websocket disconnected".into());
        }
        let audio = match process_incoming_message(&message, count as u8)
            .map_err(|e| e.to_string())?
        {
            IncomingMessage::Audio(AudioExtract::Mono(samples)) => vec![samples],
            IncomingMessage::Audio(AudioExtract::Dual { ch0, ch1 }) if count == 2 => vec![ch0, ch1],
            IncomingMessage::Audio(AudioExtract::Dual { ch0, ch1 }) => {
                vec![hypr_audio_utils::mix_audio_f32(&ch0, &ch1)]
            }
            IncomingMessage::Audio(AudioExtract::End)
            | IncomingMessage::Control(ControlMessage::CloseStream) => break,
            IncomingMessage::Control(ControlMessage::Finalize) => {
                finalized = true;
                break;
            }
            IncomingMessage::Audio(AudioExtract::Empty)
            | IncomingMessage::Control(ControlMessage::KeepAlive) => continue,
        };
        if audio.iter().any(|a| a.len() > MAX_MESSAGE_SAMPLES) {
            return Err("audio message exceeds ten seconds per channel".into());
        }
        for (i, permit) in permits.iter_mut().enumerate() {
            let used = audio.get(i).map_or(0, Vec::len);
            drop(permit.split(MAX_MESSAGE_SAMPLES - used));
        }
        let frames = audio.iter().map(Vec::len).max().unwrap_or(0);
        for offset in (0..frames).step_by(512) {
            for (index, samples) in audio.iter().enumerate() {
                let end = (offset + 512).min(samples.len());
                if offset >= end {
                    continue;
                }
                let frame = &samples[offset..end];
                let channel = &mut channels[index];
                channel.cursor += frame.len();
                channel
                    .held
                    .push_back((channel.cursor, permits[index].split(frame.len()).unwrap()));
                let ready = channel.vad.push(frame).map_err(|e| e.to_string())?;
                if !channel.vad.speech_observation().is_empty() {
                    if let Some(started) = started.take() {
                        let _ = started.send(());
                    }
                    let mut window = observations.lock().unwrap();
                    if channel.cursor > window.cutoff {
                        window.channels[index].extend_from_slice(channel.vad.speech_observation());
                        window.end = window.end.max(channel.cursor);
                    }
                }
                chunks(&tx, channel, index, ready).await?;
            }
            if observations
                .lock()
                .unwrap()
                .channels
                .iter()
                .any(|samples| samples.len() >= 5 * 16000)
            {
                evidence(&tx, &observations).await?;
            }
            tokio::task::yield_now().await;
        }
    }
    evidence(&tx, &observations).await?;
    for (index, channel) in channels.iter_mut().enumerate() {
        let ready = channel.vad.finish().map_err(|e| e.to_string())?;
        chunks(&tx, channel, index, ready).await?;
        channel.held.clear();
    }
    let duration = channels.iter().map(|c| c.cursor).max().unwrap_or(0) as f64 / 16000.0;
    send(&tx, || Job::Finish {
        duration,
        finalized,
    })
    .await
}

pub(super) async fn run(
    socket: SplitStream<WebSocket>,
    tx: mpsc::Sender<Job>,
    count: usize,
    redemption: Duration,
) -> Result<(), String> {
    let (started, speech) = tokio::sync::oneshot::channel();
    let observations = Arc::new(Mutex::new(EvidenceWindow {
        channels: vec![Vec::new(); count],
        end: 0,
        cutoff: 0,
    }));
    let deadline_observations = observations.clone();
    let deadline_tx = tx.clone();
    let deadline = async move {
        if speech.await.is_ok() {
            tokio::time::sleep(Duration::from_secs(15)).await;
            evidence(&deadline_tx, &deadline_observations).await?;
            send(&deadline_tx, || Job::Deadline).await?;
        }
        Ok::<_, String>(())
    };
    let input = run_input(socket, tx, count, redemption, started, observations);
    tokio::pin!(input, deadline);
    tokio::select! {
        result = &mut input => result,
        result = &mut deadline => { result?; input.await }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn budget_counts_samples_and_releases_on_discard() {
        let budget = Arc::new(Semaphore::new(AUDIO_BUDGET));
        let permits = budget
            .clone()
            .acquire_many_owned(AUDIO_BUDGET as u32)
            .await
            .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(10), budget.clone().acquire_owned())
                .await
                .is_err()
        );
        let work = Work {
            channel: 0,
            chunk: AudioChunk {
                sample_start: 0,
                sample_end: 1,
                samples: vec![1.0],
            },
            permits: vec![permits],
            queued: Instant::now(),
        };
        drop(work);
        assert_eq!(budget.available_permits(), AUDIO_BUDGET);
    }

    #[tokio::test]
    async fn saturated_queue_does_not_take_audio_before_capacity_is_reserved() {
        let (tx, mut rx) = mpsc::channel(1);
        tx.send(Job::Deadline).await.unwrap();
        let mut audio = Some(vec![1.0; 123]);
        let result = tokio::time::timeout(
            Duration::from_millis(10),
            send(&tx, || Job::Evidence(audio.take().unwrap())),
        )
        .await;
        assert!(result.is_err());
        assert_eq!(audio.as_ref().unwrap().len(), 123);
        rx.recv().await.unwrap();
        send(&tx, || Job::Evidence(audio.take().unwrap()))
            .await
            .unwrap();
        assert!(audio.is_none());
        assert!(matches!(rx.recv().await, Some(Job::Evidence(samples)) if samples.len() == 123));
    }
}
