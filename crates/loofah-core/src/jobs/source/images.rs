use std::{
    collections::HashSet,
    io::{Cursor, Read},
    sync::{Arc, LazyLock},
};

use base64::{
    Engine,
    engine::general_purpose::{STANDARD, STANDARD_NO_PAD},
};
use hypr_loofah_protocol::{ErrorCode, ProcessingJobFailure};
use image::{ImageDecoder, ImageReader};
use regex::Regex;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::{
    VaultContext,
    jobs::generation::SummaryImage,
    media::{MediaSource, media_with_metadata},
    vault::error,
};

const MAX_REFERENCES: usize = 2000;
const MAX_SOURCE_BYTES: usize = 8 * 1024 * 1024;
const MAX_IMAGE_BYTES: usize = 128 * 1024;
const MAX_TOTAL_BYTES: usize = 768 * 1024;
const MAX_DECODED_BYTES: u64 = 64 * 1024 * 1024;

pub(super) struct ImageReference {
    attachment_id: Option<String>,
    filename: Option<String>,
    data: Option<SummaryImage>,
}

pub(super) fn references(contents: &[&str]) -> Result<Vec<ImageReference>, ProcessingJobFailure> {
    static MARKDOWN: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"!\[[^\]]*]\((<[^>]+>|[^)\s]+)(?:\s+"[^"]*")?\)"#).unwrap());
    let mut references = Vec::new();
    for raw in contents {
        let raw = raw.trim();
        if raw.starts_with('{') {
            if let Ok(node) = serde_json::from_str::<Value>(raw) {
                let mut pending = vec![&node];
                while let Some(node) = pending.pop() {
                    let kind = node.get("type").and_then(Value::as_str).unwrap_or_default();
                    let attrs = node.get("attrs");
                    let src = attrs
                        .and_then(|attrs| attrs.get("src"))
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    let mime = attrs
                        .and_then(|attrs| attrs.get("mimeType"))
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    if kind == "image"
                        || (kind == "fileAttachment"
                            && (mime.starts_with("image/") || mime_for_name(src).is_some()))
                    {
                        let mut reference = from_src(src);
                        reference.attachment_id = attrs
                            .and_then(|attrs| attrs.get("attachmentId"))
                            .and_then(Value::as_str)
                            .filter(|id| !id.is_empty())
                            .map(str::to_owned);
                        reference.filename = reference.filename.or_else(|| {
                            attrs
                                .and_then(|attrs| attrs.get("path"))
                                .and_then(Value::as_str)
                                .and_then(attachment_name)
                        });
                        push(&mut references, reference)?;
                    }
                    if let Some(content) = node.get("content").and_then(Value::as_array) {
                        pending.extend(content.iter().rev());
                    }
                }
            }
        } else {
            for found in MARKDOWN.captures_iter(raw) {
                let src = found[1]
                    .trim_start_matches('<')
                    .trim_end_matches('>')
                    .replace("\\(", "(")
                    .replace("\\)", ")");
                push(&mut references, from_src(&src))?;
            }
        }
    }
    Ok(references)
}

fn push(
    references: &mut Vec<ImageReference>,
    reference: ImageReference,
) -> Result<(), ProcessingJobFailure> {
    if reference.data.is_some() || reference.attachment_id.is_some() || reference.filename.is_some()
    {
        if references.len() >= MAX_REFERENCES {
            return Err(ProcessingJobFailure::ResourceLimit);
        }
        references.push(reference);
    }
    Ok(())
}

fn from_src(src: &str) -> ImageReference {
    ImageReference {
        data: data_url(src),
        attachment_id: None,
        filename: attachment_name(src),
    }
}

fn data_url(src: &str) -> Option<SummaryImage> {
    let (header, data) = src.split_once(',')?;
    let header = header.to_ascii_lowercase();
    let mime = header.strip_prefix("data:")?.strip_suffix(";base64")?;
    let mime = if mime == "image/jpg" {
        "image/jpeg"
    } else {
        mime
    };
    if !matches!(
        mime,
        "image/png" | "image/jpeg" | "image/webp" | "image/gif"
    ) || data.len() > 16 * 1024 * 1024
    {
        return None;
    }
    let data: String = data
        .chars()
        .filter(|character| !character.is_whitespace() && *character != '\u{feff}')
        .collect();
    if data.len() > MAX_SOURCE_BYTES.div_ceil(3) * 4 {
        return None;
    }
    let bytes = STANDARD
        .decode(&data)
        .or_else(|_| STANDARD_NO_PAD.decode(&data))
        .ok()?;
    (bytes.len() <= MAX_SOURCE_BYTES).then(|| SummaryImage {
        bytes,
        mime_type: mime.into(),
    })
}

fn attachment_name(src: &str) -> Option<String> {
    let src = src.trim();
    let normalized = if let Ok(url) = url::Url::parse(src) {
        if !matches!(url.scheme(), "file" | "asset") {
            return None;
        }
        urlencoding::decode(url.path()).ok()?.into_owned()
    } else {
        let normalized = urlencoding::decode(src)
            .unwrap_or_else(|_| src.into())
            .into_owned();
        if !normalized.starts_with("attachments/")
            && !normalized.contains("/attachments/")
            && !normalized.contains("\\attachments\\")
        {
            return None;
        }
        normalized
    };
    let name = normalized
        .split(['/', '\\'])
        .rfind(|part| !part.is_empty())?;
    let name = urlencoding::decode(name)
        .unwrap_or_else(|_| name.into())
        .into_owned();
    hypr_vault_read::paths::validate_attachment_id(&name).ok()?;
    Some(name)
}

fn mime_for_name(name: &str) -> Option<&'static str> {
    let extension = name.rsplit('.').next()?.to_ascii_lowercase();
    match extension.as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}

fn order(count: usize) -> Vec<usize> {
    if count <= 10 {
        return (0..count).collect();
    }
    let sampled: Vec<_> = (0..10)
        .map(|index| ((index * (count - 1)) as f64 / 9.0).round() as usize)
        .collect();
    let mut ordered = sampled.clone();
    ordered.extend((0..count).filter(|index| !sampled.contains(index)));
    ordered
}

pub(super) async fn collect(
    context: Arc<VaultContext>,
    session_id: &str,
    references: Vec<ImageReference>,
    cancel: CancellationToken,
) -> Result<Vec<SummaryImage>, ProcessingJobFailure> {
    let mut selected = Vec::new();
    let mut seen = HashSet::new();
    let mut total = 0;
    let mut references: Vec<_> = references.into_iter().map(Some).collect();
    for index in order(references.len()) {
        if cancel.is_cancelled() {
            return Err(ProcessingJobFailure::Interrupted);
        }
        if total >= MAX_TOTAL_BYTES {
            break;
        }
        let reference = references[index].take().unwrap();
        let source = if let Some(data) = reference.data {
            Some(data)
        } else {
            let mut source = None;
            for id in [reference.attachment_id, reference.filename]
                .into_iter()
                .flatten()
            {
                let Some(mime) = mime_for_name(&id) else {
                    continue;
                };
                if hypr_vault_read::paths::validate_attachment_id(&id).is_err()
                    || !seen.insert(id.clone())
                {
                    continue;
                }
                if let Some(bytes) = read(&context, session_id, id).await {
                    source = Some(SummaryImage {
                        bytes,
                        mime_type: mime.into(),
                    });
                    break;
                }
            }
            source
        };
        let Some(source) = source else {
            continue;
        };
        let target = MAX_IMAGE_BYTES.min(MAX_TOTAL_BYTES - total);
        let stopping = cancel.clone();
        let image = tokio::task::spawn_blocking(move || budget(source, target, &stopping))
            .await
            .map_err(|_| ProcessingJobFailure::SourceUnavailable)?;
        if let Some(image) = image {
            total += image.bytes.len();
            selected.push((index, image));
        }
        if selected.len() >= 10 {
            break;
        }
    }
    if cancel.is_cancelled() {
        return Err(ProcessingJobFailure::Interrupted);
    }
    selected.sort_by_key(|(index, _)| *index);
    Ok(selected.into_iter().map(|(_, image)| image).collect())
}

async fn read(context: &Arc<VaultContext>, session_id: &str, id: String) -> Option<Vec<u8>> {
    context
        .read_media(session_id, MediaSource::Attachment(id), |source| {
            media_with_metadata(source, |name, mime, file, size| {
                if size > MAX_SOURCE_BYTES as u64 {
                    return Err(error(
                        ErrorCode::ResourceLimit,
                        "Image exceeds its source budget",
                    ));
                }
                let mut bytes = Vec::new();
                file.take(MAX_SOURCE_BYTES as u64 + 1)
                    .read_to_end(&mut bytes)
                    .map_err(|_| error(ErrorCode::Internal, "Cannot read source image"))?;
                if bytes.len() > MAX_SOURCE_BYTES {
                    return Err(error(
                        ErrorCode::ResourceLimit,
                        "Image exceeds its source budget",
                    ));
                }
                let metadata =
                    hypr_fs_sync_core::media::metadata_from_reader(name, mime, bytes.as_slice())
                        .map_err(|_| error(ErrorCode::Internal, "Cannot verify source image"))?;
                Ok((metadata, bytes))
            })
            .map(|(_, _, _, bytes)| bytes)
        })
        .await
        .ok()
        .flatten()
}

fn budget(source: SummaryImage, target: usize, cancel: &CancellationToken) -> Option<SummaryImage> {
    if target == 0 || source.bytes.len() > MAX_SOURCE_BYTES || cancel.is_cancelled() {
        return None;
    }
    let format = image::ImageFormat::from_mime_type(&source.mime_type)?;
    let mut reader = ImageReader::with_format(Cursor::new(&source.bytes), format);
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(MAX_DECODED_BYTES);
    limits.max_image_width = Some(16_384);
    limits.max_image_height = Some(16_384);
    reader.limits(limits);
    let mut decoder = reader.into_decoder().ok()?;
    if decoder.total_bytes() > MAX_DECODED_BYTES {
        return None;
    }
    if source.bytes.len() <= target {
        drop(decoder);
        return Some(source);
    }
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut bitmap = image::DynamicImage::from_decoder(decoder).ok()?;
    bitmap.apply_orientation(orientation);
    let (width, height) = (bitmap.width(), bitmap.height());
    let mut edge = 1280;
    while edge >= 512 {
        let scale = (edge as f64 / width.max(height) as f64).min(1.0);
        let resized = bitmap
            .resize_exact(
                (width as f64 * scale).round().max(1.0) as u32,
                (height as f64 * scale).round().max(1.0) as u32,
                image::imageops::FilterType::Triangle,
            )
            .to_rgb8();
        for quality in [82, 72, 62, 52] {
            if cancel.is_cancelled() {
                return None;
            }
            let mut bytes = Vec::new();
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, quality)
                .encode_image(&resized)
                .ok()?;
            if bytes.len() <= target {
                return Some(SummaryImage {
                    bytes,
                    mime_type: "image/jpeg".into(),
                });
            }
        }
        edge = edge * 3 / 4;
    }
    None
}

#[cfg(test)]
mod tests;
