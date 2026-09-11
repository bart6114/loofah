//! One-pass vault statistics for the About dialog.
//!
//! Counts come from the in-memory index where possible; the disk pass is limited
//! to recording presence/size (a few stat syscalls per session) and runs on a
//! blocking thread, so the command stays fast on large vaults. A session whose
//! directory can't be resolved or whose files fail to read is skipped rather than
//! failing the whole report -- stats are informational, not authoritative.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::Serialize;

use super::{SessionStore, StoreError};

#[derive(Debug, Clone, Default, PartialEq, Serialize, specta::Type)]
pub struct VaultYearStats {
    pub year: u32,
    pub sessions: u64,
    pub recordings: u64,
    pub transcript_words: u64,
    pub enhanced_docs: u64,
    pub duration_seconds: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, specta::Type)]
pub struct VaultStats {
    pub sessions: u64,
    /// Sessions with a non-empty note.
    pub notes: u64,
    pub recordings: u64,
    pub recording_bytes: u64,
    pub transcript_words: u64,
    pub enhanced_docs: u64,
    pub tasks_total: u64,
    pub tasks_done: u64,
    pub tags: u64,
    pub people: u64,
    /// Summed `ended_at - started_at` across sessions that have both.
    pub duration_seconds: u64,
    pub first_session_at: Option<String>,
    /// Ascending by year.
    pub years: Vec<VaultYearStats>,
}

struct SessionRow {
    id: String,
    year: Option<u32>,
    has_note: bool,
    word_count: u64,
    enhanced_docs: u64,
    duration_seconds: u64,
    created_at: String,
}

fn year_of(timestamp: &str) -> Option<u32> {
    timestamp.get(..4)?.parse().ok()
}

fn duration_seconds(meta: &super::SessionMeta) -> u64 {
    let (Some(start), Some(end)) = (meta.started_at.as_deref(), meta.ended_at.as_deref()) else {
        return 0;
    };
    let (Ok(start), Ok(end)) = (
        chrono::DateTime::parse_from_rfc3339(start),
        chrono::DateTime::parse_from_rfc3339(end),
    ) else {
        return 0;
    };
    (end - start).num_seconds().max(0) as u64
}

impl SessionStore {
    pub async fn vault_stats(&self) -> Result<VaultStats, StoreError> {
        let (mut stats, rows) = {
            let index = self.index.read().unwrap();
            let rows: Vec<SessionRow> = index
                .sessions
                .values()
                .map(|entry| {
                    let meta = &entry.meta;
                    SessionRow {
                        id: meta.id.clone(),
                        year: year_of(meta.started_at.as_deref().unwrap_or(&meta.created_at)),
                        has_note: entry
                            .note_markdown
                            .as_deref()
                            .is_some_and(|note| !note.trim().is_empty()),
                        word_count: index
                            .transcripts
                            .get(&meta.id)
                            .map_or(0, |summary| summary.word_count),
                        enhanced_docs: index.docs.get(&meta.id).map_or(0, |docs| docs.len() as u64),
                        duration_seconds: duration_seconds(meta),
                        created_at: meta.created_at.clone(),
                    }
                })
                .collect();

            let all_tasks = index.tasks.values().flatten();
            let stats = VaultStats {
                sessions: rows.len() as u64,
                notes: rows.iter().filter(|row| row.has_note).count() as u64,
                enhanced_docs: rows.iter().map(|row| row.enhanced_docs).sum(),
                tasks_total: index.tasks.values().map(|tasks| tasks.len() as u64).sum(),
                tasks_done: all_tasks.filter(|task| task.status == "done").count() as u64,
                tags: index.tags.len() as u64,
                people: index.people.len() as u64,
                transcript_words: rows.iter().map(|row| row.word_count).sum(),
                duration_seconds: rows.iter().map(|row| row.duration_seconds).sum(),
                first_session_at: rows.iter().map(|row| row.created_at.clone()).min(),
                ..VaultStats::default()
            };
            (stats, rows)
        };

        let mut dirs: Vec<Option<PathBuf>> = Vec::with_capacity(rows.len());
        for row in &rows {
            let dir = match self.session_dir_cached(&row.id) {
                Ok(Some(dir)) => Some(dir),
                _ => self.session_dir(&row.id).await.ok(),
            };
            dirs.push(dir);
        }

        let vault_base = self.vault_base.clone();
        let disk = tokio::task::spawn_blocking(move || {
            let mut years: BTreeMap<u32, VaultYearStats> = BTreeMap::new();
            let mut recordings = 0u64;
            let mut recording_bytes = 0u64;

            for (row, dir) in rows.iter().zip(&dirs) {
                if let Some(year) = row.year {
                    let entry = years.entry(year).or_insert_with(|| VaultYearStats {
                        year,
                        ..VaultYearStats::default()
                    });
                    entry.sessions += 1;
                    entry.enhanced_docs += row.enhanced_docs;
                    entry.duration_seconds += row.duration_seconds;
                    entry.transcript_words += row.word_count;
                }

                let Some(dir) = dir else { continue };

                let audio_path = hypr_fs_sync_core::audio::path(&vault_base.join(dir));
                if let Some(audio_path) = audio_path {
                    recordings += 1;
                    recording_bytes += std::fs::metadata(&audio_path)
                        .map(|meta| meta.len())
                        .unwrap_or(0);
                    if let Some(year) = row.year {
                        years.get_mut(&year).unwrap().recordings += 1;
                    }
                }
            }

            (years, recordings, recording_bytes)
        })
        .await
        .map_err(|e| StoreError::Io(format!("stats task failed: {e}")))?;

        let (years, recordings, recording_bytes) = disk;
        stats.recordings = recordings;
        stats.recording_bytes = recording_bytes;
        stats.years = years.into_values().collect();
        Ok(stats)
    }
}
