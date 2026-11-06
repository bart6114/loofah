use std::{sync::Arc, time::Duration};

use axum::{
    Router,
    body::Body,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::Response,
    routing::{get, put},
};
use futures_util::TryStreamExt;
use hypr_loofah_protocol::ErrorCode;

use crate::{
    auth::now,
    error::{ApiError, Result},
    routes::{ApiPath, IdentityExtension, content_read},
    service::Service,
};

pub(crate) fn router() -> Router<Arc<Service>> {
    Router::new()
        .route(
            "/vaults/{vault}/sessions/{session}/audio/leases/{lease}",
            put(open),
        )
        .route(
            "/vaults/{vault}/audio/leases/{lease}",
            get(read).head(head).delete(close),
        )
}

async fn open(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath((vault, session, lease)): ApiPath<(String, String, String)>,
) -> Result<Response> {
    let access = service.catalog.access(&identity, &vault, false).await?;
    let context = service.vault(&vault).await?;
    let owner = access.member_id.clone();
    let media_permit = service
        .media_reads
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError::busy("Recording readers are busy; retry shortly"))?;
    let leases = service.recording_leases.clone();
    content_read(&service, access, 256 * 1024, async move {
        let _permit = media_permit;
        leases.open(context, &owner, &lease, &session).await
    })
    .await
}

async fn close(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath((vault, lease)): ApiPath<(String, String)>,
) -> Result<StatusCode> {
    let access = service.catalog.access(&identity, &vault, false).await?;
    service
        .recording_leases
        .close(&access.member_id, &vault, &lease)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn head(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath((vault, lease)): ApiPath<(String, String)>,
    headers: HeaderMap,
) -> Result<Response> {
    response(service, identity, vault, lease, headers, true).await
}

async fn read(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath((vault, lease)): ApiPath<(String, String)>,
    headers: HeaderMap,
) -> Result<Response> {
    response(service, identity, vault, lease, headers, false).await
}

async fn response(
    service: Arc<Service>,
    identity: crate::auth::Identity,
    vault: String,
    lease: String,
    headers: HeaderMap,
    head: bool,
) -> Result<Response> {
    let access = service.catalog.access(&identity, &vault, false).await?;
    if service.stop.is_cancelled() {
        return Err(ApiError::new(
            ErrorCode::Unavailable,
            "Server is shutting down",
        ));
    }
    let owner = access.member_id.clone();
    let reader = service.recording_leases.reader(&owner, &vault, &lease)?;
    let metadata = reader.metadata();
    let size = metadata.size_bytes;
    let etag = format!("\"sha256:{}\"", metadata.sha256);
    let mut response = Response::new(Body::empty());
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, metadata.content_type.parse().unwrap());
    response
        .headers_mut()
        .insert(header::ETAG, etag.parse().unwrap());
    response
        .headers_mut()
        .insert(header::ACCEPT_RANGES, "bytes".parse().unwrap());
    response
        .headers_mut()
        .insert("x-content-type-options", "nosniff".parse().unwrap());
    if let Some(value) = headers.get(header::IF_MATCH)
        && value.to_str().ok().is_none_or(|value| {
            value != "*" && !value.split(',').any(|candidate| candidate.trim() == etag)
        })
    {
        *response.status_mut() = StatusCode::PRECONDITION_FAILED;
        return Ok(response);
    }
    // HTTP Range applies to GET. HEAD always describes the complete pinned representation.
    let requested = if head
        || headers
            .get(header::IF_RANGE)
            .is_some_and(|value| value.to_str().ok() != Some(etag.as_str()))
    {
        None
    } else {
        headers.get(header::RANGE)
    };
    let range = match requested {
        None => ByteRange {
            start: 0,
            length: size,
        },
        Some(value) => match value
            .to_str()
            .ok()
            .and_then(|value| ByteRange::parse(value, size))
        {
            Some(range) => {
                *response.status_mut() = StatusCode::PARTIAL_CONTENT;
                range
            }
            None => {
                *response.status_mut() = StatusCode::RANGE_NOT_SATISFIABLE;
                response.headers_mut().insert(
                    header::CONTENT_RANGE,
                    format!("bytes */{size}").parse().unwrap(),
                );
                return Ok(response);
            }
        },
    };
    response.headers_mut().insert(
        header::CONTENT_LENGTH,
        range.length.to_string().parse().unwrap(),
    );
    if response.status() == StatusCode::PARTIAL_CONTENT {
        response.headers_mut().insert(
            header::CONTENT_RANGE,
            format!(
                "bytes {}-{}/{size}",
                range.start,
                range.start + range.length - 1
            )
            .parse()
            .unwrap(),
        );
    }
    drop(access);
    if head || range.length == 0 {
        return Ok(response);
    }
    let permit = service
        .recording_streams
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError::busy("Recording stream capacity reached; retry shortly"))?;
    *response.body_mut() = recording_body(service, identity, vault, reader, range, permit);
    Ok(response)
}

fn recording_body(
    service: Arc<Service>,
    identity: crate::auth::Identity,
    vault: String,
    reader: hypr_loofah_core::recording_leases::RecordingReader,
    range: ByteRange,
    permit: tokio::sync::OwnedSemaphorePermit,
) -> Body {
    type Chunk = std::result::Result<Vec<u8>, std::io::Error>;
    let (demand, mut requests) =
        tokio::sync::mpsc::channel::<tokio::sync::oneshot::Sender<Chunk>>(1);
    let length = range.length;
    // The body owns only demand channels. An unpolled body cannot retain a file reader
    // or stream slot indefinitely; the worker stops after 30 seconds without demand.
    tokio::spawn(async move {
        let _permit = permit;
        let expiry = tokio::time::sleep(Duration::from_secs(
            identity.expires_at.saturating_sub(now()),
        ));
        tokio::pin!(expiry);
        let mut offset = range.start;
        let mut remaining = length;
        while remaining > 0 {
            let reply = tokio::select! {
                _ = service.stop.cancelled() => break,
                _ = &mut expiry => break,
                request = tokio::time::timeout(Duration::from_secs(30), requests.recv()) => {
                    let Ok(Some(reply)) = request else { break; };
                    reply
                }
            };
            let operation = async {
                let access = service
                    .catalog
                    .access(&identity, &vault, false)
                    .await
                    .map_err(std::io::Error::other)?;
                let bytes = reader
                    .read_chunk(offset, remaining.min(64 * 1024) as usize)
                    .await
                    .map_err(std::io::Error::other)?;
                drop(access);
                Ok::<_, std::io::Error>(bytes)
            };
            let result = tokio::select! {
                _ = service.stop.cancelled() => Err(std::io::Error::other("Recording service stopped")),
                _ = &mut expiry => Err(std::io::Error::other("Recording authorization expired")),
                result = operation => result,
            };
            match result {
                Ok(bytes) => {
                    offset += bytes.len() as u64;
                    remaining -= bytes.len() as u64;
                    if reply.send(Ok(bytes)).is_err() {
                        break;
                    }
                }
                Err(cause) => {
                    let _ = reply.send(Err(cause));
                    break;
                }
            }
        }
    });
    let stream = async_stream::try_stream! {
        let mut remaining = length;
        while remaining > 0 {
            let (reply, response) = tokio::sync::oneshot::channel();
            demand.send(reply).await.map_err(|_| std::io::Error::other("Recording stream stopped"))?;
            let bytes = response.await.map_err(|_| std::io::Error::other("Recording stream stopped"))??;
            remaining -= bytes.len() as u64;
            yield bytes;
        }
    };
    Body::from_stream(stream.map_err(|cause: std::io::Error| cause))
}

#[derive(Debug, PartialEq)]
struct ByteRange {
    start: u64,
    length: u64,
}

impl ByteRange {
    fn parse(value: &str, size: u64) -> Option<Self> {
        if value.len() > 128 || size == 0 {
            return None;
        }
        let (start, end) = value.trim().strip_prefix("bytes=")?.split_once('-')?;
        if start.is_empty() {
            let suffix = decimal(end)?;
            if suffix == 0 {
                return None;
            }
            let length = suffix.min(size);
            return Some(Self {
                start: size - length,
                length,
            });
        }
        let start = decimal(start)?;
        if start >= size {
            return None;
        }
        let end = if end.is_empty() {
            size - 1
        } else {
            decimal(end)?.min(size - 1)
        };
        if end < start {
            return None;
        }
        Some(Self {
            start,
            length: end - start + 1,
        })
    }
}

fn decimal(value: &str) -> Option<u64> {
    if value.is_empty() || !value.bytes().all(|value| value.is_ascii_digit()) {
        return None;
    }
    value.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_ranges_are_bounded_and_arithmetic_handles_large_or_invalid_input() {
        for (value, start, length) in [
            ("bytes=0-0", 0, 1),
            ("bytes=2-5", 2, 4),
            ("bytes=8-99", 8, 2),
            ("bytes=8-", 8, 2),
            ("bytes=-3", 7, 3),
            ("bytes=-99", 0, 10),
            (" bytes=0-9 ", 0, 10),
        ] {
            assert_eq!(
                ByteRange::parse(value, 10),
                Some(ByteRange { start, length }),
                "{value}"
            );
        }
        for value in [
            "bytes=",
            "bytes=0",
            "bytes=-0",
            "bytes=9-8",
            "bytes=10-",
            "bytes=0-1,3-4",
            "bytes=+1-2",
            "bytes=1-+2",
            "bytes= 1-2",
            "items=0-1",
            "bytes=18446744073709551616-",
            "bytes=0-18446744073709551616",
        ] {
            assert_eq!(ByteRange::parse(value, 10), None, "{value}");
        }
        assert_eq!(ByteRange::parse("bytes=0-0", 0), None);
        assert_eq!(
            ByteRange::parse("bytes=18446744073709551614-18446744073709551615", u64::MAX),
            Some(ByteRange {
                start: u64::MAX - 1,
                length: 1
            })
        );
    }
}
