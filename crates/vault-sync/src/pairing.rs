use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use magic_wormhole::{AppConfig, AppID, Code, MailboxConnection, Wormhole};
use tokio::net::TcpListener;
use tokio_tungstenite::{
    accept_hdr_async, connect_async,
    tungstenite::{
        client::IntoClientRequest,
        handshake::server::{Request, Response},
        http::HeaderValue,
    },
};
use zeroize::Zeroizing;

use crate::{Error, Result, remote::Environment};

// The library accepts a relay URL but no authorization header. A single-use,
// loopback-only bridge keeps bearer credentials out of URLs and the UI.
struct Bridge {
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Bridge {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub struct PendingPairing {
    mailbox: MailboxConnection<serde_json::Value>,
    bridge: Bridge,
}
pub struct Pairing {
    wormhole: Wormhole,
    _bridge: Bridge,
}

fn failed(_: impl std::fmt::Display) -> Error {
    Error::Invalid("Pairing failed. Create a new code on the new Mac.".into())
}

impl PendingPairing {
    pub async fn begin(
        environment: Environment,
        credential: &str,
        code: Option<&str>,
    ) -> Result<Self> {
        if code.is_some_and(|code| code.len() > 160 || code.len() < 5) {
            return Err(Error::Authentication);
        }
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
        let path = format!("/{}", uuid::Uuid::new_v4());
        let url = format!("ws://{}{}", listener.local_addr()?, path);
        let mut request = format!(
            "{}/api/pairing/mailbox",
            environment.origin().replacen("https:", "wss:", 1)
        )
        .into_client_request()
        .map_err(failed)?;
        let token = Zeroizing::new(format!("Bearer {credential}"));
        let mut header = HeaderValue::from_str(&token).map_err(failed)?;
        header.set_sensitive(true);
        request.headers_mut().insert("authorization", header);
        let task = tokio::spawn(async move {
            let _ = tokio::time::timeout(Duration::from_secs(600), async move {
                let (stream, _) = listener.accept().await.map_err(failed)?;
                #[allow(clippy::result_large_err)] // Required by tungstenite's handshake callback signature.
                let mut local = accept_hdr_async(stream, |request: &Request, response: Response| {
                    if request.uri().path() == path && request.headers().get("origin").is_none() { Ok(response) }
                    else { Err(tokio_tungstenite::tungstenite::http::Response::builder().status(403).body(None).unwrap()) }
                }).await.map_err(failed)?;
                let (mut upstream, _) = connect_async(request).await.map_err(failed)?;
                loop {
                    tokio::select! {
                        message = local.next() => match message { Some(Ok(message)) => { if message.len() > 16_384 { break; } upstream.send(message).await.map_err(failed)?; }, _ => break },
                        message = upstream.next() => match message { Some(Ok(message)) => { if message.len() > 16_384 { break; } local.send(message).await.map_err(failed)?; }, _ => break },
                    }
                }
                Ok::<(), Error>(())
            }).await;
        });
        let bridge = Bridge { task };
        let config = AppConfig {
            id: AppID::new("io.loofah.sync.pairing.v1"),
            rendezvous_url: url.into(),
            app_version: serde_json::json!({ "loofah_pairing": 1 }),
        };
        let mailbox = if let Some(code) = code {
            MailboxConnection::connect(config, code.parse::<Code>().map_err(failed)?, false)
                .await
                .map_err(failed)?
        } else {
            MailboxConnection::create(config, 3).await.map_err(failed)?
        };
        Ok(Self { mailbox, bridge })
    }
    pub fn code(&self) -> String {
        self.mailbox.code().to_string()
    }
    pub async fn connect(self) -> Result<Pairing> {
        let wormhole = Wormhole::connect(self.mailbox).await.map_err(failed)?;
        Ok(Pairing {
            wormhole,
            _bridge: self.bridge,
        })
    }
}
impl Pairing {
    pub async fn send(&mut self, bytes: &[u8]) -> Result<()> {
        if bytes.len() > 6000 {
            return Err(Error::Authentication);
        }
        self.wormhole.send(bytes.to_vec()).await.map_err(failed)
    }
    pub async fn receive(&mut self) -> Result<Zeroizing<Vec<u8>>> {
        let bytes = Zeroizing::new(self.wormhole.receive().await.map_err(failed)?);
        if bytes.len() > 6000 {
            return Err(Error::Authentication);
        }
        Ok(bytes)
    }
    pub async fn close(self) -> Result<()> {
        self.wormhole.close().await.map_err(failed)
    }
}
