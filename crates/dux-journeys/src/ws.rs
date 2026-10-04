//! The two sockets a browser tab holds open, opened with a client's cookie:
//! `/ws/events` (the live updates every tab keeps) and a PTY socket (one
//! terminal's byte stream).

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::{Error as WsError, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use crate::client::Client;

type Stream = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Why an upgrade did not become a socket: the HTTP answer dux gave instead.
#[derive(Debug)]
pub struct Refused {
    pub status: u16,
    pub body: String,
}

/// How a socket ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ended {
    /// dux sent a close frame with this code.
    Closed(u16),
    /// The stream ended with no close frame.
    Dropped,
}

/// One open WebSocket.
pub struct Socket {
    stream: Stream,
    /// Binary frames read so far (terminal output).
    pub output: Vec<u8>,
}

/// Open `path` (`/ws/events`, `/ws/sessions/<id>/pty`) as `client`: its cookie,
/// its `Origin`, its extra headers.
pub async fn connect(client: &Client, path: &str) -> Result<Socket, Refused> {
    let mut url = client.base().join(path).expect("a socket path");
    let scheme = if url.scheme() == "https" { "wss" } else { "ws" };
    url.set_scheme(scheme).expect("a ws scheme");
    let mut request = url
        .as_str()
        .into_client_request()
        .expect("a socket request");
    let headers = request.headers_mut();
    if let Some(cookie) = client.cookie_header() {
        headers.insert("Cookie", HeaderValue::from_str(&cookie).expect("cookie"));
    }
    if let Some(origin) = client.origin() {
        headers.insert("Origin", HeaderValue::from_str(origin).expect("origin"));
    }
    for (name, value) in client.extra_headers() {
        headers.insert(
            tokio_tungstenite::tungstenite::http::HeaderName::from_bytes(name.as_bytes())
                .expect("header name"),
            HeaderValue::from_str(value).expect("header value"),
        );
    }
    match tokio_tungstenite::connect_async(request).await {
        Ok((stream, _)) => Ok(Socket {
            stream,
            output: Vec::new(),
        }),
        Err(WsError::Http(response)) => Err(Refused {
            status: response.status().as_u16(),
            body: response
                .body()
                .as_ref()
                .map(|b| String::from_utf8_lossy(b).into_owned())
                .unwrap_or_default(),
        }),
        Err(err) => panic!("the socket {path} failed below HTTP: {err}"),
    }
}

/// Open `path` and panic unless it upgraded.
pub async fn connect_ok(client: &Client, path: &str) -> Socket {
    match connect(client, path).await {
        Ok(socket) => socket,
        Err(refused) => panic!(
            "the socket {path} was refused: {} {}",
            refused.status, refused.body
        ),
    }
}

/// The close code dux sends a socket whose session is missing, ended or revoked.
pub const CLOSE_SIGNED_OUT: u16 = 4401;

/// The close code dux sends a socket from a blocked address.
pub const CLOSE_BLOCKED: u16 = 4403;

/// Open `path` as `client` and require what dux does to a socket it will not
/// serve: ACCEPT the upgrade (a refused upgrade reaches a browser as a bare
/// network drop, code 1006, which it cannot tell from an outage) and then close
/// it with `code`.
pub async fn assert_accepted_then_closed(client: &Client, path: &str, code: u16, what: &str) {
    let mut socket = match connect(client, path).await {
        Ok(socket) => socket,
        Err(refused) => panic!(
            "{what}: the upgrade of {path} must be accepted and then closed with {code}, \
             not refused with {} {}",
            refused.status, refused.body
        ),
    };
    let ended = socket.wait_ended(Duration::from_secs(10)).await;
    assert_eq!(
        ended,
        Some(Ended::Closed(code)),
        "{what}: {path} must close with {code}"
    );
}

impl Socket {
    /// Send a text frame.
    pub async fn send_text(&mut self, text: &str) {
        self.stream
            .send(Message::Text(text.into()))
            .await
            .expect("send a text frame");
    }

    /// Send bytes as a binary frame (keystrokes, on a PTY socket).
    pub async fn send_bytes(&mut self, bytes: &[u8]) {
        self.stream
            .send(Message::Binary(bytes.to_vec().into()))
            .await
            .expect("send a binary frame");
    }

    /// Claim the PTY the way a foregrounded tab does: a size frame.
    pub async fn claim(&mut self, rows: u16, cols: u16) {
        self.send_text(&format!(r#"{{"rows":{rows},"cols":{cols}}}"#))
            .await;
    }

    /// Read until a text frame whose JSON `event` is `event`, or `None` at the
    /// deadline.
    pub async fn next_event(&mut self, event: &str, within: Duration) -> Option<serde_json::Value> {
        let deadline = tokio::time::Instant::now() + within;
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(Duration::from_millis(250), self.stream.next()).await {
                Ok(Some(Ok(Message::Text(text)))) => {
                    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text)
                        && value["event"].as_str() == Some(event)
                    {
                        return Some(value);
                    }
                }
                Ok(Some(Ok(Message::Binary(bytes)))) => self.output.extend_from_slice(&bytes),
                Ok(Some(Ok(_))) | Err(_) => {}
                Ok(Some(Err(_))) | Ok(None) => return None,
            }
        }
        None
    }

    /// Read terminal output until it contains `needle`; true when it did.
    pub async fn read_until(&mut self, needle: &str, within: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + within;
        while tokio::time::Instant::now() < deadline {
            if String::from_utf8_lossy(&self.output).contains(needle) {
                return true;
            }
            match tokio::time::timeout(Duration::from_millis(250), self.stream.next()).await {
                Ok(Some(Ok(Message::Binary(bytes)))) => self.output.extend_from_slice(&bytes),
                Ok(Some(Ok(_))) | Err(_) => {}
                Ok(Some(Err(_))) | Ok(None) => break,
            }
        }
        String::from_utf8_lossy(&self.output).contains(needle)
    }

    /// The terminal output read so far, as text.
    pub fn output_text(&self) -> String {
        String::from_utf8_lossy(&self.output).into_owned()
    }

    /// Keep reading (and so answering dux's pings) for `period`, like an open tab
    /// in the background. Returns how the socket ended if it ended.
    pub async fn hold_open(&mut self, period: Duration) -> Option<Ended> {
        let deadline = tokio::time::Instant::now() + period;
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(Duration::from_millis(250), self.stream.next()).await {
                Ok(Some(Ok(Message::Close(frame)))) => return Some(ended_by(frame)),
                Ok(Some(Ok(Message::Binary(bytes)))) => self.output.extend_from_slice(&bytes),
                Ok(Some(Ok(_))) | Err(_) => {}
                Ok(Some(Err(_))) | Ok(None) => return Some(Ended::Dropped),
            }
        }
        None
    }

    /// Wait for the socket to end; `None` if it is still open at the deadline.
    pub async fn wait_ended(&mut self, within: Duration) -> Option<Ended> {
        self.hold_open(within).await
    }

    /// Close from this end, the way a tab closing does.
    pub async fn close(mut self) {
        let _ = self.stream.close(None).await;
        // Drain until the server's answering close, so the server sees a clean
        // end rather than a reset.
        let _ = tokio::time::timeout(Duration::from_secs(2), async {
            while let Some(Ok(_)) = self.stream.next().await {}
        })
        .await;
    }
}

fn ended_by(frame: Option<CloseFrame>) -> Ended {
    match frame {
        Some(frame) => Ended::Closed(u16::from(frame.code)),
        None => Ended::Dropped,
    }
}
