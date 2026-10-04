use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use anyhow::{Context, Result};
use notify::Watcher;
use serde::{Deserialize, Serialize};
use tantivy::schema::{STORED, TextOptions, Value};
use tantivy::{Index, IndexReader, IndexWriter, TantivyDocument, Term};

use crate::source::{self, CachedSession, Fingerprint};

pub const FORMAT_VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum Repair {
    Changed(String),
    Deleted { deleted: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CachedHeader {
    pub meta: hypr_vault_read::SessionMeta,
    pub has_words: bool,
    pub word_count: u64,
    pub transcript_ids: Vec<String>,
    pub has_note: bool,
    pub enhanced_docs: u64,
    pub tasks: Vec<hypr_vault_read::TaskItem>,
    pub fingerprints: BTreeMap<String, Fingerprint>,
    pub digest: String,
}

#[derive(Default, Serialize, Deserialize)]
struct Maintenance {
    pending: BTreeMap<String, u64>,
    generation: u64,
    scan_cursor: Option<String>,
    verify_cursor: Option<String>,
    absent: BTreeSet<String>,
    changes: usize,
    last_publish: u64,
    built: bool,
    #[serde(default)]
    event_cursor: u64,
    #[serde(default)]
    active_generation: String,
    #[serde(default)]
    root_identity: String,
}

#[derive(Debug, Default, Serialize)]
pub struct MaintenanceReport {
    pub checked: usize,
    pub read: usize,
    pub updated: Vec<String>,
    pub removed: Vec<String>,
    pub pending: usize,
    pub errors: Vec<String>,
    pub listing_us: u128,
    pub fingerprint_us: u128,
    pub content_us: u128,
}

#[derive(Clone)]
pub struct SearchReader {
    index: Index,
    reader: IndexReader,
    local: PathBuf,
}

impl SearchReader {
    pub fn search(&self, request: crate::SearchRequest) -> Result<crate::SearchResult> {
        crate::search(&self.index, &self.reader, request).map_err(|error| {
            let _ = atomic(&self.local.join("repair-cache"), b"query failed");
            error.into()
        })
    }
}

pub struct Cache {
    pub(crate) vault: PathBuf,
    pub(crate) local: PathBuf,
    pub(crate) index_dir: PathBuf,
    pub(crate) index: Index,
    reader: IndexReader,
    writer: IndexWriter,
    headers: BTreeMap<String, CachedHeader>,
    addresses: BTreeMap<String, tantivy::DocAddress>,
    state: Maintenance,
    events: mpsc::Receiver<notify::Result<notify::Event>>,
    _watcher: Option<notify::RecommendedWatcher>,
    overflow: std::sync::Arc<std::sync::atomic::AtomicBool>,
    _lock: std::fs::File,
    #[cfg(target_os = "macos")]
    history: Option<crate::history::History>,
    pub bootstrapped: bool,
}

pub fn local_path(vault: &Path, consumer: &str) -> Result<PathBuf> {
    anyhow::ensure!(
        !consumer.is_empty()
            && consumer
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-'),
        "invalid cache consumer"
    );
    let vault = std::fs::canonicalize(vault)?;
    Ok(dirs::cache_dir()
        .context("no local cache directory")?
        .join("loofah/search-v1")
        .join(source::digest(vault.as_os_str().as_encoded_bytes()))
        .join(consumer))
}

pub(crate) fn schema() -> tantivy::schema::Schema {
    let mut builder = crate::schema::build_schema_builder();
    builder.add_text_field("header", TextOptions::default().set_stored().set_fast(None));
    builder.add_text_field("session", STORED);
    builder.build()
}

pub(crate) fn atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let parent = path.parent().context("missing parent")?;
    std::fs::create_dir_all(parent)?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    temp.write_all(bytes)?;
    temp.as_file().sync_all()?;
    temp.persist(path)?;
    std::fs::File::open(parent)?.sync_all()?;
    Ok(())
}

impl Cache {
    pub fn open(vault: &Path, consumer: &str) -> Result<Self> {
        Self::open_at(vault, &local_path(vault, consumer)?)
    }

    pub fn open_at(vault: &Path, local: &Path) -> Result<Self> {
        let resolved_vault = std::fs::canonicalize(vault)?;
        let vault = resolved_vault.as_path();
        std::fs::create_dir_all(local)?;
        // Serializes CLI invocations before opening Tantivy or maintenance state.
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(local.join("writer.lock"))?;
        lock.lock()?;
        let (tx, events) = mpsc::sync_channel(4096);
        let overflow = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let watcher_overflow = overflow.clone();
        let watcher = notify::recommended_watcher(move |event| {
            if tx.try_send(event).is_err() {
                watcher_overflow.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        })
        .ok()
        .and_then(|mut watcher| {
            watcher
                .watch(vault, notify::RecursiveMode::Recursive)
                .ok()?;
            Some(watcher)
        });
        let active = std::fs::read_to_string(local.join("active"))
            .ok()
            .filter(|s| s.starts_with("index-") && !s.contains(['/', '\\', '.']));
        let existing = active
            .as_ref()
            .filter(|_| !local.join("repair-cache").exists())
            .and_then(|name| {
                let path = local.join(name);
                let index = Index::open_in_dir(&path).ok()?;
                (index.schema() == schema()).then_some((path, index))
            });
        let reused_local = existing.is_some();
        let bootstrapped;
        let (index_dir, index) = match existing {
            Some(value) => {
                bootstrapped = true;
                value
            }
            None => {
                let name = format!("index-{}", uuid::Uuid::new_v4());
                let path = local.join(&name);
                std::fs::create_dir_all(&path)?;
                let imported = crate::snapshot::import(vault, &path).unwrap_or(false);
                let index = if imported {
                    Index::open_in_dir(&path)?
                } else {
                    Index::create_in_dir(&path, schema())?
                };
                // Old generations remain available until this generation is validated.
                atomic(&local.join("active"), name.as_bytes())?;
                bootstrapped = imported;
                (path, index)
            }
        };
        crate::register_tokenizers(&index);
        let opened = (|| -> Result<(IndexReader, IndexWriter)> {
            let reader = index
                .reader_builder()
                .reload_policy(tantivy::ReloadPolicy::Manual)
                .try_into()?;
            Ok((reader, index.writer(50_000_000)?))
        })();
        let (reader, writer) = match opened {
            Ok(opened) => opened,
            Err(error) if reused_local => {
                atomic(&local.join("repair-cache"), error.to_string().as_bytes())?;
                drop(index);
                drop(lock);
                return Self::open_at(vault, local);
            }
            Err(error) => return Err(error),
        };
        let mut state: Maintenance = std::fs::read(local.join("maintenance.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let generation_name = index_dir
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        if state.active_generation != generation_name {
            state = Maintenance {
                active_generation: generation_name,
                ..Default::default()
            };
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let meta = std::fs::metadata(vault)?;
            let identity = format!("{}:{}", meta.dev(), meta.ino());
            if state.root_identity != identity {
                state.event_cursor = 0;
                state.scan_cursor = None;
                state.root_identity = identity;
            }
        }
        let _ = std::fs::remove_file(local.join("repair-cache"));
        #[cfg(target_os = "macos")]
        let history = crate::history::History::start(vault, state.event_cursor).ok();
        let mut cache = Self {
            #[cfg(target_os = "macos")]
            history,
            vault: vault.into(),
            local: local.into(),
            index_dir,
            index,
            reader,
            writer,
            headers: BTreeMap::new(),
            addresses: BTreeMap::new(),
            state,
            events,
            _watcher: watcher,
            overflow,
            _lock: lock,
            bootstrapped,
        };
        if let Err(error) = cache.reload() {
            if active.is_some() {
                atomic(&local.join("repair-cache"), b"invalid stored documents")?;
                drop(cache);
                return Self::open_at(vault, local);
            }
            return Err(error);
        }
        Ok(cache)
    }

    pub fn reader(&self) -> SearchReader {
        SearchReader {
            index: self.index.clone(),
            reader: self.reader.clone(),
            local: self.local.clone(),
        }
    }

    pub fn headers(&self) -> &BTreeMap<String, CachedHeader> {
        &self.headers
    }
    pub fn session(&self, id: &str) -> Result<Option<CachedSession>> {
        let Some(address) = self.addresses.get(id) else {
            return Ok(None);
        };
        let doc: TantivyDocument = self.reader.searcher().doc(*address)?;
        Ok(Some(serde_json::from_str(
            doc.get_first(self.index.schema().get_field("session")?)
                .and_then(|v| v.as_str())
                .context("missing cached session")?,
        )?))
    }
    pub fn search(&self, request: crate::SearchRequest) -> Result<crate::SearchResult> {
        match crate::search(&self.index, &self.reader, request) {
            Ok(result) => Ok(result),
            Err(error) => {
                let _ = atomic(&self.local.join("repair-cache"), b"query failed");
                Err(error.into())
            }
        }
    }
    fn reload(&mut self) -> Result<()> {
        self.reader.reload()?;
        let searcher = self.reader.searcher();
        let mut headers = BTreeMap::new();
        let mut addresses = BTreeMap::new();
        for (segment_ord, segment) in searcher.segment_readers().iter().enumerate() {
            let column = segment
                .fast_fields()
                .str("header")?
                .context("missing cache headers")?;
            for doc_id in segment.doc_ids_alive() {
                let address = tantivy::DocAddress {
                    segment_ord: segment_ord as u32,
                    doc_id,
                };
                let ordinal = column
                    .term_ords(doc_id)
                    .next()
                    .context("missing cached header")?;
                let mut raw = String::new();
                anyhow::ensure!(
                    column.ord_to_str(ordinal, &mut raw)?,
                    "invalid cached header ordinal"
                );
                let value: CachedHeader = serde_json::from_str(&raw)?;
                hypr_vault_read::paths::validate_session_id(&value.meta.id)?;
                addresses.insert(value.meta.id.clone(), address);
                headers.insert(value.meta.id.clone(), value);
            }
        }
        self.headers = headers;
        self.addresses = addresses;
        Ok(())
    }
    fn save(&self) -> Result<()> {
        atomic(
            &self.local.join("maintenance.json"),
            &serde_json::to_vec(&self.state)?,
        )
    }
    fn mark_pending(&mut self, id: &str) -> Result<()> {
        hypr_vault_read::paths::validate_session_id(id)?;
        self.state.generation += 1;
        self.state.pending.insert(id.into(), self.state.generation);
        Ok(())
    }
    pub fn mark(&mut self, id: &str) -> Result<()> {
        self.mark_pending(id)?;
        self.save()
    }
    pub fn mark_many(&mut self, ids: impl IntoIterator<Item = String>) -> Result<()> {
        for id in ids {
            self.mark_pending(&id)?;
        }
        self.save()
    }
    fn observe(&mut self) -> Result<()> {
        if self
            .overflow
            .swap(false, std::sync::atomic::Ordering::Relaxed)
        {
            self.state.scan_cursor = None;
        }
        let mut consumed = Vec::new();
        if let Ok(entries) = std::fs::read_dir(self.local.join("repairs")) {
            for entry in entries.take(1024) {
                let entry = entry?;
                if !entry.file_type()?.is_file()
                    || entry.path().extension().is_none_or(|e| e != "json")
                {
                    continue;
                }
                let repair: Repair = serde_json::from_slice(&std::fs::read(entry.path())?)?;
                let id = match repair {
                    Repair::Changed(id) => id,
                    Repair::Deleted { deleted } => {
                        self.state.absent.insert(deleted.clone());
                        deleted
                    }
                };
                hypr_vault_read::paths::validate_session_id(&id)?;
                self.state.generation += 1;
                self.state.pending.insert(id, self.state.generation);
                consumed.push(entry.path());
            }
        }
        self.save()?;
        for path in consumed {
            std::fs::remove_file(path)?;
        }

        #[cfg(target_os = "macos")]
        if let Some(history) = &self.history {
            if history
                .overflow
                .swap(false, std::sync::atomic::Ordering::Relaxed)
            {
                self.state.scan_cursor = None;
            }
            for event in history.events.try_iter() {
                if event.rescan {
                    self.state.scan_cursor = None;
                    self.state.event_cursor = event.cursor;
                }
                if let Ok(relative) = event.path.strip_prefix(&self.vault) {
                    let mut parts = relative.components();
                    if parts.next().and_then(|c| c.as_os_str().to_str()) == Some("sessions") {
                        if let Some(id) = parts.next().and_then(|c| c.as_os_str().to_str()) {
                            if hypr_vault_read::paths::validate_session_id(id).is_ok()
                                && !id.starts_with('.')
                                && parts
                                    .next()
                                    .and_then(|c| c.as_os_str().to_str())
                                    .is_none_or(source::is_search_source)
                            {
                                self.state.generation += 1;
                                self.state.pending.insert(id.into(), self.state.generation);
                            }
                        } else {
                            self.state.scan_cursor = None;
                        }
                    }
                }
                self.state.event_cursor = self.state.event_cursor.max(event.cursor);
            }
        }
        let events: Vec<_> = self.events.try_iter().collect();
        for event in events {
            match event {
                Ok(event) => {
                    if event.need_rescan() {
                        self.state.scan_cursor = None;
                    }
                    for path in event.paths {
                        if let Ok(relative) = path.strip_prefix(&self.vault) {
                            let mut parts = relative.components();
                            if parts.next().and_then(|c| c.as_os_str().to_str()) != Some("sessions")
                            {
                                continue;
                            }
                            if let Some(id) = parts.next().and_then(|c| c.as_os_str().to_str()) {
                                if hypr_vault_read::paths::validate_session_id(id).is_ok()
                                    && !id.starts_with('.')
                                {
                                    let artifact =
                                        parts.next().and_then(|c| c.as_os_str().to_str());
                                    if artifact.is_none_or(source::is_search_source) {
                                        self.state.generation += 1;
                                        self.state.pending.insert(id.into(), self.state.generation);
                                    }
                                }
                            } else {
                                self.state.scan_cursor = None;
                            }
                        }
                    }
                }
                Err(_) => self.state.scan_cursor = None,
            }
        }
        self.save()
    }
    fn upsert(&mut self, session: &CachedSession) -> Result<()> {
        let schema = self.index.schema();
        let fields = crate::schema::get_fields(&schema);
        let document = session.document();
        let mut doc = TantivyDocument::new();
        doc.add_text(fields.id, &document.id);
        doc.add_text(fields.doc_type, "session");
        doc.add_text(fields.language, "");
        doc.add_text(fields.title, &document.title);
        doc.add_text(fields.content, &document.content);
        doc.add_date(
            fields.created_at,
            tantivy::DateTime::from_timestamp_millis(document.created_at),
        );
        let header = CachedHeader {
            meta: session.meta.clone(),
            has_words: session.has_words(),
            word_count: session
                .transcripts
                .iter()
                .map(|t| t.words.len() as u64)
                .sum(),
            transcript_ids: session.transcripts.iter().map(|t| t.id.clone()).collect(),
            has_note: session.note.as_ref().is_some_and(|n| !n.trim().is_empty()),
            enhanced_docs: session
                .docs
                .iter()
                .filter(|doc| doc.kind == "template_output")
                .count() as u64,
            tasks: session.tasks.clone(),
            fingerprints: session.fingerprints.clone(),
            digest: session.digest.clone(),
        };
        doc.add_text(schema.get_field("header")?, serde_json::to_string(&header)?);
        doc.add_text(
            schema.get_field("session")?,
            serde_json::to_string(session)?,
        );
        self.writer
            .delete_term(Term::from_field_text(fields.id, &document.id));
        self.writer.add_document(doc)?;
        Ok(())
    }

    /// A bounded pass persists both scan cursors and repair work. Full verification
    /// reads contents but only replaces documents whose content or fingerprints changed.
    pub fn maintain(&mut self, budget: usize, full: bool) -> Result<MaintenanceReport> {
        self.observe()?;
        let mut report = MaintenanceReport::default();
        let listing_started = std::time::Instant::now();
        let membership = hypr_vault_read::layout::session_membership(&self.vault);
        report.listing_us = listing_started.elapsed().as_micros();
        let mut confirmed_absent = BTreeSet::new();
        if let Ok(ids) = &membership {
            let present: BTreeSet<_> = ids.iter().cloned().collect();
            let sessions_available = self.vault.join("sessions").is_dir();
            let known: BTreeSet<_> = self
                .headers
                .keys()
                .chain(self.state.pending.keys())
                .cloned()
                .collect();
            for id in known
                .iter()
                .filter(|id| sessions_available && !present.contains(*id))
                .cloned()
                .collect::<Vec<_>>()
            {
                // Two successful membership observations, plus a direct absence
                // check below, guard against partial sync deliveries.
                if self.state.absent.contains(&id) {
                    confirmed_absent.insert(id.clone());
                    if !self.state.pending.contains_key(&id) {
                        self.mark_pending(&id)?;
                    }
                } else {
                    self.state.absent.insert(id);
                }
            }
            self.state.absent.retain(|id| !present.contains(id));
            let candidates: Vec<_> = ids
                .iter()
                .filter(|id| {
                    self.state
                        .scan_cursor
                        .as_ref()
                        .is_none_or(|cursor| *id > cursor)
                })
                .take(budget)
                .cloned()
                .collect();
            for id in &candidates {
                report.checked += 1;
                let fingerprint_started = std::time::Instant::now();
                match source::fingerprints(&self.vault, id) {
                    Ok(fp)
                        if self.headers.get(id).is_some_and(|h| h.fingerprints == fp) && !full => {}
                    _ => {
                        self.mark_pending(id)?;
                    }
                }
                report.fingerprint_us += fingerprint_started.elapsed().as_micros();
                self.state.scan_cursor = Some(id.clone());
            }
            if candidates.last() == ids.last() || candidates.is_empty() {
                self.state.scan_cursor = None;
                self.state.built = true;
            }
        } else if let Err(e) = membership {
            report.errors.push(e.to_string());
        }
        // One content probe per invocation makes short CLI runs eventually catch
        // edits that preserve size/mtime without turning verification into reindexing.
        if let Some(id) = self
            .headers
            .keys()
            .find(|id| {
                self.state
                    .verify_cursor
                    .as_ref()
                    .is_none_or(|cursor| *id > cursor)
            })
            .cloned()
        {
            self.mark_pending(&id)?;
            self.state.verify_cursor = Some(id);
        } else {
            self.state.verify_cursor = None;
        }
        self.save()?;
        let mut work: Vec<_> = self
            .state
            .pending
            .iter()
            .map(|(id, g)| (id.clone(), *g))
            .collect();
        work.sort_by_key(|(_, g)| *g);
        work.truncate(budget);
        let mut acknowledged = Vec::new();
        for (id, generation) in work {
            let read_started = std::time::Instant::now();
            let source = source::read(&self.vault, &id);
            report.content_us += read_started.elapsed().as_micros();
            match source {
                Ok(session) => {
                    report.read += 1;
                    let changed = self.headers.get(&id).is_none_or(|h| {
                        h.digest != session.digest || h.fingerprints != session.fingerprints
                    });
                    if changed {
                        self.upsert(&session)?;
                        report.updated.push(id.clone());
                    }
                    acknowledged.push((id, generation));
                }
                Err(e) => {
                    let path = self
                        .vault
                        .join(hypr_vault_read::paths::validated_session_dir(&id)?);
                    if confirmed_absent.contains(&id)
                        && matches!(std::fs::symlink_metadata(path), Err(ref e) if e.kind() == std::io::ErrorKind::NotFound)
                    {
                        if self.headers.contains_key(&id) {
                            self.writer.delete_term(Term::from_field_text(
                                self.index.schema().get_field("id")?,
                                &id,
                            ));
                            report.removed.push(id.clone());
                        }
                        acknowledged.push((id, generation));
                    } else {
                        report.errors.push(format!("{id}: {e}"));
                        // Failed entries move to the back so a cold file cannot
                        // starve unrelated sessions across repeated short runs.
                        self.mark_pending(&id)?;
                    }
                }
            }
        }
        if !report.updated.is_empty() || !report.removed.is_empty() {
            if let Err(e) = self.writer.commit() {
                let _ = self.writer.rollback();
                return Err(e.into());
            }
            self.reload()?;
            self.state.changes += report.updated.len() + report.removed.len();
        }
        self.observe()?;
        for (id, generation) in acknowledged {
            if self.state.pending.get(&id) == Some(&generation) {
                self.state.pending.remove(&id);
            }
        }
        self.state
            .absent
            .retain(|id| self.headers.contains_key(id) || self.state.pending.contains_key(id));
        self.save()?;
        report.pending = self.state.pending.len();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs();
        if self.state.built
            && report.pending == 0
            && (self.state.last_publish == 0
                || (self.state.changes > 0 && now.saturating_sub(self.state.last_publish) >= 300))
        {
            match self.publish() {
                Ok(_) => {
                    self.state.last_publish = now;
                    self.state.changes = 0;
                    self.save()?;
                }
                Err(e) => report.errors.push(format!("snapshot: {e}")),
            }
        }
        Ok(report)
    }

    pub fn refresh(&mut self, full: bool) -> Result<MaintenanceReport> {
        self.observe()?;
        self.state.scan_cursor = None;
        let count = hypr_vault_read::layout::session_membership(&self.vault)?
            .len()
            .max(self.headers.len())
            .max(self.state.pending.len())
            .max(1);
        // Second pass confirms absence and drains changes arriving during the first.
        let mut first = self.maintain(count, full)?;
        let second = self.maintain(count, false)?;
        first.listing_us += second.listing_us;
        first.fingerprint_us += second.fingerprint_us;
        first.content_us += second.content_us;
        first.checked += second.checked;
        first.read += second.read;
        first.updated.extend(second.updated);
        first.removed.extend(second.removed);
        first.errors.extend(second.errors);
        first.pending = second.pending;
        Ok(first)
    }
}

impl std::fmt::Debug for Cache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cache")
            .field("local", &self.local)
            .field("sessions", &self.headers.len())
            .finish()
    }
}

/// A successful canonical write must remain successful if local maintenance is
/// unavailable. Callers log failures here; replay and reconciliation repair them.
pub fn queue_repair(vault: &Path, id: &str) -> Result<()> {
    queue(vault, id, Repair::Changed(id.into()))
}

pub fn queue_deletion(vault: &Path, id: &str) -> Result<()> {
    queue(vault, id, Repair::Deleted { deleted: id.into() })
}

fn queue(vault: &Path, id: &str, repair: Repair) -> Result<()> {
    hypr_vault_read::paths::validate_session_id(id)?;
    for consumer in ["desktop", "cli"] {
        let directory = local_path(vault, consumer)?.join("repairs");
        std::fs::create_dir_all(&directory)?;
        atomic(
            &directory.join(format!("{}.json", uuid::Uuid::new_v4())),
            &serde_json::to_vec(&repair)?,
        )?;
    }
    Ok(())
}
