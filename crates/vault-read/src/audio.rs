use std::path::{Path, PathBuf};

pub fn resolve_final_audio_path(vault_base: &Path, session_id: &str) -> Option<PathBuf> {
    let directory = vault_base.join(crate::paths::validated_session_dir(session_id).ok()?);
    ["audio.mp3", "audio.wav", "audio.ogg"]
        .into_iter()
        .map(|name| directory.join(name))
        .find(|path| std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_file()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_only_canonical_recordings_in_priority_order() {
        let vault = tempfile::tempdir().unwrap();
        let directory = vault.path().join("sessions/session");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("attachment.wav"), b"attachment").unwrap();
        assert_eq!(resolve_final_audio_path(vault.path(), "session"), None);
        for name in ["audio.ogg", "audio.wav", "audio.mp3"] {
            std::fs::write(directory.join(name), b"audio").unwrap();
            assert_eq!(
                resolve_final_audio_path(vault.path(), "session"),
                Some(directory.join(name))
            );
        }
        assert_eq!(resolve_final_audio_path(vault.path(), "missing"), None);
        assert_eq!(resolve_final_audio_path(vault.path(), "../session"), None);
    }

    #[test]
    fn ignores_directories_and_transients() {
        let vault = tempfile::tempdir().unwrap();
        let directory = vault.path().join("sessions/session");
        std::fs::create_dir_all(directory.join("audio.mp3")).unwrap();
        std::fs::write(directory.join("audio.wav.tmp"), b"pending").unwrap();
        assert_eq!(resolve_final_audio_path(vault.path(), "session"), None);
        std::fs::write(directory.join("audio.ogg"), b"audio").unwrap();
        assert_eq!(
            resolve_final_audio_path(vault.path(), "session"),
            Some(directory.join("audio.ogg"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn ignores_symlinks() {
        let vault = tempfile::tempdir().unwrap();
        let directory = vault.path().join("sessions/session");
        std::fs::create_dir_all(&directory).unwrap();
        let external = vault.path().join("external.mp3");
        std::fs::write(&external, b"external").unwrap();
        std::os::unix::fs::symlink(&external, directory.join("audio.mp3")).unwrap();
        assert_eq!(resolve_final_audio_path(vault.path(), "session"), None);
    }
}
