//! A person at a browser, without the browser: a cookie jar, an `Origin` on every
//! mutation the way a browser sends one, and responses kept whole (status, every
//! header, body) so a journey can look at `Set-Cookie` attributes as well as
//! JSON.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use reqwest::cookie::{CookieStore as _, Jar};
use reqwest::{Certificate, Method, Url};

/// One HTTP response, read in full.
#[derive(Debug, Clone)]
pub struct Response {
    pub status: u16,
    /// Every header, names lowercased, in the order they arrived.
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Response {
    /// The body as JSON, panicking with the body when it is not.
    pub fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.body).unwrap_or_else(|err| {
            panic!(
                "expected JSON from a {} response ({err}): {}",
                self.status, self.body
            )
        })
    }

    /// The body as JSON, or `Null` when it is not JSON.
    pub fn json_or_null(&self) -> serde_json::Value {
        serde_json::from_str(&self.body).unwrap_or(serde_json::Value::Null)
    }

    /// The first value of a header.
    pub fn header(&self, name: &str) -> Option<&str> {
        let name = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, v)| v.as_str())
    }

    /// Every `Set-Cookie` header.
    pub fn set_cookies(&self) -> Vec<&str> {
        self.headers
            .iter()
            .filter(|(n, _)| n == "set-cookie")
            .map(|(_, v)| v.as_str())
            .collect()
    }

    /// The `error` field of a JSON error body, if there is one.
    pub fn error_code(&self) -> Option<String> {
        self.json_or_null()["error"].as_str().map(str::to_string)
    }

    /// A one-line description for assertion messages.
    pub fn describe(&self) -> String {
        format!(
            "{} {}",
            self.status,
            self.body.chars().take(300).collect::<String>()
        )
    }
}

/// The attributes of one `Set-Cookie` value, lowercased, without the name=value
/// pair: `["httponly", "samesite=strict", "path=/", "secure"]`.
pub fn cookie_attributes(set_cookie: &str) -> Vec<String> {
    set_cookie
        .split(';')
        .skip(1)
        .map(|part| part.trim().to_ascii_lowercase())
        .filter(|part| !part.is_empty())
        .collect()
}

/// Parse curl's `-i` output (headers, a blank line, the body) into a
/// [`Response`]. Interim `100 Continue` blocks are skipped.
pub fn parse_raw_response(raw: &str) -> Response {
    let mut rest = raw;
    loop {
        let (head, body) = rest
            .split_once("\r\n\r\n")
            .or_else(|| rest.split_once("\n\n"))
            .unwrap_or((rest, ""));
        let mut lines = head.lines();
        let status_line = lines.next().unwrap_or_default();
        let status: u16 = status_line
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(|| panic!("no HTTP status line in curl output: {raw:?}"));
        if status == 100 {
            rest = body;
            continue;
        }
        let headers = lines
            .filter_map(|line| line.split_once(':'))
            .map(|(n, v)| (n.trim().to_ascii_lowercase(), v.trim().to_string()))
            .collect();
        return Response {
            status,
            headers,
            body: body.to_string(),
        };
    }
}

/// A person on the host, reaching dux through one published port.
#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    jar: Arc<Jar>,
    base: Url,
    origin: Option<String>,
    headers: Vec<(String, String)>,
    root: Option<Vec<u8>>,
    resolve: Option<(String, SocketAddr)>,
}

impl Client {
    /// A client with an empty cookie jar for `base` (`http://127.0.0.1:port`).
    pub fn new(base: &str) -> Client {
        Self::build(base, None, None)
    }

    /// A client for an HTTPS `base` that trusts `root_pem` (a sidecar's own
    /// certificate authority) and nothing else extra.
    pub fn with_root(base: &str, root_pem: &[u8]) -> Client {
        Self::build(base, Some(root_pem), None)
    }

    /// A client for an HTTPS `base` naming a host (a tailnet name) that it
    /// reaches at `address` instead of through DNS, trusting `root_pem`: what a
    /// browser at that name sends (its TLS name, `Host` and `Origin`), delivered
    /// to a published port on this machine.
    pub fn with_root_at(base: &str, root_pem: &[u8], address: SocketAddr) -> Client {
        let host = Url::parse(base)
            .expect("a base URL")
            .host_str()
            .expect("a host in the base URL")
            .to_string();
        Self::build(base, Some(root_pem), Some((host, address)))
    }

    fn build(base: &str, root_pem: Option<&[u8]>, resolve: Option<(String, SocketAddr)>) -> Client {
        let jar = Arc::new(Jar::default());
        let mut builder = reqwest::Client::builder()
            .cookie_provider(Arc::clone(&jar))
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(30));
        if let Some(pem) = root_pem {
            builder = builder.add_root_certificate(
                Certificate::from_pem(pem).expect("the sidecar's root certificate parses"),
            );
        }
        if let Some((host, address)) = &resolve {
            builder = builder.resolve(host, *address);
        }
        let base = Url::parse(base).expect("a base URL");
        let origin = base.origin().ascii_serialization();
        Client {
            http: builder.build().expect("build the HTTP client"),
            jar,
            base,
            origin: Some(origin),
            headers: Vec::new(),
            root: root_pem.map(<[u8]>::to_vec),
            resolve,
        }
    }

    /// Send this header on every request from now on (a forged forwarding
    /// header, say).
    pub fn with_header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }

    /// Stop sending `Origin` on mutations, like a script rather than a browser.
    pub fn without_origin(mut self) -> Self {
        self.origin = None;
        self
    }

    /// A second client sharing nothing with this one but the address (and the
    /// certificate authority it trusts, and the headers it sends).
    pub fn fresh(&self) -> Client {
        let mut other = Client::build(
            self.base.as_str(),
            self.root.as_deref(),
            self.resolve.clone(),
        );
        other.headers = self.headers.clone();
        other.origin = self.origin.clone();
        other
    }

    /// Wait until `/healthz` answers 200 through this client's address. Polled
    /// over raw TCP rather than reqwest, because a relay with nothing behind it
    /// yet accepts and then drops the connection, which is a "not yet" here and
    /// not a failure. (The Tailscale leg binds a moment after dux's first look
    /// at the CLI, so a tailnet relay needs this.)
    pub async fn wait_answering(&self) {
        let url = self.base.clone();
        let addr = format!(
            "{}:{}",
            url.host_str().expect("a host"),
            url.port_or_known_default().expect("a port")
        );
        crate::util::eventually(
            &format!("{url} to answer /healthz"),
            Duration::from_secs(30),
            || {
                let addr = addr.clone();
                async move {
                    use tokio::io::{AsyncReadExt, AsyncWriteExt};
                    let mut stream = tokio::net::TcpStream::connect(&addr).await.ok()?;
                    let request = format!(
                        "GET /healthz HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n"
                    );
                    stream.write_all(request.as_bytes()).await.ok()?;
                    let mut answer = Vec::new();
                    let _ = tokio::time::timeout(
                        Duration::from_secs(2),
                        stream.read_to_end(&mut answer),
                    )
                    .await;
                    String::from_utf8_lossy(&answer)
                        .starts_with("HTTP/1.1 200")
                        .then_some(())
                }
            },
        )
        .await;
    }

    pub fn base(&self) -> &Url {
        &self.base
    }

    /// The `Cookie` header this jar would send to dux, for a WebSocket upgrade.
    pub fn cookie_header(&self) -> Option<String> {
        self.jar
            .cookies(&self.base)
            .map(|v| v.to_str().expect("an ASCII cookie header").to_string())
    }

    /// Put a raw cookie (`name=value`) in this jar, the way a saved cookie
    /// survives in a browser after dux forgot it.
    pub fn set_cookie(&self, name_value: &str) {
        self.jar.add_cookie_str(name_value, &self.base);
    }

    /// The headers every request and upgrade from this client carries.
    pub fn extra_headers(&self) -> &[(String, String)] {
        &self.headers
    }

    /// The `Origin` this client sends, if it sends one.
    pub fn origin(&self) -> Option<&str> {
        self.origin.as_deref()
    }

    pub async fn get(&self, path: &str) -> Response {
        self.send(Method::GET, path, None).await
    }

    pub async fn post_json(&self, path: &str, body: &serde_json::Value) -> Response {
        self.send(Method::POST, path, Some(body)).await
    }

    pub async fn post_empty(&self, path: &str) -> Response {
        self.send(Method::POST, path, None).await
    }

    pub async fn patch_json(&self, path: &str, body: &serde_json::Value) -> Response {
        self.send(Method::PATCH, path, Some(body)).await
    }

    pub async fn put_json(&self, path: &str, body: &serde_json::Value) -> Response {
        self.send(Method::PUT, path, Some(body)).await
    }

    /// One request, with the jar, the extra headers, and an `Origin` on
    /// anything but a GET.
    pub async fn send(
        &self,
        method: Method,
        path: &str,
        body: Option<&serde_json::Value>,
    ) -> Response {
        let url = self.base.join(path).expect("a request path");
        let mut request = self.http.request(method.clone(), url);
        if method != Method::GET
            && let Some(origin) = &self.origin
        {
            request = request.header("Origin", origin);
        }
        for (name, value) in &self.headers {
            request = request.header(name, value);
        }
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request
            .send()
            .await
            .unwrap_or_else(|err| panic!("{method} {path} did not get a response: {err}"));
        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .map(|(n, v)| {
                (
                    n.as_str().to_ascii_lowercase(),
                    String::from_utf8_lossy(v.as_bytes()).into_owned(),
                )
            })
            .collect();
        let body = response.text().await.unwrap_or_default();
        Response {
            status,
            headers,
            body,
        }
    }

    /// `POST /api/v1/auth/login` with `password`.
    pub async fn login(&self, password: &str) -> Response {
        self.post_json(
            "/api/v1/auth/login",
            &serde_json::json!({ "password": password }),
        )
        .await
    }

    /// Log in and panic unless it worked.
    /// Sign in, waiting out any slow-down the way a person does: a 429 says
    /// how long to wait (`Retry-After`), and dux slows every address down after
    /// a failed attempt, this machine included.
    pub async fn login_ok(&self, password: &str) {
        let mut response = self.login(password).await;
        for _ in 0..5 {
            if response.status != 429 {
                break;
            }
            let wait: u64 = response
                .header("retry-after")
                .and_then(|v| v.parse().ok())
                .unwrap_or(1);
            tokio::time::sleep(std::time::Duration::from_secs(wait.clamp(1, 30))).await;
            response = self.login(password).await;
        }
        assert_eq!(
            response.status,
            204,
            "login should succeed: {}",
            response.describe()
        );
    }

    /// `POST /api/v1/auth/logout`.
    pub async fn logout(&self) -> Response {
        self.post_empty("/api/v1/auth/logout").await
    }

    /// `GET /api/v1/auth/status` as JSON.
    pub async fn auth_status(&self) -> serde_json::Value {
        let response = self.get("/api/v1/auth/status").await;
        assert_eq!(
            response.status,
            200,
            "auth status is public and always answers: {}",
            response.describe()
        );
        response.json()
    }
}
