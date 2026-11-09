use super::*;
use hypr_loofah_protocol::MutationOutcome;

const STARTUP_LIMIT: usize = 8 * 1024;
const ABSENT: &str = "missing";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DesktopStartupSelection {
    Local {},
    Remote {
        profile_name: String,
        profile_id: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct DesktopStartupSnapshot {
    pub selection: DesktopStartupSelection,
    pub revision: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct SaveDesktopStartupRequest {
    pub selection: DesktopStartupSelection,
    pub expected_revision: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(tag = "kind", content = "snapshot", rename_all = "snake_case")]
pub enum DesktopStartupSaveOutcome {
    Saved(DesktopStartupSnapshot),
    Conflict(DesktopStartupSnapshot),
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredStartup {
    schema_version: u32,
    selection: DesktopStartupSelection,
}

impl ConnectionProfiles {
    fn desktop_startup_path(&self) -> PathBuf {
        self.root.join("desktop-startup.json")
    }

    /// Runs before vault selection; an absent device preference is the existing local path.
    pub fn desktop_startup(&self) -> Result<DesktopStartupSnapshot> {
        check_directory(&self.root)?;
        let Some(raw) = read_record(&self.desktop_startup_path(), STARTUP_LIMIT)? else {
            return Ok(DesktopStartupSnapshot {
                selection: DesktopStartupSelection::Local {},
                revision: ABSENT.into(),
            });
        };
        let version: serde_json::Value = serde_json::from_slice(&raw).map_err(|_| damaged())?;
        if version
            .get("schema_version")
            .and_then(serde_json::Value::as_u64)
            != Some(1)
        {
            return Err(failure(
                ErrorCode::IncompatibleVersion,
                "This startup preference requires a different Loofah version. Its file was kept.",
            ));
        }
        let stored: StoredStartup = serde_json::from_slice(&raw).map_err(|_| damaged())?;
        validate_selection(&stored.selection).map_err(|_| damaged())?;
        Ok(DesktopStartupSnapshot {
            selection: stored.selection,
            revision: hash(&raw),
        })
    }

    /// A removed or replaced connection is an error; it must never select a local vault.
    pub fn desktop_startup_profile(
        &self,
        snapshot: &DesktopStartupSnapshot,
    ) -> Result<Option<ConnectionProfile>> {
        validate_selection(&snapshot.selection)?;
        match &snapshot.selection {
            DesktopStartupSelection::Local {} => Ok(None),
            DesktopStartupSelection::Remote {
                profile_name,
                profile_id,
            } => {
                let current = self.read(profile_name)?.ok_or_else(not_found)?;
                if current.stored.profile.id != *profile_id {
                    return Err(changed());
                }
                Ok(Some(current.stored.profile))
            }
        }
    }

    pub async fn save_desktop_startup(
        self: &Arc<Self>,
        input: SaveDesktopStartupRequest,
    ) -> Result<DesktopStartupSaveOutcome> {
        self.save_desktop_startup_owned(input, None).await
    }

    pub async fn save_desktop_startup_with_profile(
        self: &Arc<Self>,
        input: SaveDesktopStartupRequest,
        profile: Arc<ProfileLease>,
    ) -> Result<DesktopStartupSaveOutcome> {
        let current = profile.profile().await;
        if !Arc::ptr_eq(self, &profile.store)
            || !matches!(&input.selection, DesktopStartupSelection::Remote { profile_name, profile_id }
                if profile_name == &current.name && profile_id == &current.id)
        {
            return Err(not_committed(failure(
                ErrorCode::Validation,
                "The leased profile does not match the requested startup selection",
            )));
        }
        self.save_desktop_startup_owned(input, Some(profile)).await
    }

    async fn save_desktop_startup_owned(
        self: &Arc<Self>,
        input: SaveDesktopStartupRequest,
        profile: Option<Arc<ProfileLease>>,
    ) -> Result<DesktopStartupSaveOutcome> {
        validate_selection(&input.selection).map_err(not_committed)?;
        if input.expected_revision != ABSENT && !is_hash(&input.expected_revision) {
            return Err(not_committed(failure(
                ErrorCode::Validation,
                "A startup preference revision is required",
            )));
        }
        let store = self.clone();
        tokio::task::spawn_blocking(move || {
            let held_profile = profile;
            let _startup_lock = store.lock("desktop-startup").map_err(not_committed)?;
            let current = store.desktop_startup().map_err(not_committed)?;
            if current.revision != input.expected_revision {
                return Ok(DesktopStartupSaveOutcome::Conflict(current));
            }
            let selected = DesktopStartupSnapshot {
                selection: input.selection.clone(),
                revision: current.revision.clone(),
            };
            // A no-op may refer to this process's live, exclusively leased profile.
            if current.selection == input.selection {
                store
                    .desktop_startup_profile(&selected)
                    .map_err(not_committed)?;
                return Ok(DesktopStartupSaveOutcome::Saved(current));
            }
            let _profile_lock = match &input.selection {
                DesktopStartupSelection::Local {} => None,
                DesktopStartupSelection::Remote { profile_id, .. } if held_profile.is_none() => {
                    Some(store.lock(profile_id).map_err(not_committed)?)
                }
                DesktopStartupSelection::Remote { .. } => None,
            };
            store
                .desktop_startup_profile(&selected)
                .map_err(not_committed)?;
            let raw = serde_json::to_string_pretty(&StoredStartup {
                schema_version: 1,
                selection: input.selection.clone(),
            })
            .map_err(|_| not_committed(damaged()))?;
            if raw.len() > STARTUP_LIMIT {
                return Err(not_committed(failure(
                    ErrorCode::ResourceLimit,
                    "Startup preference exceeds its size limit",
                )));
            }
            let path = store.desktop_startup_path();
            check_file(&path).map_err(not_committed)?;
            hypr_storage::fs::atomic_write(&path, &raw)
                .map_err(io_error)
                .and_then(|_| sync_parent(&path))
                .map_err(|cause| {
                    let mut error = cause;
                    error.mutation_outcome = Some(MutationOutcome::Unknown);
                    error
                })?;
            Ok(DesktopStartupSaveOutcome::Saved(DesktopStartupSnapshot {
                selection: input.selection,
                revision: hash(raw.as_bytes()),
            }))
        })
        .await
        .map_err(|_| {
            let mut error = worker_error();
            error.mutation_outcome = Some(MutationOutcome::Unknown);
            error
        })?
    }
}

fn validate_selection(selection: &DesktopStartupSelection) -> Result<()> {
    if let DesktopStartupSelection::Remote {
        profile_name,
        profile_id,
    } = selection
    {
        validate_name(profile_name)?;
        if !uuid::Uuid::parse_str(profile_id).is_ok_and(|id| id.to_string() == *profile_id) {
            return Err(failure(
                ErrorCode::Validation,
                "Invalid selected connection profile",
            ));
        }
    }
    Ok(())
}

fn not_committed(
    mut error: hypr_loofah_protocol::ApplicationError,
) -> hypr_loofah_protocol::ApplicationError {
    error.mutation_outcome = Some(MutationOutcome::NotCommitted);
    error
}

#[cfg(test)]
mod tests;
