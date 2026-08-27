//! The built web UI embedded into the binary by rust-embed and served with SPA
//! fallback. Built by build.rs.
//!
//! The embedded tree is `$OUT_DIR/ui`, a Brotli-compressed mirror build.rs stages from
//! `web/dist` on every path it can take, NOT `web/dist` itself. Reading the
//! generated directory directly meant the embedded bytes depended on the state of
//! a directory cargo was not allowed to watch (watching it re-runs the frontend
//! build forever); emptying it baked in zero files and the server answered 404 at
//! the root with nothing said anywhere. The KNOWN GAP comment in build.rs has the
//! measurements, and the honest limits of the repair.
//!
//! Interpolating `$OUT_DIR` in the `folder` attribute needs rust-embed's
//! `interpolate-folder-path` feature, which Cargo.toml enables. Nothing here has
//! to pin it, and no test could: rust-embed enforces it at COMPILE time. Read
//! against the pinned 8.11.0, the expansion is `#[cfg]`-gated on that feature,
//! and without it the literal `$OUT_DIR/ui` is a RELATIVE path joined onto the
//! crate's manifest directory, so it names a directory that does not exist and
//! the derive fails with an error quoting it. What
//! `the_embed_folder_resolves_to_something` covers is the state that DOES
//! compile: a staging directory that exists and is empty.
//!
//! The text assets are Brotli-compressed during that staging, so the bytes
//! rust-embed bakes in are already compressed (shrinking the binary). Brotli has
//! NO magic bytes, so unlike the gzip scheme this replaced the handler cannot
//! sniff compressed-ness from the payload: it decides by extension, through the
//! same [`crate::compressible_exts`] list build.rs compresses by, and serves
//! those assets with `Content-Encoding: br` for clients that accept it (every
//! browser), decompressing on the fly for the rare client that doesn't.

use axum::http::{HeaderMap, HeaderValue, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

/// Public so the integration tests can assert on the embedded set DIRECTLY
/// (`WebAssets::iter()` / `WebAssets::get()`) instead of only through the router.
/// Every other gate in this crate is indirect: it fetches a page and happens to
/// fail when the embed is empty. A direct assertion holds no matter where
/// `folder` points and reports the cause instead of a 404.
#[derive(RustEmbed)]
#[folder = "$OUT_DIR/ui"]
pub struct WebAssets;

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

/// Warning for a build stamped as complete whose embedded asset set is empty or
/// nearly empty. It gives source builders repair steps and tells binary-package
/// users to report a packaging error.
pub const UI_EMPTY_EMBED_WARNING: &str = "This binary reports a real frontend build, but almost no web assets are embedded \
     in it, so server mode will answer 404 for the web UI. The build script and \
     rust-embed disagreed about what to bake in. Building from source? Run `touch \
     crates/dux-web/web/index.html` and rebuild, or `cargo clean -p dux-web` and \
     rebuild, to force the frontend build and the staging step to run again. \
     Installed dux from a release archive, npm, or the install script? That is a \
     packaging bug, not something you can fix locally. Please report it.";

/// The smallest number of embedded files a real build can plausibly have.
///
/// The failure this guards is total: a broken embed carries ZERO files, so any
/// floor at all would catch it. 8 is a little headroom above that without
/// approaching a real build, which is 108 files (measured, 2026-07: `web/dist`
/// holds 108 files, 92 of them hashed bundles under `assets/`). Even an app that
/// abandoned code splitting entirely would still ship an `index.html`, an entry
/// chunk, a stylesheet, the service worker, the manifest, the offline page and
/// its icons. Kept deliberately low because this fires a WARNING at every server
/// start: a floor that could cry wolf on a legitimate build is worse than one
/// that only ever catches the empty case.
const MIN_PLAUSIBLE_EMBEDDED_FILES: usize = 8;

/// The single warning row `dux server` shows, or `None` when there is nothing to
/// say. Pure over its inputs so it can be tested; [`ui_startup_warning`] supplies
/// the real ones.
///
/// A skip state outranks the embed check because it EXPLAINS it: a notice-page
/// binary legitimately embeds one file, and telling that operator their embed is
/// implausibly small would send them hunting for the wrong problem.
fn startup_warning(state: UiBuildState, embedded_files: usize) -> Option<&'static str> {
    match ui_build_warning(state) {
        Some(warning) => Some(warning),
        None if embedded_files < MIN_PLAUSIBLE_EMBEDDED_FILES => Some(UI_EMPTY_EMBED_WARNING),
        None => None,
    }
}

/// This binary's startup warning row: the build-state warning when the frontend
/// build was skipped, otherwise the empty-embed warning when the build state and
/// the embedded set disagree, otherwise nothing.
///
/// Counting is bounded: it stops at the floor rather than walking all 108 entries.
pub fn ui_startup_warning() -> Option<&'static str> {
    let embedded = WebAssets::iter().take(MIN_PLAUSIBLE_EMBEDDED_FILES).count();
    startup_warning(ui_build_state(), embedded)
}

/// Cache policy per request path. Vite fingerprints everything under `assets/`
/// with a content hash in the filename, so a changed bundle is a changed URL and
/// those files can be cached forever. Everything that is NOT content-addressed
/// (the `index.html` entry point that references the hashed chunks, the PWA
/// manifest, the service worker, the offline page) must revalidate on every
/// load, or a browser keeps rendering a stale bundle after the binary is
/// rebuilt. Revalidation is cheap: responses carry a sha256 `ETag`, so an
/// unchanged file answers `304 Not Modified` with no body. Icons and images
/// are not content-addressed either (hashing them would require generating
/// the manifest, whose icon paths Vite does not rewrite), so they take the
/// same no-cache-plus-ETag policy: a changed logo shows on the next load
/// instead of up to a day late, and an unchanged one costs one empty `304`
/// per load, which is nothing on a loopback/Tailscale server.
fn cache_policy(path: &str) -> &'static str {
    if path.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    }
}

/// Weak ETag derived from rust-embed's build-time sha256 of the file. Weak
/// (`W/`) on purpose: the same URL serves Brotli or decompressed bytes depending
/// on `Accept-Encoding`, and a weak validator asserts semantic equivalence
/// across those representations.
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
    let accepts_br = accepts_br(&headers);
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    if let Some(content) = WebAssets::get(path) {
        let mime = content.metadata.mimetype().to_string();
        return serve_embedded(&mime, path, content, accepts_br, &headers);
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
        Some(content) => serve_embedded("text/html", "index.html", content, accepts_br, &headers),
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
    accepts_br: bool,
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
        crate::compressible_exts::compressible_path(path),
        accepts_br,
        Some(cache_control),
        Some(&etag),
    )
}

/// Whether the client accepts Brotli. Token-wise rather than `contains("br")`,
/// because "br" is short enough to appear inside another token; parameters
/// (`br;q=0.9`) are tolerated, `q=0` is rare enough in real browsers to ignore
/// (the previous gzip check ignored it too).
fn accepts_br(headers: &HeaderMap) -> bool {
    headers
        .get(header::ACCEPT_ENCODING)
        .and_then(|v| v.to_str().ok())
        .map(|v| {
            v.split(',').any(|token| {
                token
                    .trim()
                    .split(';')
                    .next()
                    .is_some_and(|t| t.trim() == "br")
            })
        })
        .unwrap_or(false)
}

/// Build a response for an embedded asset, transparently handling the
/// Brotli-at-build-time scheme. `compressed` says whether `bytes` are Brotli:
/// the CALLER decides, from the shared extension list, because Brotli has no
/// magic bytes to sniff (unlike the gzip scheme this replaced).
fn serve_asset(
    content_type: &str,
    bytes: Vec<u8>,
    compressed: bool,
    accepts_br: bool,
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

    if compressed {
        // Caches must key on Accept-Encoding since the same URL can serve Brotli
        // or decompressed bytes depending on the client.
        headers.insert(header::VARY, HeaderValue::from_static("Accept-Encoding"));
        if accepts_br {
            headers.insert(header::CONTENT_ENCODING, HeaderValue::from_static("br"));
            return (headers, bytes).into_response();
        }
        // Rare client without Brotli support: decompress on the fly.
        return match decompress(&bytes) {
            Ok(raw) => (headers, raw).into_response(),
            Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "decode error").into_response(),
        };
    }

    (headers, bytes).into_response()
}

fn decompress(bytes: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut out = Vec::new();
    brotli::BrotliDecompress(&mut &bytes[..], &mut out)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reusing_an_existing_dist_is_a_marked_state_not_a_normal_build() {
        // Reuse of an existing dist is its own marked state: otherwise a
        // binary serving an arbitrarily old UI is indistinguishable from a
        // fresh build (no banner row, no log line, every real-build test
        // passing).
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

    #[test]
    fn a_real_build_that_embedded_nothing_is_warned_about() {
        // The build state says only "a frontend build ran", which is a different
        // question from "did those files reach the binary". When they disagree the
        // user sees a 404 at the root and nothing anywhere says why, so the server
        // says it at startup instead.
        assert_eq!(
            startup_warning(UiBuildState::Built, 0),
            Some(UI_EMPTY_EMBED_WARNING)
        );
        assert_eq!(
            startup_warning(UiBuildState::Built, MIN_PLAUSIBLE_EMBEDDED_FILES - 1),
            Some(UI_EMPTY_EMBED_WARNING)
        );
        // A real build (108 files, measured) is silent, and so is anything at the
        // floor: this row appears at every server start, so it must not cry wolf.
        assert_eq!(
            startup_warning(UiBuildState::Built, MIN_PLAUSIBLE_EMBEDDED_FILES),
            None
        );
        assert_eq!(startup_warning(UiBuildState::Built, 108), None);
    }

    #[test]
    fn a_skip_state_outranks_the_empty_embed_check() {
        // A notice-page binary legitimately embeds ONE file, so the empty-embed
        // wording would be technically true and completely misleading: it tells
        // the operator the build script and rust-embed disagreed, when what
        // actually happened is that they asked for no web UI. The state warning
        // explains the small embed, so it wins.
        assert_eq!(
            startup_warning(UiBuildState::NotBuilt, 1),
            Some(UI_NOT_BUILT_WARNING)
        );
        assert_eq!(
            startup_warning(UiBuildState::StaleReuse, 108),
            Some(UI_STALE_WARNING)
        );
    }

    #[test]
    fn this_binarys_startup_warning_reflects_this_binarys_embed() {
        // The end-to-end shape of the guard, against whatever this binary actually
        // carries. In a normal build both halves are quiet; under the escape hatch
        // the row is the state's, never the empty-embed one, because a skip state
        // outranks it.
        let warning = ui_startup_warning();
        match ui_build_state() {
            UiBuildState::Built => assert_eq!(
                warning, None,
                "a normal build must produce no startup warning; getting {warning:?} \
                 means this binary embedded fewer than {MIN_PLAUSIBLE_EMBEDDED_FILES} \
                 files and would 404 at the root"
            ),
            state => assert_eq!(warning, ui_build_warning(state)),
        }
    }

    fn br(bytes: &[u8]) -> Vec<u8> {
        let params = brotli::enc::BrotliEncoderParams::default();
        let mut out = Vec::new();
        brotli::BrotliCompress(&mut &bytes[..], &mut out, &params).unwrap();
        out
    }

    #[test]
    fn compressed_asset_is_served_with_content_encoding_when_accepted() {
        let resp = serve_asset(
            "text/javascript",
            br(b"console.log('hi')\n"),
            true,
            true,
            None,
            None,
        );
        assert_eq!(resp.headers().get(header::CONTENT_ENCODING).unwrap(), "br");
        assert_eq!(resp.headers().get(header::VARY).unwrap(), "Accept-Encoding");
    }

    #[test]
    fn compressed_asset_is_decompressed_when_client_does_not_accept_br() {
        let resp = serve_asset(
            "text/javascript",
            br(b"console.log('hi')\n"),
            true,
            false,
            None,
            None,
        );
        assert!(resp.headers().get(header::CONTENT_ENCODING).is_none());
    }

    #[tokio::test]
    async fn decompressed_body_matches_the_original_text() {
        // The on-the-fly fallback must hand back the exact pre-compression
        // bytes, or a no-br client gets a corrupt bundle with a 200 on it.
        let resp = serve_asset(
            "text/javascript",
            br(b"console.log('hi')\n"),
            true,
            false,
            None,
            None,
        );
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&body[..], b"console.log('hi')\n");
    }

    #[test]
    fn raw_asset_is_served_unchanged() {
        // A PNG (not a compressible extension, so the caller passes
        // compressed: false) must pass through with no Content-Encoding.
        let png = vec![0x89u8, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];
        let resp = serve_asset("image/png", png, false, true, None, None);
        assert!(resp.headers().get(header::CONTENT_ENCODING).is_none());
        assert!(resp.headers().get(header::VARY).is_none());
    }

    #[test]
    fn compressed_ness_is_decided_by_the_shared_extension_list() {
        // Brotli has no magic bytes: this predicate IS the contract between
        // build.rs (which compresses by it) and serve_embedded (which passes
        // its answer to serve_asset). A drift here serves garbage.
        use crate::compressible_exts::compressible_path;
        assert!(compressible_path("assets/index-B5xQabc1.js"));
        assert!(compressible_path("index.html"));
        assert!(compressible_path("manifest.webmanifest"));
        assert!(compressible_path("assets/style-Abc.css"));
        assert!(!compressible_path("favicon.png"));
        assert!(!compressible_path("assets/codicon-ngg6Pgfi.ttf"));
        assert!(!compressible_path("no-extension"));
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
        // Images are no-cache + ETag like the entry points: not content-
        // addressed, so a max-age would show a changed logo up to a day late.
        assert_eq!(cache_policy("favicon.png"), "no-cache");
        assert_eq!(cache_policy("dux-logo.png"), "no-cache");
    }

    #[test]
    fn serve_asset_sets_etag_and_cache_control() {
        let resp = serve_asset(
            "text/html",
            b"<html></html>".to_vec(),
            false,
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
    fn accepts_br_reads_the_header_token_wise() {
        let mut h = HeaderMap::new();
        assert!(!accepts_br(&h));
        h.insert(
            header::ACCEPT_ENCODING,
            HeaderValue::from_static("gzip, deflate, br"),
        );
        assert!(accepts_br(&h));
        // Parameters are tolerated; a token merely CONTAINING "br" is not br.
        h.insert(
            header::ACCEPT_ENCODING,
            HeaderValue::from_static("br;q=0.9"),
        );
        assert!(accepts_br(&h));
        h.insert(header::ACCEPT_ENCODING, HeaderValue::from_static("zbrotli"));
        assert!(!accepts_br(&h));
        h.insert(
            header::ACCEPT_ENCODING,
            HeaderValue::from_static("gzip, deflate"),
        );
        assert!(!accepts_br(&h));
    }
}
