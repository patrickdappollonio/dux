//! The built web UI (`web/dist`) embedded into the binary by rust-embed and
//! served with SPA fallback. Built by build.rs.
//!
//! build.rs gzips the text assets IN PLACE, so the bytes rust-embed bakes in are
//! already compressed (shrinking the binary). The handler detects the gzip magic
//! bytes and serves them with `Content-Encoding: gzip` for clients that accept it
//! (every browser), inflating on the fly for the rare client that doesn't.

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
pub async fn static_handler(uri: Uri, headers: HeaderMap) -> Response {
    let accepts_gzip = accepts_gzip(&headers);
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    if let Some(content) = WebAssets::get(path) {
        let mime = content.metadata.mimetype().to_string();
        // The service worker must not be cached, or SW updates would lag behind
        // deploys. Everything else uses the browser's default caching.
        let cache_control = if path == "sw.js" {
            Some("no-cache")
        } else {
            None
        };
        return serve_asset(&mime, content.data.into_owned(), accepts_gzip, cache_control);
    }
    // The hashed bundle lives under `assets/`. A miss here means the browser is
    // requesting a chunk URL from a stale `index.html` (the binary was rebuilt
    // and restarted with a new content hash). Returning the SPA `index.html`
    // would hand back HTML for a `*.js` import(), the browser rejects HTML as a
    // module, and React.lazy unmounts the whole tree. A real 404 lets the
    // client surface a "reload needed" error instead of silently white-screening.
    if path.starts_with("assets/") {
        return (StatusCode::NOT_FOUND, "asset not found").into_response();
    }
    match WebAssets::get("index.html") {
        Some(content) => serve_asset("text/html", content.data.into_owned(), accepts_gzip, None),
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

fn accepts_gzip(headers: &HeaderMap) -> bool {
    headers
        .get(header::ACCEPT_ENCODING)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.contains("gzip"))
        .unwrap_or(false)
}

/// Build a response for an embedded asset, transparently handling the
/// gzip-at-build-time scheme. `bytes` may be gzip-compressed (detected via the
/// magic bytes); only the text assets build.rs compresses ever are, and no binary
/// asset starts with those bytes, so detection is unambiguous.
fn serve_asset(
    content_type: &str,
    bytes: Vec<u8>,
    accepts_gzip: bool,
    cache_control: Option<&'static str>,
) -> Response {
    let mut headers = HeaderMap::new();
    if let Ok(value) = HeaderValue::from_str(content_type) {
        headers.insert(header::CONTENT_TYPE, value);
    }
    if let Some(cc) = cache_control {
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(cc));
    }

    if bytes.starts_with(&[0x1f, 0x8b]) {
        // Caches must key on Accept-Encoding since the same URL can serve gzip or
        // inflated bytes depending on the client.
        headers.insert(header::VARY, HeaderValue::from_static("Accept-Encoding"));
        if accepts_gzip {
            headers.insert(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));
            return (headers, bytes).into_response();
        }
        // Rare client without gzip support: inflate on the fly.
        return match inflate(&bytes) {
            Ok(raw) => (headers, raw).into_response(),
            Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "decode error").into_response(),
        };
    }

    (headers, bytes).into_response()
}

fn inflate(bytes: &[u8]) -> std::io::Result<Vec<u8>> {
    use std::io::Read;

    use flate2::read::GzDecoder;

    let mut decoder = GzDecoder::new(bytes);
    let mut out = Vec::new();
    decoder.read_to_end(&mut out)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gzip(bytes: &[u8]) -> Vec<u8> {
        use std::io::Write;

        use flate2::Compression;
        use flate2::write::GzEncoder;

        let mut enc = GzEncoder::new(Vec::new(), Compression::best());
        enc.write_all(bytes).unwrap();
        enc.finish().unwrap()
    }

    #[test]
    fn gzipped_asset_is_served_with_content_encoding_when_accepted() {
        let resp = serve_asset("text/javascript", gzip(b"console.log('hi')\n"), true, None);
        assert_eq!(
            resp.headers().get(header::CONTENT_ENCODING).unwrap(),
            "gzip"
        );
        assert_eq!(resp.headers().get(header::VARY).unwrap(), "Accept-Encoding");
    }

    #[test]
    fn gzipped_asset_is_inflated_when_client_does_not_accept_gzip() {
        let resp = serve_asset("text/javascript", gzip(b"console.log('hi')\n"), false, None);
        assert!(resp.headers().get(header::CONTENT_ENCODING).is_none());
    }

    #[test]
    fn raw_asset_is_served_unchanged() {
        // A PNG header — not gzip — must pass through with no Content-Encoding.
        let png = vec![0x89u8, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];
        let resp = serve_asset("image/png", png, true, None);
        assert!(resp.headers().get(header::CONTENT_ENCODING).is_none());
        assert!(resp.headers().get(header::VARY).is_none());
    }

    #[test]
    fn accepts_gzip_reads_the_header() {
        let mut h = HeaderMap::new();
        assert!(!accepts_gzip(&h));
        h.insert(
            header::ACCEPT_ENCODING,
            HeaderValue::from_static("gzip, deflate, br"),
        );
        assert!(accepts_gzip(&h));
    }
}
