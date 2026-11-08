use super::*;
use hypr_loofah_core::capture_live::{
    CaptureLiveControl, CaptureLiveEngine, CaptureLiveEngineReceipt, CaptureLiveEngineSpec,
    CaptureLiveIngress, CaptureLiveSession,
};
use hypr_loofah_protocol::{CaptureLiveCaption, CaptureLiveFailure};
use hypr_transcribe_core::live_transcript::LiveTranscriptMapper;
use owhisper_interface::stream;
use sha2::{Digest, Sha256};
use tokio::sync::{mpsc, watch};

pub(super) struct Cursor {
    pub(super) end: u64,
    pub(super) last: Option<(u64, usize, Vec<u8>)>,
    pub(super) finished: bool,
}
pub(super) enum Command {
    Audio(Arc<[f32]>),
    Finish,
    Stop,
}
pub(super) struct Control {
    pub(super) sender: mpsc::Sender<Command>,
    pub(super) channels: usize,
    pub(super) cursor: Mutex<Cursor>,
    pub(super) cancelled: AtomicBool,
}
impl CaptureLiveControl for Control {
    fn try_accept(
        &self,
        first_frame: u64,
        pcm: &Arc<[f32]>,
    ) -> Result<CaptureLiveIngress, CaptureLiveFailure> {
        if pcm.is_empty()
            || !pcm.len().is_multiple_of(self.channels)
            || pcm.len() > 16000 * self.channels
            || pcm.iter().any(|sample| !sample.is_finite())
        {
            return Err(CaptureLiveFailure::SourceChanged);
        }
        let mut hash = Sha256::new();
        for sample in pcm.iter() {
            hash.update(sample.to_le_bytes());
        }
        let digest = hash.finalize().to_vec();
        let mut cursor = self.cursor.lock().unwrap_or_else(|e| e.into_inner());
        if self.cancelled.load(Ordering::Acquire) {
            return Err(CaptureLiveFailure::Interrupted);
        }
        if cursor
            .last
            .as_ref()
            .is_some_and(|last| last == &(first_frame, pcm.len(), digest.clone()))
        {
            return Ok(CaptureLiveIngress::Duplicate);
        }
        if cursor.finished || first_frame != cursor.end {
            return Err(CaptureLiveFailure::SourceChanged);
        }
        let end = first_frame
            .checked_add((pcm.len() / self.channels) as u64)
            .ok_or(CaptureLiveFailure::SourceChanged)?;
        match self.sender.try_send(Command::Audio(pcm.clone())) {
            Ok(()) => {
                cursor.end = end;
                cursor.last = Some((first_frame, pcm.len(), digest));
                Ok(CaptureLiveIngress::Accepted)
            }
            Err(mpsc::error::TrySendError::Full(_)) => Ok(CaptureLiveIngress::Busy),
            Err(_) => Err(CaptureLiveFailure::EngineFailed),
        }
    }
    fn finish(&self, final_frame: u64) -> Result<(), CaptureLiveFailure> {
        let mut cursor = self.cursor.lock().unwrap_or_else(|e| e.into_inner());
        if final_frame != cursor.end {
            return Err(CaptureLiveFailure::SourceChanged);
        }
        if cursor.finished {
            return Ok(());
        }
        // Finish is a boundary flag, not another queued audio frame. Closing the
        // queue after its accepted frames must not fail because it is full.
        cursor.finished = true;
        let _ = self.sender.try_send(Command::Finish);
        Ok(())
    }
    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        let _ = self.sender.try_send(Command::Stop);
    }
}

impl CaptureLiveEngine for OnnxHost {
    fn start(
        &self,
        spec: CaptureLiveEngineSpec,
        source_owner: Box<dyn Send>,
    ) -> TranscriptionFuture<'_, Result<CaptureLiveSession, CaptureLiveFailure>> {
        Box::pin(async move {
            hypr_loofah_protocol::validate_capture_live_request(&spec.request)
                .map_err(|_| CaptureLiveFailure::InvalidOutput)?;
            hypr_loofah_protocol::validate_capture_format(&spec.format)
                .map_err(|_| CaptureLiveFailure::SourceChanged)?;
            let generation = uuid::Uuid::parse_str(&spec.generation_id)
                .map_err(|_| CaptureLiveFailure::InvalidOutput)?;
            if generation.is_nil() || generation.to_string() != spec.generation_id {
                return Err(CaptureLiveFailure::InvalidOutput);
            }
            options(
                &self.model,
                &spec.request.model,
                &spec.request.languages,
                &spec.request.keywords,
                true,
            )
            .map_err(|_| CaptureLiveFailure::EngineUnavailable)?;
            let channels = spec.format.channels.len();
            if !(1..=2).contains(&channels) {
                return Err(CaptureLiveFailure::SourceChanged);
            }
            let permit = self.admit().map_err(|error| match error {
                TranscriptionFailure::ResourceLimit => CaptureLiveFailure::ResourceLimit,
                _ => CaptureLiveFailure::EngineUnavailable,
            })?;
            let model = self
                .resources
                .models
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_ref()
                .map(|models| models.streaming.clone())
                .ok_or(CaptureLiveFailure::EngineUnavailable)?;
            let (sender, receiver) = mpsc::channel(16);
            let control = Arc::new(Control {
                sender,
                channels,
                cursor: Mutex::new(Cursor {
                    end: 0,
                    last: None,
                    finished: false,
                }),
                cancelled: AtomicBool::new(false),
            });
            let (captions, captions_rx) = mpsc::channel(16);
            let (completion, completed) = watch::channel(None);
            let worker_control = control.clone();
            let watcher_control = control.clone();
            let generation = spec.generation_id.clone();
            let resources = self.resources.clone();
            let shutdown = resources.shutdown.clone();
            let worker = tokio::task::spawn_blocking(move || {
                let _permit = permit;
                let _source = source_owner;
                run(model, spec, worker_control, receiver, captions)
            });
            tokio::spawn(async move {
                let mut worker = worker;
                let result = tokio::select! {
                    result = &mut worker => result,
                    _ = shutdown.cancelled() => { watcher_control.cancel(); worker.await },
                };
                let admitted = watcher_control
                    .cursor
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .end;
                let receipt = match result {
                    Ok((consumed, count, failure)) => CaptureLiveEngineReceipt {
                        generation_id: generation,
                        admitted_end_frame: admitted,
                        input_consumed_end_frame: consumed,
                        completed_through_frame: failure.is_none().then_some(consumed),
                        caption_count: count,
                        failure,
                        native_joined: true,
                        owners_released: true,
                    },
                    Err(_) => {
                        resources.phase.store(FAILED, Ordering::Release);
                        CaptureLiveEngineReceipt {
                            generation_id: generation,
                            admitted_end_frame: admitted,
                            input_consumed_end_frame: 0,
                            completed_through_frame: None,
                            caption_count: 0,
                            failure: Some(CaptureLiveFailure::EngineFailed),
                            native_joined: true,
                            owners_released: true,
                        }
                    }
                };
                completion.send_replace(Some(Arc::new(receipt)));
            });
            Ok(CaptureLiveSession {
                control,
                captions: captions_rx,
                completion: completed,
            })
        })
    }
}

fn run(
    model: Arc<VerifiedModel>,
    spec: CaptureLiveEngineSpec,
    control: Arc<Control>,
    mut receiver: mpsc::Receiver<Command>,
    captions: mpsc::Sender<CaptureLiveCaption>,
) -> (u64, u64, Option<CaptureLiveFailure>) {
    let mut consumed = 0_u64;
    let mut count = 0_u64;
    let operation = || -> Result<(), CaptureLiveFailure> {
        let mut sessions = LiveSession::load_verified_pair(model)
            .map_err(|_| CaptureLiveFailure::EngineUnavailable)?;
        let mut emission = Emission {
            spec: &spec,
            mapper: LiveTranscriptMapper::new(control.channels)
                .ok_or(CaptureLiveFailure::InvalidOutput)?,
            starts: [0.0; 2],
            captions: &captions,
            count: &mut count,
        };
        loop {
            if control.cancelled.load(Ordering::Acquire) {
                return Err(CaptureLiveFailure::Interrupted);
            }
            let finished = {
                let cursor = control.cursor.lock().unwrap_or_else(|e| e.into_inner());
                cursor.finished && consumed == cursor.end
            };
            if finished {
                break;
            }
            match receiver.blocking_recv() {
                Some(Command::Audio(samples)) => {
                    let end = consumed + (samples.len() / control.channels) as u64;
                    for (physical, session) in
                        sessions.iter_mut().enumerate().take(control.channels)
                    {
                        let mono = samples
                            .iter()
                            .skip(physical)
                            .step_by(control.channels)
                            .copied()
                            .collect::<Vec<_>>();
                        for partial in session
                            .append(&mono)
                            .map_err(|_| CaptureLiveFailure::EngineFailed)?
                        {
                            emission.emit(physical, partial, end)?;
                        }
                    }
                    consumed = end;
                }
                Some(Command::Finish) => {}
                _ => return Err(CaptureLiveFailure::Interrupted),
            }
        }
        for (physical, session) in sessions.iter_mut().enumerate().take(control.channels) {
            for partial in session
                .finalize()
                .map_err(|_| CaptureLiveFailure::EngineFailed)?
            {
                emission.emit(physical, partial, consumed)?;
            }
        }
        let tail = emission.mapper.flush();
        if !tail.replaced_ids.is_empty() {
            return Err(CaptureLiveFailure::InvalidOutput);
        }
        for physical in 0..control.channels {
            let canonical = if control.channels == 1 {
                2
            } else {
                physical as i32
            };
            let words = tail
                .new_words
                .iter()
                .filter(|word| word.channel == canonical)
                .cloned()
                .collect::<Vec<_>>();
            if words.is_empty() {
                continue;
            }
            captions
                .try_send(CaptureLiveCaption {
                    generation_id: spec.generation_id.clone(),
                    sequence: *emission.count,
                    physical_channel: physical as u8,
                    role: spec.format.channels[physical],
                    new_words: words,
                    partials: Vec::new(),
                    replaced_ids: Vec::new(),
                    languages: vec!["en".into()],
                })
                .map_err(|_| CaptureLiveFailure::InvalidOutput)?;
            *emission.count += 1;
        }
        Ok(())
    };
    let failure = operation().err();
    (consumed, count, failure)
}

struct Emission<'a> {
    spec: &'a CaptureLiveEngineSpec,
    mapper: LiveTranscriptMapper,
    starts: [f64; 2],
    captions: &'a mpsc::Sender<CaptureLiveCaption>,
    count: &'a mut u64,
}
impl Emission<'_> {
    fn emit(
        &mut self,
        physical: usize,
        partial: hypr_transcribe_onnx::Partial,
        end_frame: u64,
    ) -> Result<(), CaptureLiveFailure> {
        let end = end_frame as f64 / 16000.0;
        let text = partial.text.split_whitespace().collect::<Vec<_>>();
        let duration = (end - self.starts[physical]).max(0.0);
        let words = text
            .iter()
            .enumerate()
            .map(|(index, word)| stream::Word {
                word: (*word).into(),
                punctuated_word: Some((*word).into()),
                confidence: 1.0,
                speaker: None,
                language: Some("en".into()),
                start: self.starts[physical] + duration * index as f64 / text.len() as f64,
                end: self.starts[physical] + duration * (index + 1) as f64 / text.len() as f64,
            })
            .collect();
        let response = stream::StreamResponse::TranscriptResponse {
            start: self.starts[physical],
            duration,
            is_final: partial.is_final,
            speech_final: partial.is_final,
            from_finalize: false,
            channel_index: vec![physical as i32, self.spec.format.channels.len() as i32],
            metadata: stream::Metadata::default(),
            channel: stream::Channel {
                alternatives: vec![stream::Alternatives {
                    transcript: partial.text,
                    words,
                    confidence: 1.0,
                    languages: vec!["en".into()],
                }],
            },
        };
        if partial.is_final {
            self.starts[physical] = end;
        }
        if let Some(delta) = self.mapper.process(&response) {
            self.captions
                .try_send(CaptureLiveCaption {
                    generation_id: self.spec.generation_id.clone(),
                    sequence: *self.count,
                    physical_channel: physical as u8,
                    role: self.spec.format.channels[physical],
                    partials: delta.partials,
                    new_words: delta.new_words,
                    replaced_ids: delta.replaced_ids,
                    languages: vec!["en".into()],
                })
                .map_err(|_| CaptureLiveFailure::InvalidOutput)?;
            *self.count += 1;
        }
        Ok(())
    }
}
