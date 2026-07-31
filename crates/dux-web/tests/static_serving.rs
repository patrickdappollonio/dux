//! Static-asset serving, including the guard that the page baked into the binary
//! is a REAL frontend build.
//!
//! ## Why the real-build guard exists
//!
//! `crates/dux-web/build.rs` used to turn a failed `npm run build` into a
//! placeholder page plus a `cargo:warning`, and let `cargo build` succeed. The
//! test that should have caught it asserted the served page contained
//! `<!doctype html` OR `id="root"`, and the placeholder contained BOTH, so a
//! release could have shipped four platform binaries with no web UI and every
//! check green. `doctype_and_root_element_are_not_evidence_of_a_build` pins that
//! finding so nobody reintroduces the weak assertion, and
//! `index_references_a_real_hashed_asset_that_is_actually_served` is the check
//! that a placeholder cannot satisfy.
//!
//! That weak assertion had SURVIVED in two routing tests here even after the
//! finding was pinned, so this file simultaneously argued that the assertion
//! proves nothing and made it twice. Both now go through [`assert_is_spa_shell`],
//! which demands a hashed bundle reference when this binary has a real build and
//! demands the notice page when it does not; routing has to work either way, so
//! these two assert rather than skip.
//!
//! ## Reaching past the page
//!
//! Checking the page alone is not enough, and `chunk_refs` is why. The terminal
//! emulator and the editor's viewers are code-split, and a lazy `import()` target
//! is written relative to the importing CHUNK, so those names appear nowhere in
//! `index.html`. A dist missing them serves a perfectly good-looking page and goes
//! blank the moment a terminal is opened. The real-build checks therefore walk the
//! whole reference graph, and the archive smoke test in
//! `.github/scripts/smoke_archive.sh` performs the same walk on the artifact that
//! actually ships.
//!
//! ## Reaching past the router
//!
//! Every check described above is INDIRECT: it drives the router, and it happens
//! to fail when nothing is embedded, reporting a 404 that says nothing about why.
//! `the_embedded_asset_set_is_a_whole_frontend_build` asserts on `WebAssets`
//! itself instead, so it holds regardless of where rust-embed's `folder` points
//! and it names the cause. `the_embed_folder_resolves_to_something` is the cheap
//! unconditional twin of it, pinning the `interpolate-folder-path` feature that
//! makes `folder = "$OUT_DIR/ui"` resolve at all.
//!
//! ## The skip
//!
//! With `DUX_DISABLE_UI_BUILD` set, no frontend build happened for this binary, so
//! the tests that need one SKIP via [`require_real_ui_build`] rather than pass.
//! That covers BOTH skip routes: the notice page, and a previously built
//! `web/dist` embedded unchanged. The second used to be invisible (build.rs marked
//! nothing, so these tests asserted happily against a UI of unknown age); build.rs
//! now stamps `DUX_UI_BUILD_STATE=stale` and it skips here too, with its own
//! reason, since the notice-page sentence is false of it.
//!
//! The two tests that do NOT skip, because they are about routing, read the same
//! state to decide WHICH page to demand. Reading a "was it skipped" boolean
//! instead made them demand the notice page from a binary serving a real reused
//! app, so they failed in a configuration CONTRIBUTING.md documents as supported.
//!
//! A silently-passing test is exactly how the original defect survived, so the
//! reason is printed, and build.rs also emits a `cargo:warning` on those paths so
//! the reason surfaces even when the test harness captures stdout. A skipped test
//! is still a hiding place, which is why the release and PR workflows refuse to
//! build with `DUX_DISABLE_UI_BUILD` set at all.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use dux_core::config::DuxPaths;
use dux_web::bootstrap::bootstrap_engine;
use dux_web::engine_actor::spawn_engine_thread;
use dux_web::server::router;
use tower::ServiceExt;

/// Return early from a test that requires a genuine frontend build, printing why.
/// Run with `cargo test -- --nocapture` to see the line; the build script's
/// `cargo:warning` says the same thing unconditionally.
///
/// The reason is per-STATE, not one sentence covering both skip routes. It used to
/// say "no previously built web/dist, so the embedded page is build.rs's notice
/// page" whichever route had been taken, which is false on the reuse route: there
/// IS a previous dist and the page is a real app. A printed reason nobody can
/// trust is how people stop reading printed reasons, and this module's docs
/// promise that the reason is printed.
macro_rules! require_real_ui_build {
    ($test:literal) => {
        match dux_web::web_assets::ui_build_state() {
            dux_web::web_assets::UiBuildState::Built => {}
            dux_web::web_assets::UiBuildState::NotBuilt => {
                println!(
                    "SKIPPED {}: this binary was built with DUX_DISABLE_UI_BUILD set and no \
                     previously built web/dist, so the embedded page is build.rs's \"web UI not \
                     built\" notice and there is no real frontend build to assert on. Unset \
                     DUX_DISABLE_UI_BUILD and rebuild to run this test.",
                    $test
                );
                return;
            }
            dux_web::web_assets::UiBuildState::StaleReuse => {
                println!(
                    "SKIPPED {}: this binary was built with DUX_DISABLE_UI_BUILD set and \
                     embedded a web/dist that was already on disk. The page it serves is a real \
                     app with real hashed assets, but it was NOT built from this source and may \
                     be arbitrarily old, so asserting on it would say nothing about the code \
                     under test. Unset DUX_DISABLE_UI_BUILD and rebuild to run this test.",
                    $test
                );
                return;
            }
        }
    };
}

fn temp_paths() -> (tempfile::TempDir, DuxPaths) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let paths = DuxPaths {
        root: root.clone(),
        config_path: root.join("config.toml"),
        sessions_db_path: root.join("sessions.sqlite3"),
        worktrees_root: root.join("worktrees"),
        lock_path: root.join("dux.lock"),
    };
    std::fs::create_dir_all(&paths.worktrees_root).unwrap();
    (tmp, paths)
}

/// Assert that some served HTML is the SPA shell.
///
/// `<!doctype html> OR id="root"` is deliberately NOT the assertion, and this
/// helper exists so that stays true in one place. The notice page satisfies both
/// halves of it (it is a real HTML document), so the weak form cannot tell the
/// shell from the notice, which is the finding
/// `doctype_and_root_element_are_not_evidence_of_a_build` pins.
///
/// What is asserted therefore depends on which page this binary carries. With a
/// real build, the shell must reference a hashed bundle. Without one, the served
/// page must be the notice, and saying so is worth more than skipping: these two
/// tests are about ROUTING, and routing has to work in both modes.
///
/// Which page that is has THREE answers, not two, and collapsing them onto the
/// "was a build skipped" boolean broke the documented escape hatch. Only the
/// notice-page state serves the notice; a reused `dist` serves a real single-page
/// app, so demanding the notice there made these two tests FAIL, in exactly the
/// configuration CONTRIBUTING.md describes as supported (the hatch set with a
/// `dist` already on disk). They carry no skip guard on purpose, so they had no
/// way to opt out. Branch on the state.
fn assert_is_spa_shell(html: &str, what: &str) {
    use dux_web::web_assets::UiBuildState;

    let lower = html.to_lowercase();
    assert!(
        lower.contains("<!doctype html"),
        "{what}: not an HTML document at all: {html}"
    );
    if dux_web::web_assets::ui_build_state() == UiBuildState::NotBuilt {
        assert!(
            lower.contains("dux-ui-not-built-notice") || lower.contains("dux_disable_ui_build"),
            "{what}: this binary carries build.rs's notice page, so that is what the \
             page served here must be. It is neither that nor a build: {html}"
        );
        return;
    }
    // Built, or a reused dist: either way the page is a real SPA shell, and a
    // reused one is still expected to route and to reference its own bundles.
    assert!(
        lower.contains("id=\"root\""),
        "{what}: the SPA shell must carry the React mount point: {html}"
    );
    assert!(
        !hashed_asset_refs(html).is_empty(),
        "{what}: the page references no content-hashed bundle, so it is not the \
         built SPA shell: {html}"
    );
}

#[tokio::test]
async fn serves_embedded_index_at_root() {
    let (_tmp, paths) = temp_paths();
    let engine = bootstrap_engine(&paths).unwrap();
    let (handle, _join) = spawn_engine_thread(engine);
    let app = router(handle);
    let resp = app
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_is_spa_shell(&String::from_utf8_lossy(&bytes), "GET /");
}

#[tokio::test]
async fn unknown_path_falls_back_to_index() {
    let (_tmp, paths) = temp_paths();
    let engine = bootstrap_engine(&paths).unwrap();
    let (handle, _join) = spawn_engine_thread(engine);
    let app = router(handle);
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/some/client/route")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

/// Build a router backed by a throwaway engine for static-asset assertions.
fn test_router() -> (tempfile::TempDir, axum::Router) {
    let (tmp, paths) = temp_paths();
    let engine = bootstrap_engine(&paths).unwrap();
    let (handle, _join) = spawn_engine_thread(engine);
    (tmp, router(handle))
}

async fn get(app: axum::Router, uri: &str) -> axum::http::Response<Body> {
    app.oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap()
}

fn header(resp: &axum::http::Response<Body>, name: &str) -> Option<String> {
    resp.headers()
        .get(name)
        .map(|v| v.to_str().unwrap().to_string())
}

#[tokio::test]
async fn manifest_served_with_manifest_mime() {
    require_real_ui_build!("manifest_served_with_manifest_mime");
    let (_tmp, app) = test_router();
    let resp = get(app, "/manifest.webmanifest").await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        header(&resp, "content-type").as_deref(),
        Some("application/manifest+json"),
        "the web manifest must advertise the PWA manifest MIME type"
    );
}

#[tokio::test]
async fn service_worker_served_no_cache_and_js_mime() {
    require_real_ui_build!("service_worker_served_no_cache_and_js_mime");
    let (_tmp, app) = test_router();
    let resp = get(app, "/sw.js").await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        header(&resp, "cache-control").as_deref(),
        Some("no-cache"),
        "the service worker must not be cached so SW updates are picked up promptly"
    );
    let ctype = header(&resp, "content-type").unwrap_or_default();
    assert!(
        ctype.contains("javascript"),
        "sw.js must be served with a JavaScript MIME type, got {ctype}"
    );
}

#[tokio::test]
async fn missing_hashed_asset_returns_404_not_spa_fallback() {
    // A request for a hashed bundle chunk that does not exist must 404, NOT fall
    // back to index.html. Serving HTML for a `*.js` import() makes the browser
    // reject it as a module, which unmounts the React tree (white screen). This
    // happens after a rebuild+restart while a stale tab is still open.
    let (_tmp, app) = test_router();
    let resp = get(app, "/assets/nonexistent-deadbeef.js").await;
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "a missing hashed asset must 404 so the client can recover, not get the SPA shell"
    );
    let ctype = header(&resp, "content-type").unwrap_or_default();
    assert!(
        !ctype.contains("html"),
        "a missing asset must not be served as text/html, got {ctype}"
    );
}

#[tokio::test]
async fn unknown_non_asset_path_still_serves_spa_shell() {
    // Client-side routes (anything outside `assets/`) must keep falling back to
    // the SPA index so deep links and the router keep working.
    let (_tmp, app) = test_router();
    let resp = get(app, "/some/client/route").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let ctype = header(&resp, "content-type").unwrap_or_default();
    assert!(
        ctype.contains("html"),
        "an unknown non-asset path must serve the SPA shell as text/html, got {ctype}"
    );
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_is_spa_shell(
        &String::from_utf8_lossy(&bytes),
        "an unknown non-asset path",
    );
}

#[tokio::test]
async fn offline_page_reachable_and_not_shadowed_by_spa_fallback() {
    require_real_ui_build!("offline_page_reachable_and_not_shadowed_by_spa_fallback");
    let (_tmp, app) = test_router();
    let resp = get(app, "/offline.html").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = String::from_utf8_lossy(&bytes);
    // The real offline page, not the SPA index served by the fallback.
    assert!(
        html.contains("dux is unreachable"),
        "the offline page itself must be served, not the SPA index fallback"
    );
    assert!(
        !html.contains("id=\"root\""),
        "offline.html should not be the SPA shell"
    );
}

// ---------------------------------------------------------------------------
// The real-build guard.
// ---------------------------------------------------------------------------

/// Every `assets/...` path the given HTML references whose filename carries a
/// content hash, in document order.
///
/// Vite fingerprints its bundle output as `<name>-<hash>.<ext>`, so the test for
/// "this came out of a real build" is: the final dash-separated segment of the
/// filename stem is at least [`MIN_HASH_LEN`] characters of `[A-Za-z0-9_]`. That
/// accepts `assets/index-x8kEp4D8.js` and `assets/rolldown-runtime-QTnfLwEv.js`
/// (multiple dashes, the LAST segment is the hash) and rejects a hand-written
/// `assets/dux-logo.png`, along with any page that references no bundle at all.
///
/// Deliberately no digit or mixed-case requirement: a base64url hash that happens
/// to be all lowercase letters is unlikely but possible, and a gate that fails
/// once in a few thousand builds is worse than a slightly looser shape check. The
/// caller closes the remaining gap by FETCHING what it finds.
/// Vite's default hash length. Anything shorter is a human-chosen name.
const MIN_HASH_LEN: usize = 8;

/// Whether a filename stem ends in what looks like a content hash.
///
/// The obvious rule, "the last dash-separated segment is at least
/// [`MIN_HASH_LEN`] hash characters", is WRONG, and a real build proves it: the
/// bundler emits `TerminalPane-BrP-ENHg.css`, whose 8-character hash `BrP-ENHg`
/// contains a dash of its own. Splitting at the last dash sees `ENHg`, four
/// characters, and rejects a perfectly good asset.
///
/// That matters more than a missed chunk. `hashed_asset_refs` used the last-dash
/// rule, so an entry bundle whose hash happened to contain a dash was invisible to
/// the real-build gate, and the gate would have reported a genuine build as a
/// placeholder. The hash alphabet is base64url, so a dash is not a rare accident;
/// it is one character out of sixty-four, in every hash, in every build.
///
/// So: try each dash from the right and accept the first suffix that qualifies.
/// This is deliberately a shape check and deliberately loose (a hand-written
/// `my-component-library.js` would satisfy it). The caller closes that gap by
/// FETCHING what it finds, which a name alone can never establish.
fn looks_hashed(stem: &str) -> bool {
    stem.match_indices('-').rev().any(|(idx, _)| {
        let suffix = &stem[idx + 1..];
        suffix.chars().count() >= MIN_HASH_LEN
            && suffix
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    })
}

fn hashed_asset_refs(html: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut rest = html;
    while let Some(at) = rest.find("assets/") {
        rest = &rest[at..];
        let end = rest
            .find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '>' | ')' | '(' | '`'))
            .unwrap_or(rest.len());
        let (candidate, tail) = rest.split_at(end);
        rest = if tail.is_empty() { "" } else { &tail[1..] };
        let Some(file) = candidate.strip_prefix("assets/") else {
            continue;
        };
        // Nested asset directories are fine; the hash lives in the last segment.
        let file = file.rsplit('/').next().unwrap_or(file);
        let Some((stem, _ext)) = file.rsplit_once('.') else {
            continue;
        };
        if looks_hashed(stem) {
            found.push(candidate.to_string());
        }
    }
    found
}

/// Every hashed sibling chunk the given JavaScript bundle imports.
///
/// This is the half the page-level check cannot see. Vite/rolldown writes a lazy
/// `import()` target as a path RELATIVE to the importing chunk, so
/// `assets/index-<hash>.js` refers to the terminal emulator as
/// `` `./TerminalPane-<hash>.js` `` and to the editor's viewers as
/// `` `./DiffViewer-<hash>.js` `` and `` `./MarkdownPreview-<hash>.js` ``. None of
/// those names appear in `index.html` at all. Measured against a real build, not
/// assumed: grepping the entry bundle for `assets/` finds ZERO matches, while the
/// relative form finds 90 distinct chunk names.
///
/// So a dist missing exactly those chunks still serves a page that references a
/// hashed bundle, still serves that bundle, and still completes a websocket
/// handshake. It goes blank the moment the user opens a terminal or the editor,
/// which is the failure both real-build checks were blind to.
///
/// A reference is a string literal in emitted JavaScript, so a delimiter is part
/// of the pattern; that is what keeps a stray word in a comment or a string of
/// prose from being mistaken for a chunk. Which delimiter, though, is not a
/// choice this function gets to make, and getting that wrong is THE FINDING this
/// version exists to repair. Matching only `"./` saw 8 of the 90 names in the
/// entry bundle, because the bundler writes 85 of them as BACKTICK template
/// literals; deleting the 88 assets the walk therefore never reached left both
/// real-build gates passing on a dist whose editor was entirely gone. So all
/// three JavaScript string delimiters count, and the leading `./` is optional,
/// because the editor's web worker is built from `` new Worker(""+new
/// URL(`editor.worker-<hash>.js`,import.meta.url)) `` with no prefix at all.
///
/// Deliberately NOT implemented by pairing delimiters off against each other, and
/// that is measured rather than fastidious: a pairing version of this function
/// (open quote, next quote of the same kind, repeat) still found only 6 chunks in
/// the real entry bundle, because 4.3 MB of minified JavaScript is full of
/// apostrophes and backticks inside other strings and regexes, and one of them
/// flips the parity for everything after it. Instead each delimiter OCCURRENCE is
/// treated as a possible opener and the filename shape is matched directly after
/// it, which is what the sibling shell implementation's regex does too.
///
/// Loosening the delimiter loosens nothing else: the candidate must still be a
/// bare filename with a `.js`/`.css` extension and a hash-shaped stem.
fn chunk_refs(js: &str) -> Vec<String> {
    let mut hits: Vec<(usize, &str)> = Vec::new();
    for delim in ['"', '\'', '`'] {
        for (open, _) in js.match_indices(delim) {
            let after = &js[open + delim.len_utf8()..];
            let after = after.strip_prefix("./").unwrap_or(after);
            let end = after
                .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')))
                .unwrap_or(after.len());
            // The literal has to CLOSE right here, or this run of filename-shaped
            // characters is part of something longer and is not a chunk name.
            if !after[end..].starts_with(delim) {
                continue;
            }
            let file = &after[..end];
            let Some((stem, ext)) = file.rsplit_once('.') else {
                continue;
            };
            if ext != "js" && ext != "css" {
                continue;
            }
            if looks_hashed(stem) {
                hits.push((open, file));
            }
        }
    }
    hits.sort_by_key(|(at, _)| *at);
    let mut found: Vec<String> = Vec::new();
    for (_, file) in hits {
        if !found.iter().any(|f| f == file) {
            found.push(file.to_string());
        }
    }
    found
}

/// The exact placeholder `build.rs` used to write on a failed frontend build, kept
/// verbatim so the assertions below are about the real thing and not a paraphrase.
const HISTORICAL_PLACEHOLDER: &str = "<!doctype html><title>dux</title><div id=\"root\">web assets not built - run npm run build in crates/dux-web/web</div>";

#[test]
fn doctype_and_root_element_are_not_evidence_of_a_build() {
    // THE FINDING, pinned. The assertion this suite used to make passes happily on
    // the placeholder, which is why a placeholder release would have been green.
    let placeholder = HISTORICAL_PLACEHOLDER.to_lowercase();
    assert!(
        placeholder.contains("<!doctype html") && placeholder.contains("id=\"root\""),
        "the placeholder satisfies both halves of the old assertion, so the old \
         assertion could never have caught it"
    );
    // The real check rejects it, because it references no built bundle.
    assert!(
        hashed_asset_refs(HISTORICAL_PLACEHOLDER).is_empty(),
        "the placeholder must not look like a build"
    );
}

#[test]
fn hashed_asset_refs_finds_vite_bundles_and_ignores_hand_named_files() {
    let real = r#"<!doctype html><html><head>
        <link rel="icon" type="image/png" href="./favicon.png" />
        <script type="module" crossorigin src="./assets/index-x8kEp4D8.js"></script>
        <link rel="modulepreload" crossorigin href="./assets/rolldown-runtime-QTnfLwEv.js">
        <link rel="stylesheet" crossorigin href="./assets/index-TXcOh_oH.css">
        <img src="./assets/dux-logo.png">
        </head><body><div id="root"></div></body></html>"#;
    assert_eq!(
        hashed_asset_refs(real),
        vec![
            "assets/index-x8kEp4D8.js".to_string(),
            "assets/rolldown-runtime-QTnfLwEv.js".to_string(),
            "assets/index-TXcOh_oH.css".to_string(),
        ],
        "the hashed bundle entries must be found and the hand-named logo skipped"
    );
    // Names a human chose, of the shape a notice page or a static site would use.
    assert!(hashed_asset_refs(r#"<img src="assets/logo.png">"#).is_empty());
    assert!(hashed_asset_refs(r#"<script src="assets/main.js">"#).is_empty());
    assert!(hashed_asset_refs(r#"<script src="assets/app-v2.js">"#).is_empty());
    assert!(hashed_asset_refs("no assets at all").is_empty());
}

#[test]
fn a_hash_containing_a_dash_is_still_a_hash() {
    // Taken from a real build: `BrP-ENHg` is an 8-character base64url hash with a
    // dash in it. Splitting the stem at the LAST dash sees `ENHg`, decides four
    // characters cannot be a hash, and throws the asset away.
    //
    // This is not a curiosity. The hash alphabet includes `-`, so a dash lands in
    // roughly one hash in eight, every build. While the entry bundle's own hash
    // was clean the gate worked; the first build that dealt `index-<a-b>.js` would
    // have reported a real frontend build as a placeholder.
    assert_eq!(
        hashed_asset_refs(r#"<link href="assets/TerminalPane-BrP-ENHg.css">"#),
        vec!["assets/TerminalPane-BrP-ENHg.css".to_string()]
    );
    assert_eq!(
        hashed_asset_refs(r#"<script src="assets/index-x8k-p4D8.js">"#),
        vec!["assets/index-x8k-p4D8.js".to_string()]
    );
    // And the short hand-written names are still rejected, dash or no dash.
    assert!(hashed_asset_refs(r#"<img src="assets/dux-logo.png">"#).is_empty());
    assert!(hashed_asset_refs(r#"<img src="assets/a-b-c.png">"#).is_empty());
}

/// The smallest the whole embedded bundle graph can plausibly be.
///
/// Chosen from a MEASUREMENT, not a feeling: a real build's closure is about
/// 5.5 MB (a 4.3 MB entry chunk, a 270 KB stylesheet, a 190 KB React vendor
/// chunk, a 429 KB terminal chunk and a 336 KB markdown chunk). 512 KiB leaves an
/// order of magnitude of headroom for the app shrinking, while a set of stub
/// files could not approach it.
///
/// This is the aggregate gate. A PER-ASSET floor cannot be raised much, and that
/// is also measured: `rolldown-runtime-<hash>.js` is a legitimate 694-byte chunk
/// referenced straight from `index.html`, so a per-file floor of even 1 KB would
/// fail a perfectly good build.
const MIN_BUNDLE_TOTAL_BYTES: usize = 512 * 1024;

/// The smallest number of lazily loaded chunks a real build can plausibly name.
///
/// `followed_chunks > 0` was the canary, and it was useless: the reviewer deleted
/// 88 of the 98 files in `assets/` and this walk still passed, because the
/// double-quote-only `chunk_refs` only ever reached 10 of them and followed 6.
/// A threshold of one is satisfied by whatever happens to survive.
///
/// Measured on a real build of this app (2026-07): the walk starts from the 4
/// bundles `index.html` names and follows 88 further chunks, reaching 92 of the
/// 98 files in `assets/` (the 6 it does not reach are fonts, referenced from CSS,
/// which this walk does not parse). 40 leaves more than a factor of two of
/// headroom for the app shedding code-split chunks, while sitting far above
/// anything a stub dist or a broken matcher could reach.
const MIN_FOLLOWED_CHUNKS: usize = 40;

#[test]
fn chunk_refs_finds_lazy_imports_the_page_never_mentions() {
    // The literal shapes a real entry bundle contains, taken from one: an
    // `import()` target and a preload-manifest array entry, both relative.
    let entry = r#"
        const d=(m.f||(m.f=["./DiffViewer-CenXBp36.js","./TerminalPane-BrP-ENHg.css"]));
        Zy(()=>jh(()=>import("./TerminalPane-Clml_YOa.js")));
        Zy(()=>jh(()=>import("./MarkdownPreview-CbmmEjdW.js")));
    "#;
    assert_eq!(
        chunk_refs(entry),
        vec![
            "DiffViewer-CenXBp36.js".to_string(),
            "TerminalPane-BrP-ENHg.css".to_string(),
            "TerminalPane-Clml_YOa.js".to_string(),
            "MarkdownPreview-CbmmEjdW.js".to_string(),
        ],
        "the lazily imported chunks must be found, in order, without duplicates"
    );

    // A repeat of the same chunk is reported once.
    assert_eq!(
        chunk_refs(r#"import("./a-ABCDEFGH.js");import("./a-ABCDEFGH.js")"#),
        vec!["a-ABCDEFGH.js".to_string()]
    );
    // Names a human chose, and things that merely look similar.
    assert!(chunk_refs(r#"import("./helper.js")"#).is_empty());
    assert!(chunk_refs(r#"import("./app-v2.js")"#).is_empty());
    assert!(chunk_refs(r#"fetch("./api/thing-ABCDEFGH.js")"#).is_empty());
    assert!(chunk_refs(r#"const s="./styles-ABCDEFGH.scss""#).is_empty());
    assert!(chunk_refs("no imports at all").is_empty());
    // The page-level form is NOT this form, which is the whole point.
    assert!(chunk_refs(r#"<script src="assets/index-x8kEp4D8.js">"#).is_empty());
}

#[test]
fn chunk_refs_reads_backticks_and_a_missing_leading_dot_slash() {
    // THE FINDING. Matching only `"./` saw 8 of the 90 chunk names the real entry
    // bundle carries: the bundler writes the overwhelming majority as BACKTICK
    // template literals, and the editor's web worker is constructed from a URL
    // with no `./` prefix at all. Both shapes below are copied from a real build.
    assert_eq!(
        chunk_refs(r#"Zy(()=>jh(()=>import(`./DiffViewer-CenXBp36.js`)));"#),
        vec!["DiffViewer-CenXBp36.js".to_string()],
        "a backtick template literal is the form the bundler actually emits"
    );
    assert_eq!(
        chunk_refs(r#"new Worker(""+new URL(`editor.worker-Bo1cU3Rq.js`,import.meta.url))"#),
        vec!["editor.worker-Bo1cU3Rq.js".to_string()],
        "a worker URL carries no leading ./ and must still be followed"
    );
    assert_eq!(
        chunk_refs(r#"const a='./css-mode-DEadBeEf.js'"#),
        vec!["css-mode-DEadBeEf.js".to_string()],
        "single quotes are a legal JavaScript string delimiter too"
    );
    // Order is DOCUMENT order across all three delimiters, not one delimiter's
    // matches followed by another's.
    assert_eq!(
        chunk_refs(
            r#"import(`./a-AAAAAAAA.js`);import("./b-BBBBBBBB.js");import('./c-CCCCCCCC.js')"#
        ),
        vec![
            "a-AAAAAAAA.js".to_string(),
            "b-BBBBBBBB.js".to_string(),
            "c-CCCCCCCC.js".to_string(),
        ]
    );
    // Loosening the delimiters must not loosen anything else: a hand-chosen name,
    // a nested path and a non-bundle extension are still rejected.
    assert!(chunk_refs("const h=`./helper.js`").is_empty());
    assert!(chunk_refs("const n=`./nested/thing-ABCDEFGH.js`").is_empty());
    assert!(chunk_refs("const s=`./styles-ABCDEFGH.scss`").is_empty());
}

#[tokio::test]
async fn index_references_a_real_hashed_asset_that_is_actually_served() {
    // The one assertion a placeholder cannot satisfy: the page must reference a
    // content-hashed bundle, and that bundle must actually be embedded. A doctype
    // and a root element prove nothing (see the test above).
    //
    // It walks the whole graph rather than the page's own references, because the
    // terminal emulator and the editor's viewers are reachable ONLY from inside
    // the entry bundle (see `chunk_refs`). Checking the page alone passes on a
    // dist that goes blank as soon as the user opens a terminal.
    require_real_ui_build!("index_references_a_real_hashed_asset_that_is_actually_served");

    let (_tmp, app) = test_router();
    let resp = get(app.clone(), "/").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    // No Accept-Encoding was sent, so the handler inflates the build-time gzip for
    // us and this is the real HTML either way.
    let html = String::from_utf8_lossy(&bytes).to_string();

    let seeds = hashed_asset_refs(&html);
    assert!(
        !seeds.is_empty(),
        "the embedded index.html references no content-hashed bundle, so it is not a \
         real frontend build. Run `npm run build` in crates/dux-web/web and rebuild. \
         Served page:\n{html}"
    );

    // Breadth-first over the reference graph. Every asset the page names must
    // resolve, and so must every chunk those assets import: a build that names
    // three chunks and can serve one is broken too.
    let mut queue = seeds.clone();
    let mut seen: Vec<String> = seeds.clone();
    let mut total = 0usize;
    let mut followed_chunks = 0usize;

    while let Some(asset) = queue.pop() {
        let resp = get(app.clone(), &format!("/{asset}")).await;
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "the build references /{asset} but the binary does not serve it. A chunk \
             that 404s is a blank screen the moment the feature behind it is opened."
        );
        let ctype = header(&resp, "content-type").unwrap_or_default();
        assert!(
            !ctype.contains("html"),
            "/{asset} was served as {ctype}; the SPA fallback answered instead of the asset"
        );
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        assert!(
            body.len() > 64,
            "/{asset} is only {} bytes, which is not a real bundle",
            body.len()
        );
        total += body.len();

        if !asset.ends_with(".js") {
            continue;
        }
        for chunk in chunk_refs(&String::from_utf8_lossy(&body)) {
            let path = format!("assets/{chunk}");
            if !seen.contains(&path) {
                seen.push(path.clone());
                queue.push(path);
                followed_chunks += 1;
            }
        }
    }

    assert!(
        followed_chunks >= MIN_FOLLOWED_CHUNKS,
        "only {followed_chunks} lazily loaded chunk(s) were discovered inside the \
         bundles, under the floor of {MIN_FOLLOWED_CHUNKS}. The app code-splits the \
         terminal emulator, the editor and every Monaco language, so finding this \
         few means either the build is not a real one, chunks are missing from the \
         dist, or `chunk_refs` has stopped matching what the bundler emits. Assets \
         reached: {} of them, {seen:?}",
        seen.len()
    );

    assert!(
        total >= MIN_BUNDLE_TOTAL_BYTES,
        "the whole embedded bundle graph is only {total} bytes across {} assets, \
         under the {MIN_BUNDLE_TOTAL_BYTES}-byte floor. That is stub territory, not \
         a real build of this app.",
        seen.len()
    );
}

/// The smallest number of content-hashed `assets/*.{js,css}` files a real build
/// can plausibly EMBED.
///
/// Measured on a real build of this app (2026-07): `web/dist/assets` holds 92
/// hashed `.js`/`.css` files. 40 leaves more than a factor of two of headroom for
/// the app shedding code-split chunks, and matches the floor
/// [`MIN_FOLLOWED_CHUNKS`] uses for the reference walk, while sitting far above
/// anything an empty or stubbed embed could reach.
const MIN_HASHED_EMBEDDED_ASSETS: usize = 40;

#[test]
fn the_embedded_asset_set_is_a_whole_frontend_build() {
    // The check every other gate in this file makes only INDIRECTLY. The rest
    // fetch a page through the router and happen to fail when the embed is empty,
    // which reports a 404 and says nothing about why. This one asserts on
    // `WebAssets` itself, so it holds regardless of where rust-embed's `folder`
    // points and it names the cause.
    //
    // It is the in-process twin of the static grep in
    // `.github/scripts/smoke_archive.sh`, and it is what would have caught the
    // defect this file's `folder` change repairs: emptying `web/dist` re-ran the
    // build script ZERO times, rust-embed baked in nothing, and the server
    // answered 404 at the root with no warning anywhere.
    //
    // The byte floor is compared against the EMBEDDED bytes, which are gzipped for
    // the text assets, so it is measuring less than the raw dist does. A real
    // build embeds about 2.3 MB that way, four times the floor.
    require_real_ui_build!("the_embedded_asset_set_is_a_whole_frontend_build");

    let mut hashed = Vec::new();
    let mut total = 0usize;
    let mut files = 0usize;
    for name in dux_web::web_assets::WebAssets::iter() {
        let Some(file) = dux_web::web_assets::WebAssets::get(&name) else {
            panic!("{name} is listed in the embed but cannot be fetched from it");
        };
        files += 1;
        total += file.data.len();
        let Some(rest) = name.strip_prefix("assets/") else {
            continue;
        };
        let Some((stem, ext)) = rest.rsplit_once('.') else {
            continue;
        };
        if (ext == "js" || ext == "css") && looks_hashed(stem) {
            hashed.push(name.to_string());
        }
    }

    assert!(
        hashed.len() >= MIN_HASHED_EMBEDDED_ASSETS,
        "only {} content-hashed assets/*.js|css files are embedded in this binary, \
         under the floor of {MIN_HASHED_EMBEDDED_ASSETS} ({files} embedded files in \
         total). The web UI is not baked into this binary, so server mode will 404 \
         at the root. Run `touch crates/dux-web/web/index.html` and rebuild to force \
         the frontend build script to run.",
        hashed.len()
    );
    assert!(
        total >= MIN_BUNDLE_TOTAL_BYTES,
        "the embedded asset set is only {total} bytes across {files} files, under the \
         {MIN_BUNDLE_TOTAL_BYTES}-byte floor. That is stub territory, not a real \
         build of this app."
    );
}

#[test]
fn the_embed_folder_resolves_to_something() {
    // Deliberately NO skip guard: this is the unconditional canary that the
    // staged tree build.rs writes is NON-EMPTY, and it holds on every path that
    // script can take. On a real build the staged copy is the whole frontend; on
    // the notice-page route build.rs writes its page into `dist` and stages that;
    // on the REUSE route nothing is written at all, but that route is only chosen
    // when `dist/index.html` already exists, so there is something to stage
    // either way. An empty embed is a 404 at the root with nothing said anywhere,
    // which is the defect this layout was changed to avoid, so it gets a cheap
    // assertion that runs in every configuration.
    //
    // What this does NOT do, despite an earlier version of this comment saying
    // so, is pin the rust-embed `interpolate-folder-path` feature. That feature
    // is enforced by rust-embed itself, at COMPILE time, so no runtime assertion
    // could reach it: read against the pinned 8.11.0, the `$OUT_DIR` expansion in
    // `rust-embed-impl` is `#[cfg(feature = "interpolate-folder-path")]`, and
    // without it the literal string `$OUT_DIR/ui` is RELATIVE, so it is joined
    // onto `CARGO_MANIFEST_DIR` and yields a path containing the placeholder
    // text. That path does not exist, and the derive returns an error naming it
    // (plus a hint about the feature). There is no "resolves somewhere that
    // exists and is empty" branch to guard against; dropping the feature simply
    // fails to build.
    assert!(
        dux_web::web_assets::WebAssets::iter().next().is_some(),
        "the binary embeds ZERO files, so server mode will 404 at the root. \
         $OUT_DIR/ui exists (a missing folder is a compile error, so this binary \
         would not have built) but is empty, which means build.rs staged nothing \
         into it. Run `touch crates/dux-web/web/index.html` and rebuild, or \
         `cargo clean -p dux-web` and rebuild, to make the staging step run again."
    );
}

#[tokio::test]
async fn served_index_is_not_the_ui_not_built_notice() {
    // The inverse framing, so a regression that reintroduces a placeholder-on-
    // failure path fails here even if it changes the page's wording enough to
    // dodge the hashed-asset check.
    require_real_ui_build!("served_index_is_not_the_ui_not_built_notice");

    let (_tmp, app) = test_router();
    let resp = get(app, "/").await;
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = String::from_utf8_lossy(&bytes).to_lowercase();
    for marker in [
        "not built",
        "web assets not built",
        "dux_disable_ui_build",
        "npm run build",
    ] {
        assert!(
            !html.contains(marker),
            "the served page contains {marker:?}, so it is a not-built notice rather \
             than the web UI"
        );
    }
}

/// The composite "a real server on a real port serves the real page AND accepts a
/// websocket handshake", which is the in-repo mirror of the release workflow's
/// archive smoke test.
///
/// This is NOT a duplicate of the existing websocket coverage. The handshake alone
/// is well covered (`serve_with_engine.rs` and `first_load_routes.rs` both await a
/// `connected` frame on `/ws/events`, and `tab_routes.rs` opens per-PTY sockets),
/// and every other test in THIS file drives the router through `tower::oneshot`,
/// which never binds a socket. What nothing covered is the pair together against
/// one live listener: a build whose page is a placeholder, or a router where the
/// static fallback swallows `/ws/...`, is exactly the shape that survives when the
/// page and the socket are only ever checked apart. The release workflow checks the
/// same three things on the shipping archive; this test makes the PR fail first.
#[tokio::test]
async fn live_server_serves_the_built_page_and_accepts_a_websocket() {
    use std::net::SocketAddr;

    use futures_util::StreamExt;

    require_real_ui_build!("live_server_serves_the_built_page_and_accepts_a_websocket");

    let (_tmp, paths) = temp_paths();
    let engine = bootstrap_engine(&paths).unwrap();
    let (handle, _join) = spawn_engine_thread(engine);
    let app = router(handle);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await;
    });

    // 1. The page loads over real HTTP.
    let resp = reqwest::get(format!("http://{addr}/"))
        .await
        .expect("GET /");
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let html = resp.text().await.expect("index body");

    // 2. It references a content-hashed bundle, that bundle really downloads, and
    // so does every chunk it imports. Over real HTTP this time, so a route or
    // encoding problem that only appears on the wire is caught as well.
    let seeds = hashed_asset_refs(&html);
    assert!(
        !seeds.is_empty(),
        "the live server served a page with no hashed bundle reference:\n{html}"
    );
    let mut queue = seeds.clone();
    let mut seen = seeds.clone();
    let mut total = 0usize;
    while let Some(asset) = queue.pop() {
        let asset_resp = reqwest::get(format!("http://{addr}/{asset}"))
            .await
            .expect("GET asset");
        assert_eq!(
            asset_resp.status(),
            reqwest::StatusCode::OK,
            "the live server does not serve /{asset}, which the build references"
        );
        let body = asset_resp.bytes().await.expect("asset body");
        assert!(
            body.len() > 64,
            "/{asset} came back too small to be a real bundle"
        );
        total += body.len();
        if !asset.ends_with(".js") {
            continue;
        }
        for chunk in chunk_refs(&String::from_utf8_lossy(&body)) {
            let path = format!("assets/{chunk}");
            if !seen.contains(&path) {
                seen.push(path.clone());
                queue.push(path);
            }
        }
    }
    assert!(
        total >= MIN_BUNDLE_TOTAL_BYTES,
        "the live server's whole bundle graph is only {total} bytes, under the \
         {MIN_BUNDLE_TOTAL_BYTES}-byte floor"
    );

    // 3. The websocket handshake completes on the same server and the engine
    // answers, so the static fallback has not swallowed the socket route.
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws/events"))
        .await
        .expect("websocket handshake on /ws/events");
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut saw_connected = false;
    while !saw_connected && tokio::time::Instant::now() < deadline {
        if let Ok(Some(Ok(message))) =
            tokio::time::timeout(std::time::Duration::from_millis(300), ws.next()).await
            && let Ok(text) = message.into_text()
            && text.contains("\"event\":\"connected\"")
        {
            saw_connected = true;
        }
    }
    assert!(
        saw_connected,
        "the websocket connected but never delivered the `connected` frame"
    );
}
