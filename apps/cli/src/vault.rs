use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use crate::{Args, Error, Result};

pub fn open(args: &Args) -> Result<PathBuf> {
    let path = resolve_path(args)?;
    if !path.is_dir() {
        return Err(Error::VaultNotFound(path));
    }
    vault_sync::owner::recover(&path)
        .map_err(|error| Error::operation("recover sync before vault access", error.to_string()))?;
    Ok(path)
}

pub(crate) fn resolve_path(args: &Args) -> Result<PathBuf> {
    if let Some(path) = &args.vault_path {
        return Ok(path.clone());
    }
    if let Some(base) = &args.base {
        return Ok(base.clone());
    }
    if let Some(path) = std::env::var_os("FMTR_VAULT_PATH").map(PathBuf::from) {
        return Ok(path);
    }
    if let Some(path) = std::env::var_os("FMTR_BASE").map(PathBuf::from) {
        return Ok(path);
    }

    let data_dir = dirs::data_dir()
        .ok_or_else(|| Error::operation("resolve vault path", "data directory is unavailable"))?;
    Ok(resolve_default_path(&data_dir))
}

fn resolve_default_path(data_dir: &Path) -> PathBuf {
    if cfg!(feature = "staging") {
        return resolve_default_path_for_command(data_dir, Some(OsStr::new("loof-staging")));
    }
    let command_name = std::env::args_os()
        .next()
        .and_then(|path| Path::new(&path).file_name().map(|name| name.to_owned()));
    resolve_default_path_for_command(data_dir, command_name.as_deref())
}

fn resolve_default_path_for_command(data_dir: &Path, command_name: Option<&OsStr>) -> PathBuf {
    let (current, legacy) = match command_name.and_then(OsStr::to_str) {
        Some("loof-dev" | "loofah-dev" | "fmtr-dev") => (
            data_dir.join("io.loofah.dev"),
            data_dir.join("org.freemeetingtranscriber.dev"),
        ),
        Some("loof-staging" | "loofah-staging" | "fmtr-staging") => (
            data_dir.join("io.loofah.staging"),
            data_dir.join("org.freemeetingtranscriber.staging"),
        ),
        _ => (
            data_dir.join("loofah"),
            data_dir.join("free-meeting-transcriber"),
        ),
    };
    let base = if current.exists() || !legacy.exists() {
        current
    } else {
        legacy
    };
    apply_vault_redirect(base)
}

/// The desktop app keeps its vault in the application-data directory by default, but a user
/// can relocate it; the app then records the absolute vault path under `vault_path` in the
/// app-data directory's `global.json`. Follow that redirect so the CLI reads the same vault
/// the desktop app uses.
fn apply_vault_redirect(base: PathBuf) -> PathBuf {
    let Ok(raw) = std::fs::read_to_string(base.join("global.json")) else {
        return base;
    };
    let Ok(config) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return base;
    };
    match config.get("vault_path").and_then(|value| value.as_str()) {
        Some(vault_path) if Path::new(vault_path).is_absolute() => PathBuf::from(vault_path),
        _ => base,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_path_targets_the_app_data_vault() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            resolve_default_path_for_command(dir.path(), Some(OsStr::new("loof"))),
            dir.path().join("loofah")
        );
    }

    #[test]
    fn default_path_falls_back_to_the_legacy_app_data_vault() {
        let dir = tempfile::tempdir().unwrap();
        let legacy = dir.path().join("free-meeting-transcriber");
        std::fs::create_dir_all(&legacy).unwrap();

        assert_eq!(
            resolve_default_path_for_command(dir.path(), Some(OsStr::new("loof"))),
            legacy
        );
    }

    #[test]
    fn channel_commands_target_their_channel_vault() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            resolve_default_path_for_command(dir.path(), Some(OsStr::new("loof-dev"))),
            dir.path().join("io.loofah.dev")
        );
        assert_eq!(
            resolve_default_path_for_command(dir.path(), Some(OsStr::new("loof-staging"))),
            dir.path().join("io.loofah.staging")
        );
        assert_eq!(
            resolve_default_path_for_command(dir.path(), Some(OsStr::new("fmtr-dev"))),
            dir.path().join("io.loofah.dev")
        );
    }

    #[test]
    fn default_path_follows_the_global_json_vault_redirect() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("loofah");
        let vault = dir.path().join("my-vault");
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(
            base.join("global.json"),
            serde_json::json!({ "vault_path": vault.to_string_lossy() }).to_string(),
        )
        .unwrap();

        assert_eq!(
            resolve_default_path_for_command(dir.path(), Some(OsStr::new("loof"))),
            vault
        );
    }

    #[test]
    fn relative_or_malformed_redirects_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("loofah");
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(
            base.join("global.json"),
            serde_json::json!({ "vault_path": "relative/vault" }).to_string(),
        )
        .unwrap();
        assert_eq!(
            resolve_default_path_for_command(dir.path(), Some(OsStr::new("loof"))),
            base
        );

        std::fs::write(base.join("global.json"), "{ invalid").unwrap();
        assert_eq!(
            resolve_default_path_for_command(dir.path(), Some(OsStr::new("loof"))),
            base
        );
    }
}
