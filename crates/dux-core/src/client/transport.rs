//! How a request reaches a dux. Both transports carry the same HTTP requests
//! to the same routes: [`UnixTransport`] over the control socket of the dux on
//! this machine, [`HttpTransport`] over a remote's web listeners.
//!
//! The Unix transport is a small HTTP/1.1 client written here: one request
//! per connection (`Connection: close`), a body by `Content-Length`, by
//! chunks, or up to the close. ureq reaches a Unix socket only through its
//! semver-exempt transport API.

use std::io::{BufRead, BufReader, Read, Write};
use std::ops::ControlFlow;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The longest any reply may take: room for a waiting operation read's
/// [`super::wait::READ_WAIT`] and a slow network.
pub const REPLY_TIMEOUT: Duration = Duration::from_secs(60);

/// How long reaching a remote may take before it counts as not answering.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// How long the discovery probe waits for the control socket to accept.
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// The largest reply body read. A workspace document is far smaller.
const BODY_LIMIT: u64 = 64 * 1024 * 1024;

/// The most header (or trailer) lines a reply may carry, and their bytes
/// together.
const MAX_HEAD_LINES: usize = 100;
const MAX_HEAD_BYTES: usize = 64 * 1024;

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
    /// The longest the whole exchange may take; at most [`REPLY_TIMEOUT`].
    pub timeout: Option<Duration>,
}

impl Request {
    pub fn get(path: impl Into<String>) -> Self {
        Self {
            method: Method::Get,
            path: path.into(),
            body: None,
            bearer: None,
            timeout: None,
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
    /// Not sent, for the reason given.
    Refused(String),
}

impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransportError::Unreachable(why)
            | TransportError::Dropped(why)
            | TransportError::Refused(why) => f.write_str(why),
        }
    }
}

/// The one interface the client reaches a dux through.
pub trait Transport {
    fn send(&self, request: &Request) -> Result<Response, TransportError>;

    /// Send `request` and hand its reply's body to `on_body` as it arrives, with
    /// the reply's status, until the reply ends or `on_body` breaks.
    ///
    /// `on_body` first gets an empty piece as soon as the status is known. Once
    /// connected nothing has a deadline: a quiet stream is not a dead one.
    ///
    /// # Errors
    ///
    /// [`TransportError::Dropped`] when the reply ends before its framing says it is whole.
    fn stream(
        &self,
        request: &Request,
        on_body: &mut dyn FnMut(u16, &[u8]) -> ControlFlow<()>,
    ) -> Result<(), TransportError>;
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

    /// Whether something accepts a connection at the socket now, asked for
    /// at most [`PROBE_TIMEOUT`].
    pub fn answers(&self) -> bool {
        connect_unix(&self.path, PROBE_TIMEOUT).is_ok()
    }
}

/// Connect to the socket at `path` within `timeout`. A socket whose queue is
/// full refuses at once rather than leaving the caller waiting.
fn connect_unix(path: &Path, timeout: Duration) -> std::io::Result<UnixStream> {
    let socket = socket2::Socket::new(socket2::Domain::UNIX, socket2::Type::STREAM, None)?;
    socket.connect_timeout(&socket2::SockAddr::unix(path)?, timeout)?;
    socket.set_nonblocking(false)?;
    Ok(UnixStream::from(std::os::fd::OwnedFd::from(socket)))
}

/// The stream of one exchange, which gives up once its deadline passes,
/// however slowly the bytes before it arrived.
struct Deadline {
    stream: UnixStream,
    until: Instant,
}

impl Deadline {
    fn left(&self) -> std::io::Result<Duration> {
        let left = self.until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "the reply took longer than the client waits",
            ));
        }
        Ok(left)
    }
}

impl Read for Deadline {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let left = self.left()?;
        unless_shut_down(self.stream.set_read_timeout(Some(left)))?;
        self.stream.read(buf)
    }
}

impl Write for Deadline {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let left = self.left()?;
        unless_shut_down(self.stream.set_write_timeout(Some(left)))?;
        self.stream.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.stream.flush()
    }
}

/// macOS refuses to set a timeout (`EINVAL`) once the peer has shut the connection down; that
/// is no failure, since the read or write after it cannot wait anyway.
fn unless_shut_down(set: std::io::Result<()>) -> std::io::Result<()> {
    match set {
        Err(error) if error.raw_os_error() == Some(libc::EINVAL) => Ok(()),
        other => other,
    }
}

/// How long `request` may take as a whole.
fn time_allowed(request: &Request) -> Duration {
    request.timeout.unwrap_or(REPLY_TIMEOUT).min(REPLY_TIMEOUT)
}

impl Transport for UnixTransport {
    fn send(&self, request: &Request) -> Result<Response, TransportError> {
        let allowed = time_allowed(request);
        let until = Instant::now() + allowed;
        let stream = connect_unix(&self.path, allowed).map_err(|error| {
            TransportError::Unreachable(format!("{}: {error}", self.path.display()))
        })?;
        let dropped = |error: std::io::Error| TransportError::Dropped(error.to_string());
        let mut stream = Deadline { stream, until };
        stream.write_all(&request_bytes(request)).map_err(dropped)?;
        stream.flush().map_err(dropped)?;
        read_response(BufReader::new(stream))
    }

    fn stream(
        &self,
        request: &Request,
        on_body: &mut dyn FnMut(u16, &[u8]) -> ControlFlow<()>,
    ) -> Result<(), TransportError> {
        let stream = connect_unix(&self.path, CONNECT_TIMEOUT).map_err(|error| {
            TransportError::Unreachable(format!("{}: {error}", self.path.display()))
        })?;
        let dropped = |error: std::io::Error| TransportError::Dropped(error.to_string());
        let mut stream = Deadline {
            stream,
            until: Instant::now() + NO_DEADLINE,
        };
        stream.write_all(&request_bytes(request)).map_err(dropped)?;
        stream.flush().map_err(dropped)?;
        let mut reader = BufReader::new(stream);
        let (status, framing) = read_head(&mut reader)?;
        let mut piece = |bytes: &[u8]| on_body(status, bytes);
        if piece(&[]).is_break() {
            return Ok(());
        }
        match framing {
            Framing::Chunked => each_chunk_piece(&mut reader, &mut piece),
            Framing::Length(length) => {
                let mut left = length;
                let mut buffer = [0u8; STREAM_PIECE];
                while left > 0 {
                    let want = left.min(STREAM_PIECE as u64) as usize;
                    let read = reader
                        .read(&mut buffer[..want])
                        .map_err(|e| TransportError::Dropped(e.to_string()))?;
                    if read == 0 {
                        return Err(TransportError::Dropped(
                            "the connection closed inside the reply's body".to_string(),
                        ));
                    }
                    left -= read as u64;
                    if piece(&buffer[..read]).is_break() {
                        return Ok(());
                    }
                }
                Ok(())
            }
            Framing::UntilClose => {
                let mut buffer = [0u8; STREAM_PIECE];
                loop {
                    let read = reader
                        .read(&mut buffer)
                        .map_err(|e| TransportError::Dropped(e.to_string()))?;
                    if read == 0 || piece(&buffer[..read]).is_break() {
                        return Ok(());
                    }
                }
            }
        }
    }
}

/// How long a stream's body may take as a whole: longer than any run of dux.
const NO_DEADLINE: Duration = Duration::from_secs(60 * 60 * 24 * 365);

/// The most a streamed body hands over at once.
const STREAM_PIECE: usize = 8 * 1024;

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

/// Read a reply's status line and headers: its status and how its body is
/// delimited. Anything short of that is [`TransportError::Dropped`].
fn read_head(reader: &mut impl BufRead) -> Result<(u16, Framing), TransportError> {
    let dropped = |why: &str| TransportError::Dropped(why.to_string());
    let status_line =
        read_line(reader)?.ok_or_else(|| dropped("the connection closed before a reply"))?;
    let status = status_line
        .split_whitespace()
        .nth(1)
        .filter(|_| status_line.starts_with("HTTP/1."))
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(|| dropped("the reply did not start with an HTTP status line"))?;
    let mut framing = Framing::UntilClose;
    let mut head = HeadBudget::default();
    loop {
        let line = read_line(reader)?
            .ok_or_else(|| dropped("the connection closed inside the reply's headers"))?;
        if line.is_empty() {
            break;
        }
        head.take(&line)?;
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
    Ok((status, framing))
}

/// Read one HTTP/1.1 reply from `reader`. Anything short of a whole reply is
/// [`TransportError::Dropped`].
fn read_response(mut reader: impl BufRead) -> Result<Response, TransportError> {
    let dropped = |why: &str| TransportError::Dropped(why.to_string());
    let (status, framing) = read_head(&mut reader)?;
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

/// What is left of the header (or trailer) lines and bytes a reply may use.
#[derive(Default)]
struct HeadBudget {
    lines: usize,
    bytes: usize,
}

impl HeadBudget {
    fn take(&mut self, line: &str) -> Result<(), TransportError> {
        self.lines += 1;
        self.bytes += line.len();
        if self.lines > MAX_HEAD_LINES || self.bytes > MAX_HEAD_BYTES {
            return Err(TransportError::Dropped(
                "the reply carried more header lines than the client reads".to_string(),
            ));
        }
        Ok(())
    }
}

fn read_chunks(reader: &mut impl BufRead) -> Result<Vec<u8>, TransportError> {
    let mut body = Vec::new();
    let mut too_large = false;
    each_chunk_piece(reader, &mut |piece| {
        if (body.len() as u64).saturating_add(piece.len() as u64) > BODY_LIMIT {
            too_large = true;
            return ControlFlow::Break(());
        }
        body.extend_from_slice(piece);
        ControlFlow::Continue(())
    })?;
    if too_large {
        return Err(TransportError::Dropped(
            "the reply was larger than the client reads".to_string(),
        ));
    }
    Ok(body)
}

/// Read a chunked body, handing it over in pieces of at most [`STREAM_PIECE`], until the
/// last chunk or until `on_piece` breaks.
fn each_chunk_piece(
    reader: &mut impl BufRead,
    on_piece: &mut dyn FnMut(&[u8]) -> ControlFlow<()>,
) -> Result<(), TransportError> {
    let dropped = |why: &str| TransportError::Dropped(why.to_string());
    loop {
        let size_line = read_line(reader)?
            .ok_or_else(|| dropped("the connection closed inside a chunked reply"))?;
        let size_text = size_line.split(';').next().unwrap_or_default().trim();
        let size = u64::from_str_radix(size_text, 16)
            .map_err(|_| dropped("the reply carried a malformed chunk size"))?;
        if size == 0 {
            // Trailers, if any, end at an empty line.
            let mut trailers = HeadBudget::default();
            while let Some(line) = read_line(reader)? {
                if line.is_empty() {
                    break;
                }
                trailers.take(&line)?;
            }
            return Ok(());
        }
        let mut left = size;
        let mut buffer = [0u8; STREAM_PIECE];
        while left > 0 {
            let want = left.min(STREAM_PIECE as u64) as usize;
            let read = reader
                .read(&mut buffer[..want])
                .map_err(|e| TransportError::Dropped(e.to_string()))?;
            if read == 0 {
                return Err(dropped("the connection closed inside a chunk"));
            }
            left -= read as u64;
            if on_piece(&buffer[..read]).is_break() {
                return Ok(());
            }
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
    url: Option<url::Url>,
    insecure: bool,
    agent: ureq::Agent,
}

impl HttpTransport {
    /// `base` is the remote's URL, to which request paths are appended; `insecure` says the
    /// remote was added with `--insecure`.
    pub fn new(base: &str, insecure: bool) -> Self {
        let url = url::Url::parse(base).ok();
        let mut builder = ureq::Agent::config_builder()
            .timeout_connect(Some(CONNECT_TIMEOUT))
            .timeout_global(Some(REPLY_TIMEOUT))
            .http_status_as_error(false)
            .user_agent(USER_AGENT);
        if !url
            .as_ref()
            .is_some_and(|url| uses_environment_proxy(url, insecure))
        {
            builder = builder.proxy(None);
        }
        Self {
            base: base.trim_end_matches('/').to_string(),
            url,
            insecure,
            agent: builder.build().into(),
        }
    }

    /// Run `request` against `base` and answer once the reply has begun. `whole` bounds the
    /// whole exchange; with `None` only connecting is bounded.
    fn open(
        &self,
        request: &Request,
        base: &str,
        host: Option<&str>,
        whole: Option<Duration>,
    ) -> Result<ureq::http::Response<ureq::Body>, TransportError> {
        let mut builder = ureq::http::Request::builder()
            .method(request.method.as_str())
            .uri(format!("{base}{}", request.path))
            .header("Accept", "application/json");
        if let Some(host) = host {
            builder = builder.header("Host", host);
        }
        if let Some(token) = &request.bearer {
            builder = builder.header("Authorization", format!("Bearer {token}"));
        }
        if request.body.is_some() {
            builder = builder.header("Content-Type", "application/json");
        }
        let http_request = builder
            .body(request.body.clone().unwrap_or_default())
            .map_err(|error| TransportError::Unreachable(error.to_string()))?;
        let configured = self
            .agent
            .configure_request(http_request)
            .timeout_global(whole);
        let http_request = configured.build();
        self.agent.run(http_request).map_err(|error| match error {
            ureq::Error::ConnectionFailed
            | ureq::Error::HostNotFound
            | ureq::Error::BadUri(_)
            | ureq::Error::Tls(_) => TransportError::Unreachable(error.to_string()),
            ureq::Error::Io(ref io) if io.kind() == std::io::ErrorKind::ConnectionRefused => {
                TransportError::Unreachable(error.to_string())
            }
            other => TransportError::Dropped(other.to_string()),
        })
    }

    /// One attempt at `request`: its whole reply.
    fn attempt(
        &self,
        request: &Request,
        base: &str,
        host: Option<&str>,
    ) -> Result<Response, TransportError> {
        let mut response = self.open(request, base, host, Some(time_allowed(request)))?;
        let status = response.status().as_u16();
        let body = response
            .body_mut()
            .with_config()
            .limit(BODY_LIMIT)
            .read_to_vec()
            .map_err(|error| TransportError::Dropped(error.to_string()))?;
        Ok(Response { status, body })
    }

    /// One attempt at `request`, its body handed to `on_body` as it arrives.
    fn attempt_stream(
        &self,
        request: &Request,
        base: &str,
        host: Option<&str>,
        on_body: &mut dyn FnMut(u16, &[u8]) -> ControlFlow<()>,
    ) -> Result<(), TransportError> {
        let mut response = self.open(request, base, host, None)?;
        let status = response.status().as_u16();
        if on_body(status, &[]).is_break() {
            return Ok(());
        }
        let mut reader = response.body_mut().as_reader();
        let mut buffer = [0u8; STREAM_PIECE];
        loop {
            let read = reader
                .read(&mut buffer)
                .map_err(|error| TransportError::Dropped(error.to_string()))?;
            if read == 0 || on_body(status, &buffer[..read]).is_break() {
                return Ok(());
            }
        }
    }

    /// Run `attempt` against the remote as written, or at each address its name must resolve
    /// to, moving to the next address only when one is unreachable.
    fn through_destinations<T>(
        &self,
        mut attempt: impl FnMut(&str, Option<&str>) -> Result<T, TransportError>,
    ) -> Result<T, TransportError> {
        let Some(url) = &self.url else {
            return attempt(&self.base, None);
        };
        let resolve = |host: &str, port: u16| {
            std::net::ToSocketAddrs::to_socket_addrs(&(host, port)).map(Iterator::collect)
        };
        let Some(addrs) = destinations(url, self.insecure, resolve)? else {
            return attempt(&self.base, None);
        };
        let host = match url.port() {
            Some(port) => format!("{}:{port}", url.host_str().unwrap_or_default()),
            None => url.host_str().unwrap_or_default().to_string(),
        };
        let mut last = TransportError::Unreachable(format!("{host} has no address to reach"));
        for addr in addrs {
            match attempt(&format!("http://{addr}"), Some(&host)) {
                Err(TransportError::Unreachable(why)) => last = TransportError::Unreachable(why),
                other => return other,
            }
        }
        Err(last)
    }
}

/// Whether a request to `url` may go through an environment proxy. Plain HTTP without
/// `--insecure` never may: it is allowed only because it stays on this machine or the tailnet.
pub(crate) fn uses_environment_proxy(url: &url::Url, insecure: bool) -> bool {
    let loopback = match url.host() {
        Some(url::Host::Domain(name)) => name.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    };
    !loopback && (url.scheme() == "https" || insecure)
}

/// Where a plain-HTTP request to `url` must connect, or `None` to reach it as written. A
/// Tailscale name must resolve to Tailscale addresses only: that is why plain HTTP is allowed.
pub(crate) fn destinations(
    url: &url::Url,
    insecure: bool,
    resolve: impl Fn(&str, u16) -> std::io::Result<Vec<std::net::SocketAddr>>,
) -> Result<Option<Vec<std::net::SocketAddr>>, TransportError> {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
    if url.scheme() != "http" || insecure {
        return Ok(None);
    }
    let port = url.port_or_known_default().unwrap_or(80);
    let Some(url::Host::Domain(name)) = url.host() else {
        return Ok(None);
    };
    let name = name.to_ascii_lowercase();
    if name == "localhost" {
        return Ok(Some(vec![
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port),
            SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), port),
        ]));
    }
    if !name.ends_with(".ts.net") {
        return Ok(None);
    }
    let addrs = resolve(&name, port).map_err(|error| {
        TransportError::Unreachable(format!("could not look up {name}: {error}"))
    })?;
    if addrs.is_empty() {
        return Err(TransportError::Unreachable(format!(
            "{name} did not resolve to any address"
        )));
    }
    let tailnet = |ip: IpAddr| match ip {
        IpAddr::V4(ip) => crate::tailscale::is_tailscale_cgnat(ip),
        IpAddr::V6(ip) => crate::tailscale::is_tailscale_ipv6(ip),
    };
    if let Some(outside) = addrs.iter().find(|addr| !tailnet(addr.ip())) {
        return Err(TransportError::Refused(format!(
            "{name} resolved to {}, which is not a Tailscale address, so dux will not send to \
             it over plain HTTP. Use https://, or remove the remote and add it again with \
             --insecure if you trust every network between you and it",
            outside.ip()
        )));
    }
    Ok(Some(addrs))
}

impl Transport for HttpTransport {
    fn send(&self, request: &Request) -> Result<Response, TransportError> {
        self.through_destinations(|base, host| self.attempt(request, base, host))
    }

    fn stream(
        &self,
        request: &Request,
        on_body: &mut dyn FnMut(u16, &[u8]) -> ControlFlow<()>,
    ) -> Result<(), TransportError> {
        self.through_destinations(|base, host| self.attempt_stream(request, base, host, on_body))
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
                timeout: None,
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
    fn a_reply_is_read_whole_and_only_within_the_client_limits() {
        let dir = private_dir();
        let socket = dir.path().join("dux.sock");
        let _fake = FakeDux::unix(&socket, |seen| {
            match seen.path.as_str() {
            "/chunked" => Reply::Raw(
                "HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n5\r\nhello\r\n7;x=1\r\n, world\r\n0\r\n\r\n"
                    .to_string(),
            ),
            "/overflowing-chunk" => Reply::Raw(
                "HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n1\r\nx\r\nffffffffffffffff\r\nabc"
                    .to_string(),
            ),
            "/oversized-chunk" => Reply::Raw(
                "HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n4000001\r\nabc".to_string(),
            ),
            "/many-headers" => Reply::Raw(format!(
                "HTTP/1.1 200 OK\r\n{}content-length: 0\r\n\r\n",
                "x-filler: 1\r\n".repeat(500)
            )),
            "/endless" => Reply::Slow(
                std::iter::once("HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n".to_string())
                    .chain(std::iter::repeat_n("6\r\nline\r\n\r\n".to_string(), 20))
                    .collect(),
                Duration::from_millis(700),
            ),
            "/no-body" => Reply::Raw(
                "HTTP/1.1 401 Unauthorized\r\ncontent-length: 0\r\n\r\n".to_string(),
            ),
            "/cut-short" => Reply::Raw(
                "HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n5\r\nhello\r\n".to_string(),
            ),
            "/trickle" => Reply::Slow(
                std::iter::once("HTTP/1.1 200 OK\r\n".to_string())
                    .chain(std::iter::repeat_n("x-slow: 1\r\n".to_string(), 40))
                    .collect(),
                Duration::from_millis(100),
            ),
            _ => Reply::Raw("HTTP/1.1 404 Not Found\r\n\r\nunknown session".to_string()),
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
        for path in ["/overflowing-chunk", "/oversized-chunk", "/many-headers"] {
            assert!(
                matches!(
                    transport.send(&Request::get(path)),
                    Err(TransportError::Dropped(_))
                ),
                "{path}"
            );
        }
        // A body is handed over as it arrives and ends with the reply, whatever
        // its framing, with the status it came with.
        let mut got = Vec::new();
        let ended = transport.stream(&Request::get("/chunked"), &mut |status, piece| {
            got.push((status, piece.to_vec()));
            std::ops::ControlFlow::Continue(())
        });
        assert_eq!(ended, Ok(()));
        assert!(got.iter().all(|(status, _)| *status == 200), "{got:?}");
        let joined: Vec<u8> = got.into_iter().flat_map(|(_, piece)| piece).collect();
        assert_eq!(joined, b"hello, world");
        let mut refused = Vec::new();
        let ended = transport.stream(&Request::get("/other"), &mut |status, piece| {
            refused.push((status, piece.to_vec()));
            std::ops::ControlFlow::Continue(())
        });
        assert_eq!(ended, Ok(()));
        assert_eq!(
            refused.last(),
            Some(&(404, b"unknown session".to_vec())),
            "the body, with the status it came with"
        );
        // The status reaches the caller even when the body has nothing in it:
        // a reply with no body is told apart from no reply.
        let mut statuses = Vec::new();
        let ended = transport.stream(&Request::get("/no-body"), &mut |status, piece| {
            assert!(piece.is_empty());
            statuses.push(status);
            std::ops::ControlFlow::Continue(())
        });
        assert_eq!((ended, statuses), (Ok(()), vec![401]));
        assert!(
            matches!(
                transport.stream(&Request::get("/cut-short"), &mut |_, _| {
                    std::ops::ControlFlow::Continue(())
                }),
                Err(TransportError::Dropped(_))
            ),
            "a stream that ends without its last chunk was cut off"
        );

        // A stream that never ends is read as it comes: its first line arrives
        // after the request's own time (300 ms) has passed, which a stream is
        // not held to, and it stops when the caller says so, long before the
        // server would have finished (fourteen seconds of lines).
        let started = std::time::Instant::now();
        let mut first = None;
        let stopped = transport.stream(
            &Request {
                timeout: Some(Duration::from_millis(300)),
                ..Request::get("/endless")
            },
            &mut |_, piece| {
                if piece.is_empty() {
                    return std::ops::ControlFlow::Continue(());
                }
                first = Some((started.elapsed(), piece.to_vec()));
                std::ops::ControlFlow::Break(())
            },
        );
        assert_eq!(stopped, Ok(()));
        let (at, piece) = first.expect("a piece arrived");
        assert_eq!(piece, b"line\r\n");
        assert!(at > Duration::from_millis(500), "{at:?}");
        assert!(at < Duration::from_secs(3), "{at:?}");

        let started = std::time::Instant::now();
        let trickled = transport.send(&Request {
            timeout: Some(Duration::from_secs(1)),
            ..Request::get("/trickle")
        });
        assert!(
            matches!(trickled, Err(TransportError::Dropped(_))),
            "{trickled:?}"
        );
        assert!(
            started.elapsed() < Duration::from_millis(2500),
            "one deadline for the whole reply, not one per read: {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn the_probe_gives_up_at_once_on_a_socket_that_never_accepts() {
        let dir = private_dir();
        let path = dir.path().join("dux.sock");
        let listener =
            socket2::Socket::new(socket2::Domain::UNIX, socket2::Type::STREAM, None).unwrap();
        listener
            .bind(&socket2::SockAddr::unix(&path).unwrap())
            .unwrap();
        listener.listen(0).unwrap();
        let (done, finished) = std::sync::mpsc::channel();
        let probe = path.clone();
        std::thread::spawn(move || {
            let transport = UnixTransport::new(probe);
            let mut held = Vec::new();
            // Connections pile up until the queue is full: one on Linux, more
            // on macOS, which sizes even a zero backlog's queue itself.
            for _ in 0..1024 {
                match std::os::unix::net::UnixStream::connect(transport.path()) {
                    Ok(stream) => held.push(stream),
                    // macOS refuses a connect to a full queue at once instead
                    // of waiting, so the queue is full here.
                    Err(_) => break,
                }
                if !transport.answers() {
                    let _ = done.send(false);
                    return;
                }
            }
            let _ = done.send(transport.answers());
        });
        let answered = finished
            .recv_timeout(Duration::from_secs(5))
            .expect("the probe never hangs on a full socket");
        assert!(!answered, "a socket that accepts nobody does not answer");
        drop(listener);
    }

    #[test]
    fn only_https_and_insecure_remotes_may_go_through_an_environment_proxy() {
        for (url, insecure, proxied) in [
            ("https://dux.example.com", false, true),
            ("http://192.168.1.20:3890", true, true),
            ("http://100.101.2.3:3890", false, false),
            ("http://box.tail1234.ts.net:3890", false, false),
            ("http://localhost:3890", false, false),
            ("http://127.0.0.1:3890", false, false),
            ("https://127.0.0.1:3890", false, false),
        ] {
            let url = url::Url::parse(url).unwrap();
            assert_eq!(uses_environment_proxy(&url, insecure), proxied, "{url}");
        }
    }

    #[test]
    fn localhost_is_loopback_and_a_tailnet_name_must_resolve_to_the_tailnet() {
        let never = |_: &str, _: u16| -> std::io::Result<Vec<std::net::SocketAddr>> {
            panic!("this destination is not looked up")
        };
        let resolves_to = |addrs: &'static [&'static str]| {
            move |_: &str, port: u16| -> std::io::Result<Vec<std::net::SocketAddr>> {
                Ok(addrs
                    .iter()
                    .map(|ip| std::net::SocketAddr::new(ip.parse().unwrap(), port))
                    .collect())
            }
        };
        let url = |text: &str| url::Url::parse(text).unwrap();

        assert_eq!(
            destinations(&url("http://localhost:3890"), false, never).unwrap(),
            Some(vec![
                "127.0.0.1:3890".parse().unwrap(),
                "[::1]:3890".parse().unwrap()
            ])
        );
        assert_eq!(
            destinations(
                &url("http://box.tail1234.ts.net:3890"),
                false,
                resolves_to(&["100.101.2.3", "fd7a:115c:a1e0::5"])
            )
            .unwrap(),
            Some(vec![
                "100.101.2.3:3890".parse().unwrap(),
                "[fd7a:115c:a1e0::5]:3890".parse().unwrap()
            ])
        );
        let refused = destinations(
            &url("http://box.tail1234.ts.net:3890"),
            false,
            resolves_to(&["100.101.2.3", "203.0.113.9"]),
        )
        .unwrap_err();
        let TransportError::Refused(why) = refused else {
            panic!("{refused:?}")
        };
        assert!(why.contains("--insecure"), "{why}");
        assert!(why.contains("203.0.113.9"), "{why}");
        assert_eq!(
            destinations(&url("http://box.tail1234.ts.net:3890"), true, never).unwrap(),
            None,
            "a remote added with --insecure is reached as written"
        );
        assert_eq!(
            destinations(&url("https://dux.example.com"), false, never).unwrap(),
            None
        );
        assert_eq!(
            destinations(&url("http://100.101.2.3:3890"), false, never).unwrap(),
            None
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
        let (fake, addr) = FakeDux::tcp(|seen| match seen.path.as_str() {
            "/endless" => Reply::Slow(
                std::iter::once(
                    "HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n".to_string(),
                )
                .chain(std::iter::repeat_n("6\r\nline\r\n\r\n".to_string(), 20))
                .collect(),
                Duration::from_millis(700),
            ),
            "/no-body" => {
                Reply::Raw("HTTP/1.1 401 Unauthorized\r\ncontent-length: 0\r\n\r\n".to_string())
            }
            _ => Reply::json(200, r#"{"ok":true}"#),
        });
        let reply = HttpTransport::new(&format!("http://{addr}/"), false)
            .send(&Request {
                method: Method::Delete,
                path: "/api/v1/sessions/a1?operation=1".to_string(),
                body: None,
                bearer: Some("t0k".to_string()),
                timeout: None,
            })
            .unwrap();
        assert_eq!(reply.status, 200);
        assert_eq!(reply.body, br#"{"ok":true}"#);
        let seen = fake.seen();
        assert_eq!(seen[0].method, "DELETE");
        assert_eq!(seen[0].path, "/api/v1/sessions/a1?operation=1");
        assert_eq!(seen[0].header("authorization"), Some("Bearer t0k"));

        let by_name = format!("http://localhost:{}", addr.port());
        let reply = HttpTransport::new(&by_name, false)
            .send(&Request::get("/api/v1/build"))
            .expect("localhost reaches this machine's loopback");
        assert_eq!(reply.status, 200);
        assert_eq!(
            fake.seen()[1].header("host"),
            Some(format!("localhost:{}", addr.port()).as_str()),
            "the request still names the host it was given"
        );

        let mut statuses = Vec::new();
        let ended = HttpTransport::new(&format!("http://{addr}"), false).stream(
            &Request::get("/no-body"),
            &mut |status, _| {
                statuses.push(status);
                std::ops::ControlFlow::Continue(())
            },
        );
        assert_eq!((ended, statuses), (Ok(()), vec![401]));

        // Its body is handed over as it arrives too, past the request's own time.
        let started = std::time::Instant::now();
        let mut first = None;
        let stopped = HttpTransport::new(&format!("http://{addr}"), false).stream(
            &Request {
                timeout: Some(Duration::from_millis(300)),
                ..Request::get("/endless")
            },
            &mut |status, piece| {
                if piece.is_empty() {
                    return std::ops::ControlFlow::Continue(());
                }
                first = Some((status, started.elapsed(), piece.to_vec()));
                std::ops::ControlFlow::Break(())
            },
        );
        assert_eq!(stopped, Ok(()));
        let (status, at, piece) = first.expect("a piece arrived");
        assert_eq!((status, piece.as_slice()), (200, &b"line\r\n"[..]));
        assert!(at > Duration::from_millis(500), "{at:?}");
        assert!(at < Duration::from_secs(3), "{at:?}");
    }
}
