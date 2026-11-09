use std::{collections::BTreeMap, io, net::SocketAddr, time::Duration};

use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::mpsc,
    task::{JoinHandle, JoinSet},
};

pub const SLAB_BYTES: usize = 16 * 1024;
const MAX_SLABS_PER_DIRECTION: usize = 64;
const MAX_DOWNSTREAM_PROOF_BYTES: usize = 64 * 1024;

pub fn delay_budget(rtt: Duration, connections: usize) -> Duration {
    rtt / 2 * (MAX_SLABS_PER_DIRECTION * 2 * connections) as u32
}

#[derive(Debug, Default)]
pub struct Direction {
    pub bytes: usize,
    pub slabs: usize,
    pub completed_write_bytes: usize,
    pub peer_closed: bool,
    server_bytes: Vec<u8>,
}

#[derive(Debug)]
pub struct Connection {
    pub index: usize,
    pub upstream: Direction,
    pub downstream: Direction,
}

impl Connection {
    pub fn require_server_close(&self) {
        assert!(self.downstream.peer_closed);
        assert!(server_close(&self.downstream.server_bytes).unwrap());
    }
}

fn server_close(bytes: &[u8]) -> io::Result<bool> {
    use tokio_tungstenite::tungstenite::protocol::frame::{
        FrameSocket,
        coding::{Control, OpCode},
    };
    if !bytes.starts_with(b"HTTP/1.1 101 ") {
        return Err(io::Error::other("missing original WebSocket upgrade"));
    }
    let body = bytes
        .windows(4)
        .position(|value| value == b"\r\n\r\n")
        .filter(|offset| *offset < SLAB_BYTES)
        .ok_or_else(|| io::Error::other("missing bounded upgrade header"))?
        + 4;
    let mut frames = FrameSocket::new(std::io::Cursor::new(&bytes[body..]));
    let mut closed = false;
    let mut consumed = 0;
    while let Some(frame) = frames
        .read(Some(MAX_DOWNSTREAM_PROOF_BYTES))
        .map_err(io::Error::other)?
    {
        consumed += frame.len();
        if frame.header().mask.is_some() || closed {
            return Err(io::Error::other("invalid server frame after upgrade/close"));
        }
        if frame.header().opcode == OpCode::Control(Control::Close) {
            if !frame.header().is_final || frame.payload().len() > 125 {
                return Err(io::Error::other("invalid server Close frame"));
            }
            closed = true;
        }
    }
    if consumed != bytes.len() - body {
        return Err(io::Error::other("incomplete server frame before EOF"));
    }
    Ok(closed)
}

pub struct DelayedProxy {
    pub address: SocketAddr,
    completed: mpsc::Receiver<io::Result<Connection>>,
    observed: BTreeMap<usize, Connection>,
    task: Option<JoinHandle<io::Result<()>>>,
}

impl DelayedProxy {
    pub async fn start(upstream: SocketAddr, rtt: Duration, connections: usize) -> Self {
        assert!(upstream.ip().is_loopback());
        assert!((1..=6).contains(&connections));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (completed, receiver) = mpsc::channel(connections);
        let task = tokio::spawn(async move {
            let mut owned = JoinSet::new();
            for index in 0..connections {
                let (client, _) = listener.accept().await?;
                let server = TcpStream::connect(upstream).await?;
                client.set_nodelay(true)?;
                server.set_nodelay(true)?;
                let completed = completed.clone();
                owned.spawn(async move {
                    let (client_read, client_write) = client.into_split();
                    let (server_read, server_write) = server.into_split();
                    let result = tokio::try_join!(
                        forward(client_read, server_write, rtt / 2, false),
                        forward(server_read, client_write, rtt / 2, true),
                    )
                    .map(|(upstream, downstream)| Connection {
                        index,
                        upstream,
                        downstream,
                    });
                    let _ = completed.send(result).await;
                });
            }
            drop(completed);
            while let Some(result) = owned.join_next().await {
                result.map_err(io::Error::other)?;
            }
            Ok(())
        });
        Self {
            address,
            completed: receiver,
            observed: BTreeMap::new(),
            task: Some(task),
        }
    }

    pub async fn connection(&mut self, index: usize) -> Connection {
        loop {
            if let Some(result) = self.observed.remove(&index) {
                return result;
            }
            let result = self.completed.recv().await.unwrap().unwrap();
            assert!(self.observed.insert(result.index, result).is_none());
        }
    }

    pub async fn finish(&mut self) {
        self.task.take().unwrap().await.unwrap().unwrap();
    }
}

impl Drop for DelayedProxy {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

async fn forward(
    mut source: impl AsyncRead + Unpin,
    mut destination: impl AsyncWrite + Unpin,
    one_way: Duration,
    source_is_server: bool,
) -> io::Result<Direction> {
    let mut slab = [0_u8; SLAB_BYTES];
    let mut proof = Direction::default();
    let mut destination_open = true;
    loop {
        let count = match source.read(&mut slab).await {
            Ok(count) => count,
            Err(error) if !source_is_server && error.kind() == io::ErrorKind::ConnectionReset => 0,
            Err(error) => return Err(error),
        };
        if count == 0 {
            proof.peer_closed = true;
            if destination_open {
                let _ = destination.shutdown().await;
            }
            return Ok(proof);
        }
        proof.bytes += count;
        proof.slabs += 1;
        if source_is_server {
            if proof.server_bytes.len() + count > MAX_DOWNSTREAM_PROOF_BYTES {
                return Err(io::Error::other(
                    "server-frame proof exceeds fixed byte limit",
                ));
            }
            proof.server_bytes.extend_from_slice(&slab[..count]);
        }
        if proof.slabs > MAX_SLABS_PER_DIRECTION {
            return Err(io::Error::other("proxy exceeded its bounded slab budget"));
        }
        if destination_open {
            tokio::time::sleep(one_way).await;
            match destination.write_all(&slab[..count]).await {
                Ok(()) => proof.completed_write_bytes += count,
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::BrokenPipe
                            | io::ErrorKind::ConnectionReset
                            | io::ErrorKind::ConnectionAborted
                    ) =>
                {
                    destination_open = false;
                }
                Err(error) => return Err(error),
            }
        }
    }
}

#[tokio::test]
async fn delayed_proxy_preserves_bytes_and_half_close_in_both_directions() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let server = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut proxy =
            DelayedProxy::start(server.local_addr().unwrap(), Duration::from_millis(50), 1).await;
        let payload = vec![37_u8; SLAB_BYTES + 7];
        let expected = payload.clone();
        let echo = tokio::spawn(async move {
            let (mut socket, _) = server.accept().await.unwrap();
            let mut body = Vec::new();
            socket.read_to_end(&mut body).await.unwrap();
            assert_eq!(body, expected);
            socket.write_all(&body).await.unwrap();
            socket.shutdown().await.unwrap();
        });
        let started = std::time::Instant::now();
        let mut client = TcpStream::connect(proxy.address).await.unwrap();
        client.write_all(&payload).await.unwrap();
        client.shutdown().await.unwrap();
        let mut echoed = Vec::new();
        client.read_to_end(&mut echoed).await.unwrap();
        assert_eq!(echoed, payload);
        let proof = proxy.connection(0).await;
        for direction in [&proof.upstream, &proof.downstream] {
            assert_eq!(direction.bytes, payload.len());
            assert_eq!(direction.completed_write_bytes, payload.len());
            assert!(direction.slabs >= 2);
            assert!(direction.peer_closed);
        }
        let minimum_delay =
            Duration::from_millis(25) * (proof.upstream.slabs + proof.downstream.slabs) as u32;
        assert!(started.elapsed() >= minimum_delay);
        echo.await.unwrap();
        proxy.finish().await;
    })
    .await
    .expect("bounded delayed proxy did not drain");
}

#[tokio::test]
async fn delayed_proxy_drains_original_source_after_destination_disconnect() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let (mut producer, source) = tokio::io::duplex(32);
        let (destination, disconnected) = tokio::io::duplex(32);
        drop(disconnected);
        let work = tokio::spawn(forward(source, destination, Duration::from_millis(1), true));
        producer.write_all(&[19; 256]).await.unwrap();
        producer.shutdown().await.unwrap();
        let proof = work.await.unwrap().unwrap();
        assert_eq!(proof.bytes, 256);
        assert_eq!(proof.completed_write_bytes, 0);
        assert!(proof.peer_closed);
    })
    .await
    .expect("proxy abandoned original source after downstream disconnect");
}

#[test]
fn close_proof_decodes_real_frames_and_rejects_eof_or_payload_lookalikes() {
    use tokio_tungstenite::tungstenite::protocol::frame::{
        Frame,
        coding::{Data, OpCode},
    };
    let mut bytes = b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\n\r\n".to_vec();
    assert!(!server_close(&bytes).unwrap());
    Frame::message(vec![0x88, 0x00], OpCode::Data(Data::Binary), true)
        .format(&mut bytes)
        .unwrap();
    assert!(!server_close(&bytes).unwrap());
    let mut truncated = bytes.clone();
    truncated.push(0x88);
    assert!(!server_close(&truncated).unwrap_or(false));
    Frame::close(None).format(&mut bytes).unwrap();
    assert!(server_close(&bytes).unwrap());
    Frame::message(vec![1], OpCode::Data(Data::Binary), true)
        .format(&mut bytes)
        .unwrap();
    assert!(server_close(&bytes).is_err());
}
