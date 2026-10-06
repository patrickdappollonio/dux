//! Serving the API on the control socket: the listener that admits only this
//! process's own user, the routes the socket serves, and the serve task every
//! servicing core runs over its clone of the one bound listener.
//!
//! The socket is bound once per process by the lock holder
//! ([`dux_core::control_socket`]); this module only serves it. A request over
//! it reaches the same router as a browser's, carrying [`Arrival::Unix`], which
//! the Host guard, the Origin check and the auth layer read as this machine's
//! owner.

use std::future::Future;
use std::io;

use axum::Router;
use axum::extract::Request;
use axum::extract::connect_info::Connected;
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::serve::IncomingStream;

use crate::auth::Arrival;

/// Patterns under `/api/v1/` the socket refuses, `*` standing for one segment:
/// the browser's file editor and file drop, which write files its checks guard.
const NOT_ON_SOCKET: &[&str] = &[
    "file-drop",
    "file",
    "sessions/*/files",
    "terminals/*/files",
    "projects/*/terminals/*/files",
];

/// Whether the control socket serves `path`.
pub(crate) fn served_on_control_socket(path: &str) -> bool {
    let Some(rest) = path.strip_prefix("/api/v1/") else {
        return false;
    };
    let segments: Vec<&str> = rest.split('/').collect();
    !NOT_ON_SOCKET.iter().any(|pattern| {
        let pattern: Vec<&str> = pattern.split('/').collect();
        segments.len() >= pattern.len()
            && pattern
                .iter()
                .zip(&segments)
                .all(|(want, got)| *want == "*" || want == got)
    })
}

/// Must be the outermost layer of every router: a request over the control
/// socket for a route it does not serve is answered 404 before anything else.
pub(crate) async fn control_socket_routes(request: Request, next: Next) -> Response {
    if crate::auth::provenance::over_control_socket(&request)
        && !served_on_control_socket(request.uri().path())
    {
        return not_on_control_socket();
    }
    next.run(request).await
}

fn not_on_control_socket() -> Response {
    (
        StatusCode::NOT_FOUND,
        "this route is not served on the control socket",
    )
        .into_response()
}

/// What answers a request no route matched, on every listener.
pub(crate) async fn fallback(request: Request) -> Response {
    if request.uri().path().starts_with("/api/") {
        return (
            StatusCode::NOT_FOUND,
            axum::Json(serde_json::json!({ "error": "not_found" })),
        )
            .into_response();
    }
    if crate::auth::provenance::over_control_socket(&request) {
        return not_on_control_socket();
    }
    let (parts, _) = request.into_parts();
    crate::web_assets::static_handler(parts.uri, parts.headers).await
}

/// The control socket's listener: accepts only connections whose peer runs as
/// `uid`, closing any other before a byte of its request is read.
pub struct ControlListener {
    inner: tokio::net::UnixListener,
    uid: u32,
}

impl ControlListener {
    /// Serve `listener` (a clone of the bound control socket) for this
    /// process's own user. Needs an entered tokio runtime.
    pub fn new(listener: std::os::unix::net::UnixListener) -> io::Result<Self> {
        Self::for_uid(listener, dux_core::control_socket::current_uid())
    }

    /// The same, admitting `uid`. Tests use it to stand in for another user.
    pub(crate) fn for_uid(
        listener: std::os::unix::net::UnixListener,
        uid: u32,
    ) -> io::Result<Self> {
        listener.set_nonblocking(true)?;
        Ok(Self {
            inner: tokio::net::UnixListener::from_std(listener)?,
            uid,
        })
    }
}

impl axum::serve::Listener for ControlListener {
    type Io = tokio::net::UnixStream;
    type Addr = Arrival;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            // axum's own accept for a Unix listener, its error handling
            // included.
            let (stream, _) = axum::serve::Listener::accept(&mut self.inner).await;
            match stream.peer_cred() {
                Ok(cred) if cred.uid() == self.uid => {
                    return (stream, Arrival::Unix { uid: cred.uid() });
                }
                Ok(cred) => dux_core::logger::warn(&format!(
                    "[server] closed a control socket connection from user {}: only this \
                     dux's own user ({}) may use it",
                    cred.uid(),
                    self.uid
                )),
                Err(err) => dux_core::logger::warn(&format!(
                    "[server] closed a control socket connection whose user could not be \
                     read: {err}"
                )),
            }
        }
    }

    fn local_addr(&self) -> io::Result<Self::Addr> {
        Ok(Arrival::Unix { uid: self.uid })
    }
}

impl Connected<IncomingStream<'_, ControlListener>> for Arrival {
    fn connect_info(stream: IncomingStream<'_, ControlListener>) -> Self {
        *stream.remote_addr()
    }
}

/// A handle on the control socket `engine`'s lock holds, for a core to serve,
/// or `None` when dux runs without one. A clone that cannot be made is logged
/// and leaves this core without the socket.
pub fn listener_of(engine: &dux_core::engine::Engine) -> Option<std::os::unix::net::UnixListener> {
    let socket = engine.single_instance_lock.control_socket()?;
    match socket.listener() {
        Ok(listener) => Some(listener),
        Err(err) => {
            dux_core::logger::error(&format!(
                "[server] could not hand the control socket {} to the next core: {err}. \
                 Command-line clients cannot reach this dux until it restarts.",
                socket.path().display()
            ));
            None
        }
    }
}

/// Serve `app` on the control socket, on the current runtime, until `stop`
/// resolves, then finish the requests already accepted. A failure ends only this task.
pub(crate) fn spawn_control_leg(
    app: Router,
    listener: std::os::unix::net::UnixListener,
    stop: impl Future<Output = ()> + Send + 'static,
) -> io::Result<tokio::task::JoinHandle<()>> {
    let listener = ControlListener::new(listener)?;
    Ok(tokio::spawn(async move {
        let served = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<Arrival>(),
        )
        .with_graceful_shutdown(stop)
        .await;
        if let Err(err) = served {
            dux_core::logger::error(&format!(
                "[server] the control socket stopped serving: {err}. Command-line clients \
                 cannot reach this dux until it restarts."
            ));
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use tower::ServiceExt;

    async fn status_over_socket(app: &Router, path: &str) -> StatusCode {
        let mut request = Request::builder().uri(path).body(Body::empty()).unwrap();
        request
            .extensions_mut()
            .insert(axum::extract::ConnectInfo(Arrival::Unix { uid: 1000 }));
        app.clone().oneshot(request).await.unwrap().status()
    }

    /// The socket serves the REST API and nothing a browser drives: no PTY or
    /// event socket, no static UI, no file editor, no file drop.
    #[tokio::test]
    async fn the_socket_serves_the_rest_routes_and_not_the_pty_sockets() {
        let (_tmp, app) = crate::test_support::router_no_auth();
        assert_eq!(
            status_over_socket(&app, "/api/v1/workspace").await,
            StatusCode::OK
        );
        for absent in [
            "/ws/sessions/s1/pty",
            "/ws/sessions/s1/tabs/t1/pty",
            "/ws/terminals/t1/pty",
            "/ws/events",
            "/",
            "/index.html",
            "/healthz",
            "/api/v1/sessions/s1/files/read",
            "/api/v1/terminals/t1/files/tree",
            "/api/v1/projects/p1/terminals/t1/files/raw",
            "/api/v1/file/read",
            "/api/v1/file-drop",
            "/api/v1/no-such-route",
        ] {
            assert_eq!(
                status_over_socket(&app, absent).await,
                StatusCode::NOT_FOUND,
                "{absent}"
            );
        }
    }

    /// Bind a socket in a fresh folder and serve `app` on it for `uid`.
    fn serve_for(uid: u32) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dux.sock");
        let std_listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        let listener = ControlListener::for_uid(std_listener, uid).unwrap();
        let app = Router::new().route("/api/v1/build", axum::routing::get(|| async { "ok" }));
        tokio::spawn(async move {
            let _ = axum::serve(
                listener,
                app.into_make_service_with_connect_info::<Arrival>(),
            )
            .await;
        });
        (dir, path)
    }

    /// Send one request and read whatever comes back until the server closes.
    async fn exchange(path: &std::path::Path) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut stream = tokio::net::UnixStream::connect(path).await.unwrap();
        let _ = stream
            .write_all(b"GET /api/v1/build HTTP/1.1\r\nHost: dux\r\nConnection: close\r\n\r\n")
            .await;
        let mut answer = Vec::new();
        let _ = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            stream.read_to_end(&mut answer),
        )
        .await
        .expect("the server answers or closes");
        String::from_utf8_lossy(&answer).into_owned()
    }

    /// Another user's connection is closed with nothing read or answered; this
    /// user's is served.
    #[tokio::test]
    async fn a_connection_from_another_user_is_closed_before_its_request_is_read() {
        let me = dux_core::control_socket::current_uid();
        let (_dir, mine) = serve_for(me);
        assert!(exchange(&mine).await.starts_with("HTTP/1.1 200"));

        let (_dir, theirs) = serve_for(me.wrapping_add(1));
        assert_eq!(exchange(&theirs).await, "");
    }

    /// The patterns match whole segments, never a prefix of one.
    #[test]
    fn the_route_set_matches_whole_segments() {
        for (path, served) in [
            ("/api/v1/filesystem", true),
            ("/api/v1/sessions/s1/terminals/t1", true),
            ("/api/v1/sessions/s1/files", false),
            ("/api/v1", false),
        ] {
            assert_eq!(served_on_control_socket(path), served, "{path}");
        }
    }
}
