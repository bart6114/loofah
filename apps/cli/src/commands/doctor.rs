use std::path::PathBuf;

use serde::Serialize;

use crate::{Args, Result, output, vault};

#[derive(Debug, Serialize)]
struct DoctorReport {
    cli_version: &'static str,
    ready: bool,
    vault: VaultReport,
    sync: serde_json::Value,
}

#[derive(Debug, Serialize)]
struct VaultReport {
    path: PathBuf,
    exists: bool,
    is_directory: bool,
    sessions: Option<usize>,
    agents_md: Option<String>,
    error: Option<String>,
}

pub fn run(args: &Args, json: bool) -> Result<bool> {
    let report = inspect(args)?;
    let rendered = if json {
        output::json("doctor", &report, None)?
    } else {
        render(&report)
    };
    output::emit(&rendered);
    Ok(report.ready)
}

fn inspect(args: &Args) -> Result<DoctorReport> {
    let path = vault::resolve_path(args)?;
    let exists = path.exists();
    let mut report = VaultReport {
        path: path.clone(),
        exists,
        is_directory: false,
        sessions: None,
        agents_md: None,
        error: None,
    };

    if !exists {
        report.error = Some("vault directory does not exist".to_string());
    } else if !path.is_dir() {
        report.error = Some("vault path is not a directory".to_string());
    } else {
        report.is_directory = true;
        let recovered = vault_sync::owner::recover(&path);
        if let Err(error) = recovered {
            report.error = Some(format!(
                "sync recovery blocks vault access: {error}; restore the original sync state directory before retrying"
            ));
        }
        match if report.error.is_none() {
            hypr_vault_read::discover_sessions(&path)
        } else {
            return Ok(DoctorReport {
                cli_version: env!("LOOFAH_VERSION"),
                ready: false,
                sync: vault_sync::owner::diagnostics(&path),
                vault: report,
            });
        } {
            Ok(scan) => {
                report.sessions = Some(scan.sessions.len());
                if !scan.errors.is_empty() {
                    report.error = Some(
                        scan.errors
                            .iter()
                            .map(ToString::to_string)
                            .collect::<Vec<_>>()
                            .join("; "),
                    );
                }
            }
            Err(error) => report.error = Some(format!("vault scan failed: {error}")),
        }
        // Only repair AGENTS.md inside a directory that already is a vault
        // (mirrors `classify_vault_dir`) — doctor pointed at an arbitrary
        // folder must not seed files into it, or `move_vault`'s
        // empty-destination check would start rejecting it.
        if path.join("sessions").is_dir() || path.join("config.json").is_file() {
            report.agents_md = Some(
                match hypr_vault_write::agents_doc::ensure_agents_doc(&path) {
                    Ok(true) => "written".to_string(),
                    Ok(false) => "up-to-date".to_string(),
                    Err(error) => format!("repair failed: {error}"),
                },
            );
        }
    }

    Ok(DoctorReport {
        cli_version: env!("LOOFAH_VERSION"),
        ready: report.is_directory && report.sessions.is_some(),
        sync: vault_sync::owner::diagnostics(&path),
        vault: report,
    })
}

fn render(report: &DoctorReport) -> String {
    let status = |value| if value { "yes" } else { "no" };
    let mut lines = vec![
        format!("loof CLI {}", report.cli_version),
        format!("Ready: {}", status(report.ready)),
        format!("Vault: {}", report.vault.path.display()),
        format!("Exists: {}", status(report.vault.exists)),
        format!("Directory: {}", status(report.vault.is_directory)),
    ];
    if let Some(sessions) = report.vault.sessions {
        lines.push(format!("Sessions: {sessions}"));
    }
    if let Some(agents_md) = &report.vault.agents_md {
        lines.push(format!("AGENTS.md: {agents_md}"));
    }
    if let Some(error) = &report.vault.error {
        lines.push(format!("Issue: {error}"));
    }
    lines.push(format!("Sync: {}", report.sync));
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use crate::cli::Command;

    use super::*;

    fn args(path: PathBuf) -> Args {
        Args {
            base: None,
            vault_path: Some(path),
            json: true,
            command: Command::Doctor,
        }
    }

    #[test]
    fn reports_missing_vault_as_not_ready() {
        let dir = tempfile::tempdir().unwrap();
        let report = inspect(&args(dir.path().join("missing-vault"))).unwrap();

        assert!(!report.ready);
        assert!(!report.vault.exists);
        assert_eq!(
            report.vault.error.as_deref(),
            Some("vault directory does not exist")
        );
    }

    #[test]
    fn reports_a_vault_file_path_as_not_ready() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vault");
        std::fs::write(&path, "not a directory").unwrap();
        let report = inspect(&args(path)).unwrap();

        assert!(!report.ready);
        assert!(report.vault.exists);
        assert!(!report.vault.is_directory);
        assert_eq!(
            report.vault.error.as_deref(),
            Some("vault path is not a directory")
        );
    }

    #[test]
    fn reports_a_scannable_vault_as_ready() {
        let dir = tempfile::tempdir().unwrap();
        let session_dir = dir.path().join("sessions/meeting-1");
        std::fs::create_dir_all(&session_dir).unwrap();
        std::fs::write(
            session_dir.join("_meta.json"),
            serde_json::json!({
                "id": "meeting-1",
                "title": "Planning",
                "started_at": null,
                "ended_at": null,
                "created_at": "2026-07-13T00:00:00Z",
                "tags": [],
            })
            .to_string(),
        )
        .unwrap();

        let report = inspect(&args(dir.path().to_path_buf())).unwrap();

        assert!(report.ready);
        assert!(report.vault.is_directory);
        assert_eq!(report.vault.sessions, Some(1));
        assert!(report.vault.error.is_none());
    }

    #[test]
    fn does_not_seed_agents_md_into_a_non_vault_directory() {
        let dir = tempfile::tempdir().unwrap();
        let report = inspect(&args(dir.path().to_path_buf())).unwrap();

        assert!(report.vault.agents_md.is_none());
        assert!(!dir.path().join("AGENTS.md").exists());
    }

    #[test]
    fn repairs_agents_md_in_a_vault() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("sessions")).unwrap();

        let report = inspect(&args(dir.path().to_path_buf())).unwrap();
        assert_eq!(report.vault.agents_md.as_deref(), Some("written"));
        assert!(dir.path().join("AGENTS.md").exists());

        let report = inspect(&args(dir.path().to_path_buf())).unwrap();
        assert_eq!(report.vault.agents_md.as_deref(), Some("up-to-date"));
    }
    #[test]
    fn reports_noncanonical_sources_without_migrating_or_hiding_healthy_sessions() {
        let dir = tempfile::tempdir().unwrap();
        for (name, id) in [("healthy", "healthy"), ("Readable note", "old-id")] {
            let session = dir.path().join("sessions").join(name);
            std::fs::create_dir_all(&session).unwrap();
            std::fs::write(
                session.join("_meta.json"),
                serde_json::json!({
                    "id": id, "title": name, "started_at": null, "ended_at": null,
                    "created_at": "2026-09-15T00:00:00Z", "tags": [],
                })
                .to_string(),
            )
            .unwrap();
        }
        let report = inspect(&args(dir.path().to_path_buf())).unwrap();
        assert_eq!(report.vault.sessions, Some(1));
        let issue = report.vault.error.unwrap();
        assert!(issue.contains("Readable note"));
        assert!(issue.contains("updated desktop app"));
        assert!(
            dir.path()
                .join("sessions/Readable note/_meta.json")
                .is_file()
        );
        assert!(!dir.path().join("sessions/old-id").exists());
    }
}
