use std::{
    collections::BTreeMap,
    fs::{self, File, Metadata, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{MaintenanceError, Result};

pub(super) type Inventory = BTreeMap<String, Entry>;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Entry {
    Directory {
        mode: u32,
    },
    File {
        mode: u32,
        bytes: u64,
        sha256: String,
    },
}

pub(super) fn relative_name(path: &Path) -> Result<String> {
    let Some(name) = path.to_str() else {
        return Err(MaintenanceError("Backup requires UTF-8 file names"));
    };
    if name.is_empty()
        || name.contains(['\\', ':'])
        || path
            .components()
            .any(|p| !matches!(p, Component::Normal(_)))
        || name
            .split('/')
            .any(|s| s.is_empty() || s == "." || s == "..")
    {
        return Err(MaintenanceError("Backup contains an unsafe relative path"));
    }
    Ok(name.into())
}

pub(super) fn existing_directory(path: &Path) -> Result<PathBuf> {
    let absolute = std::path::absolute(path)?;
    if !fs::symlink_metadata(&absolute)?.is_dir() {
        return Err(MaintenanceError("Expected an existing directory"));
    }
    Ok(absolute.canonicalize()?)
}

pub(super) fn destination(path: &Path) -> Result<PathBuf> {
    let absolute = std::path::absolute(path)?;
    let parent = existing_directory(
        absolute
            .parent()
            .ok_or(MaintenanceError("Destination needs an existing parent"))?,
    )?;
    let name = absolute
        .file_name()
        .ok_or(MaintenanceError("Destination needs a new directory name"))?;
    let destination = parent.join(name);
    match fs::symlink_metadata(&destination) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(destination),
        _ => Err(MaintenanceError(
            "Destination already exists or cannot be checked",
        )),
    }
}

pub(super) fn disjoint(paths: &[&Path]) -> Result<()> {
    for (index, path) in paths.iter().enumerate() {
        for other in paths.iter().skip(index + 1) {
            if path.starts_with(other) || other.starts_with(path) {
                return Err(MaintenanceError(
                    "Storage and bundle paths must not overlap",
                ));
            }
        }
    }
    Ok(())
}

fn metadata(path: &Path) -> Result<Metadata> {
    let meta = fs::symlink_metadata(path)?;
    if !meta.is_file() && !meta.is_dir() {
        return Err(MaintenanceError(
            "Symbolic links and special files are not supported in backups",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.is_file() && meta.nlink() != 1 {
            return Err(MaintenanceError(
                "Hard-linked files are not supported in backups",
            ));
        }
    }
    Ok(meta)
}

fn mode(meta: &Metadata) -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o777
    }
    #[cfg(not(unix))]
    {
        if meta.is_dir() { 0o700 } else { 0o600 }
    }
}

fn permissions(path: &Path, mode: u32) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
    }
    #[cfg(not(unix))]
    let _ = (path, mode);
    Ok(())
}

fn create_directory(path: &Path) -> Result<()> {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)?;
    Ok(())
}

fn create_file(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    Ok(options.open(path)?)
}

fn read_file(path: &Path) -> Result<File> {
    let before = metadata(path)?;
    if !before.is_file() {
        return Err(MaintenanceError("Expected a regular backup file"));
    }
    let file = File::open(path)?;
    let after = file.metadata()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if (before.dev(), before.ino()) != (after.dev(), after.ino()) {
            return Err(MaintenanceError("Backup file changed while opening"));
        }
    }
    if !after.is_file() || before.len() != after.len() {
        return Err(MaintenanceError("Backup file changed while opening"));
    }
    Ok(file)
}

fn transfer(source: &Path, mut target: Option<&mut File>) -> Result<(u64, String)> {
    let mut file = read_file(source)?;
    let mut hash = Sha256::new();
    let mut bytes = 0;
    let mut buffer = [0; 128 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        if let Some(target) = &mut target {
            target.write_all(&buffer[..count])?;
        }
        bytes += count as u64;
        hash.update(&buffer[..count]);
    }
    Ok((bytes, format!("{:x}", hash.finalize())))
}

pub(super) fn inventory(root: &Path) -> Result<Inventory> {
    fn visit(root: &Path, directory: &Path, entries: &mut Inventory) -> Result<()> {
        if !metadata(directory)?.is_dir() {
            return Err(MaintenanceError("Expected a backup directory"));
        }
        for child in fs::read_dir(directory)? {
            let path = child?.path();
            let name = relative_name(
                path.strip_prefix(root)
                    .map_err(|_| MaintenanceError("Invalid backup root"))?,
            )?;
            let meta = metadata(&path)?;
            let entry = if meta.is_dir() {
                visit(root, &path, entries)?;
                Entry::Directory { mode: mode(&meta) }
            } else {
                let (bytes, sha256) = transfer(&path, None)?;
                Entry::File {
                    mode: mode(&meta),
                    bytes,
                    sha256,
                }
            };
            entries.insert(name, entry);
        }
        Ok(())
    }
    let mut entries = Inventory::new();
    visit(root, root, &mut entries)?;
    Ok(entries)
}

pub(super) fn verify_inventory(root: &Path, expected: &Inventory) -> Result<()> {
    for (name, entry) in expected {
        relative_name(Path::new(name))?;
        let mode = match entry {
            Entry::Directory { mode } | Entry::File { mode, .. } => mode,
        };
        if mode & !0o777 != 0 {
            return Err(MaintenanceError("Invalid backup permissions"));
        }
        if let Some(parent) = Path::new(name)
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
        {
            if !matches!(
                expected.get(&relative_name(parent)?),
                Some(Entry::Directory { .. })
            ) {
                return Err(MaintenanceError(
                    "Backup inventory is missing a parent directory",
                ));
            }
        }
    }
    let mut actual = inventory(root)?;
    for (name, entry) in &mut actual {
        match (entry, expected.get(name)) {
            (Entry::Directory { mode }, Some(Entry::Directory { mode: original }))
            | (Entry::File { mode, .. }, Some(Entry::File { mode: original, .. })) => {
                if *mode & 0o077 != 0 {
                    return Err(MaintenanceError("Backup permissions must be owner-only"));
                }
                *mode = *original;
            }
            _ => {
                return Err(MaintenanceError(
                    "Backup inventory does not match its files",
                ));
            }
        }
    }
    if &actual != expected {
        return Err(MaintenanceError("Backup inventory or file digest mismatch"));
    }
    Ok(())
}

pub(super) fn copy_inventory(
    source: &Path,
    target: &Path,
    entries: &Inventory,
    restore: bool,
) -> Result<()> {
    if !target.exists() {
        create_directory(target)?;
    }
    for (name, entry) in entries {
        let destination = target.join(name);
        match entry {
            Entry::Directory { .. } => {
                if !destination.exists() {
                    create_directory(&destination)?;
                }
                if !metadata(&destination)?.is_dir() {
                    return Err(MaintenanceError("Restore directory conflict"));
                }
            }
            Entry::File {
                mode,
                bytes,
                sha256,
            } => {
                let mut file = create_file(&destination)?;
                let (copied, hash) = transfer(&source.join(name), Some(&mut file))?;
                if copied != *bytes || hash != *sha256 {
                    return Err(MaintenanceError("Source changed during copy"));
                }
                if restore {
                    permissions(&destination, *mode)?;
                }
                file.sync_all()?;
            }
        }
    }
    for (name, entry) in entries.iter().rev() {
        if matches!(entry, Entry::Directory { .. }) {
            File::open(target.join(name))?.sync_all()?;
        }
    }
    File::open(target)?.sync_all()?;
    Ok(())
}

pub(super) fn restore_directory_modes(root: &Path, entries: &Inventory) -> Result<()> {
    for (name, entry) in entries.iter().rev() {
        if let Entry::Directory { mode } = entry {
            permissions(&root.join(name), *mode)?;
            File::open(root.join(name))?.sync_all()?;
        }
    }
    Ok(())
}

pub(super) fn stage(destination: &Path) -> Result<PathBuf> {
    let stage = destination.with_file_name(format!(
        ".loofah-maintenance-{}.partial",
        uuid::Uuid::new_v4()
    ));
    create_directory(&stage)?;
    Ok(stage)
}

pub(super) fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = create_file(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

pub(super) fn publish(stage: &Path, target: &Path) -> Result<()> {
    File::open(stage)?.sync_all()?;
    hypr_storage::fs::rename_no_replace(stage, target)?;
    File::open(
        target
            .parent()
            .ok_or(MaintenanceError("Missing destination parent"))?,
    )?
    .sync_all()?;
    Ok(())
}

pub(super) fn read_manifest(path: &Path) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    read_file(path)?
        .take(64 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 64 * 1024 * 1024 {
        return Err(MaintenanceError("Backup manifest exceeds the size limit"));
    }
    Ok(bytes)
}

pub(super) fn check_bundle(path: &Path) -> Result<()> {
    existing_directory(path)?;
    if mode(&metadata(path)?) & 0o077 != 0
        || mode(&metadata(&path.join("manifest.json"))?) & 0o077 != 0
        || mode(&metadata(&path.join("data"))?) & 0o077 != 0
    {
        return Err(MaintenanceError(
            "Backup bundles must have owner-only permissions",
        ));
    }
    let names: std::collections::BTreeSet<_> = fs::read_dir(path)?
        .map(|e| e.map(|e| e.file_name()))
        .collect::<std::io::Result<_>>()?;
    if names
        != ["data".into(), "manifest.json".into()]
            .into_iter()
            .collect()
    {
        return Err(MaintenanceError("Backup bundle contains unexpected files"));
    }
    Ok(())
}
