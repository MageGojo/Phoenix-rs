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
