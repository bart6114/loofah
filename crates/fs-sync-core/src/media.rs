use std::{fmt::Write, io::Read};

use sha2::{Digest, Sha256};

use hypr_vault_read::media::FileMetadata;

pub fn metadata_from_reader(
    filename: &str,
    content_type: &str,
    mut reader: impl Read,
) -> std::io::Result<FileMetadata> {
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    let mut size_bytes = 0_u64;

    loop {
        let bytes_read = reader.read(&mut buffer)?;
        if bytes_read == 0 {
            break;
        }
        hasher.update(&buffer[..bytes_read]);
        size_bytes = size_bytes
            .checked_add(bytes_read as u64)
            .ok_or_else(|| std::io::Error::other("media_file_too_large"))?;
    }

    let sha256 = hasher
        .finalize()
        .iter()
        .fold(String::with_capacity(64), |mut output, byte| {
            write!(&mut output, "{byte:02x}").unwrap();
            output
        });

    Ok(FileMetadata {
        filename: filename.to_string(),
        content_type: content_type.to_string(),
        size_bytes,
        sha256,
    })
}
