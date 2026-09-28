use std::path::Path;

pub fn write_json(path: &Path, value: &impl serde::Serialize) -> Result<(), String> {
    write(
        path,
        &serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?,
    )
}

pub fn write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    hypr_storage::fs::atomic_write_bytes(path, bytes).map_err(|e| e.to_string())
}

pub fn hash(path: &Path) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut buffer = [0u8; 65536];
    let mut hasher = Sha256::new();
    loop {
        let count = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

pub fn optional_hash(path: &Path) -> Result<Option<String>, String> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
            hash(path).map(Some)
        }
        Ok(_) => Err("Expected a regular vault file".into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}
