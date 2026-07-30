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

/// True when this binary was compiled with `DUX_DISABLE_UI_BUILD` set and there
/// was no previously built `web/dist` to embed, so the page served at `/` is
/// build.rs's "web UI not built" notice rather than the real single-page app.
///
/// `build.rs` sets `DUX_UI_BUILD_SKIPPED=1` exactly on that path (and declares
/// `cargo:rerun-if-env-changed=DUX_DISABLE_UI_BUILD` so toggling the hatch is not
/// masked by cargo's build-script cache). Two consumers read this back: the
/// `dux server` startup banner, which turns it into a warning row, and the static
/// serving tests, which SKIP with a printed reason rather than pass on a page
/// that is not a build.
pub const fn ui_build_skipped() -> bool {
    option_env!("DUX_UI_BUILD_SKIPPED").is_some()
}

/// Operator-facing warning for a binary built without the web UI. Shown as a
/// startup banner row and logged to `dux.log`, because the person who can fix
/// this is the one who launched the server, and they may never open a browser.
/// The served page carries the same message for whoever does open one.
pub const UI_NOT_BUILT_WARNING: &str = "This binary was built with DUX_DISABLE_UI_BUILD set, so it contains NO web UI. \
     Every page serves a notice explaining that. Rebuild without DUX_DISABLE_UI_BUILD \
     (run `npm ci` in crates/dux-web/web first) to serve the real web UI.";

/// Cache policy per request path. Vite fingerprints everything under `assets/`
/// with a content hash in the filename, so a changed bundle is a changed URL and
/// those files can be cached forever. Everything that is NOT content-addressed
/// (the `index.html` entry point that references the hashed chunks, the PWA
/// manifest, the service worker, the offline page) must revalidate on every
/// load, or a browser keeps rendering a stale bundle after the binary is
/// rebuilt. Revalidation is cheap: responses carry a sha256 `ETag`, so an
/// unchanged file answers `304 Not Modified` with no body. Icons and images are
/// not content-addressed but change essentially never; they get a modest
/// max-age so reloads stay light.
fn cache_policy(path: &str) -> &'static str {
    if path.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else if matches!(
        path,
        "index.html" | "sw.js" | "manifest.webmanifest" | "offline.html"
    ) {
        "no-cache"
    } else {
        "public, max-age=86400"
    }
}

/// Weak ETag derived from rust-embed's build-time sha256 of the file. Weak
/// (`W/`) on purpose: the same URL serves gzip or inflated bytes depending on
/// `Accept-Encoding`, and a weak validator asserts semantic equivalence across
/// those representations.
fn etag_for(content: &rust_embed::EmbeddedFile) -> String {
    let mut tag = String::with_capacity(4 + 64 + 1);
    tag.push_str("W/\"");
    for b in content.metadata.sha256_hash() {
        use std::fmt::Write;
        let _ = write!(tag, "{b:02x}");
    }
    tag.push('"');
    tag
}

fn if_none_match_matches(headers: &HeaderMap, etag: &str) -> bool {
    headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.split(',').any(|candidate| candidate.trim() == etag))
}

/// Serve an embedded asset by request path, falling back to `index.html` for
/// unknown paths (client-side routing). `/ws` and `/healthz` are matched before
/// this fallback, so they never reach here.
///
/// Special cases for PWA support:
/// - `.webmanifest` is served as `application/manifest+json` (mime-guess already
///   maps it correctly, so the generic path below handles it).
/// - `offline.html` is a real embedded asset, so it is served here directly and
///   never shadowed by the SPA `index.html` fallback below.
pub async fn static_handler(uri: Uri, headers: HeaderMap) -> Response {
    let accepts_gzip = accepts_gzip(&headers);
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    if let Some(content) = WebAssets::get(path) {
        let mime = content.metadata.mimetype().to_string();
        return serve_embedded(&mime, path, content, accepts_gzip, &headers);
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
        Some(content) => serve_embedded("text/html", "index.html", content, accepts_gzip, &headers),
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

/// Answer a request for an embedded file: `304 Not Modified` when the client
/// already holds the current bytes (matching `If-None-Match`), the full body
/// otherwise. Both carry the ETag and the path's cache policy.
fn serve_embedded(
    content_type: &str,
    path: &str,
    content: rust_embed::EmbeddedFile,
    accepts_gzip: bool,
    request_headers: &HeaderMap,
) -> Response {
    let etag = etag_for(&content);
    let cache_control = cache_policy(path);
    if if_none_match_matches(request_headers, &etag) {
        let mut headers = HeaderMap::new();
        if let Ok(value) = HeaderValue::from_str(&etag) {
            headers.insert(header::ETAG, value);
        }
        headers.insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static(cache_control),
        );
        return (StatusCode::NOT_MODIFIED, headers).into_response();
    }
    serve_asset(
        content_type,
        content.data.into_owned(),
        accepts_gzip,
        Some(cache_control),
        Some(&etag),
    )
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
    etag: Option<&str>,
) -> Response {
    let mut headers = HeaderMap::new();
    if let Ok(value) = HeaderValue::from_str(content_type) {
        headers.insert(header::CONTENT_TYPE, value);
    }
    if let Some(cc) = cache_control {
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(cc));
    }
    if let Some(tag) = etag
        && let Ok(value) = HeaderValue::from_str(tag)
    {
        headers.insert(header::ETAG, value);
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
        let resp = serve_asset(
            "text/javascript",
            gzip(b"console.log('hi')\n"),
            true,
            None,
            None,
        );
        assert_eq!(
            resp.headers().get(header::CONTENT_ENCODING).unwrap(),
            "gzip"
        );
        assert_eq!(resp.headers().get(header::VARY).unwrap(), "Accept-Encoding");
    }

    #[test]
    fn gzipped_asset_is_inflated_when_client_does_not_accept_gzip() {
        let resp = serve_asset(
            "text/javascript",
            gzip(b"console.log('hi')\n"),
            false,
            None,
            None,
        );
        assert!(resp.headers().get(header::CONTENT_ENCODING).is_none());
    }

    #[test]
    fn raw_asset_is_served_unchanged() {
        // A PNG header — not gzip — must pass through with no Content-Encoding.
        let png = vec![0x89u8, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];
        let resp = serve_asset("image/png", png, true, None, None);
        assert!(resp.headers().get(header::CONTENT_ENCODING).is_none());
        assert!(resp.headers().get(header::VARY).is_none());
    }

    #[test]
    fn cache_policy_is_immutable_for_hashed_assets_and_no_cache_for_entry_points() {
        assert_eq!(
            cache_policy("assets/index-B5xQabc1.js"),
            "public, max-age=31536000, immutable"
        );
        assert_eq!(cache_policy("index.html"), "no-cache");
        assert_eq!(cache_policy("sw.js"), "no-cache");
        assert_eq!(cache_policy("manifest.webmanifest"), "no-cache");
        assert_eq!(cache_policy("offline.html"), "no-cache");
        assert_eq!(cache_policy("favicon.png"), "public, max-age=86400");
    }

    #[test]
    fn serve_asset_sets_etag_and_cache_control() {
        let resp = serve_asset(
            "text/html",
            b"<html></html>".to_vec(),
            true,
            Some("no-cache"),
            Some("W/\"abc123\""),
        );
        assert_eq!(resp.headers().get(header::ETAG).unwrap(), "W/\"abc123\"");
        assert_eq!(
            resp.headers().get(header::CACHE_CONTROL).unwrap(),
            "no-cache"
        );
    }

    #[test]
    fn if_none_match_matches_exact_and_list_forms() {
        let etag = "W/\"deadbeef\"";
        let mut h = HeaderMap::new();
        assert!(!if_none_match_matches(&h, etag));
        h.insert(
            header::IF_NONE_MATCH,
            HeaderValue::from_static("W/\"other\""),
        );
        assert!(!if_none_match_matches(&h, etag));
        h.insert(
            header::IF_NONE_MATCH,
            HeaderValue::from_static("W/\"other\", W/\"deadbeef\""),
        );
        assert!(if_none_match_matches(&h, etag));
    }

    #[tokio::test]
    async fn index_html_revalidates_to_304_with_matching_etag() {
        // First request: full body plus the validator headers.
        let uri: Uri = "/".parse().unwrap();
        let first = static_handler(uri.clone(), HeaderMap::new()).await;
        assert_eq!(first.status(), StatusCode::OK);
        assert_eq!(
            first.headers().get(header::CACHE_CONTROL).unwrap(),
            "no-cache"
        );
        let etag = first.headers().get(header::ETAG).unwrap().clone();

        // Second request presenting the ETag: 304, no re-download.
        let mut headers = HeaderMap::new();
        headers.insert(header::IF_NONE_MATCH, etag.clone());
        let second = static_handler(uri, headers).await;
        assert_eq!(second.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(second.headers().get(header::ETAG).unwrap(), &etag);
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
