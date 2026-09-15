//! Desktop-startup cutover to immutable `sessions/<id>` directories.
use super::{SessionStore, paths};
use std::collections::HashMap;

#[derive(Debug, Default, Clone, PartialEq, serde::Serialize, specta::Type)]
pub struct MigrationReport {
    pub renamed: Vec<(String, String)>,
    pub skipped: Vec<String>,
    pub failed: Vec<String>,
}

impl SessionStore {
    pub(super) async fn migrate_from_scan(
        &self,
        _guard: &super::WriteGuard<'_>,
        scan: &mut super::rebuild::SessionLayoutScan,
    ) -> MigrationReport {
        let vault = self.vault_base.clone();
        let mut snapshot = std::mem::take(scan);
        let result = tokio::task::spawn_blocking(move || {
            let report = migrate_snapshot(&vault, &mut snapshot);
            (snapshot, report)
        })
        .await;
        match result {
            Ok((snapshot, report)) => {
                *scan = snapshot;
                report
            }
            Err(error) => {
                scan.broken_dirs.push(paths::sessions_root());
                let reason = format!("session migration task failed: {error}");
                scan.errors.push(reason.clone());
                MigrationReport {
                    failed: vec![reason],
                    ..Default::default()
                }
            }
        }
    }
}

fn migrate_snapshot(
    vault: &std::path::Path,
    scan: &mut super::rebuild::SessionLayoutScan,
) -> MigrationReport {
    let mut report = MigrationReport {
        skipped: scan.errors.clone(),
        ..Default::default()
    };
    let mut candidates = Vec::new();
    let mut claims = HashMap::<String, usize>::new();
    for (index, (location, meta)) in scan.sessions.iter().enumerate() {
        let target = match paths::validated_session_dir(&meta.id) {
            Ok(target) => target,
            Err(error) => {
                report
                    .skipped
                    .push(format!("{}: {error}", location.relative_dir.display()));
                continue;
            }
        };
        if location.relative_dir == target {
            continue;
        }
        let basename = location
            .relative_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if hypr_fs_sync_core::is_uuid(basename) {
            report.skipped.push(format!(
                "{}: directory UUID does not match metadata id {:?}",
                location.relative_dir.display(),
                meta.id
            ));
            continue;
        }
        *claims.entry(meta.id.clone()).or_default() += 1;
        candidates.push((index, location.relative_dir.clone(), target));
    }
    // Preflight the entire batch before any moves; a valid canonical copy wins.
    let mut eligible = Vec::new();
    for (index, from, to) in candidates {
        let reason = if claims[&scan.sessions[index].1.id] > 1 {
            Some("multiple migration sources claim this id".to_string())
        } else {
            match std::fs::symlink_metadata(vault.join(&to)) {
                Ok(_) => Some(format!("destination {} is occupied", to.display())),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(e) => Some(format!("cannot inspect destination {}: {e}", to.display())),
            }
        };
        if let Some(reason) = reason {
            report.skipped.push(format!("{}: {reason}", from.display()));
        } else {
            eligible.push((index, from, to));
        }
    }
    for (index, from, to) in eligible {
        let source = vault.join(&from);
        let target = vault.join(&to);
        let result = hypr_storage::fs::rename_no_replace(&source, &target);
        match result {
            Ok(()) => {
                scan.sessions[index].0.relative_dir = to.clone();
                report
                    .renamed
                    .push((from.display().to_string(), to.display().to_string()));
            }
            result => report.failed.push(format!(
                "{} -> {}: {result:?}",
                from.display(),
                to.display()
            )),
        }
    }
    scan.sessions.retain(|(location, meta)| {
        paths::validated_session_dir(&meta.id).is_ok_and(|dir| dir == location.relative_dir)
    });
    scan.errors
        .extend(report.skipped.iter().chain(&report.failed).cloned());
    scan.errors.sort();
    scan.errors.dedup();
    report
}
