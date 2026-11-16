//! Helpers shared by extractor tests.

/// Serves one HTTP request on a local port with the given status line and
/// JSON body, returning the server's base URL.
pub(crate) async fn serve_once(status: &'static str, body: &'static str) -> String {
    serve_in_order(vec![(status, body)]).await
}

/// Serves one HTTP request per `(status line, JSON body)` pair, in order, on a
/// local port, returning the server's base URL. Each response closes its
/// connection. Once the pairs run out the port closes, so a further request
/// fails.
pub(crate) async fn serve_in_order(responses: Vec<(&'static str, &'static str)>) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        for (status, body) in responses {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let mut buf = [0u8; 8192];
            let _ = socket.read(&mut buf).await;
            let response = format!(
                "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len(),
            );
            let _ = socket.write_all(response.as_bytes()).await;
            let _ = socket.shutdown().await;
        }
    });
    format!("http://{addr}/")
}
