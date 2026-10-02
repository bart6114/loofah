//! File-backed iCloud reconciliation. Missing files are never deletion evidence.
//! Callers serialize this engine with local store writes and rebuild the index
//! after a successful pass. Credentials, transient recordings and loose
//! user attachments never enter the sync inventory.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

pub mod coordination;
pub use coordination::coordinate;

const ROOT_TEXT_NAMES: &[&str] = &["config.json", "people.json", "tags.json", "tasks.json"];

const TEXT_NAMES: &[&str] = &[
    "_meta.json",
    "notes.md",
    "_memo.md",
    "summary.md",
    "transcript.json",
    "tasks.json",
];
const MAX_TEXT_BYTES: u64 = 16 * 1024 * 1024;
const MAX_FILES: usize = 50_000;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Vault(#[from] hypr_vault_read::Error),
    #[error("Unsafe sync path: {}", .0.display())]
    UnsafePath(PathBuf),
    #[error("File coordination failed: {0}")]
    Coordination(String),
    #[error("{0}")]
    Conflict(String),
}
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conflict {
    pub id: String,
    pub path: String,
    pub local_hash: String,
    pub remote_hash: String,
    #[serde(default)]
    pub media: bool,
    #[serde(default)]
    pub provider_version: Option<String>,
    #[serde(default)]
    pub provider_current_hash: Option<String>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SyncStatus {
    pub connected: bool,
    pub state: String,
    pub pending: usize,
    pub error: Option<String>,
    pub conflicts: Vec<Conflict>,
    pub changed: usize,
    pub recovered_sessions: Vec<String>,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Resolution {
    KeepLocal,
    KeepRemote,
    KeepBoth,
}
#[derive(Default, Serialize, Deserialize)]
struct State {
    #[serde(default)]
    remote_root: Option<PathBuf>,
    #[serde(default)]
    hydration_cursor: Option<String>,
    #[serde(default)]
    hashes: BTreeMap<String, String>,
    #[serde(default)]
    conflicts: BTreeMap<String, Conflict>,
    #[serde(default)]
    tombstones: BTreeMap<String, String>,
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default)]
    media_pending: BTreeSet<String>,
    #[serde(default)]
    media_uploaded: BTreeMap<String, String>,
    #[serde(default)]
    media_hashes: BTreeMap<String, String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Tombstone {
    generation: String,
    path: String,
    deleted: bool,
}

pub struct Engine {
    local: PathBuf,
    remote: PathBuf,
    state_dir: PathBuf,
    state: State,
}

impl Engine {
    pub fn open(
        local: impl AsRef<Path>,
        remote: impl AsRef<Path>,
        state_dir: impl AsRef<Path>,
    ) -> Result<Self> {
        std::fs::create_dir_all(local.as_ref())?;
        std::fs::create_dir_all(state_dir.as_ref())?;
        let local = local.as_ref().canonicalize()?;
        let remote = remote.as_ref().canonicalize()?;
        let state_dir = state_dir.as_ref().canonicalize()?;
        if local.starts_with(&remote)
            || remote.starts_with(&local)
            || state_dir.starts_with(&remote)
        {
            return Err(Error::Conflict(
                "Local working copy and sync state must be outside the selected vault".into(),
            ));
        }
        let mut state: State = match std::fs::read(state_dir.join("state.json")) {
            Ok(bytes) => serde_json::from_slice(&bytes)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => State::default(),
            Err(e) => return Err(e.into()),
        };
        if state
            .remote_root
            .as_ref()
            .is_some_and(|previous| previous != &remote)
        {
            state = State::default();
        }
        state.remote_root = Some(remote.clone());
        Ok(Self {
            local,
            remote,
            state_dir,
            state,
        })
    }

    pub fn has_session_audio(&self, session_id: &str) -> Result<bool> {
        let session = hypr_vault_read::paths::validated_session_dir(session_id)?;
        for media in [
            "audio.mp3",
            "audio.wav",
            "audio.ogg",
            "audio/audio.mp3",
            "audio/audio.wav",
            "audio/audio.ogg",
        ] {
            let relative = session.join(media).to_string_lossy().into_owned();
            if self.state.media_uploaded.contains_key(&relative)
                || self.state.media_hashes.contains_key(&relative)
            {
                return Ok(true);
            }
            let source = safe_path(&self.remote, &relative)?;
            if std::fs::symlink_metadata(&source).is_ok_and(|metadata| metadata.is_file()) {
                return Ok(true);
            }
            let name = source.file_name().unwrap().to_string_lossy();
            let placeholder = source.with_file_name(format!(".{name}.icloud"));
            let relative_placeholder = placeholder
                .strip_prefix(&self.remote)
                .map_err(|_| Error::UnsafePath(placeholder.clone()))?;
            let placeholder = safe_path(&self.remote, &relative_placeholder.to_string_lossy())?;
            if std::fs::symlink_metadata(placeholder).is_ok_and(|metadata| metadata.is_file()) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub fn conflicts(&self) -> Vec<Conflict> {
        self.state.conflicts.values().cloned().collect()
    }

    pub fn is_session_deleted(&self, session_id: &str) -> Result<bool> {
        hypr_vault_read::paths::validate_session_id(session_id)?;
        let path = format!("sessions/{session_id}");
        let Some(generation) = self.state.tombstones.get(&path) else {
            return Ok(false);
        };
        Ok(read_tombstone(&self.local, &path)?
            .is_some_and(|marker| marker.deleted && &marker.generation == generation))
    }

    pub fn reconcile(&mut self, limit: usize) -> Result<SyncStatus> {
        self.request_text_hydration(limit.clamp(1, 256))?;
        let remote = self.remote.clone();
        coordinate(&remote, true, || self.reconcile_locked(limit.clamp(1, 256)))?
    }

    fn request_text_hydration(&mut self, limit: usize) -> Result<()> {
        for &name in ROOT_TEXT_NAMES {
            let _ = coordination::ensure_available(&safe_path(&self.remote, name)?);
        }
        let directory = safe_path(&self.remote, "sessions")?;
        if !directory.is_dir() {
            return Ok(());
        }
        let mut candidates = Vec::new();
        for entry in std::fs::read_dir(directory)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if hypr_vault_read::paths::validate_session_id(&name).is_ok() {
                candidates.push(name);
            }
        }
        candidates.sort();
        let batch: Vec<_> = candidates
            .iter()
            .filter(|name| {
                self.state
                    .hydration_cursor
                    .as_ref()
                    .is_none_or(|cursor| *name > cursor)
            })
            .take(limit)
            .cloned()
            .collect();
        for id in &batch {
            for name in TEXT_NAMES {
                let relative = hypr_vault_read::paths::validated_session_dir(id)?.join(name);
                let path = safe_path(&self.remote, &relative.to_string_lossy())?;
                // Optional files may genuinely not exist. Hydration failure is not
                // absence/deletion evidence and the reconciliation read is retried.
                let _ = coordination::ensure_available(&path);
            }
            let enhanced = safe_path(&self.remote, &format!("sessions/{id}/enhanced"))?;
            if enhanced.is_dir() {
                for entry in std::fs::read_dir(enhanced)?.take(MAX_FILES) {
                    let entry = entry?;
                    let name = entry.file_name().to_string_lossy().into_owned();
                    let name = placeholder_name(&name).unwrap_or(&name);
                    if !name.starts_with('.') && name.ends_with(".md") {
                        let path =
                            safe_path(&self.remote, &format!("sessions/{id}/enhanced/{name}"))?;
                        let _ = coordination::ensure_available(&path);
                    }
                }
            }
        }
        self.state.hydration_cursor = batch.last().cloned();
        if self.state.hydration_cursor == candidates.last().cloned() {
            self.state.hydration_cursor = None;
        }
        Ok(())
    }

    fn reconcile_locked(&mut self, limit: usize) -> Result<SyncStatus> {
        enable_participation(&self.local)?;
        enable_participation(&self.remote)?;
        let mut status = SyncStatus {
            connected: true,
            state: "idle".into(),
            ..Default::default()
        };
        self.sync_tombstones(&mut status)?;
        let local = inventory(&self.local)?;
        let remote = inventory(&self.remote)?;
        let mut paths: BTreeSet<String> = local.union(&remote).cloned().collect();
        paths.extend(self.state.hashes.keys().cloned());
        let after = self.state.cursor.clone();
        let batch: Vec<String> = paths
            .iter()
            .filter(|p| after.as_ref().is_none_or(|cursor| *p > cursor))
            .take(limit)
            .cloned()
            .collect();
        for path in &batch {
            validate_text_path(path)?;
            if self.is_deleted(path)? {
                continue;
            }
            if self.state.conflicts.values().any(|c| c.path == *path) {
                continue;
            }
            let left = read_text(&self.local, path)?;
            let right = read_text(&self.remote, path)?;
            if let (Some(local), Some(remote)) = (&left, &right) {
                if self.ingest_provider_conflict(path, local, remote)? {
                    continue;
                }
            }
            let lh = left.as_deref().map(hash);
            let rh = right.as_deref().map(hash);
            let base = self.state.hashes.get(path);
            match (left, right) {
                (Some(l), Some(_)) if lh == rh => {
                    self.state.hashes.insert(path.clone(), hash(&l));
                }
                (Some(l), Some(r)) => {
                    if base == lh.as_ref() {
                        atomic_write(&self.local, path, &r)?;
                        self.state.hashes.insert(path.clone(), hash(&r));
                        status.changed += 1;
                    } else if base == rh.as_ref() {
                        atomic_write(&self.remote, path, &l)?;
                        self.state.hashes.insert(path.clone(), hash(&l));
                        status.changed += 1;
                    } else {
                        self.save_conflict(path, &l, &r)?;
                    }
                }
                (Some(l), None) if base.is_none() => {
                    atomic_write(&self.remote, path, &l)?;
                    self.state.hashes.insert(path.clone(), hash(&l));
                    status.changed += 1;
                }
                (None, Some(r)) if base.is_none() => {
                    atomic_write(&self.local, path, &r)?;
                    self.state.hashes.insert(path.clone(), hash(&r));
                    status.changed += 1;
                }
                // A known path missing on either side can be an evicted provider
                // item. Keep the surviving bytes and wait for materialization.
                _ => {}
            }
        }
        self.state.cursor = batch.last().cloned();
        status.pending = paths
            .iter()
            .filter(|p| self.state.cursor.as_ref().is_some_and(|cursor| *p > cursor))
            .count();
        if status.pending == 0 {
            self.state.cursor = None;
        }
        self.queue_local_media()?;
        // Persist the queue before transfer so interruption or background suspension
        // retries the immutable finalized recording on the next launch.
        self.persist()?;
        if let Some(path) = self.state.media_pending.iter().next().cloned() {
            let parts: Vec<_> = path.splitn(3, '/').collect();
            if parts.len() == 3 {
                match self.upload_media_locked(parts[1], parts[2]) {
                    Ok(_) => {
                        let fingerprint = media_fingerprint(&safe_path(&self.local, &path)?)?;
                        self.state.media_uploaded.insert(path.clone(), fingerprint);
                        self.state.media_pending.remove(&path);
                        status.changed += 1;
                    }
                    Err(Error::Conflict(_))
                        if self.state.conflicts.values().any(|c| c.path == path) =>
                    {
                        self.state.media_pending.remove(&path);
                    }
                    Err(error) => return Err(error),
                }
            }
        }
        status.pending += self.state.media_pending.len();
        status.conflicts = self.conflicts();
        if !status.conflicts.is_empty() {
            status.state = "conflict".into();
        } else if status.pending > 0 {
            status.state = "syncing".into();
        }
        self.persist()?;
        Ok(status)
    }

    fn save_conflict(&mut self, path: &str, local: &[u8], remote: &[u8]) -> Result<String> {
        let id = uuid::Uuid::new_v4().to_string();
        let prefix = format!(".loofah-sync/conflicts/{id}");
        for root in [&self.local, &self.remote] {
            atomic_write(root, &format!("{prefix}/local"), local)?;
            atomic_write(root, &format!("{prefix}/remote"), remote)?;
        }
        let conflict = Conflict {
            id: id.clone(),
            path: path.into(),
            local_hash: hash(local),
            remote_hash: hash(remote),
            media: false,
            provider_version: None,
            provider_current_hash: None,
        };
        atomic_write(
            &self.remote,
            &format!("{prefix}/conflict.json"),
            &serde_json::to_vec(&conflict)?,
        )?;
        self.state.conflicts.insert(id.clone(), conflict);
        Ok(id)
    }

    fn ingest_provider_conflict(
        &mut self,
        path: &str,
        local: &[u8],
        current: &[u8],
    ) -> Result<bool> {
        let canonical = safe_path(&self.remote, path)?;
        for (version, token) in coordination::unresolved_versions(&canonical)? {
            let bytes = read_limited(&version)?;
            if bytes == current {
                coordination::resolve_version(&canonical, &token)?;
                continue;
            }
            let id = self.save_conflict(path, local, &bytes)?;
            self.set_provider_conflict(&id, token, current)?;
            return Ok(true);
        }
        Ok(false)
    }

    fn set_provider_conflict(&mut self, id: &str, token: String, current: &[u8]) -> Result<()> {
        // A provider conflict can contain three different versions. Keep the
        // canonical cloud bytes too before any user choice replaces that file.
        atomic_write(
            &self.remote,
            &format!(".loofah-sync/conflicts/{id}/current"),
            current,
        )?;
        let conflict = self.state.conflicts.get_mut(id).unwrap();
        conflict.provider_version = Some(token);
        conflict.provider_current_hash = Some(hash(current));
        atomic_write(
            &self.remote,
            &format!(".loofah-sync/conflicts/{id}/conflict.json"),
            &serde_json::to_vec(conflict)?,
        )
    }

    pub fn resolve(&mut self, id: &str, resolution: Resolution) -> Result<()> {
        let remote = self.remote.clone();
        coordinate(&remote, true, || {
            let conflict = self
                .state
                .conflicts
                .get(id)
                .cloned()
                .ok_or_else(|| Error::Conflict("Conflict no longer exists".into()))?;
            if self.is_deleted(&conflict.path)? {
                return Err(Error::Conflict(
                    "This content was deleted; sync before reviewing conflicts".into(),
                ));
            }
            if conflict.media {
                let parts: Vec<_> = conflict.path.splitn(3, '/').collect();
                if parts.len() != 3 || parts[0] != "sessions" {
                    return Err(Error::UnsafePath(conflict.path.into()));
                }
                hypr_vault_read::paths::validate_session_id(parts[1])?;
                validate_media(parts[2])?;
                return self.resolve_media(&conflict, resolution);
            }
            validate_text_path(&conflict.path)?;
            let left = read_text(&self.local, &conflict.path)?
                .ok_or_else(|| Error::Conflict("Local file is unavailable".into()))?;
            let current = read_text(&self.remote, &conflict.path)?
                .ok_or_else(|| Error::Conflict("iCloud file is unavailable".into()))?;
            let right = if conflict.provider_version.is_some() {
                read_limited(&safe_path(
                    &self.remote,
                    &format!(".loofah-sync/conflicts/{id}/remote"),
                )?)?
            } else {
                current.clone()
            };
            let expected_current = conflict
                .provider_current_hash
                .as_ref()
                .unwrap_or(&conflict.remote_hash);
            if hash(&left) != conflict.local_hash
                || hash(&right) != conflict.remote_hash
                || hash(&current) != *expected_current
            {
                let next = self.save_conflict(&conflict.path, &left, &right)?;
                if let Some(version) = conflict.provider_version {
                    self.set_provider_conflict(&next, version, &current)?;
                }
                self.state.conflicts.remove(id);
                self.persist()?;
                return Err(Error::Conflict(
                    "File changed while reviewing; review the new versions".into(),
                ));
            }
            if matches!(resolution, Resolution::KeepBoth) {
                let segments: Vec<_> = conflict.path.split('/').collect();
                if segments.len() < 3 || segments[0] != "sessions" {
                    return Err(Error::Conflict("For vault-wide files, choose a version; both original files remain in conflict backups".into()));
                }
                self.recover_session(&self.local, &format!("sessions/{}", segments[1]))?;
            }
            let bytes = match resolution {
                Resolution::KeepLocal => left,
                Resolution::KeepRemote | Resolution::KeepBoth => right,
            };
            atomic_write(&self.local, &conflict.path, &bytes)?;
            atomic_write(&self.remote, &conflict.path, &bytes)?;
            if let Some(version) = &conflict.provider_version {
                coordination::resolve_version(&safe_path(&self.remote, &conflict.path)?, version)?;
            }
            self.state.hashes.insert(conflict.path, hash(&bytes));
            self.state.conflicts.remove(id);
            self.persist()
        })?
    }

    /// Only canonical recording names and app-managed attachment paths are accepted.
    pub fn download_media(&mut self, session_id: &str, media: &str) -> Result<PathBuf> {
        let session = hypr_vault_read::paths::validated_session_dir(session_id)?;
        validate_media(media)?;
        let relative = session.join(media).to_string_lossy().into_owned();
        if self.is_deleted(&relative)? {
            return Err(Error::Conflict("Session was deleted".into()));
        }
        let cached = safe_path(&self.local, &relative)?.is_file();
        if !cached {
            coordination::ensure_available(&safe_path(&self.remote, &relative)?)?;
        }
        let remote = self.remote.clone();
        let target = coordinate(&remote, false, || {
            if self.is_deleted(&relative)? {
                return Err(Error::Conflict("Session was deleted".into()));
            }
            let source = safe_path(&self.remote, &relative)?;
            let target = safe_path(&self.local, &relative)?;
            if target.is_file() {
                return Ok(target);
            }
            std::fs::create_dir_all(target.parent().unwrap())?;
            let temporary = target.with_file_name(format!(".sync-{}", uuid::Uuid::new_v4()));
            let result = (|| {
                std::fs::copy(source, &temporary)?;
                std::fs::File::open(&temporary)?.sync_all()?;
                std::fs::rename(&temporary, &target)?;
                Ok(target)
            })();
            if result.is_err() {
                let _ = std::fs::remove_file(temporary);
            }
            result
        })??;
        if !cached {
            self.state
                .media_hashes
                .insert(relative, hash_file(&target)?);
            self.persist()?;
        }
        Ok(target)
    }

    fn queue_local_media(&mut self) -> Result<()> {
        for (location, _) in hypr_vault_read::discover_sessions(&self.local)?.sessions {
            let session = location.relative_dir.to_string_lossy();
            for media in media_paths(&self.local, &session)? {
                let path = format!("{session}/{media}");
                if self.is_deleted(&path)? || self.state.conflicts.values().any(|c| c.path == path)
                {
                    continue;
                }
                let fingerprint = media_fingerprint(&safe_path(&self.local, &path)?)?;
                if self.state.media_uploaded.get(&path) != Some(&fingerprint) {
                    self.state.media_pending.insert(path);
                }
            }
        }
        Ok(())
    }

    /// Upload a finalized local recording or embedded attachment, preserving a
    /// different cloud version as a conflict backup before replacing it.
    pub fn upload_media(&mut self, session_id: &str, media: &str) -> Result<PathBuf> {
        let remote = self.remote.clone();
        let target = coordinate(&remote, true, || {
            self.upload_media_locked(session_id, media)
        })??;
        self.persist()?;
        Ok(target)
    }

    fn upload_media_locked(&mut self, session_id: &str, media: &str) -> Result<PathBuf> {
        let session = hypr_vault_read::paths::validated_session_dir(session_id)?;
        validate_media(media)?;
        let relative = session.join(media).to_string_lossy().into_owned();
        if self.is_deleted(&relative)? {
            return Err(Error::Conflict("Session was deleted".into()));
        }
        let source = safe_path(&self.local, &relative)?;
        let target = safe_path(&self.remote, &relative)?;
        coordination::ensure_available(&target)?;
        if target.is_file() {
            let local_hash = hash_file(&source)?;
            if local_hash == hash_file(&target)? {
                self.state.media_hashes.insert(relative, local_hash);
                return Ok(target);
            }
            self.save_media_conflict(&relative, &source, &target)?;
            self.persist()?;
            return Err(Error::Conflict(
                "A different recording exists in iCloud; both versions have been preserved".into(),
            ));
        }
        copy_atomic(&source, &target)?;
        self.state
            .media_hashes
            .insert(relative, hash_file(&source)?);
        Ok(target)
    }

    fn save_media_conflict(&mut self, path: &str, local: &Path, remote: &Path) -> Result<()> {
        let id = uuid::Uuid::new_v4().to_string();
        let prefix = format!(".loofah-sync/conflicts/{id}");
        copy_atomic(local, &safe_path(&self.remote, &format!("{prefix}/local"))?)?;
        copy_atomic(
            remote,
            &safe_path(&self.remote, &format!("{prefix}/remote"))?,
        )?;
        let conflict = Conflict {
            id: id.clone(),
            path: path.into(),
            local_hash: hash_file(local)?,
            remote_hash: hash_file(remote)?,
            media: true,
            provider_version: None,
            provider_current_hash: None,
        };
        atomic_write(
            &self.remote,
            &format!("{prefix}/conflict.json"),
            &serde_json::to_vec(&conflict)?,
        )?;
        self.state.conflicts.insert(id, conflict);
        Ok(())
    }

    fn resolve_media(&mut self, conflict: &Conflict, resolution: Resolution) -> Result<()> {
        let local = safe_path(&self.local, &conflict.path)?;
        let remote = safe_path(&self.remote, &conflict.path)?;
        if hash_file(&local)? != conflict.local_hash || hash_file(&remote)? != conflict.remote_hash
        {
            self.state.conflicts.remove(&conflict.id);
            self.save_media_conflict(&conflict.path, &local, &remote)?;
            self.persist()?;
            return Err(Error::Conflict(
                "Recording changed while reviewing; review the new versions".into(),
            ));
        }
        if matches!(resolution, Resolution::KeepBoth) {
            let parts: Vec<_> = conflict.path.split('/').collect();
            self.recover_session(&self.local, &format!("sessions/{}", parts[1]))?;
        }
        match resolution {
            Resolution::KeepLocal => copy_atomic(&local, &remote)?,
            Resolution::KeepRemote | Resolution::KeepBoth => copy_atomic(&remote, &local)?,
        }
        self.state
            .media_uploaded
            .insert(conflict.path.clone(), media_fingerprint(&local)?);
        self.state
            .media_hashes
            .insert(conflict.path.clone(), hash_file(&local)?);
        self.state.media_pending.remove(&conflict.path);
        self.state.conflicts.remove(&conflict.id);
        self.persist()
    }

    fn persist(&self) -> Result<()> {
        atomic_write(
            &self.state_dir,
            "state.json",
            &serde_json::to_vec(&self.state)?,
        )
    }

    fn is_deleted(&self, path: &str) -> Result<bool> {
        for root in [&self.local, &self.remote] {
            for target in deletion_targets(path)? {
                if let Some(marker) = read_tombstone(root, &target)? {
                    if marker.deleted {
                        return Ok(true);
                    }
                }
            }
        }
        Ok(false)
    }

    fn sync_tombstones(&mut self, status: &mut SyncStatus) -> Result<()> {
        let mut markers = BTreeMap::<String, Tombstone>::new();
        for root in [&self.remote, &self.local] {
            let dir = safe_path(root, ".loofah-sync/tombstones")?;
            if !dir.exists() {
                continue;
            }
            for entry in std::fs::read_dir(dir)?.take(MAX_FILES) {
                let entry = entry?;
                if !entry.file_type()?.is_file() {
                    continue;
                }
                let name = entry.file_name().to_string_lossy().into_owned();
                let name = placeholder_name(&name).unwrap_or(&name);
                if name.starts_with('.') || !name.ends_with(".json") {
                    continue;
                }
                let marker_path = safe_path(root, &format!(".loofah-sync/tombstones/{name}"))?;
                coordination::ensure_available(&marker_path)?;
                let bytes = read_limited(&marker_path)?;
                let marker: Tombstone = serde_json::from_slice(&bytes)?;
                validate_target(&marker.path)?;
                // A local unprocessed user action must be exported before reading
                // the previously observed cloud action for the same target.
                let seen = self.state.tombstones.get(&marker.path);
                if !markers.contains_key(&marker.path) || seen != Some(&marker.generation) {
                    markers.insert(marker.path.clone(), marker);
                }
            }
        }
        for (path, marker) in markers {
            if self.state.tombstones.get(&path) == Some(&marker.generation) {
                continue;
            }
            if marker.deleted {
                let local_paths = inventory(&self.local)?;
                let affected: Vec<_> = local_paths
                    .into_iter()
                    .filter(|p| *p == path || p.starts_with(&format!("{path}/")))
                    .collect();
                let original_session = path.split('/').take(2).collect::<Vec<_>>().join("/");
                let session_deleted = path == original_session;
                if has_changes(&self.local, &affected, &self.state.hashes)?
                    || (session_deleted && self.has_media_changes(&self.local, &path)?)
                {
                    status
                        .recovered_sessions
                        .push(self.recover_session(&self.local, &original_session)?);
                }
                let remote_paths: Vec<_> = inventory(&self.remote)?
                    .into_iter()
                    .filter(|p| *p == path || p.starts_with(&format!("{path}/")))
                    .collect();
                if has_changes(&self.remote, &remote_paths, &self.state.hashes)?
                    || (session_deleted && self.has_media_changes(&self.remote, &path)?)
                {
                    status
                        .recovered_sessions
                        .push(self.recover_session(&self.remote, &original_session)?);
                }
                for root in [&self.local, &self.remote] {
                    if session_deleted {
                        for media in media_paths(root, &path)? {
                            std::fs::remove_file(safe_path(root, &format!("{path}/{media}"))?)?;
                        }
                    }
                    let paths = inventory(root)?;
                    for p in paths
                        .into_iter()
                        .filter(|p| *p == path || p.starts_with(&format!("{path}/")))
                    {
                        let abs = safe_path(root, &p)?;
                        if abs.is_file() {
                            std::fs::remove_file(abs)?;
                        }
                    }
                }
                self.state
                    .hashes
                    .retain(|p, _| *p != path && !p.starts_with(&format!("{path}/")));
                self.state
                    .media_pending
                    .retain(|p| *p != path && !p.starts_with(&format!("{path}/")));
                self.state
                    .media_uploaded
                    .retain(|p, _| *p != path && !p.starts_with(&format!("{path}/")));
                self.state
                    .media_hashes
                    .retain(|p, _| *p != path && !p.starts_with(&format!("{path}/")));
                self.state.conflicts.retain(|_, conflict| {
                    conflict.path != path && !conflict.path.starts_with(&format!("{path}/"))
                });
                status.changed += 1;
            }
            for root in [&self.local, &self.remote] {
                atomic_write(root, &tombstone_path(&path), &serde_json::to_vec(&marker)?)?;
            }
            self.state.tombstones.insert(path, marker.generation);
        }
        Ok(())
    }

    fn has_media_changes(&self, root: &Path, session: &str) -> Result<bool> {
        for media in media_paths(root, session)? {
            let relative = format!("{session}/{media}");
            if self.state.media_hashes.get(&relative)
                != Some(&hash_file(&safe_path(root, &relative)?)?)
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn recover_session(&self, root: &Path, original: &str) -> Result<String> {
        let id = uuid::Uuid::new_v4().to_string();
        let destination = hypr_vault_read::paths::validated_session_dir(&id)?;
        for path in inventory(root)?
            .into_iter()
            .filter(|p| p.starts_with(&format!("{original}/")))
        {
            let Some(mut bytes) = read_text(root, &path)? else {
                continue;
            };
            let suffix = path.strip_prefix(&format!("{original}/")).unwrap();
            if suffix == "_meta.json" {
                let mut meta: hypr_vault_read::SessionMeta = serde_json::from_slice(&bytes)?;
                meta.id = id.clone();
                meta.title = format!("{} (Recovered)", meta.title);
                bytes = serde_json::to_vec_pretty(&meta)?;
            }
            atomic_write(root, &destination.join(suffix).to_string_lossy(), &bytes)?;
        }
        for media in media_paths(root, original)? {
            let source = safe_path(root, &format!("{original}/{media}"))?;
            let target = safe_path(root, &destination.join(&media).to_string_lossy())?;
            copy_atomic(&source, &target)?;
        }
        Ok(id)
    }
}

pub fn participation_enabled(root: &Path) -> bool {
    root.join(".loofah-sync").is_dir()
}
pub fn enable_participation(root: &Path) -> Result<()> {
    std::fs::create_dir_all(safe_path(root, ".loofah-sync/tombstones")?)?;
    Ok(())
}
/// Write inside the same native-coordinated transaction as the user deletion.
pub fn record_deletion(root: &Path, relative: &Path) -> Result<()> {
    record_action(root, relative, true)
}
pub fn record_restore(root: &Path, relative: &Path) -> Result<()> {
    record_action(root, relative, false)
}
pub fn record_write(root: &Path, relative: &Path) -> Result<()> {
    if !participation_enabled(root) {
        return Ok(());
    }
    let Some(path) = relative.to_str() else {
        return Err(Error::UnsafePath(relative.into()));
    };
    if let Some(marker) = read_tombstone(root, path)? {
        if marker.deleted {
            record_restore(root, relative)?;
        }
    }
    Ok(())
}
fn record_action(root: &Path, relative: &Path, deleted: bool) -> Result<()> {
    if !participation_enabled(root) {
        return Ok(());
    }
    let path = relative
        .to_str()
        .ok_or_else(|| Error::UnsafePath(relative.into()))?;
    validate_target(path)?;
    let marker = Tombstone {
        generation: uuid::Uuid::new_v4().to_string(),
        path: path.into(),
        deleted,
    };
    atomic_write(root, &tombstone_path(path), &serde_json::to_vec(&marker)?)
}
fn placeholder_name(name: &str) -> Option<&str> {
    name.strip_prefix('.')?.strip_suffix(".icloud")
}
fn tombstone_path(path: &str) -> String {
    format!(".loofah-sync/tombstones/{}.json", hash(path.as_bytes()))
}
fn read_tombstone(root: &Path, path: &str) -> Result<Option<Tombstone>> {
    let marker = safe_path(root, &tombstone_path(path))?;
    coordination::ensure_available(&marker)?;
    match std::fs::read(marker) {
        Ok(bytes) => {
            let marker: Tombstone = serde_json::from_slice(&bytes)?;
            if marker.path != path {
                return Err(Error::Conflict("Invalid tombstone target".into()));
            }
            Ok(Some(marker))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}
fn deletion_targets(path: &str) -> Result<Vec<String>> {
    let components: Vec<_> = path.split('/').collect();
    if components.len() >= 3 && components[0] == "sessions" {
        Ok(vec![format!("sessions/{}", components[1]), path.into()])
    } else {
        Ok(vec![path.into()])
    }
}
fn validate_target(path: &str) -> Result<()> {
    let segments: Vec<_> = path.split('/').collect();
    if segments.len() < 2 || segments[0] != "sessions" {
        return Err(Error::UnsafePath(path.into()));
    }
    hypr_vault_read::paths::validate_session_id(segments[1])?;
    if segments.len() == 2
        || (segments.len() == 3 && TEXT_NAMES.contains(&segments[2]))
        || (segments.len() == 4
            && segments[2] == "enhanced"
            && !segments[3].starts_with('.')
            && segments[3].ends_with(".md"))
    {
        return Ok(());
    }
    Err(Error::UnsafePath(path.into()))
}
fn validate_media(media: &str) -> Result<()> {
    let p = Path::new(media);
    if p.components()
        .any(|c| !matches!(c, Component::Normal(n) if !n.to_string_lossy().starts_with('.')))
        || media.contains('\\')
    {
        return Err(Error::UnsafePath(p.into()));
    }
    if [
        "audio.mp3",
        "audio.wav",
        "audio.ogg",
        "audio.peaks.json",
        "audio/audio.mp3",
        "audio/audio.wav",
        "audio/audio.ogg",
    ]
    .contains(&media)
        || (media.starts_with("attachments/") && p.components().count() > 1)
    {
        return Ok(());
    }
    Err(Error::UnsafePath(p.into()))
}
fn has_changes(root: &Path, paths: &[String], hashes: &BTreeMap<String, String>) -> Result<bool> {
    for path in paths {
        if let Some(bytes) = read_text(root, path)? {
            if hashes.get(path) != Some(&hash(&bytes)) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}
fn validate_text_path(path: &str) -> Result<()> {
    if ROOT_TEXT_NAMES.contains(&path) {
        return Ok(());
    }
    validate_target(path)?;
    if path.split('/').count() < 3 {
        return Err(Error::UnsafePath(path.into()));
    }
    Ok(())
}
fn media_paths(root: &Path, session: &str) -> Result<Vec<String>> {
    let mut output = Vec::new();
    for name in [
        "audio.mp3",
        "audio.wav",
        "audio.ogg",
        "audio.peaks.json",
        "audio/audio.mp3",
        "audio/audio.wav",
        "audio/audio.ogg",
    ] {
        if safe_path(root, &format!("{session}/{name}"))?.is_file() {
            output.push(name.into());
        }
    }
    let mut pending = vec!["attachments".to_owned()];
    while let Some(relative) = pending.pop() {
        let dir = safe_path(root, &format!("{session}/{relative}"))?;
        if !dir.is_dir() {
            continue;
        }
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let path = format!("{relative}/{name}");
            let kind = entry.file_type()?;
            if kind.is_dir() {
                pending.push(path);
            } else if kind.is_file() {
                output.push(path);
            }
            if output.len() + pending.len() > MAX_FILES {
                return Err(Error::Conflict("Too many embedded attachments".into()));
            }
        }
    }
    Ok(output)
}
fn media_fingerprint(path: &Path) -> Result<String> {
    let metadata = std::fs::metadata(path)?;
    let modified = metadata
        .modified()?
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    Ok(format!("{}:{modified}", metadata.len()))
}
fn hash_file(path: &Path) -> Result<String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(hex(&digest.finalize()))
}
fn copy_atomic(source: &Path, target: &Path) -> Result<()> {
    let parent = target.parent().unwrap();
    std::fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(".sync-{}", uuid::Uuid::new_v4()));
    let result = (|| {
        std::fs::copy(source, &temporary)?;
        std::fs::File::open(&temporary)?.sync_all()?;
        std::fs::rename(&temporary, &target)?;
        std::fs::File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
}
fn inventory(root: &Path) -> Result<BTreeSet<String>> {
    safe_path(root, "sessions")?;
    let mut output = BTreeSet::new();
    for &name in ROOT_TEXT_NAMES {
        let p = safe_path(root, name)?;
        if p.is_file() {
            output.insert(name.into());
        }
    }
    let sessions = safe_path(root, "sessions")?;
    if sessions.is_dir() {
        for entry in std::fs::read_dir(&sessions)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() || entry.file_name().to_string_lossy().starts_with('.')
            {
                continue;
            }
            let metadata = entry.path().join("_meta.json");
            if std::fs::symlink_metadata(&metadata).is_ok_and(|m| m.file_type().is_symlink()) {
                return Err(Error::UnsafePath(metadata));
            }
        }
    }
    let scan = hypr_vault_read::discover_sessions(root)?;
    for (location, _) in scan.sessions {
        for name in TEXT_NAMES {
            let relative = location
                .relative_dir
                .join(name)
                .to_string_lossy()
                .into_owned();
            if safe_path(root, &relative)?.is_file() {
                output.insert(relative);
            }
        }
        let enhanced = location
            .relative_dir
            .join("enhanced")
            .to_string_lossy()
            .into_owned();
        let dir = safe_path(root, &enhanced)?;
        if dir.is_dir() {
            for entry in std::fs::read_dir(dir)? {
                let entry = entry?;
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') || !name.ends_with(".md") || !entry.file_type()?.is_file()
                {
                    continue;
                }
                output.insert(format!("{enhanced}/{name}"));
            }
        }
        if output.len() > MAX_FILES {
            return Err(Error::Conflict("Vault exceeds beta sync file limit".into()));
        }
    }
    Ok(output)
}
fn read_text(root: &Path, relative: &str) -> Result<Option<Vec<u8>>> {
    let path = safe_path(root, relative)?;
    coordination::ensure_available(&path)?;
    match std::fs::metadata(&path) {
        Ok(meta) if meta.len() > MAX_TEXT_BYTES => {
            Err(Error::Conflict(format!("Text file too large: {relative}")))
        }
        Ok(_) => {
            let bytes = read_limited(&path)?;
            if relative.ends_with("/_meta.json") {
                let meta: hypr_vault_read::SessionMeta = serde_json::from_slice(&bytes)?;
                let expected =
                    hypr_vault_read::paths::validated_session_dir(&meta.id)?.join("_meta.json");
                if expected != Path::new(relative) {
                    return Err(Error::UnsafePath(relative.into()));
                }
            }
            Ok(Some(bytes))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}
fn read_limited(path: &Path) -> Result<Vec<u8>> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(MAX_TEXT_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_TEXT_BYTES {
        return Err(Error::Conflict("Sync document exceeds byte limit".into()));
    }
    Ok(bytes)
}
fn safe_path(root: &Path, relative: &str) -> Result<PathBuf> {
    let mut output = root.to_path_buf();
    let relative_path = Path::new(relative);
    if relative.is_empty() || relative.contains('\\') {
        return Err(Error::UnsafePath(relative_path.into()));
    }
    for component in relative_path.components() {
        let Component::Normal(name) = component else {
            return Err(Error::UnsafePath(relative_path.into()));
        };
        output.push(name);
        match std::fs::symlink_metadata(&output) {
            Ok(meta) if meta.file_type().is_symlink() => return Err(Error::UnsafePath(output)),
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(output)
}
fn atomic_write(root: &Path, relative: &str, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let target = safe_path(root, relative)?;
    let parent = target.parent().unwrap();
    std::fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(".sync-{}", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&temporary, &target)?;
        std::fs::File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
}
fn hash(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

#[cfg(test)]
mod tests;

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(
        String::with_capacity(bytes.len() * 2),
        |mut output, byte| {
            write!(output, "{byte:02x}").expect("String formatting cannot fail");
            output
        },
    )
}
