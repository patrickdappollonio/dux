//! The built web UI (`web/dist`) embedded into the binary by rust-embed and
//! served with SPA fallback. Built by build.rs.

use axum::http::{HeaderMap, HeaderValue, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "web/dist"]
struct WebAssets;

/// Serve an embedded asset by request path, falling back to `index.html` for
/// unknown paths (client-side routing). `/ws` and `/healthz` are matched before
/// this fallback, so they never reach here.
///
/// Special cases for PWA support:
/// - `.webmanifest` is served as `application/manifest+json` (mime-guess already
///   maps it correctly, so the generic path below handles it).
/// - `sw.js` (the service worker) is served with `Cache-Control: no-cache` so the
///   browser revalidates it on every load and picks up SW updates promptly.
/// - `offline.html` is a real embedded asset, so it is served here directly and
///   never shadowed by the SPA `index.html` fallback below.
pub async fn static_handler(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    if let Some(content) = WebAssets::get(path) {
        let mime = content.metadata.mimetype();
        let mut headers = HeaderMap::new();
        if let Ok(value) = HeaderValue::from_str(mime) {
            headers.insert(header::CONTENT_TYPE, value);
        }
        // The service worker must not be cached, or SW updates would lag behind
        // deploys. Everything else uses the browser's default caching.
        if path == "sw.js" {
            headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
        }
        return (headers, content.data.into_owned()).into_response();
    }
    match WebAssets::get("index.html") {
        Some(content) => (
            [(header::CONTENT_TYPE, "text/html".to_string())],
            content.data.into_owned(),
        )
            .into_response(),
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}
