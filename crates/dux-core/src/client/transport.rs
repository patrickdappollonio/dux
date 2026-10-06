//! How a request reaches a dux. Both transports carry the same HTTP requests
//! to the same routes: [`UnixTransport`] over the control socket of the dux on
//! this machine, [`HttpTransport`] over a remote's web listeners.
//!
//! The Unix transport is a small HTTP/1.1 client written here: one request
//! per connection (`Connection: close`), a body by `Content-Length`, by
//! chunks, or up to the close. ureq reaches a Unix socket only through its
//! semver-exempt transport API.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The longest any reply may take. A waiting operation read holds its reply
/// for at most 25 seconds, so this leaves room for that and a slow network.
pub const REPLY_TIMEOUT: Duration = Duration::from_secs(60);

/// How long reaching a remote may take before it counts as not answering.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// The largest reply body read. A workspace document is far smaller.
const BODY_LIMIT: u64 = 64 * 1024 * 1024;

/// The `User-Agent` both transports send, the device label dux shows for a
/// command-line client.
pub const USER_AGENT: &str = "dux CLI";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
    Put,
    Delete,
}

impl Method {
    pub fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
            Method::Put => "PUT",
            Method::Delete => "DELETE",
        }
    }
}

/// One request: a path with its query, a JSON body when there is one, and
/// the CLI sign-in token for a remote that asks for one.
#[derive(Clone, Debug)]
pub struct Request {
    pub method: Method,
    pub path: String,
    pub body: Option<Vec<u8>>,
    pub bearer: Option<String>,
}

impl Request {
    pub fn get(path: impl Into<String>) -> Self {
        Self {
            method: Method::Get,
            path: path.into(),
            body: None,
            bearer: None,
        }
    }
}

/// A reply: its status and its whole body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
}

/// Why a request got no reply.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransportError {
    /// Nothing could be reached at the address.
    Unreachable(String),
    /// Reached, but the connection ended before a whole reply arrived.
    Dropped(String),
}

impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransportError::Unreachable(why) | TransportError::Dropped(why) => f.write_str(why),
        }
    }
}

/// The one interface the client reaches a dux through.
pub trait Transport {
    fn send(&self, request: &Request) -> Result<Response, TransportError>;
}

/// HTTP/1.1 over the control socket of the dux on this machine.
#[derive(Clone, Debug)]
pub struct UnixTransport {
    path: PathBuf,
}

impl UnixTransport {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Whether something accepts a connection at the socket now.
    pub fn answers(&self) -> bool {
        UnixStream::connect(&self.path).is_ok()
    }
}

impl Transport for UnixTransport {
    fn send(&self, request: &Request) -> Result<Response, TransportError> {
        let mut stream = UnixStream::connect(&self.path).map_err(|error| {
            TransportError::Unreachable(format!("{}: {error}", self.path.display()))
        })?;
        let dropped = |error: std::io::Error| TransportError::Dropped(error.to_string());
        stream
            .set_read_timeout(Some(REPLY_TIMEOUT))
            .map_err(dropped)?;
        stream
            .set_write_timeout(Some(REPLY_TIMEOUT))
            .map_err(dropped)?;
        stream.write_all(&request_bytes(request)).map_err(dropped)?;
        stream.flush().map_err(dropped)?;
        read_response(BufReader::new(stream))
    }
}

/// `request` as HTTP/1.1 bytes, closing the connection after the reply.
fn request_bytes(request: &Request) -> Vec<u8> {
    let mut head = format!(
        "{} {} HTTP/1.1\r\nHost: dux\r\nUser-Agent: {USER_AGENT}\r\nAccept: application/json\r\nConnection: close\r\n",
        request.method.as_str(),
        request.path
    );
    if let Some(token) = &request.bearer {
        head.push_str(&format!("Authorization: Bearer {token}\r\n"));
    }
    let body = request.body.as_deref().unwrap_or_default();
    if request.body.is_some() {
        head.push_str("Content-Type: application/json\r\n");
    }
    head.push_str(&format!("Content-Length: {}\r\n\r\n", body.len()));
    let mut bytes = head.into_bytes();
    bytes.extend_from_slice(body);
    bytes
}

/// How a reply's body is delimited.
enum Framing {
    Length(u64),
    Chunked,
    UntilClose,
}

/// Read one HTTP/1.1 reply from `reader`. Anything short of a whole reply is
/// [`TransportError::Dropped`].
fn read_response(mut reader: impl BufRead) -> Result<Response, TransportError> {
    let dropped = |why: &str| TransportError::Dropped(why.to_string());
    let status_line =
        read_line(&mut reader)?.ok_or_else(|| dropped("the connection closed before a reply"))?;
    let status = status_line
        .split_whitespace()
        .nth(1)
        .filter(|_| status_line.starts_with("HTTP/1."))
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(|| dropped("the reply did not start with an HTTP status line"))?;
    let mut framing = Framing::UntilClose;
    loop {
        let line = read_line(&mut reader)?
            .ok_or_else(|| dropped("the connection closed inside the reply's headers"))?;
        if line.is_empty() {
            break;
        }
        let Some((name, value)) = line.split_once(':') else {
            return Err(dropped("the reply carried a malformed header"));
        };
        let (name, value) = (name.trim(), value.trim());
        if name.eq_ignore_ascii_case("transfer-encoding") && value.eq_ignore_ascii_case("chunked") {
            framing = Framing::Chunked;
        } else if name.eq_ignore_ascii_case("content-length")
            && !matches!(framing, Framing::Chunked)
        {
            let length = value
                .parse::<u64>()
                .map_err(|_| dropped("the reply's Content-Length is not a number"))?;
            framing = Framing::Length(length);
        }
    }
    let body = match framing {
        Framing::Length(length) => {
            if length > BODY_LIMIT {
                return Err(dropped("the reply was larger than the client reads"));
            }
            let mut body = Vec::with_capacity(length as usize);
            (&mut reader)
                .take(length)
                .read_to_end(&mut body)
                .map_err(|e| TransportError::Dropped(e.to_string()))?;
            if body.len() as u64 != length {
                return Err(dropped("the connection closed inside the reply's body"));
            }
            body
        }
        Framing::Chunked => read_chunks(&mut reader)?,
        Framing::UntilClose => {
            let mut body = Vec::new();
            (&mut reader)
                .take(BODY_LIMIT + 1)
                .read_to_end(&mut body)
                .map_err(|e| TransportError::Dropped(e.to_string()))?;
            if body.len() as u64 > BODY_LIMIT {
                return Err(dropped("the reply was larger than the client reads"));
            }
            body
        }
    };
    Ok(Response { status, body })
}

fn read_chunks(reader: &mut impl BufRead) -> Result<Vec<u8>, TransportError> {
    let dropped = |why: &str| TransportError::Dropped(why.to_string());
    let mut body = Vec::new();
    loop {
        let size_line = read_line(reader)?
            .ok_or_else(|| dropped("the connection closed inside a chunked reply"))?;
        let size_text = size_line.split(';').next().unwrap_or_default().trim();
        let size = u64::from_str_radix(size_text, 16)
            .map_err(|_| dropped("the reply carried a malformed chunk size"))?;
        if size == 0 {
            // Trailers, if any, end at an empty line.
            while let Some(line) = read_line(reader)? {
                if line.is_empty() {
                    break;
                }
            }
            return Ok(body);
        }
        if body.len() as u64 + size > BODY_LIMIT {
            return Err(dropped("the reply was larger than the client reads"));
        }
        let start = body.len();
        reader
            .take(size)
            .read_to_end(&mut body)
            .map_err(|e| TransportError::Dropped(e.to_string()))?;
        if (body.len() - start) as u64 != size {
            return Err(dropped("the connection closed inside a chunk"));
        }
        match read_line(reader)? {
            Some(line) if line.is_empty() => {}
            _ => return Err(dropped("a chunk did not end where its size said")),
        }
    }
}

/// One line without its `\r\n`, or `None` at the end of the stream.
fn read_line(reader: &mut impl BufRead) -> Result<Option<String>, TransportError> {
    let mut line = Vec::new();
    let read = reader
        .take(64 * 1024)
        .read_until(b'\n', &mut line)
        .map_err(|e| TransportError::Dropped(e.to_string()))?;
    if read == 0 {
        return Ok(None);
    }
    if !line.ends_with(b"\n") {
        return Err(TransportError::Dropped(
            "the connection closed inside a line of the reply".to_string(),
        ));
    }
    line.pop();
    if line.ends_with(b"\r") {
        line.pop();
    }
    Ok(Some(String::from_utf8_lossy(&line).into_owned()))
}

/// HTTP or HTTPS to a remote dux's web listeners, through ureq.
pub struct HttpTransport {
    base: String,
    agent: ureq::Agent,
}

impl HttpTransport {
    /// `base` is the remote's URL with no trailing slash; request paths are
    /// appended to it.
    pub fn new(base: &str) -> Self {
        let mut builder = ureq::Agent::config_builder()
            .timeout_connect(Some(CONNECT_TIMEOUT))
            .timeout_global(Some(REPLY_TIMEOUT))
            .http_status_as_error(false)
            .user_agent(USER_AGENT);
        // A proxy from the environment never reaches this machine's own
        // loopback, where a remote may be a forwarded port.
        if is_loopback_url(base) {
            builder = builder.proxy(None);
        }
        Self {
            base: base.trim_end_matches('/').to_string(),
            agent: builder.build().into(),
        }
    }
}

fn is_loopback_url(base: &str) -> bool {
    url::Url::parse(base)
        .ok()
        .and_then(|url| match url.host()? {
            url::Host::Domain(name) => Some(name.eq_ignore_ascii_case("localhost")),
            url::Host::Ipv4(ip) => Some(ip.is_loopback()),
            url::Host::Ipv6(ip) => Some(ip.is_loopback()),
        })
        .unwrap_or(false)
}

impl Transport for HttpTransport {
    fn send(&self, request: &Request) -> Result<Response, TransportError> {
        let mut builder = ureq::http::Request::builder()
            .method(request.method.as_str())
            .uri(format!("{}{}", self.base, request.path))
            .header("Accept", "application/json");
        if let Some(token) = &request.bearer {
            builder = builder.header("Authorization", format!("Bearer {token}"));
        }
        if request.body.is_some() {
            builder = builder.header("Content-Type", "application/json");
        }
        let http_request = builder
            .body(request.body.clone().unwrap_or_default())
            .map_err(|error| TransportError::Unreachable(error.to_string()))?;
        let mut response = self.agent.run(http_request).map_err(|error| match error {
            ureq::Error::ConnectionFailed
            | ureq::Error::HostNotFound
            | ureq::Error::BadUri(_)
            | ureq::Error::Tls(_) => TransportError::Unreachable(error.to_string()),
            ureq::Error::Io(ref io) if io.kind() == std::io::ErrorKind::ConnectionRefused => {
                TransportError::Unreachable(error.to_string())
            }
            other => TransportError::Dropped(other.to_string()),
        })?;
        let status = response.status().as_u16();
        let body = response
            .body_mut()
            .with_config()
            .limit(BODY_LIMIT)
            .read_to_vec()
            .map_err(|error| TransportError::Dropped(error.to_string()))?;
        Ok(Response { status, body })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::test_server::{FakeDux, Reply, private_dir};

    #[test]
    fn a_request_reaches_the_socket_with_its_body_and_token() {
        let dir = private_dir();
        let socket = dir.path().join("dux.sock");
        let fake = FakeDux::unix(&socket, |_| Reply::json(201, r#"{"id":"a1"}"#));
        let reply = UnixTransport::new(&socket)
            .send(&Request {
                method: Method::Post,
                path: "/api/v1/projects?operation=1".to_string(),
                body: Some(br#"{"path":"/src"}"#.to_vec()),
                bearer: Some("t0k".to_string()),
            })
            .expect("a reply");
        assert_eq!(reply.status, 201);
        assert_eq!(reply.body, br#"{"id":"a1"}"#);
        let seen = fake.seen();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].method, "POST");
        assert_eq!(seen[0].path, "/api/v1/projects?operation=1");
        assert_eq!(seen[0].body, r#"{"path":"/src"}"#);
        assert_eq!(seen[0].header("authorization"), Some("Bearer t0k"));
        assert_eq!(seen[0].header("user-agent"), Some("dux CLI"));
    }

    #[test]
    fn a_chunked_reply_and_one_read_to_the_close_are_read_whole() {
        let dir = private_dir();
        let socket = dir.path().join("dux.sock");
        let _fake = FakeDux::unix(&socket, |seen| {
            if seen.path == "/chunked" {
                Reply::Raw(
                    "HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n5\r\nhello\r\n7;x=1\r\n, world\r\n0\r\n\r\n"
                        .to_string(),
                )
            } else {
                Reply::Raw("HTTP/1.1 404 Not Found\r\n\r\nunknown session".to_string())
            }
        });
        let transport = UnixTransport::new(&socket);
        let chunked = transport.send(&Request::get("/chunked")).unwrap();
        assert_eq!(
            (chunked.status, chunked.body.as_slice()),
            (200, &b"hello, world"[..])
        );
        let to_close = transport.send(&Request::get("/other")).unwrap();
        assert_eq!(
            (to_close.status, to_close.body.as_slice()),
            (404, &b"unknown session"[..])
        );
    }

    #[test]
    fn a_connection_that_closes_without_a_reply_is_dropped_and_a_missing_socket_unreachable() {
        let dir = private_dir();
        let socket = dir.path().join("dux.sock");
        let _fake = FakeDux::unix(&socket, |_| Reply::Close);
        let transport = UnixTransport::new(&socket);
        assert!(matches!(
            transport.send(&Request::get("/api/v1/build")),
            Err(TransportError::Dropped(_))
        ));
        let missing = UnixTransport::new(dir.path().join("absent.sock"));
        assert!(!missing.answers());
        assert!(matches!(
            missing.send(&Request::get("/api/v1/build")),
            Err(TransportError::Unreachable(_))
        ));
    }

    #[test]
    fn the_http_transport_carries_the_same_request() {
        let (fake, addr) = FakeDux::tcp(|_| Reply::json(200, r#"{"ok":true}"#));
        let reply = HttpTransport::new(&format!("http://{addr}/"))
            .send(&Request {
                method: Method::Delete,
                path: "/api/v1/sessions/a1?operation=1".to_string(),
                body: None,
                bearer: Some("t0k".to_string()),
            })
            .unwrap();
        assert_eq!(reply.status, 200);
        assert_eq!(reply.body, br#"{"ok":true}"#);
        let seen = fake.seen();
        assert_eq!(seen[0].method, "DELETE");
        assert_eq!(seen[0].path, "/api/v1/sessions/a1?operation=1");
        assert_eq!(seen[0].header("authorization"), Some("Bearer t0k"));
    }
}
