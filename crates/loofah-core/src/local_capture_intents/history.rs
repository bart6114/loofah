use super::*;
use std::{fs, path::Path};

const VERSION: u32 = 1;
const PAGE_SIZE: u64 = 256;
const PAGE_BYTES: u64 = 64 * 1024;
const MAX_PAGE_READS: usize = 4;
const RECORD_BYTES: u64 = 128 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct LocalCaptureHistoryPage {
    pub items: Vec<LocalCaptureControlSnapshot>,
    pub next_cursor: Option<String>,
}

#[derive(Clone)]
pub(super) struct History {
    active: PathBuf,
    directory: PathBuf,
    vault: String,
}
pub(super) struct LocatedRecord {
    pub record: Record,
    pub directory: PathBuf,
}
#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Layout {
    schema_version: u32,
    vault_id: String,
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Head {
    schema_version: u32,
    total: u64,
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Item {
    request: String,
    session_sha256: String,
    record_sha256: String,
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Page {
    schema_version: u32,
    start: u64,
    items: Vec<Item>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Pending {
    schema_version: u32,
    sequence: u64,
    item: Item,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    schema_version: u32,
    vault_id: String,
    session_sha256: Option<String>,
    snapshot: u64,
    before: u64,
}

pub(super) fn validate_layout(active: &Path, vault: &str) -> Result<bool, ApplicationError> {
    let directory = active.join("history");
    if !ledger::existing_directory(&directory)? {
        return Ok(false);
    }
    let actual: Layout = read_json(&directory.join("layout.json"), 4096)?;
    if actual.schema_version != VERSION || actual.vault_id != vault {
        return Err(review("Unsupported Local capture history layout"));
    }
    Ok(true)
}

impl History {
    pub fn open(active: &Path, vault: &str) -> Result<Self, ApplicationError> {
        ledger::ensure_directory(active)?;
        let this = Self {
            active: active.into(),
            directory: active.join("history"),
            vault: vault.into(),
        };
        ledger::ensure_directory(&this.directory)?;
        let path = this.directory.join("layout.json");
        let wanted = Layout {
            schema_version: VERSION,
            vault_id: vault.into(),
        };
        if ledger::check_file(&path)? {
            let actual: Layout = read_json(&path, 4096)?;
            if actual != wanted {
                return Err(review("Unsupported Local capture history layout"));
            }
        } else {
            // Never infer a layout for an existing nonempty archive.
            if fs::read_dir(&this.directory)
                .map_err(review)?
                .next()
                .is_some()
            {
                return Err(review("Local capture history has no layout proof"));
            }
            write_new(&path, &wanted)?;
        }
        ledger::ensure_directory(&this.directory.join("receipts"))?;
        ledger::ensure_directory(&this.directory.join("pages"))?;
        let head = this.directory.join("head.json");
        if !ledger::check_file(&head)? {
            if fs::read_dir(this.directory.join("receipts"))
                .map_err(review)?
                .next()
                .is_some()
                || fs::read_dir(this.directory.join("pages"))
                    .map_err(review)?
                    .next()
                    .is_some()
                || ledger::check_file(&this.directory.join("pending.json"))?
            {
                return Err(review("Local capture history head is missing"));
            }
            write_new(
                &head,
                &Head {
                    schema_version: VERSION,
                    total: 0,
                },
            )?;
        }
        this.head()?;
        this.recover()?;
        Ok(this)
    }

    pub fn read(&self, request: &str) -> Result<Option<LocatedRecord>, ApplicationError> {
        let directory = self.receipt_directory(request)?;
        if !ledger::existing_directory(&directory)? {
            return Ok(None);
        }
        let (record, _) = self.read_receipt(&directory, request)?;
        if !eligible(&record) {
            return Err(review("Archived Local intent is not terminal"));
        }
        Ok(Some(LocatedRecord { record, directory }))
    }

    pub fn archive(&self, expected: &Record) -> Result<(), ApplicationError> {
        if !eligible(expected) {
            return Err(review("Local intent has no confirmed terminal ownership"));
        }
        self.recover()?;
        let request = &expected.status.declaration.request_id;
        let source = self.active.join(request);
        let target = self.receipt_directory(request)?;
        if ledger::existing_directory(&target)? {
            if ledger::existing_directory(&source)? {
                return Err(review(
                    "Local intent exists in both active and history storage",
                ));
            }
            let (actual, _) = self.read_receipt(&target, request)?;
            if !same_record(&actual, expected)? {
                return Err(review("Archived Local intent changed"));
            }
            return Ok(());
        }
        let (actual, raw) = self.read_receipt(&source, request)?;
        if !same_record(&actual, expected)? {
            return Err(review("Local intent changed before archival"));
        }
        check_directory(&source)?;
        let head = self.head()?;
        let pending = Pending {
            schema_version: VERSION,
            sequence: head.total,
            item: Item {
                request: request.clone(),
                session_sha256: ledger::hash(actual.status.declaration.session_id.as_bytes()),
                record_sha256: ledger::hash(&raw),
            },
        };
        head.total
            .checked_add(1)
            .ok_or_else(|| review("Local history sequence exhausted"))?;
        write_new(&self.directory.join("pending.json"), &pending)?;
        self.checkpoint("intent")?;
        self.finish(&pending)
    }

    pub fn page(
        &self,
        session: Option<&str>,
        cursor: Option<&str>,
        limit: u32,
    ) -> Result<LocalCaptureHistoryPage, ApplicationError> {
        if !(1..=100).contains(&limit) {
            return Err(error(ErrorCode::Validation, "Invalid history page limit"));
        }
        if let Some(session) = session {
            hypr_loofah_protocol::validate_session_id(session)
                .map_err(|_| error(ErrorCode::Validation, "Invalid history session"))?;
        }
        let head = self.head()?;
        let session_sha256 = session.map(|value| ledger::hash(value.as_bytes()));
        let mut cursor = match cursor {
            Some(value) if value.len() <= 4096 => serde_json::from_str::<Cursor>(value)
                .map_err(|_| error(ErrorCode::Validation, "Invalid Local history cursor"))?,
            Some(_) => {
                return Err(error(
                    ErrorCode::Validation,
                    "Local history cursor exceeds its budget",
                ));
            }
            None => Cursor {
                schema_version: VERSION,
                vault_id: self.vault.clone(),
                session_sha256: session_sha256.clone(),
                snapshot: head.total,
                before: head.total,
            },
        };
        if cursor.schema_version != VERSION
            || cursor.vault_id != self.vault
            || cursor.session_sha256 != session_sha256
            || cursor.before > cursor.snapshot
            || cursor.snapshot > head.total
        {
            return Err(error(
                ErrorCode::Validation,
                "Local history cursor binding changed",
            ));
        }
        let mut items = Vec::new();
        let mut reads = 0;
        while cursor.before > 0 && items.len() < limit as usize && reads < MAX_PAGE_READS {
            let number = (cursor.before - 1) / PAGE_SIZE;
            let page = self
                .read_page(number)?
                .ok_or_else(|| review("Local history page is missing"))?;
            reads += 1;
            let start = number * PAGE_SIZE;
            let end = usize::try_from(cursor.before - start).map_err(review)?;
            if end > page.items.len() {
                return Err(review("Local history page is incomplete"));
            }
            for item in page.items[..end].iter().rev() {
                cursor.before -= 1;
                if session_sha256
                    .as_ref()
                    .is_some_and(|session| session != &item.session_sha256)
                {
                    continue;
                }
                let (record, raw) =
                    self.read_receipt(&self.receipt_directory(&item.request)?, &item.request)?;
                if ledger::hash(&raw) != item.record_sha256
                    || ledger::hash(record.status.declaration.session_id.as_bytes())
                        != item.session_sha256
                    || !eligible(&record)
                {
                    return Err(review(
                        "Local history index does not match original receipt",
                    ));
                }
                items.push(LocalCaptureControlSnapshot {
                    revision: controls::control_revision(&record)?,
                    status: record.status,
                    resolution: record.resolution,
                    actions: vec![],
                });
                if items.len() == limit as usize {
                    break;
                }
            }
        }
        Ok(LocalCaptureHistoryPage {
            items,
            next_cursor: (cursor.before > 0)
                .then(|| serde_json::to_string(&cursor).map_err(review))
                .transpose()?,
        })
    }

    fn head(&self) -> Result<Head, ApplicationError> {
        let head: Head = read_json(&self.directory.join("head.json"), 4096)?;
        if head.schema_version != VERSION {
            return Err(review("Unsupported Local history head"));
        }
        Ok(head)
    }
    fn receipt_directory(&self, request: &str) -> Result<PathBuf, ApplicationError> {
        uuid(request)?;
        Ok(self
            .directory
            .join("receipts")
            .join(&request[..2])
            .join(request))
    }
    fn read_receipt(
        &self,
        directory: &Path,
        request: &str,
    ) -> Result<(Record, Vec<u8>), ApplicationError> {
        uuid(request)?;
        if !ledger::existing_directory(directory)? {
            return Err(review("Original Local receipt directory is missing"));
        }
        let raw = ledger::read(&directory.join("intent.json"), RECORD_BYTES)?;
        let record: Record = serde_json::from_slice(&raw).map_err(review)?;
        journal::validate_record(&record)?;
        if record.vault_id != self.vault || record.status.declaration.request_id != request {
            return Err(review("Original Local history identity changed"));
        }
        Ok((record, raw))
    }
    fn page_path(&self, number: u64) -> PathBuf {
        self.directory
            .join("pages")
            .join(format!("{number:016x}.json"))
    }
    fn read_page(&self, number: u64) -> Result<Option<Page>, ApplicationError> {
        let path = self.page_path(number);
        if !ledger::check_file(&path)? {
            return Ok(None);
        }
        let page: Page = read_json(&path, PAGE_BYTES)?;
        if page.schema_version != VERSION
            || page.start
                != number
                    .checked_mul(PAGE_SIZE)
                    .ok_or_else(|| review("History page overflow"))?
            || page.items.is_empty()
            || page.items.len() > PAGE_SIZE as usize
        {
            return Err(review("Invalid Local history page"));
        }
        for item in &page.items {
            validate_item(item)?;
        }
        Ok(Some(page))
    }
    fn recover(&self) -> Result<(), ApplicationError> {
        let path = self.directory.join("pending.json");
        if !ledger::check_file(&path)? {
            return Ok(());
        }
        let pending: Pending = read_json(&path, 4096)?;
        self.finish(&pending)
    }
    fn finish(&self, pending: &Pending) -> Result<(), ApplicationError> {
        if pending.schema_version != VERSION {
            return Err(review("Unsupported Local archive intent"));
        }
        validate_item(&pending.item)?;
        let next = pending
            .sequence
            .checked_add(1)
            .ok_or_else(|| review("Local history sequence exhausted"))?;
        let head = self.head()?;
        if head.total != pending.sequence && head.total != next {
            return Err(review("Local archive head changed"));
        }
        let source = self.active.join(&pending.item.request);
        let target = self.receipt_directory(&pending.item.request)?;
        let source_present = ledger::existing_directory(&source)?;
        let target_present = ledger::existing_directory(&target)?;
        let current = match (source_present, target_present) {
            (true, false) if head.total == pending.sequence => &source,
            (false, true) => &target,
            _ => return Err(review("Local archive move requires review")),
        };
        let (record, raw) = self.read_receipt(current, &pending.item.request)?;
        if !eligible(&record)
            || ledger::hash(&raw) != pending.item.record_sha256
            || ledger::hash(record.status.declaration.session_id.as_bytes())
                != pending.item.session_sha256
        {
            return Err(review("Original Local archive receipt changed"));
        }
        check_directory(current)?;
        if source_present {
            ledger::ensure_directory(
                target
                    .parent()
                    .ok_or_else(|| review("Invalid Local history path"))?,
            )?;
            hypr_storage::fs::rename_no_replace(&source, &target).map_err(review)?;
            ledger::sync_parent(&source)?;
            ledger::sync_parent(&target)?;
            self.checkpoint("move")?;
        }
        let number = pending.sequence / PAGE_SIZE;
        let offset = (pending.sequence % PAGE_SIZE) as usize;
        let mut page = self.read_page(number)?.unwrap_or(Page {
            schema_version: VERSION,
            start: number * PAGE_SIZE,
            items: vec![],
        });
        if page.items.len() == offset {
            page.items.push(pending.item.clone());
            save(&self.page_path(number), &page)?;
        } else if page.items.len() != offset + 1 || page.items.get(offset) != Some(&pending.item) {
            return Err(review("Local archive index slot changed"));
        }
        self.checkpoint("page")?;
        if head.total == pending.sequence {
            save(
                &self.directory.join("head.json"),
                &Head {
                    schema_version: VERSION,
                    total: next,
                },
            )?;
        }
        self.checkpoint("head")?;
        fs::remove_file(self.directory.join("pending.json")).map_err(review)?;
        ledger::sync_parent(&self.directory.join("pending.json"))?;
        Ok(())
    }
    fn checkpoint(&self, _stage: &str) -> Result<(), ApplicationError> {
        #[cfg(test)]
        tests::checkpoint(&self.directory, _stage)?;
        Ok(())
    }
}

pub(super) fn eligible(record: &Record) -> bool {
    if record.status.state == LocalCaptureState::NotStarted {
        return record.physical.is_none()
            && record.persistence.is_none()
            && !record.inference_dispatched
            && record.source.is_none()
            && record.result_sha256.is_none()
            && record.published_revision.is_none()
            && record.status.summary_job_id.is_none()
            && !record.summary_dispatch_pending
            && !record.cancel_requested;
    }
    matches!(record.version, 2 | 3)
        && resolution::confirmed_drain(record)
        && matches!(
            record.status.state,
            LocalCaptureState::Succeeded
                | LocalCaptureState::Skipped
                | LocalCaptureState::Cancelled
                | LocalCaptureState::Closed
        )
}
fn validate_item(item: &Item) -> Result<(), ApplicationError> {
    uuid(&item.request)?;
    if !hypr_loofah_protocol::is_transcription_sha256(&item.session_sha256)
        || !hypr_loofah_protocol::is_transcription_sha256(&item.record_sha256)
    {
        return Err(review("Invalid Local archive index identity"));
    }
    Ok(())
}
fn same_record(left: &Record, right: &Record) -> Result<bool, ApplicationError> {
    Ok(ledger::encode_bounded_result(left, RECORD_BYTES as usize)?
        == ledger::encode_bounded_result(right, RECORD_BYTES as usize)?)
}
fn read_json<T: serde::de::DeserializeOwned>(
    path: &Path,
    limit: u64,
) -> Result<T, ApplicationError> {
    serde_json::from_slice(&ledger::read(path, limit)?).map_err(review)
}
fn write_new(path: &Path, value: &impl Serialize) -> Result<(), ApplicationError> {
    ledger::write_new(
        path,
        &ledger::encode_bounded_result(value, PAGE_BYTES as usize)?,
    )
}
fn save(path: &Path, value: &impl Serialize) -> Result<(), ApplicationError> {
    let bytes = ledger::encode_bounded_result(value, PAGE_BYTES as usize)?;
    if ledger::check_file(path)? {
        hypr_storage::fs::atomic_write(path, std::str::from_utf8(&bytes).map_err(review)?)
            .map_err(review)?;
        ledger::sync_parent(path)
    } else {
        ledger::write_new(path, &bytes)
    }
}
fn check_directory(directory: &Path) -> Result<(), ApplicationError> {
    if !ledger::existing_directory(directory)? {
        return Err(review("Local archive directory disappeared"));
    }
    for (count, entry) in fs::read_dir(directory).map_err(review)?.enumerate() {
        if count >= 64 {
            return Err(review("Local archive directory exceeds its entry budget"));
        }
        let entry = entry.map_err(review)?;
        let metadata = fs::symlink_metadata(entry.path()).map_err(review)?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(review("Unsafe entry in Local archive directory"));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if metadata.nlink() != 1 {
                return Err(review("Shared file in Local archive directory"));
            }
        }
        #[cfg(windows)]
        {
            let file = fs::File::open(entry.path()).map_err(review)?;
            if winapi_util::file::information(&file)
                .map_err(review)?
                .number_of_links()
                != 1
            {
                return Err(review("Shared file in Local archive directory"));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
