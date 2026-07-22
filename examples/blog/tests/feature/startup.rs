use std::time::Duration;

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

#[tokio::test]
async fn service_binds_to_a_socket_and_serves_a_request() {
    let server = phoenix_blog_example::application()
        .expect("example routes should build")
        .spawn("127.0.0.1:0")
        .await
        .expect("server should bind");
    let address = server.local_addr();

    let mut stream = TcpStream::connect(address)
        .await
        .expect("client should connect");
    stream
        .write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .expect("request should be written");

    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .await
        .expect("response should be read");
    let response = String::from_utf8(response).expect("response should be UTF-8");

    assert!(response.starts_with("HTTP/1.1 200 OK"));
    assert!(response.contains("\"status\":\"healthy\""));
    assert!(response.contains("x-powered-by: Phoenix"));

    server.shutdown().await.expect("server should stop cleanly");
}

#[tokio::test]
async fn oversized_request_bodies_receive_413() {
    let server = phoenix_blog_example::application()
        .expect("example routes should build")
        .spawn("127.0.0.1:0")
        .await
        .expect("server should bind");
    let mut stream = TcpStream::connect(server.local_addr())
        .await
        .expect("client should connect");
    let body = "x".repeat(70 * 1024);
    let request = format!(
        "POST /register HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(request.as_bytes())
        .await
        .expect("request should be written");

    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .await
        .expect("response should be read");
    let response = String::from_utf8(response).expect("response should be UTF-8");
    assert!(response.starts_with("HTTP/1.1 413 Payload Too Large"));

    server.shutdown().await.expect("server should stop cleanly");
}

#[tokio::test]
async fn incomplete_headers_are_closed_after_the_configured_timeout() {
    let server = phoenix_blog_example::application()
        .expect("example routes should build")
        .header_read_timeout(Duration::from_millis(50))
        .spawn("127.0.0.1:0")
        .await
        .expect("server should bind");
    let mut stream = TcpStream::connect(server.local_addr())
        .await
        .expect("client should connect");
    stream
        .write_all(b"GET /health HTTP/1.1\r\nHost: localhost")
        .await
        .expect("partial request should be written");

    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(1), stream.read_to_end(&mut response))
        .await
        .expect("server should not leave a slow header connection open")
        .expect("connection close should be readable");

    server.shutdown().await.expect("server should stop cleanly");
}

#[tokio::test]
async fn incomplete_bodies_receive_408_after_the_configured_timeout() {
    let server = phoenix_blog_example::application()
        .expect("example routes should build")
        .body_read_timeout(Duration::from_millis(50))
        .spawn("127.0.0.1:0")
        .await
        .expect("server should bind");
    let mut stream = TcpStream::connect(server.local_addr())
        .await
        .expect("client should connect");
    stream
        .write_all(
            b"POST /register HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: 32\r\nConnection: close\r\n\r\n{",
        )
        .await
        .expect("partial body should be written");

    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(1), stream.read_to_end(&mut response))
        .await
        .expect("server should enforce the body timeout")
        .expect("timeout response should be readable");
    let response = String::from_utf8(response).expect("response should be UTF-8");
    assert!(response.starts_with("HTTP/1.1 408 Request Timeout"));

    server.shutdown().await.expect("server should stop cleanly");
}
