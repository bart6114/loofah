use super::*;

#[tokio::test]
async fn reads_a_fragmented_request_instead_of_assuming_one_tcp_packet() {
    let (mut reader, mut writer) = tokio::io::duplex(1);
    let write = tokio::spawn(async move {
        writer
            .write_all(b"GET /callback?state=proof&code=grant HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();
    });
    assert_eq!(
        request_target(&mut reader).await.unwrap(),
        "/callback?state=proof&code=grant"
    );
    write.await.unwrap();
}

#[tokio::test]
async fn malformed_truncated_and_oversized_requests_are_rejected() {
    for bytes in [
        b"POST /callback HTTP/1.1\r\n\r\n".to_vec(),
        b"GET https://attacker.invalid/ HTTP/1.1\r\n\r\n".to_vec(),
        b"GET //attacker.invalid/ HTTP/1.1\r\n\r\n".to_vec(),
        b"GET /callback HTTP/2\r\n\r\n".to_vec(),
        b"GET /callback HTTP/1.1 extra\r\n\r\n".to_vec(),
        b"GET /callback HTTP/1.1\r\n".to_vec(),
        b"\xff\r\n\r\n".to_vec(),
        vec![b'x'; 8193],
    ] {
        assert!(request_target(&mut &bytes[..]).await.is_err());
    }
}

#[tokio::test]
async fn unrelated_local_requests_cannot_consume_the_login_and_valid_state_is_required() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        receive_callback(
            &listener,
            &format!("http://{address}/random-callback"),
            "proof",
        )
        .await
    });
    for target in [
        "/favicon.ico",
        "/random-callback?state=wrong&code=stolen",
        "/random-callback?state=proof&state=wrong&code=stolen",
        "/random-callback?state=proof&code=one&code=two",
        "/random-callback?state=proof&code=",
    ] {
        let mut socket = tokio::net::TcpStream::connect(address).await.unwrap();
        socket
            .write_all(format!("GET {target} HTTP/1.1\r\nHost: localhost\r\n\r\n").as_bytes())
            .await
            .unwrap();
        let mut response = String::new();
        socket.read_to_string(&mut response).await.unwrap();
        assert!(response.starts_with("HTTP/1.1 400"));
    }
    let mut socket = tokio::net::TcpStream::connect(address).await.unwrap();
    socket
        .write_all(
            b"GET /random-callback?state=proof&code=grant HTTP/1.1\r\nHost: localhost\r\n\r\n",
        )
        .await
        .unwrap();
    let mut response = String::new();
    socket.read_to_string(&mut response).await.unwrap();
    assert!(response.starts_with("HTTP/1.1 200"));
    assert!(response.contains("Cache-Control: no-store"));
    assert_eq!(task.await.unwrap().unwrap(), "grant");
}
