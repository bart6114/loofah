use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tantivy::{DateTime, Index, IndexReader, ReloadPolicy, TantivyDocument, Term};

use crate::{
    Error, PROJECTION_VERSION, Result, SCHEMA_VERSION, SearchDocument, SearchRequest, SearchResult,
    SessionListItem, TagItem, build_schema, projection, register_tokenizers, search_index,
};

const BATCH_SIZE: usize = 8;
const LOCK_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone)]
pub struct Cache {
    vault: PathBuf,
    root: PathBuf,
}

#[derive(Debug, Serialize)]
pub struct CacheStatus {
    pub path: PathBuf,
    pub ready: bool,
    pub error: Option<String>,
}

#[derive(Debug, Default, Serialize)]
pub struct ReconcileReport {
    pub added: usize,
    pub updated: usize,
    pub deleted: usize,
    pub sessions: usize,
}

#[derive(Debug, Serialize)]
pub struct Progress {
    pub schema_version: &'static str,
    pub phase: &'static str,
    pub processed: usize,
    pub total: Option<usize>,
    pub added: usize,
    pub updated: usize,
    pub deleted: usize,
    pub elapsed_ms: u128,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Stamp {
    length: u64,
    modified_ns: u128,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(unix)]
    changed_s: i64,
    #[cfg(unix)]
    changed_ns: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceState {
    sessions: BTreeMap<String, BTreeMap<String, Stamp>>,
    tags: Option<Stamp>,
}

impl SourceState {
    pub fn changed_ids(&self, other: &Self) -> Vec<String> {
        self.sessions
            .keys()
            .chain(other.sessions.keys())
            .filter(|id| self.sessions.get(*id) != other.sessions.get(*id))
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }
}

#[derive(Serialize, Deserialize)]
struct Manifest {
    schema_version: u32,
    projection_version: u32,
    vault: PathBuf,
    sources: SourceState,
    sessions: BTreeMap<String, SessionListItem>,
    tags: Vec<TagItem>,
}

impl Cache {
    pub fn new(global_base: &Path, vault: &Path) -> Result<Self> {
        let vault = fs::canonicalize(vault)?;
        let hash = Sha256::digest(vault.as_os_str().as_encoded_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        Ok(Self {
            vault,
            root: global_base.join("search_index").join(hash),
        })
    }

    pub fn for_vault(vault: &Path) -> Result<Self> {
        let data = dirs::data_dir()
            .ok_or_else(|| Error::InvalidInput("application-data directory unavailable".into()))?;
        let command = std::env::args_os()
            .next()
            .and_then(|path| Path::new(&path).file_name().map(|name| name.to_owned()));
        let global = default_global_base(&data, command.as_deref());
        Self::new(&global, vault)
    }

    pub fn vault(&self) -> &Path {
        &self.vault
    }
    pub fn path(&self) -> &Path {
        &self.root
    }

    pub fn status(&self) -> CacheStatus {
        let result = self.check();
        CacheStatus {
            path: self.root.clone(),
            ready: result.is_ok(),
            error: result.err().map(|error| error.to_string()),
        }
    }

    pub fn check(&self) -> Result<()> {
        let _writer = self.lock(false, &mut |_| {})?;
        let _publication = self.publication_lock(false)?;
        self.open().map(|_| ())
    }

    pub fn initialize(&self, progress: &mut impl FnMut(Progress)) -> Result<ReconcileReport> {
        fs::create_dir_all(&self.root)?;
        let _lock = self.lock(true, progress)?;
        let publication = self.publication_lock(false)?;
        if let Ok((index, manifest)) = self.open()
            && index
                .validate_checksum()
                .is_ok_and(|damaged| damaged.is_empty())
        {
            return self.reconcile_index(&index, manifest, None, progress);
        }
        drop(publication);
        let start = Instant::now();
        progress(event("scan", 0, None, &ReconcileReport::default(), start));
        let generations = self.root.join("generations");
        fs::create_dir_all(&generations)?;
        let staging = tempfile::Builder::new()
            .prefix("generation-")
            .tempdir_in(&generations)?;
        let index = Index::create_in_dir(staging.path(), build_schema())?;
        register_tokenizers(&index);
        let manifest = Manifest {
            schema_version: SCHEMA_VERSION,
            projection_version: PROJECTION_VERSION,
            vault: self.vault.clone(),
            sources: SourceState::default(),
            sessions: BTreeMap::new(),
            tags: Vec::new(),
        };
        let report = self.reconcile_index(&index, manifest, None, progress)?;
        // The pointer is the readiness boundary. Never expose a partly built generation.
        let _publication = self.publication_lock(true)?;
        let generation = staging.keep();
        let name = generation.file_name().unwrap().to_str().unwrap();
        let mut pointer = tempfile::NamedTempFile::new_in(&self.root)?;
        pointer.write_all(name.as_bytes())?;
        pointer.as_file().sync_all()?;
        pointer
            .persist(self.root.join("CURRENT"))
            .map_err(|error| error.error)?;
        // Windows cannot open a directory with File::open. The pointer itself
        // is already flushed; directory fsync is available on Unix.
        #[cfg(unix)]
        File::open(&self.root)?.sync_all()?;
        // Every reader holds the shared cache lock, so retired generations can now be removed.
        for entry in fs::read_dir(&generations)? {
            let path = entry?.path();
            if path != generation && path.is_dir() {
                let _ = fs::remove_dir_all(path);
            }
        }
        progress(event(
            "ready",
            report.sessions,
            Some(report.sessions),
            &report,
            start,
        ));
        Ok(report)
    }

    pub fn reconcile(&self, progress: &mut impl FnMut(Progress)) -> Result<ReconcileReport> {
        let _lock = self.lock(true, progress)?;
        let _publication = self.publication_lock(false)?;
        let (index, manifest) = self.open()?;
        self.reconcile_index(&index, manifest, None, progress)
    }

    pub fn reconcile_sessions(&self, ids: &[String]) -> Result<ReconcileReport> {
        let _lock = self.lock(true, &mut |_| {})?;
        let _publication = self.publication_lock(false)?;
        let (index, manifest) = self.open()?;
        self.reconcile_index(&index, manifest, Some(ids), &mut |_| {})
    }

    pub fn search(&self, request: SearchRequest) -> Result<SearchResult> {
        let _publication = self.publication_lock(false)?;
        let (index, _) = self.open_without_count_check()?;
        let reader = reader(&index)?;
        search_index(&index, &reader, request)
    }

    pub fn search_fresh(&self, request: SearchRequest) -> Result<SearchResult> {
        let _lock = self.lock(true, &mut |_| {})?;
        let _publication = self.publication_lock(false)?;
        let (index, manifest) = self.open()?;
        self.reconcile_index(&index, manifest, None, &mut |_| {})?;
        search_index(&index, &reader(&index)?, request)
    }

    pub fn list_fresh(
        &self,
        query: Option<&str>,
        tags: &[String],
        untagged: bool,
        offset: usize,
        limit: usize,
    ) -> Result<(Vec<SessionListItem>, usize)> {
        if untagged && !tags.is_empty() {
            return Err(Error::InvalidInput(
                "tags cannot be combined with untagged".into(),
            ));
        }
        let wanted: Vec<_> = tags
            .iter()
            .filter_map(|tag| hypr_vault_read::normalize_tag_name(tag))
            .collect();
        let _lock = self.lock(true, &mut |_| {})?;
        let _publication = self.publication_lock(false)?;
        let (index, manifest) = self.open()?;
        self.reconcile_index(&index, manifest, None, &mut |_| {})?;
        let manifest = self.manifest(&index)?;
        let eligible: BTreeMap<_, _> = manifest
            .sessions
            .into_iter()
            .filter(|(_, session)| {
                (!untagged || session.tags.is_empty())
                    && wanted.iter().all(|tag| session.tags.contains(tag))
            })
            .collect();
        let mut sessions = if let Some(query) = query.filter(|query| !query.trim().is_empty()) {
            let request = SearchRequest {
                query: normalize_query(query),
                collection: None,
                filters: Default::default(),
                options: Default::default(),
                limit: offset.saturating_add(limit).max(1),
            };
            let result = crate::search::search_filtered(
                &index,
                &reader(&index)?,
                request,
                Some(eligible.keys().cloned().collect()),
            )?;
            let total = result.count;
            let page = result
                .hits
                .into_iter()
                .skip(offset)
                .take(limit)
                .filter_map(|hit| eligible.get(&hit.document.id).cloned())
                .collect();
            return Ok((page, total));
        } else {
            let mut sessions: Vec<_> = eligible.into_values().collect();
            sessions.sort_by(|a, b| {
                fn occurred(session: &SessionListItem) -> &str {
                    if session.started_at.is_empty() {
                        &session.created_at
                    } else {
                        &session.started_at
                    }
                }
                (occurred(b), &b.created_at, &b.id).cmp(&(occurred(a), &a.created_at, &a.id))
            });
            sessions
        };
        let total = sessions.len();
        let page = sessions.drain(..).skip(offset).take(limit).collect();
        Ok((page, total))
    }

    pub fn tags_fresh(&self) -> Result<Vec<TagItem>> {
        let _lock = self.lock(true, &mut |_| {})?;
        let _publication = self.publication_lock(false)?;
        let (index, manifest) = self.open()?;
        self.reconcile_index(&index, manifest, None, &mut |_| {})?;
        Ok(self.manifest(&index)?.tags)
    }

    pub fn sources(&self) -> Result<SourceState> {
        scan(&self.vault)
    }

    pub fn sources_for(&self, ids: &[String]) -> Result<SourceState> {
        scan_selected(&self.vault, ids)
    }

    fn open(&self) -> Result<(Index, Manifest)> {
        let (index, manifest) = self.open_without_count_check()?;
        if reader(&index)?.searcher().num_docs() as usize != manifest.sessions.len() {
            return Err(Error::Corrupt("document count mismatch".into()));
        }
        Ok((index, manifest))
    }

    fn open_without_count_check(&self) -> Result<(Index, Manifest)> {
        let name = fs::read_to_string(self.root.join("CURRENT")).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                Error::NotReady
            } else {
                error.into()
            }
        })?;
        if !name.starts_with("generation-") || Path::new(&name).components().count() != 1 {
            return Err(Error::Corrupt("invalid generation pointer".into()));
        }
        let index = Index::open_in_dir(self.root.join("generations").join(name))
            .map_err(|error| Error::Corrupt(error.to_string()))?;
        if index.schema() != build_schema() {
            return Err(Error::NotReady);
        }
        register_tokenizers(&index);
        let manifest = self.manifest(&index)?;
        Ok((index, manifest))
    }

    fn manifest(&self, index: &Index) -> Result<Manifest> {
        let meta = index
            .load_metas()
            .map_err(|error| Error::Corrupt(error.to_string()))?;
        let manifest: Manifest =
            serde_json::from_str(meta.payload.as_deref().ok_or(Error::NotReady)?)
                .map_err(|error| Error::Corrupt(error.to_string()))?;
        if manifest.schema_version != SCHEMA_VERSION
            || manifest.projection_version != PROJECTION_VERSION
            || manifest.vault != self.vault
        {
            return Err(Error::NotReady);
        }
        Ok(manifest)
    }

    fn reconcile_index(
        &self,
        index: &Index,
        mut manifest: Manifest,
        selected: Option<&[String]>,
        progress: &mut impl FnMut(Progress),
    ) -> Result<ReconcileReport> {
        let start = Instant::now();
        let mut report = ReconcileReport::default();
        progress(event("scan", 0, None, &report, start));
        let snapshot = match selected {
            Some(ids) => scan_selected(&self.vault, ids)?,
            None => scan_with_progress(&self.vault, &mut |processed| {
                progress(event("scan", processed, None, &report, start));
            })?,
        };
        let mut sources = snapshot.clone();
        if let Some(ids) = selected {
            sources.sessions = manifest.sources.sessions.clone();
            for id in ids {
                match snapshot.sessions.get(id) {
                    Some(files) => {
                        sources.sessions.insert(id.clone(), files.clone());
                    }
                    None => {
                        sources.sessions.remove(id);
                    }
                }
            }
        }
        let ids = manifest.sources.changed_ids(&sources);
        let total = ids.len();
        let tags_changed = manifest.sources.tags != sources.tags;
        let first_commit = index.load_metas()?.payload.is_none();
        if total == 0 && !tags_changed && !first_commit {
            report.sessions = manifest.sessions.len();
            progress(event("ready", 0, Some(0), &report, start));
            return Ok(report);
        }
        let tags = if tags_changed || first_commit {
            read_tags(&self.vault)?
        } else {
            manifest.tags.clone()
        };
        if stamp(&self.vault.join("tags.json"))? != sources.tags {
            return Err(Error::Changed);
        }
        let mut writer = index.writer::<TantivyDocument>(50_000_000)?;
        let fields = crate::schema::get_fields(&index.schema());
        let mut processed = 0;
        for batch in ids
            .chunks(BATCH_SIZE)
            .chain((ids.is_empty()).then_some(&[][..]))
        {
            let mut projected = Vec::new();
            for id in batch {
                let projection = if sources.sessions.contains_key(id) {
                    projection::project(&self.vault, id)
                        .map_err(|error| Error::InvalidInput(format!("session {id}: {error}")))?
                } else {
                    None
                };
                if fingerprint(&self.vault.join("sessions").join(id))?.as_ref()
                    != sources.sessions.get(id)
                {
                    return Err(Error::Changed);
                }
                projected.push((id, projection));
            }
            for (id, projection) in projected {
                writer.delete_term(Term::from_field_text(fields.id, id));
                match projection {
                    Some((document, item)) => {
                        if manifest.sessions.insert(id.clone(), item).is_some() {
                            report.updated += 1;
                        } else {
                            report.added += 1;
                        }
                        writer.add_document(index_document(index, &document))?;
                    }
                    None => {
                        if manifest.sessions.remove(id).is_some() {
                            report.deleted += 1;
                        }
                    }
                }
                if let Some(fingerprint) = sources.sessions.get(id) {
                    manifest
                        .sources
                        .sessions
                        .insert(id.clone(), fingerprint.clone());
                } else {
                    manifest.sources.sessions.remove(id);
                }
            }
            manifest.tags = tags.clone();
            manifest.sources.tags = sources.tags.clone();
            processed += batch.len();
            report.sessions = manifest.sessions.len();
            progress(event("index", processed, Some(total), &report, start));
            std::thread::yield_now();
        }
        // Do not report fresh results if an edit raced the source reads or batch commits.
        let rescan = || match selected {
            Some(ids) => scan_selected(&self.vault, ids),
            None => scan(&self.vault),
        };
        if rescan()? != snapshot {
            return Err(Error::Changed);
        }
        let mut commit = writer.prepare_commit()?;
        commit.set_payload(&serde_json::to_string(&manifest)?);
        progress(event("commit", processed, Some(total), &report, start));
        commit.commit()?;
        writer.wait_merging_threads()?;
        if rescan()? != snapshot {
            return Err(Error::Changed);
        }
        progress(event(
            if first_commit { "built" } else { "ready" },
            processed,
            Some(total),
            &report,
            start,
        ));
        Ok(report)
    }

    fn lock(&self, exclusive: bool, progress: &mut impl FnMut(Progress)) -> Result<File> {
        self.lock_file("writer.lock", exclusive, LOCK_TIMEOUT, progress)
    }

    fn publication_lock(&self, exclusive: bool) -> Result<File> {
        self.lock_file("cache.lock", exclusive, LOCK_TIMEOUT, &mut |_| {})
    }

    fn lock_file(
        &self,
        name: &str,
        exclusive: bool,
        timeout: Duration,
        progress: &mut impl FnMut(Progress),
    ) -> Result<File> {
        let path = self.root.join(name);
        if !self.root.is_dir() {
            return Err(Error::NotReady);
        }
        let file = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        let start = Instant::now();
        let mut last_notice = Duration::ZERO;
        loop {
            let result = if exclusive {
                file.try_lock()
            } else {
                file.try_lock_shared()
            };
            match result {
                Ok(()) => return Ok(file),
                Err(std::fs::TryLockError::WouldBlock) => {
                    if start.elapsed() >= timeout {
                        return Err(Error::Busy);
                    }
                    if start.elapsed() >= last_notice {
                        progress(event(
                            "waiting",
                            0,
                            None,
                            &ReconcileReport::default(),
                            start,
                        ));
                        last_notice += Duration::from_secs(1);
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(std::fs::TryLockError::Error(error)) => return Err(error.into()),
            }
        }
    }
}

pub fn default_global_base(data: &Path, command: Option<&std::ffi::OsStr>) -> PathBuf {
    let command = command.and_then(|name| {
        let path = Path::new(name);
        if path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
        {
            path.file_stem().and_then(std::ffi::OsStr::to_str)
        } else {
            name.to_str()
        }
    });
    let (current, legacy) = match command {
        Some("loof-dev" | "loofah-dev" | "fmtr-dev") => {
            ("io.loofah.dev", "org.freemeetingtranscriber.dev")
        }
        Some("loof-staging" | "loofah-staging" | "fmtr-staging") => {
            ("io.loofah.staging", "org.freemeetingtranscriber.staging")
        }
        _ => ("loofah", "free-meeting-transcriber"),
    };
    let current = data.join(current);
    let legacy = data.join(legacy);
    if current.exists() || !legacy.exists() {
        current
    } else {
        legacy
    }
}

pub fn normalize_query(query: &str) -> String {
    query.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn reader(index: &Index) -> Result<IndexReader> {
    let reader = index
        .reader_builder()
        .reload_policy(ReloadPolicy::Manual)
        .try_into()?;
    Ok(reader)
}

fn index_document(index: &Index, document: &SearchDocument) -> TantivyDocument {
    let fields = crate::schema::get_fields(&index.schema());
    let mut doc = TantivyDocument::new();
    doc.add_text(fields.id, &document.id);
    doc.add_text(fields.doc_type, &document.doc_type);
    doc.add_text(fields.language, document.language.as_deref().unwrap_or(""));
    doc.add_text(fields.title, &document.title);
    doc.add_text(fields.content, &document.content);
    doc.add_date(
        fields.created_at,
        DateTime::from_timestamp_millis(document.created_at),
    );
    for facet in &document.facets {
        if let Ok(facet) = tantivy::schema::Facet::from_text(facet) {
            doc.add_facet(fields.facets, facet);
        }
    }
    doc
}

fn event(
    phase: &'static str,
    processed: usize,
    total: Option<usize>,
    report: &ReconcileReport,
    start: Instant,
) -> Progress {
    Progress {
        schema_version: "1",
        phase,
        processed,
        total,
        added: report.added,
        updated: report.updated,
        deleted: report.deleted,
        elapsed_ms: start.elapsed().as_millis(),
    }
}

fn stamp(path: &Path) -> Result<Option<Stamp>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if metadata.file_type().is_symlink() {
        return Err(Error::InvalidInput(format!(
            "cache source is a symlink: {}",
            path.display()
        )));
    }
    #[cfg(unix)]
    use std::os::unix::fs::MetadataExt;
    Ok(Some(Stamp {
        length: metadata.len(),
        modified_ns: metadata
            .modified()?
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        #[cfg(unix)]
        device: metadata.dev(),
        #[cfg(unix)]
        inode: metadata.ino(),
        #[cfg(unix)]
        changed_s: metadata.ctime(),
        #[cfg(unix)]
        changed_ns: metadata.ctime_nsec(),
    }))
}

fn presence_stamp() -> Stamp {
    Stamp {
        length: 0,
        modified_ns: 0,
        #[cfg(unix)]
        device: 0,
        #[cfg(unix)]
        inode: 0,
        #[cfg(unix)]
        changed_s: 0,
        #[cfg(unix)]
        changed_ns: 0,
    }
}

fn fingerprint(dir: &Path) -> Result<Option<BTreeMap<String, Stamp>>> {
    stamp(dir)?;
    if stamp(&dir.join("_meta.json"))?.is_none() {
        return Ok(None);
    }
    let mut files = BTreeMap::new();
    for name in hypr_vault_read::SESSION_OWNED_FILES {
        if let Some(mut stamp) = stamp(&dir.join(name))? {
            if name.starts_with("audio.") {
                stamp = presence_stamp();
            }
            files.insert(name.into(), stamp);
        }
    }
    if let Some(stamp) = stamp(&dir.join("attachments"))? {
        files.insert("attachments".into(), stamp);
    }
    for name in ["audio", "enhanced"] {
        if name == "audio" {
            if stamp(&dir.join(name))?.is_some() {
                files.insert(name.into(), presence_stamp());
            }
            continue;
        }
        stamp(&dir.join(name))?;
        match fs::read_dir(dir.join(name)) {
            Ok(entries) => {
                for entry in entries {
                    let entry = entry?;
                    let path = entry.path();
                    let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
                        continue;
                    };
                    if stem.starts_with('.')
                        || stem.contains(".conflict-")
                        || path.extension().and_then(|extension| extension.to_str()) != Some("md")
                    {
                        continue;
                    }
                    if let Some(stamp) = stamp(&path)? {
                        files.insert(
                            format!("enhanced/{}", entry.file_name().to_string_lossy()),
                            stamp,
                        );
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(Some(files))
}

fn scan_selected(vault: &Path, ids: &[String]) -> Result<SourceState> {
    stamp(&vault.join("sessions"))?;
    let mut state = SourceState {
        tags: stamp(&vault.join("tags.json"))?,
        ..Default::default()
    };
    for id in ids {
        let relative = hypr_vault_read::paths::validated_session_dir(id)?;
        if let Some(files) = fingerprint(&vault.join(relative))? {
            state.sessions.insert(id.clone(), files);
        }
    }
    Ok(state)
}

fn scan(vault: &Path) -> Result<SourceState> {
    scan_with_progress(vault, &mut |_| {})
}

fn scan_with_progress(vault: &Path, progress: &mut impl FnMut(usize)) -> Result<SourceState> {
    stamp(&vault.join("sessions"))?;
    let mut state = SourceState {
        tags: stamp(&vault.join("tags.json"))?,
        ..Default::default()
    };
    match fs::read_dir(vault.join("sessions")) {
        Ok(entries) => {
            for entry in entries {
                let entry = entry?;
                let id = entry.file_name().to_string_lossy().into_owned();
                if id.starts_with('.') || !entry.file_type()?.is_dir() {
                    continue;
                }
                if let Some(fingerprint) = fingerprint(&entry.path())? {
                    state.sessions.insert(id, fingerprint);
                    if state.sessions.len().is_multiple_of(64) {
                        progress(state.sessions.len());
                    }
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(state)
}

fn read_tags(vault: &Path) -> Result<Vec<TagItem>> {
    #[derive(Deserialize)]
    struct Tags {
        #[serde(default)]
        tags: Vec<TagItem>,
    }
    let mut tags = match fs::read(vault.join("tags.json")) {
        Ok(raw) => {
            serde_json::from_slice::<Tags>(&raw)
                .map_err(|error| Error::InvalidInput(format!("tags.json: {error}")))?
                .tags
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(error.into()),
    };
    tags.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
    Ok(tags)
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
