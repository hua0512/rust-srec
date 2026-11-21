//! Helpers shared by extractor tests.

use std::sync::{Arc, Mutex};

/// The request lines and headers a local server received, one entry per request.
pub(crate) type ReceivedRequests = Arc<Mutex<Vec<String>>>;

/// One response of a local test server.
pub(crate) struct Reply {
    status: &'static str,
    headers: Vec<(&'static str, &'static str)>,
    body: &'static str,
}

impl Reply {
    /// A JSON body with the given status line.
    pub(crate) fn json(status: &'static str, body: &'static str) -> Self {
        Self {
            status,
            headers: vec![("content-type", "application/json")],
            body,
        }
    }

    /// A body sent as is, such as an HTML page or a JSONP script.
    pub(crate) fn raw(
        status: &'static str,
        content_type: &'static str,
        body: &'static str,
    ) -> Self {
        Self {
            status,
            headers: vec![("content-type", content_type)],
            body,
        }
    }

    /// Adds a response header, such as `location`.
    pub(crate) fn header(mut self, name: &'static str, value: &'static str) -> Self {
        self.headers.push((name, value));
        self
    }
}

impl From<(&'static str, &'static str)> for Reply {
    fn from((status, body): (&'static str, &'static str)) -> Self {
        Self::json(status, body)
    }
}

/// Serves one HTTP request on a local port with the given status line and
/// JSON body, returning the server's base URL.
pub(crate) async fn serve_once(status: &'static str, body: &'static str) -> String {
    serve_in_order(vec![(status, body)]).await.0
}

/// Serves one HTTP request per reply (a [`Reply`] or a `(status line, JSON
/// body)` pair), in order, on a local port, returning the server's base URL
/// and the requests it receives. Each response closes its connection. Once
/// the replies run out the port closes, so a further request fails.
pub(crate) async fn serve_in_order<R: Into<Reply>>(
    responses: Vec<R>,
) -> (String, ReceivedRequests) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let responses: Vec<Reply> = responses.into_iter().map(Into::into).collect();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let received = ReceivedRequests::default();
    let log = Arc::clone(&received);
    tokio::spawn(async move {
        for reply in responses {
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
            let headers: String = reply
                .headers
                .iter()
                .map(|(name, value)| format!("{name}: {value}\r\n"))
                .collect();
            let response = format!(
                "HTTP/1.1 {}\r\n{headers}content-length: {}\r\nconnection: close\r\n\r\n{}",
                reply.status,
                reply.body.len(),
                reply.body,
            );
            let _ = socket.write_all(response.as_bytes()).await;
            let _ = socket.shutdown().await;
        }
    });
    (format!("http://{addr}/"), received)
}
