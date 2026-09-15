mod disk;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use ractor::{Actor, ActorName, ActorProcessingErr, ActorRef};

pub enum RecMsg {
    AudioSingle(Arc<[f32]>),
    AudioDual(Arc<[f32]>, Arc<[f32]>),
}

pub struct RecArgs {
    pub vault_dir: PathBuf,
    pub session_id: String,
}

pub struct RecState {
    sink: RecorderSink,
}

enum RecorderSink {
    Disk(disk::DiskSink),
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
        let session_dir = find_session_dir(&args.vault_dir, &args.session_id)?;
        std::fs::create_dir_all(&session_dir)?;

        Ok(RecState {
            sink: RecorderSink::Disk(disk::create_disk_sink(&session_dir)?),
        })
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
