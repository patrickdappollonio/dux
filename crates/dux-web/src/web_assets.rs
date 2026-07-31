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

/// What the page served at `/` actually is, decided at compile time by `build.rs`.
///
/// Three states rather than a bool, because skipping the frontend build has two
/// outcomes that need DIFFERENT things said about them. Telling an operator their
/// binary "contains NO web UI" when it is serving a real (if old) one sends them
/// hunting for the wrong problem, and so does the reverse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiBuildState {
    /// The frontend was built during this binary's compilation. The normal case.
    Built,
    /// `DUX_DISABLE_UI_BUILD` was set and there was no previously built
    /// `web/dist`, so the page served at `/` is build.rs's notice page.
    NotBuilt,
    /// `DUX_DISABLE_UI_BUILD` was set and a previously built `web/dist` was
    /// embedded unchanged. The served page is a REAL single-page app with real
    /// hashed assets, of unknown age: it may predate this checkout by any amount.
    StaleReuse,
}

/// Map the build-script marker onto a state. Split out from [`ui_build_state`]
/// so it can be tested; `option_env!` is fixed at compile time and cannot be
/// varied from a test.
///
/// ONE marker with three values, not two booleans, and not because it is tidier.
/// `option_env!` reads the AMBIENT rustc environment as well as what the build
/// script emits, and a `cargo:rustc-env` ALWAYS wins over an ambient value of the
/// same name (measured with a throwaway crate, not assumed). The previous scheme
/// emitted nothing at all on the SUCCESS path, so nothing overrode an ambient
/// `DUX_UI_BUILD_SKIPPED=1`: setting it as a workflow-level `env:` made every
/// real-build test print SKIPPED on a completely genuine build, and CI guarded
/// only `DUX_DISABLE_UI_BUILD`. Emitting this marker on ALL THREE paths closes
/// that, because there is no path left on which the ambient value survives.
///
/// An unrecognised or absent value means Built, which is the safe default here:
/// the only states that let a test skip are the two spelled out below, so a
/// garbled marker fails loudly against whatever page is embedded rather than
/// quietly excusing the suite.
fn state_from(marker: Option<&str>) -> UiBuildState {
    match marker {
        Some("not_built") => UiBuildState::NotBuilt,
        Some("stale") => UiBuildState::StaleReuse,
        _ => UiBuildState::Built,
    }
}

/// This binary's UI build state.
///
/// `build.rs` sets `cargo:rustc-env=DUX_UI_BUILD_STATE` to `built`, `not_built`
/// or `stale` on every path it can take (and declares
/// `cargo:rerun-if-env-changed=DUX_DISABLE_UI_BUILD` so toggling the hatch is not
/// masked by cargo's build-script cache).
pub fn ui_build_state() -> UiBuildState {
    state_from(option_env!("DUX_UI_BUILD_STATE"))
}

/// Operator-facing warning for a binary built without the web UI. Shown as a
/// startup banner row and logged to `dux.log`, because the person who can fix
/// this is the one who launched the server, and they may never open a browser.
/// The served page carries the same message for whoever does open one.
pub const UI_NOT_BUILT_WARNING: &str = "This binary was built with DUX_DISABLE_UI_BUILD set, so it contains NO web UI. \
     Every page serves a notice explaining that. Rebuild without DUX_DISABLE_UI_BUILD \
     (run `npm ci` in crates/dux-web/web first) to serve the real web UI.";

/// Operator-facing warning for a binary that reused an existing `web/dist`.
///
/// Deliberately different wording from [`UI_NOT_BUILT_WARNING`]: there IS a web
/// UI here and it will look completely normal, which is exactly why it has to be
/// said out loud. Nothing records when that `dist` was built, so "old" is the
/// strongest claim available and the message does not pretend otherwise.
pub const UI_STALE_WARNING: &str = "This binary was built with DUX_DISABLE_UI_BUILD set and embedded a web/dist that \
     was already on disk, so the web UI it serves was NOT built from this source \
     and may be arbitrarily out of date. It will otherwise look and behave \
     normally. Rebuild without DUX_DISABLE_UI_BUILD to serve a current web UI.";

/// The warning row for a state, or `None` when there is nothing to say.
pub const fn ui_build_warning(state: UiBuildState) -> Option<&'static str> {
    match state {
        UiBuildState::Built => None,
        UiBuildState::NotBuilt => Some(UI_NOT_BUILT_WARNING),
        UiBuildState::StaleReuse => Some(UI_STALE_WARNING),
    }
}

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

    #[test]
    fn reusing_an_existing_dist_is_a_marked_state_not_a_normal_build() {
        // THE FINDING. build.rs used to emit nothing on the reuse path, so a
        // binary serving an arbitrarily old UI was indistinguishable from a fresh
        // one: no banner row, no log line, and every real-build test passing.
        assert_eq!(state_from(Some("stale")), UiBuildState::StaleReuse);
        assert_eq!(state_from(Some("not_built")), UiBuildState::NotBuilt);
        assert_eq!(state_from(Some("built")), UiBuildState::Built);
    }

    #[test]
    fn an_absent_or_garbled_marker_is_a_build_and_never_an_excuse_to_skip() {
        // The safe default, and the reason the marker is one name rather than
        // the two booleans it replaced. build.rs now writes this on every path it
        // can take, so `None` cannot occur in a real build; if it somehow does,
        // or if the value is misspelt, the suite must ASSERT against whatever is
        // embedded rather than print SKIPPED and prove nothing. Only the two
        // exact spellings above buy a skip.
        assert_eq!(state_from(None), UiBuildState::Built);
        assert_eq!(state_from(Some("")), UiBuildState::Built);
        assert_eq!(state_from(Some("1")), UiBuildState::Built);
        assert_eq!(state_from(Some("NOT_BUILT")), UiBuildState::Built);
        assert_eq!(state_from(Some("skipped")), UiBuildState::Built);
    }

    #[test]
    fn both_skip_states_warn_and_a_real_build_does_not() {
        // What the banner and the static-serving tests gate on: "was a frontend
        // build performed for THIS binary". False either way it was skipped, so
        // both skip states carry a warning and only the real build is silent.
        assert!(ui_build_warning(UiBuildState::Built).is_none());
        assert!(ui_build_warning(UiBuildState::NotBuilt).is_some());
        assert!(ui_build_warning(UiBuildState::StaleReuse).is_some());
    }

    #[test]
    fn the_two_skip_warnings_say_different_things() {
        // Collapsing these onto one message is the tempting fix and the wrong
        // one: "contains NO web UI" is false of a reuse binary, which serves a
        // real app, and a message the operator can see is wrong is a message
        // they stop reading.
        let not_built = ui_build_warning(UiBuildState::NotBuilt).unwrap();
        let stale = ui_build_warning(UiBuildState::StaleReuse).unwrap();
        assert_ne!(not_built, stale);
        assert!(
            not_built.contains("NO web UI"),
            "the notice-page warning must say there is no web UI: {not_built}"
        );
        assert!(
            !stale.contains("NO web UI"),
            "the reuse warning must NOT claim there is no web UI, there is one: {stale}"
        );
        assert!(
            stale.contains("out of date"),
            "the reuse warning must say what is actually wrong: {stale}"
        );
        for warning in [not_built, stale] {
            assert!(
                warning.contains("DUX_DISABLE_UI_BUILD"),
                "each warning must name the variable that caused it: {warning}"
            );
        }
    }

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
