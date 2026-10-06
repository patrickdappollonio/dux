//! A stand-in dux for the client's tests: it accepts connections on a Unix
//! socket or an ephemeral loopback port, records each request and answers it
//! with whatever the test's handler says, closing the connection after.

use std::io::{BufRead, BufReader, Read, Write};
use std::sync::{Arc, Mutex};

/// A request as the stand-in received it.
#[derive(Clone, Debug)]
pub struct Seen {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Seen {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

pub enum Reply {
    /// These exact bytes, then close.
    Raw(String),
    /// Close without answering.
    Close,
    /// These pieces, one every `gap`, then close.
    Slow(Vec<String>, std::time::Duration),
    /// Say nothing for this long, then close.
    Hang(std::time::Duration),
}

impl Reply {
    pub fn json(status: u16, body: &str) -> Self {
        Reply::Raw(format!(
            "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
            body.len()
        ))
    }
}

type Handler = Arc<dyn Fn(&Seen) -> Reply + Send + Sync>;

pub struct FakeDux {
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl FakeDux {
    pub fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }

    pub fn unix(
        path: &std::path::Path,
        handler: impl Fn(&Seen) -> Reply + Send + Sync + 'static,
    ) -> Self {
        let listener = std::os::unix::net::UnixListener::bind(path).expect("bind the test socket");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let handler: Handler = Arc::new(handler);
        let record = seen.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { return };
                serve(stream, &record, &handler);
            }
        });
        Self { seen }
    }

    pub fn tcp(
        handler: impl Fn(&Seen) -> Reply + Send + Sync + 'static,
    ) -> (Self, std::net::SocketAddr) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind a test port");
        let addr = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let handler: Handler = Arc::new(handler);
        let record = seen.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { return };
                serve(stream, &record, &handler);
            }
        });
        (Self { seen }, addr)
    }
}

fn serve<S: Read + Write>(stream: S, record: &Mutex<Vec<Seen>>, handler: &Handler) {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    if reader.read_line(&mut line).unwrap_or(0) == 0 {
        return;
    }
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let path = parts.next().unwrap_or_default().to_string();
    let mut headers = Vec::new();
    let mut length = 0usize;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).unwrap_or(0) == 0 {
            return;
        }
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            let (name, value) = (name.trim().to_string(), value.trim().to_string());
            if name.eq_ignore_ascii_case("content-length") {
                length = value.parse().unwrap_or(0);
            }
            headers.push((name, value));
        }
    }
    let mut body = vec![0u8; length];
    let _ = reader.read_exact(&mut body);
    let seen = Seen {
        method,
        path,
        headers,
        body: String::from_utf8_lossy(&body).into_owned(),
    };
    record.lock().unwrap().push(seen.clone());
    let mut stream = reader.into_inner();
    match handler(&seen) {
        Reply::Raw(bytes) => {
            let _ = stream.write_all(bytes.as_bytes());
            let _ = stream.flush();
        }
        Reply::Close => {}
        Reply::Slow(pieces, gap) => {
            for piece in pieces {
                if stream.write_all(piece.as_bytes()).is_err() || stream.flush().is_err() {
                    return;
                }
                std::thread::sleep(gap);
            }
        }
        Reply::Hang(how_long) => std::thread::sleep(how_long),
    }
}

/// A temp folder only this user can enter, as dux's config folder is.
pub fn private_dir() -> tempfile::TempDir {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    dir
}
