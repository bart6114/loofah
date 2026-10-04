use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result, bail};
use hypr_vault_read::{EnhancedDoc, SessionMeta, TranscriptWithData};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Fingerprint {
    pub size: u64,
    pub modified_ns: u128,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedSession {
    pub meta: SessionMeta,
    pub note: Option<String>,
    pub docs: Vec<EnhancedDoc>,
    pub transcripts: Vec<TranscriptWithData>,
    pub tasks: Vec<hypr_vault_read::TaskItem>,
    pub fingerprints: BTreeMap<String, Fingerprint>,
    pub digest: String,
}

pub fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

pub(crate) fn is_search_source(name: &str) -> bool {
    matches!(
        name,
        "_meta.json"
            | "notes.md"
            | "_memo.md"
            | "summary.md"
            | "transcript.json"
            | "tasks.json"
            | "enhanced"
    )
}

pub fn fingerprints(vault: &Path, id: &str) -> Result<BTreeMap<String, Fingerprint>> {
    let dir = vault.join(hypr_vault_read::paths::validated_session_dir(id)?);
    let kind = std::fs::symlink_metadata(&dir)?;
    anyhow::ensure!(
        kind.is_dir() && !kind.file_type().is_symlink(),
        "invalid session directory"
    );
    let mut names: Vec<String> = [
        "_meta.json",
        "notes.md",
        "_memo.md",
        "summary.md",
        "transcript.json",
        "tasks.json",
    ]
    .into_iter()
    .map(str::to_string)
    .collect();
    match std::fs::symlink_metadata(dir.join("enhanced")) {
        Ok(meta) => {
            anyhow::ensure!(
                meta.is_dir() && !meta.file_type().is_symlink(),
                "invalid enhanced directory"
            );
            for entry in std::fs::read_dir(dir.join("enhanced"))? {
                let entry = entry?;
                let Some(name) = entry.file_name().to_str().map(str::to_string) else {
                    continue;
                };
                if !name.starts_with('.') && !name.contains(".conflict-") && name.ends_with(".md") {
                    names.push(format!("enhanced/{name}"));
                }
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    let mut result = BTreeMap::new();
    for name in names {
        match std::fs::symlink_metadata(dir.join(&name)) {
            Ok(meta) => {
                anyhow::ensure!(
                    meta.is_file() && !meta.file_type().is_symlink(),
                    "invalid source {name}"
                );
                result.insert(
                    name,
                    Fingerprint {
                        size: meta.len(),
                        modified_ns: meta
                            .modified()?
                            .duration_since(std::time::UNIX_EPOCH)?
                            .as_nanos(),
                    },
                );
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    anyhow::ensure!(result.contains_key("_meta.json"), "metadata unavailable");
    Ok(result)
}

pub(crate) fn read(vault: &Path, id: &str) -> Result<CachedSession> {
    let before = fingerprints(vault, id)?;
    let relative = hypr_vault_read::paths::validated_session_dir(id)?;
    let mut bytes = BTreeMap::new();
    for name in before.keys() {
        let path = vault.join(&relative).join(name);
        #[cfg(target_os = "macos")]
        ensure_downloaded(&path)?;
        bytes.insert(name.clone(), std::fs::read(path)?);
    }
    let meta: SessionMeta = serde_json::from_slice(&bytes["_meta.json"])?;
    anyhow::ensure!(meta.id == id, "metadata identity mismatch");
    let text = |name: &str| -> Result<Option<String>> {
        bytes
            .get(name)
            .map(|b| String::from_utf8(b.clone()).context("invalid UTF-8"))
            .transpose()
    };
    let note = text("notes.md")?
        .or(text("_memo.md")?)
        .map(hypr_vault_read::strip_leading_frontmatter);
    let mut docs = Vec::new();
    for name in bytes.keys().filter(|name| name.starts_with("enhanced/")) {
        let stem = Path::new(name).file_stem().unwrap().to_str().unwrap();
        docs.push(hypr_vault_read::parse_enhanced_file(
            stem,
            id,
            &text(name)?.unwrap(),
        )?);
    }
    docs.sort_by(|a, b| (a.sort_order, &a.id).cmp(&(b.sort_order, &b.id)));
    let legacy = docs
        .iter()
        .find(|d| d.kind == "summary")
        .map(|d| d.markdown.clone());
    docs.retain(|d| d.kind != "summary");
    if let Some(markdown) = legacy.or(text("summary.md")?) {
        docs.push(hypr_vault_read::summary::as_document(id, markdown));
    }
    docs.sort_by(|a, b| (a.sort_order, &a.id).cmp(&(b.sort_order, &b.id)));
    let mut transcripts = match bytes.get("transcript.json") {
        Some(bytes) => {
            serde_json::from_slice::<hypr_vault_read::TranscriptJson>(bytes)?.transcripts
        }
        None => Vec::new(),
    };
    transcripts.sort_by(|a, b| a.started_at.total_cmp(&b.started_at).then(a.id.cmp(&b.id)));
    if before != fingerprints(vault, id)? {
        bail!("session changed while reading");
    }
    let tasks = bytes
        .get("tasks.json")
        .map(|b| serde_json::from_slice::<hypr_vault_read::TasksFile>(b))
        .transpose()?
        .unwrap_or_default()
        .tasks;
    Ok(CachedSession {
        tasks,
        meta,
        note,
        docs,
        transcripts,
        fingerprints: before,
        digest: digest(&serde_json::to_vec(&bytes)?),
    })
}

#[cfg(target_os = "macos")]
fn ensure_downloaded(path: &Path) -> Result<()> {
    use std::os::macos::fs::MetadataExt;
    use std::sync::atomic::{AtomicUsize, Ordering};
    // SF_DATALESS is a File Provider placeholder. Reading it synchronously can
    // hold the writer lock through a network download and block other CLI runs.
    if std::fs::symlink_metadata(path)?.st_flags() & 0x40000000 == 0 {
        return Ok(());
    }
    static REQUESTS: AtomicUsize = AtomicUsize::new(0);
    if REQUESTS
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
            (n < 2).then_some(n + 1)
        })
        .is_ok()
    {
        let path = path.to_path_buf();
        if std::thread::Builder::new()
            .name("search-download".into())
            .spawn(move || {
                objc2::rc::autoreleasepool(|_| {
                    use objc2_foundation::{NSFileManager, NSString, NSURL};
                    if let Some(path) = path.to_str() {
                        let url = NSURL::fileURLWithPath(&NSString::from_str(path));
                        let _ = NSFileManager::defaultManager()
                            .startDownloadingUbiquitousItemAtURL_error(&url);
                    }
                });
                REQUESTS.fetch_sub(1, Ordering::Relaxed);
            })
            .is_err()
        {
            REQUESTS.fetch_sub(1, Ordering::Relaxed);
        }
    }
    bail!("source download pending: {}", path.display())
}

impl CachedSession {
    pub fn has_words(&self) -> bool {
        self.transcripts.iter().any(|t| !t.words.is_empty())
    }

    pub fn document(&self) -> crate::SearchDocument {
        let mut parts = Vec::new();
        if let Some(note) = &self.note {
            parts.push(plain_text(note));
        }
        parts.extend(self.docs.iter().map(|d| plain_text(&d.markdown)));
        parts.extend(self.transcripts.iter().map(|t| {
            t.words
                .iter()
                .map(|w| w.text.trim())
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" ")
        }));
        crate::SearchDocument {
            id: self.meta.id.clone(),
            doc_type: "session".into(),
            language: None,
            title: if self.meta.title.trim().is_empty() {
                "Untitled".into()
            } else {
                self.meta.title.trim().into()
            },
            content: parts
                .iter()
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" "),
            created_at: chrono::DateTime::parse_from_rfc3339(&self.meta.created_at)
                .map(|d| d.timestamp_millis())
                .unwrap_or(0),
            facets: Vec::new(),
        }
    }
}

pub fn plain_text(value: &str) -> String {
    fn flatten(v: &serde_json::Value, out: &mut Vec<String>) {
        if let Some(text) = v.get("text").and_then(|v| v.as_str()) {
            out.push(text.into());
        }
        if let Some(children) = v.get("content").and_then(|v| v.as_array()) {
            for child in children {
                flatten(child, out);
            }
        }
    }
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(value)
        && v["type"] == "doc"
        && v["content"].is_array()
    {
        let mut out = Vec::new();
        flatten(&v, &mut out);
        return out
            .join(" ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
    }
    value.trim().into()
}
