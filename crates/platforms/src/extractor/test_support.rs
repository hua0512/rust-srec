//! Helpers shared by extractor tests.

use std::sync::{Arc, Mutex};

/// The request lines and headers a local server received, one entry per request.
pub(crate) type ReceivedRequests = Arc<Mutex<Vec<String>>>;

/// Serves one HTTP request on a local port with the given status line and
/// JSON body, returning the server's base URL.
pub(crate) async fn serve_once(status: &'static str, body: &'static str) -> String {
    serve_in_order(vec![(status, body)]).await.0
}

/// Serves one HTTP request per `(status line, JSON body)` pair, in order, on a
/// local port, returning the server's base URL and the requests it receives.
/// Each response closes its connection. Once the pairs run out the port
/// closes, so a further request fails.
pub(crate) async fn serve_in_order(
    responses: Vec<(&'static str, &'static str)>,
) -> (String, ReceivedRequests) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let received = ReceivedRequests::default();
    let log = Arc::clone(&received);
    tokio::spawn(async move {
        for (status, body) in responses {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let mut request = Vec::new();
            let mut buf = [0u8; 8192];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                match socket.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => request.extend_from_slice(&buf[..n]),
                }
            }
            log.lock()
                .unwrap()
                .push(String::from_utf8_lossy(&request).into_owned());
            let response = format!(
                "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len(),
            );
            let _ = socket.write_all(response.as_bytes()).await;
            let _ = socket.shutdown().await;
        }
    });
    (format!("http://{addr}/"), received)
}
