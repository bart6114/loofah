use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tantivy::directory::{Directory, META_LOCK};

use crate::{
    Cache, FORMAT_VERSION,
    cache::{atomic, schema},
    source::digest,
};

const ROOT: &str = ".loofah-cache/search-v1";
const MAX_PACK_BYTES: u64 = 8 * 1024 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
struct Manifest {
    format: u32,
    tantivy: String,
    files: BTreeMap<String, (u64, String)>,
}

impl Cache {
    pub fn publish(&self) -> Result<PathBuf> {
        let publisher_path = self.local.join("publisher");
        let publisher = match std::fs::read_to_string(&publisher_path) {
            Ok(id) if uuid::Uuid::parse_str(&id).is_ok() => id,
            _ => {
                let id = uuid::Uuid::new_v4().to_string();
                atomic(&publisher_path, id.as_bytes())?;
                id
            }
        };
        let directory = self.vault.join(ROOT).join(publisher);
        std::fs::create_dir_all(&directory)?;
        let mut temporary = tempfile::NamedTempFile::new_in(&directory)?;
        // Holding META_LOCK prevents commit/GC from changing the manifest or
        // deleting referenced segments until their bytes have been copied.
        let _guard = self.index.directory().acquire_lock(&META_LOCK)?;
        let segments = self.index.searchable_segment_metas()?;
        let mut names: BTreeSet<PathBuf> = segments.iter().flat_map(|s| s.list_files()).collect();
        names.insert("meta.json".into());
        let mut manifest = Manifest {
            format: FORMAT_VERSION,
            tantivy: "0.25".into(),
            files: BTreeMap::new(),
        };
        let mut archive = tar::Builder::new(&mut temporary);
        for name in names {
            let path = self.index_dir.join(&name);
            if !path.try_exists()? {
                continue;
            }
            let bytes = std::fs::read(path)?;
            let name = name.to_str().context("non UTF-8 segment")?;
            manifest
                .files
                .insert(name.into(), (bytes.len() as u64, digest(&bytes)));
            append(&mut archive, name, &bytes)?;
        }
        append(
            &mut archive,
            "manifest.json",
            &serde_json::to_vec(&manifest)?,
        )?;
        archive.finish()?;
        drop(archive);
        temporary.as_file().sync_all()?;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let destination = directory.join(format!("{stamp:020}-{}.pack", uuid::Uuid::new_v4()));
        temporary.persist_noclobber(&destination)?;
        std::fs::File::open(&directory)?.sync_all()?;
        let mut completed: Vec<_> = std::fs::read_dir(&directory)?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "pack"))
            .collect();
        completed.sort();
        for path in completed.iter().rev().skip(2) {
            std::fs::remove_file(path)?;
        }
        Ok(destination)
    }
}

fn append<W: Write>(archive: &mut tar::Builder<W>, name: &str, bytes: &[u8]) -> Result<()> {
    let mut header = tar::Header::new_gnu();
    header.set_size(bytes.len() as u64);
    header.set_mode(0o600);
    header.set_cksum();
    archive.append_data(&mut header, name, bytes)?;
    Ok(())
}

pub(crate) fn import(vault: &Path, target: &Path) -> Result<bool> {
    let mut packs = Vec::new();
    let Ok(publishers) = std::fs::read_dir(vault.join(ROOT)) else {
        return Ok(false);
    };
    for publisher in publishers.flatten() {
        if !publisher.file_type()?.is_dir() {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(publisher.path()) else {
            continue;
        };
        for entry in entries.flatten() {
            if entry.file_type()?.is_file() && entry.path().extension().is_some_and(|e| e == "pack")
            {
                packs.push(entry.path());
            }
        }
    }
    packs.sort_by(|a, b| b.file_name().cmp(&a.file_name()));
    for pack in packs {
        let temporary = tempfile::tempdir_in(target.parent().context("missing cache parent")?)?;
        if unpack(&pack, temporary.path()).is_err() {
            continue;
        }
        std::fs::remove_dir(target)?;
        std::fs::rename(temporary.path(), target)?;
        return Ok(true);
    }
    Ok(false)
}

fn unpack(pack: &Path, destination: &Path) -> Result<()> {
    anyhow::ensure!(
        std::fs::metadata(pack)?.len() <= MAX_PACK_BYTES,
        "snapshot too large"
    );
    let mut archive = tar::Archive::new(std::fs::File::open(pack)?);
    let mut actual = BTreeMap::new();
    let mut manifest = None;
    let mut total = 0u64;
    for entry in archive.entries()? {
        let mut entry = entry?;
        let name = entry
            .path()?
            .to_str()
            .context("invalid archive name")?
            .to_string();
        anyhow::ensure!(
            entry.header().entry_type().is_file()
                && !name.contains(['/', '\\'])
                && !name.starts_with('.')
                && !name.contains("lock"),
            "unsafe archive entry"
        );
        total = total
            .checked_add(entry.size())
            .context("archive overflow")?;
        anyhow::ensure!(
            total <= MAX_PACK_BYTES && entry.size() <= MAX_PACK_BYTES,
            "snapshot too large"
        );
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes)?;
        if name == "manifest.json" {
            anyhow::ensure!(manifest.is_none(), "duplicate manifest");
            manifest = Some(serde_json::from_slice::<Manifest>(&bytes)?);
        } else {
            anyhow::ensure!(!actual.contains_key(&name), "duplicate archive entry");
            actual.insert(name.clone(), (bytes.len() as u64, digest(&bytes)));
            std::fs::write(destination.join(&name), bytes)?;
        }
    }
    let manifest = manifest.context("incomplete snapshot")?;
    anyhow::ensure!(
        manifest.format == FORMAT_VERSION && manifest.tantivy == "0.25" && manifest.files == actual,
        "snapshot integrity/version mismatch"
    );
    // Recreate the local GC inventory; snapshots carry no lock or maintenance files.
    std::fs::write(
        destination.join(".managed.json"),
        serde_json::to_vec(&actual.keys().collect::<BTreeSet<_>>())?,
    )?;
    let index = tantivy::Index::open_in_dir(destination)?;
    anyhow::ensure!(index.schema() == schema(), "incompatible schema");
    anyhow::ensure!(
        index.validate_checksum()?.is_empty(),
        "segment checksum mismatch"
    );
    crate::register_tokenizers(&index);
    let reader: tantivy::IndexReader = index.reader()?;
    let searcher = reader.searcher();
    let header_field = index.schema().get_field("header")?;
    let session_field = index.schema().get_field("session")?;
    use tantivy::schema::Value;
    for (segment_ord, segment) in searcher.segment_readers().iter().enumerate() {
        for doc_id in segment.doc_ids_alive() {
            let doc: tantivy::TantivyDocument = searcher.doc(tantivy::DocAddress {
                segment_ord: segment_ord as u32,
                doc_id,
            })?;
            let header: crate::CachedHeader = serde_json::from_str(
                doc.get_first(header_field)
                    .and_then(|v| v.as_str())
                    .context("missing header")?,
            )?;
            let session: crate::CachedSession = serde_json::from_str(
                doc.get_first(session_field)
                    .and_then(|v| v.as_str())
                    .context("missing session")?,
            )?;
            hypr_vault_read::paths::validate_session_id(&header.meta.id)?;
            anyhow::ensure!(
                header.meta == session.meta && header.digest == session.digest,
                "inconsistent snapshot document"
            );
        }
    }
    Ok(())
}
