#[cfg(test)]
use super::live_worker::Work;
use super::{
    live_pipeline::{Evidence, EvidenceWindow, Pipeline, evidence, send},
    live_worker::Job,
    message::{AudioExtract, IncomingMessage, process_incoming_message},
};
use axum::extract::ws::{Message, WebSocket};
use futures_util::{StreamExt, stream::SplitStream};
#[cfg(test)]
use hypr_audio_chunking::AudioChunk;
use owhisper_interface::ControlMessage;
#[cfg(test)]
use std::time::Instant;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::{Semaphore, mpsc};

pub(super) use super::live_pipeline::{AUDIO_BUDGET, MAX_MESSAGE_SAMPLES};

async fn run_input(
    mut socket: SplitStream<WebSocket>,
    tx: mpsc::Sender<Job>,
    count: usize,
    redemption: Duration,
    started: tokio::sync::oneshot::Sender<()>,
    observations: Evidence,
) -> Result<SplitStream<WebSocket>, String> {
    let mut pipeline = Pipeline::new(
        tx.clone(),
        count,
        redemption,
        started,
        observations.clone(),
        0,
    )?;
    let budgets: Vec<_> = (0..count)
        .map(|_| Arc::new(Semaphore::new(AUDIO_BUDGET)))
        .collect();
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
        pipeline.push(audio, permits).await?;
    }
    pipeline.finish(finalized).await?;
    Ok(socket)
}

pub(super) async fn run(
    socket: SplitStream<WebSocket>,
    tx: mpsc::Sender<Job>,
    count: usize,
    redemption: Duration,
) -> Result<SplitStream<WebSocket>, String> {
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
