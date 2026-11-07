mod device;
mod disk;
pub use device::*;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use ractor::{Actor, ActorName, ActorProcessingErr, ActorRef, RpcReplyPort};

pub enum RecMsg {
    AudioSingle(Arc<[f32]>),
    AudioDual(Arc<[f32]>, Arc<[f32]>),
    DeviceAudio {
        mic: Arc<[f32]>,
        speaker: Arc<[f32]>,
        credit: DeviceFrameCredit,
    },
    DeviceFinish(
        DeviceRecordingEnd,
        RpcReplyPort<Result<DeviceFinishReceipt, DeviceSinkError>>,
    ),
}

#[derive(Clone)]
pub enum RecordingDestination {
    LocalDisk {
        vault_dir: PathBuf,
        session_id: String,
    },
    Device {
        key: DeviceCaptureKey,
        sink: Arc<dyn DeviceRecordingSink>,
        mode: super::ChannelMode,
    },
}

impl RecordingDestination {
    pub fn is_device(&self) -> bool {
        matches!(self, Self::Device { .. })
    }
}

pub struct RecArgs {
    pub destination: RecordingDestination,
}

pub struct RecState {
    sink: RecorderSink,
}

enum RecorderSink {
    Disk(disk::DiskSink),
    Device(device::DeviceSink),
}

pub struct RecorderActor;

impl Default for RecorderActor {
    fn default() -> Self {
        Self::new()
    }
}

impl RecorderActor {
    pub fn new() -> Self {
        Self
    }

    pub fn name() -> ActorName {
        "recorder_actor".into()
    }
}

#[ractor::async_trait]
impl Actor for RecorderActor {
    type Msg = RecMsg;
    type State = RecState;
    type Arguments = RecArgs;

    async fn pre_start(
        &self,
        _myself: ActorRef<Self::Msg>,
        args: Self::Arguments,
    ) -> Result<Self::State, ActorProcessingErr> {
        let sink = match args.destination {
            RecordingDestination::LocalDisk {
                vault_dir,
                session_id,
            } => {
                let session_dir = find_session_dir(&vault_dir, &session_id)?;
                std::fs::create_dir_all(&session_dir)?;
                RecorderSink::Disk(disk::create_disk_sink(&session_dir)?)
            }
            RecordingDestination::Device { sink, mode, .. } => {
                RecorderSink::Device(device::DeviceSink::new(sink, mode))
            }
        };
        Ok(RecState { sink })
    }

    async fn handle(
        &self,
        _myself: ActorRef<Self::Msg>,
        msg: Self::Msg,
        st: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        match (&mut st.sink, msg) {
            (RecorderSink::Disk(sink), RecMsg::AudioSingle(samples)) => {
                disk::write_single(sink, &samples)?;
            }
            (RecorderSink::Disk(sink), RecMsg::AudioDual(mic, spk)) => {
                disk::write_dual(sink, &mic, &spk)?;
            }
            (
                RecorderSink::Device(sink),
                RecMsg::DeviceAudio {
                    mic,
                    speaker,
                    credit,
                },
            ) => {
                sink.write(&mic, &speaker, credit).await?;
            }
            (RecorderSink::Device(sink), RecMsg::DeviceFinish(end, reply)) => {
                let result = sink.finish(end).await;
                let failure = result.as_ref().err().cloned();
                let _ = reply.send(result);
                if let Some(error) = failure {
                    return Err(error.into());
                }
            }
            _ => {
                return Err(DeviceSinkError("Recorder destination/message mismatch".into()).into());
            }
        }

        Ok(())
    }

    async fn post_stop(
        &self,
        _myself: ActorRef<Self::Msg>,
        st: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        match &mut st.sink {
            RecorderSink::Disk(sink) => {
                disk::finalize_disk_sink(sink)?;
            }
            RecorderSink::Device(_) => {
                // Only the explicit drain barrier may finish a device recording.
            }
        }

        Ok(())
    }
}

pub fn find_session_dir(
    vault_base: &Path,
    session_id: &str,
) -> Result<PathBuf, hypr_vault_read::Error> {
    Ok(vault_base.join(hypr_vault_read::paths::validated_session_dir(session_id)?))
}

pub fn resolve_final_audio_path(vault_base: &Path, session_id: &str) -> Option<PathBuf> {
    let session_dir = find_session_dir(vault_base, session_id).ok()?;
    let mp3_path = session_dir.join("audio.mp3");
    if mp3_path.exists() {
        return Some(mp3_path);
    }

    let wav_path = session_dir.join("audio.wav");
    if wav_path.exists() {
        return Some(wav_path);
    }

    let ogg_path = session_dir.join("audio.ogg");
    if ogg_path.exists() {
        return Some(ogg_path);
    }

    None
}

fn into_actor_err<E>(err: E) -> ActorProcessingErr
where
    E: std::error::Error + Send + Sync + 'static,
{
    Box::new(err)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recorder_and_finalization_use_direct_validated_paths() {
        let vault = tempfile::tempdir().unwrap();
        let id = "legacy-session";
        let dir = vault.path().join("sessions").join(id);
        assert_eq!(find_session_dir(vault.path(), id).unwrap(), dir);
        assert!(find_session_dir(vault.path(), "../escape").is_err());
        assert_eq!(resolve_final_audio_path(vault.path(), id), None);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("audio.wav"), b"audio").unwrap();
        assert_eq!(
            resolve_final_audio_path(vault.path(), id),
            Some(dir.join("audio.wav"))
        );
        assert_eq!(resolve_final_audio_path(vault.path(), "missing"), None);
    }
}
