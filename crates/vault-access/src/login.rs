use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};

use crate::{Error, Result};

async fn request_target(socket: &mut (impl AsyncRead + Unpin)) -> Result<String> {
    let mut bytes = Vec::new();
    loop {
        let mut chunk = [0; 1024];
        let count = socket.read(&mut chunk).await?;
        if count == 0 || bytes.len() + count > 8192 {
            return Err(Error::new(
                "unauthenticated",
                "invalid sign-in callback headers",
            ));
        }
        bytes.extend_from_slice(&chunk[..count]);
        if bytes.windows(4).any(|end| end == b"\r\n\r\n") {
            break;
        }
    }
    let request = std::str::from_utf8(&bytes)
        .map_err(|_| Error::new("unauthenticated", "invalid sign-in callback encoding"))?;
    let mut parts = request
        .lines()
        .next()
        .unwrap_or_default()
        .split_whitespace();
    let method = parts.next();
    let target = parts.next().unwrap_or_default();
    let version = parts.next();
    if method != Some("GET")
        || !target.starts_with('/')
        || target.starts_with("//")
        || !matches!(version, Some("HTTP/1.0" | "HTTP/1.1"))
        || parts.next().is_some()
    {
        return Err(Error::new(
            "unauthenticated",
            "invalid sign-in callback request",
        ));
    }
    Ok(target.into())
}

pub async fn receive_callback(
    listener: &tokio::net::TcpListener,
    callback: &str,
    state: &str,
) -> Result<String> {
    let expected = reqwest::Url::parse(callback)
        .map_err(|_| Error::new("invalid_request", "invalid native callback"))?;
    loop {
        let (mut socket, _) = listener.accept().await?;
        let target = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            request_target(&mut socket),
        )
        .await;
        let url = target
            .ok()
            .and_then(std::result::Result::ok)
            .and_then(|target| reqwest::Url::parse(&format!("http://127.0.0.1{target}")).ok());
        let fields = url
            .as_ref()
            .map(|url| url.query_pairs().collect::<Vec<_>>())
            .unwrap_or_default();
        let unique = |key: &str| {
            let mut values = fields
                .iter()
                .filter(|(name, _)| name == key)
                .map(|(_, value)| value.as_ref());
            let value = values.next()?;
            values.next().is_none().then_some(value)
        };
        let valid = url
            .as_ref()
            .is_some_and(|url| url.path() == expected.path() && url.fragment().is_none())
            && unique("state") == Some(state);
        let code = unique("code").filter(|code| !code.is_empty() && code.len() <= 512);
        let declined = valid && unique("error").is_some();
        let body = if valid && code.is_some() && !declined {
            "Sign-in complete. Return to Loofah."
        } else {
            "Invalid sign-in callback."
        };
        let status = if valid && code.is_some() && !declined {
            "200 OK"
        } else {
            "400 Bad Request"
        };
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            socket.write_all(response.as_bytes()),
        )
        .await;
        if declined {
            return Err(Error::new("unauthenticated", "sign-in was declined"));
        }
        if valid && let Some(code) = code {
            return Ok(code.to_string());
        }
    }
}

#[cfg(test)]
mod tests;
