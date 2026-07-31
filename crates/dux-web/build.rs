//! Builds the React frontend in `web/` and leaves the result in `web/dist` for
//! `rust_embed` to bake into the binary.
//!
//! ## Failure policy
//!
//! A frontend build that is ATTEMPTED and fails is FATAL: this script panics and
//! `cargo build` fails. It used to write a placeholder page, print a
//! `cargo:warning` and succeed, which meant a release could ship four platform
//! binaries containing "web assets not built" with every check green. That path
//! is gone: nothing here turns a FAILURE into a successful build.
//!
//! ## Escape hatch
//!
//! Rust-only contributors (and machines with no Node toolchain) set
//! [`DISABLE_ENV`] to any non-empty value. The frontend build is then not
//! attempted at all and the Rust build succeeds. Skipping deliberately is
//! supported; failing silently is not.
//!
//! ## Marking the binary, in both skip cases
//!
//! Skipping has two outcomes, and BOTH mark the binary, because in neither case
//! did a frontend build happen and in neither case can the user tell by looking:
//!
//! * **No previously built `dist/`.** This script writes a plain notice page (see
//!   `NOT_BUILT_PAGE`) and sets `cargo:rustc-env=DUX_UI_BUILD_STATE=not_built`.
//! * **A previously built `dist/` is present.** It is left alone and embedded as
//!   it is, and this script sets `DUX_UI_BUILD_STATE=stale`.
//!
//! The second case used to set nothing at all. The result was a binary serving an
//! arbitrarily old interface, with real hashed assets, that passed every test
//! including the real-build ones, showed no banner row and wrote nothing to the
//! log. A single build-time `cargo:warning` was the only trace, and nobody reads
//! build output from last week.
//!
//! The state is spelled out rather than collapsed into one flag because the
//! operator-facing message differs and the difference matters: the notice-page
//! binary contains NO web UI, while the reuse binary contains a REAL one that may
//! simply be old. Collapsing them would make the banner assert something false in
//! whichever case it was not written for. `web_assets::ui_build_state` reads it
//! back, the `dux server` startup banner turns the state into the matching
//! warning row, and the static-serving tests use it to SKIP with a printed reason
//! instead of passing on a page that is not a build.
//!
//! ## Why the SUCCESS path emits a marker too
//!
//! It has nothing to say, and it says it anyway, because `option_env!` reads the
//! AMBIENT rustc environment and not only what this script emits. The earlier
//! scheme used two markers and emitted NEITHER on success, so an ambient
//! `DUX_UI_BUILD_SKIPPED=1` (a workflow-level `env:`, say) was never overridden
//! and made every real-build test print SKIPPED on a genuine build, with the CI
//! guard, which only ever looked at `DUX_DISABLE_UI_BUILD`, green throughout. A
//! `cargo:rustc-env` always beats an ambient value of the same name (measured
//! with a throwaway crate, not assumed), so writing the marker on EVERY path is
//! what makes it unspoofable.

use std::io::Write;
use std::path::Path;
use std::process::Command;

use flate2::Compression;
use flate2::write::GzEncoder;

/// Set this to any non-empty value to skip the frontend build entirely.
const DISABLE_ENV: &str = "DUX_DISABLE_UI_BUILD";

/// The marker this script stamps into the binary on every path it can take, read
/// back by `web_assets::ui_build_state`. Emitted even on success; see the module
/// docs for why silence there was spoofable.
const STATE_ENV: &str = "DUX_UI_BUILD_STATE";

/// Stamp the UI build state into the binary. `cargo:rustc-env` overrides any
/// ambient value of the same name, which is the whole point of calling this on
/// the success path as well as the two skip paths.
fn mark_state(state: &str) {
    println!("cargo:rustc-env={STATE_ENV}={state}");
}

/// Sentinel embedded in `NOT_BUILT_PAGE` so a LATER build can tell "this dist is
/// my own notice page" from "this dist is a real build".
///
/// Without it, the second consecutive hatch build finds the `index.html` the
/// first one wrote, concludes a real dist is present, and stops setting
/// `DUX_UI_BUILD_SKIPPED`. The tests then ran against the notice page and FAILED
/// instead of skipping. That is not a hypothetical: the first implementation of
/// this file used no sentinel, and a second `DUX_DISABLE_UI_BUILD=1 cargo test`
/// proved it. A sentinel inside the page is preferable to a marker file, which
/// `rust_embed` would either serve as a stray asset or require its
/// `include-exclude` feature (and a `globset` dependency) to filter out.
const NOT_BUILT_SENTINEL: &str = "dux-ui-not-built-notice";

/// The page embedded when the frontend build was deliberately skipped and there
/// is no previously built `dist/` to embed. A user who reaches the server in a
/// browser must be told what happened and how to fix it rather than staring at a
/// blank page.
///
/// Deliberately NOT the SPA shell: it carries no `id="root"` and no hashed asset
/// reference. Be precise about what that does and does not buy, because an
/// earlier version of this comment claimed the page carries nothing a test could
/// mistake for a build, and two tests in `tests/static_serving.rs` were at that
/// moment asserting only `<!doctype html> OR id="root"`, which this page
/// satisfies, since it is a real HTML document with a doctype. What the missing
/// root element and missing hashed reference defeat is a test that checks for
/// THOSE; they cannot defeat a test that checks for a doctype. The tests were
/// corrected rather than the page, and the sentence is now scoped to what it can
/// actually support.
const NOT_BUILT_PAGE: &str = r#"<!doctype html>
<!-- dux-ui-not-built-notice -->
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>dux: web UI not built</title>
    <style>
      :root { color-scheme: dark light; }
      body {
        margin: 0; min-height: 100vh; display: grid; place-items: center;
        padding: 2rem; background: #0a0a0a; color: #e5e5e5;
        font: 15px/1.6 ui-sans-serif, system-ui, sans-serif;
      }
      main { max-width: 40rem; }
      h1 { font-size: 1.35rem; margin: 0 0 1rem; }
      code, pre { font-family: ui-monospace, SFMono-Regular, Menlo, monospace; }
      pre {
        background: #171717; border: 1px solid #262626; border-radius: 6px;
        padding: 0.75rem 1rem; overflow-x: auto;
      }
      p { margin: 0 0 1rem; }
      .muted { color: #a3a3a3; }
    </style>
  </head>
  <body>
    <main>
      <h1>The dux web UI was not built into this binary</h1>
      <p>
        This binary was compiled with <code>DUX_DISABLE_UI_BUILD</code> set, so the
        React frontend was never built and there is nothing to serve here. The
        terminal UI (<code>dux</code> with no arguments) is unaffected.
      </p>
      <p>To get the web UI, rebuild without the escape hatch:</p>
      <pre>cd crates/dux-web/web &amp;&amp; npm ci
cargo build --release</pre>
      <p class="muted">
        Installed dux from a release archive, npm, or the install script and still
        seeing this page? That is a packaging bug, not something you can fix
        locally. Please report it.
      </p>
    </main>
  </body>
</html>
"#;

/// Text asset extensions worth gzipping. Binary assets (fonts, images, wasm) are
/// already compressed, so they're left raw.
const COMPRESSIBLE: &[&str] = &[
    "js",
    "css",
    "html",
    "json",
    "svg",
    "webmanifest",
    "txt",
    "map",
];

fn main() {
    let web = Path::new("web");
    println!("cargo:rerun-if-changed=web/src");
    println!("cargo:rerun-if-changed=web/public");
    println!("cargo:rerun-if-changed=web/index.html");
    println!("cargo:rerun-if-changed=web/package.json");
    println!("cargo:rerun-if-changed=web/package-lock.json");
    println!("cargo:rerun-if-changed=web/vite.config.ts");
    // The rest of the build's inputs. `npm run build` runs tsc before Vite, so a
    // tsconfig edit (a compiler target, a path alias, strictness) changes the
    // output; the lint config is part of what `npm run build` can fail on; and
    // web/scripts holds the helper scripts package.json invokes. Without these,
    // editing a compiler target and rebuilding left the OLD bundle embedded,
    // which looks exactly like the change having no effect.
    println!("cargo:rerun-if-changed=web/tsconfig.json");
    println!("cargo:rerun-if-changed=web/tsconfig.app.json");
    println!("cargo:rerun-if-changed=web/tsconfig.node.json");
    println!("cargo:rerun-if-changed=web/eslint.config.js");
    // KNOWN GAP, deliberately not "fixed": emptying web/dist WITHOUT touching a
    // source file leaves this script un-run, rust-embed then bakes in zero files,
    // and the server answers 404 at the root with no warning anywhere. Measured,
    // not theorised: `rm -rf web/dist/*` followed by `cargo build -p dux-web`
    // re-runs this script ZERO times and leaves dist empty.
    //
    // The obvious repair, `cargo:rerun-if-changed=web/dist`, is WORSE, and that
    // was measured too: it re-runs this script on EVERY build, forever. Cargo
    // creates its `invoked.timestamp` BEFORE running a build script, so any file
    // the script itself writes under a watched path is newer than the reference
    // the next build compares against, and the path is permanently dirty. Both
    // Vite and the gzip step below write all over web/dist, so watching it means
    // running `npm run build` on every single cargo invocation. Preserving mtimes
    // across the gzip does not save it either, because the Vite run inside the
    // same script rewrites the files regardless.
    //
    // Watching a generated directory is simply not what rerun-if-changed is for.
    // The real fix is to stop generating into the watched tree at all: stage the
    // gzipped output under OUT_DIR and point rust-embed's `folder` there, leaving
    // web/dist as pure Vite output. That is a layout change spanning this file and
    // web_assets.rs, so it is written down here rather than smuggled in.
    //
    // Until then: if the root 404s, run `touch crates/dux-web/web/index.html` (or
    // any web source) and rebuild.
    println!("cargo:rerun-if-changed=web/scripts");
    // Without this, cargo caches the build-script result and toggling the hatch
    // appears to do nothing: the previous `DUX_UI_BUILD_STATE` sticks across
    // builds. Verified by probe, not assumed.
    println!("cargo:rerun-if-env-changed={DISABLE_ENV}");

    let dist = web.join("dist");
    let dist_index = dist.join("index.html");

    if hatch_set() {
        skip_frontend_build(&dist, &dist_index);
        gzip_dist(&dist);
        return;
    }

    // Always (re)build when this script runs. The `rerun-if-changed` lines above
    // gate WHEN cargo re-runs it (the first build and whenever the web sources
    // change) so the embedded site is never stale, while Rust-only rebuilds skip
    // this entirely. node_modules persists, so steady-state cost is one fast Vite
    // build only when the frontend actually changed.
    //
    // Install dependencies when the lockfile is out of sync with what's on disk,
    // not only when `node_modules` is missing entirely. A `git pull` that adds a
    // frontend dependency leaves the existing `node_modules` in place but stale;
    // without this the next `tsc`/Vite build fails with "cannot find module".
    // `npm ci` is clean and reproducible; fall back to `npm install` if the
    // lockfile itself is stale.
    if deps_stale(web)
        && run(web, "npm", &["ci"]).is_none()
        && run(web, "npm", &["install"]).is_none()
    {
        fail(
            "installing the frontend dependencies failed (`npm ci` and `npm install` both failed)",
        );
    }
    if run(web, "npm", &["run", "build"]).is_none() {
        fail("the frontend build failed (`npm run build`)");
    }
    if !dist_index.exists() {
        fail("the frontend build reported success but left no web/dist/index.html");
    }

    // A real build. Say so explicitly: an ambient DUX_UI_BUILD_STATE would
    // otherwise reach `option_env!` unopposed and could talk the test suite into
    // skipping on a genuine build. See the module docs.
    mark_state("built");

    // Gzip the text assets IN PLACE so rust-embed bakes the compressed bytes into
    // the binary (and `web_assets` serves them with `Content-Encoding: gzip`).
    // Runs after the Vite build (which writes raw files); idempotent via the gzip
    // magic-byte check, so an already-compressed dist isn't double-compressed.
    gzip_dist(&dist);
}

/// Whether the escape hatch is engaged. ANY non-empty value counts as set, so
/// `DUX_DISABLE_UI_BUILD=1`, `=true` and `=please` all skip the build; an empty
/// value is treated as unset so `DUX_DISABLE_UI_BUILD=` does not surprise anyone.
fn hatch_set() -> bool {
    std::env::var_os(DISABLE_ENV).is_some_and(|value| !value.is_empty())
}

/// Abort the Rust build, naming what to run. This is the whole point of the
/// change: a frontend build that was attempted and failed must not produce a
/// binary. `panic!` is how a build script fails, and cargo prints the message.
fn fail(what: &str) -> ! {
    panic!(
        "dux-web: {what}.\n\
         The web UI is compiled into the dux binary, so this is a hard build failure \
         rather than a warning.\n\
         To see the npm/tsc/Vite error, run: cargo build -vv -p dux-web\n\
         To reproduce it directly, run: cd crates/dux-web/web && npm ci && npm run build\n\
         If you only work on the Rust side and do not need the web UI, set \
         {DISABLE_ENV}=1 to skip the frontend build (the binary then serves a \
         \"web UI not built\" notice page instead of the app)."
    );
}

/// The deliberate-skip path. Nothing failed here, so an already-built `dist/` is
/// left exactly as it is and embedded: destroying a good build would be worse
/// than embedding one that may be a few commits stale. With no `dist/` at all,
/// write the notice page.
///
/// Either way the binary is MARKED as not carrying a fresh build. The reuse path
/// is the one that needs saying twice, because it is invisible: the binary serves
/// a real single-page app with real hashed assets, so nothing about running it
/// suggests the source in this checkout was never compiled into it.
fn skip_frontend_build(dist: &Path, dist_index: &Path) {
    if dist_index.exists() && !is_not_built_notice(dist_index) {
        mark_state("stale");
        println!(
            "cargo:warning=dux-web: {DISABLE_ENV} is set, so the frontend build was skipped. \
             Embedding the existing web/dist, which may be ARBITRARILY stale. This binary is \
             marked as carrying no fresh web UI: `dux server` says so in its startup banner and \
             in dux.log, and the static-serving tests skip rather than assert against it. Unset \
             {DISABLE_ENV} to rebuild it."
        );
        return;
    }
    std::fs::create_dir_all(dist)
        .and_then(|()| std::fs::write(dist_index, NOT_BUILT_PAGE))
        .unwrap_or_else(|err| {
            panic!("dux-web: could not write the notice page to {dist_index:?}: {err}")
        });
    mark_state("not_built");
    println!(
        "cargo:warning=dux-web: {DISABLE_ENV} is set and web/dist is empty, so this binary has NO \
         web UI. Server mode will serve a notice page explaining that. The terminal UI is \
         unaffected. Unset {DISABLE_ENV} to build the web UI in."
    );
}

/// Whether `node_modules` is missing or older than the dependency manifests, so
/// a checkout whose `package.json`/`package-lock.json` changed since the last
/// install gets a reinstall instead of a "cannot find module" build failure.
///
/// npm writes `node_modules/.package-lock.json` after every install/ci as a
/// snapshot of exactly what it laid down; comparing its mtime against the two
/// manifests tells us whether the install is current. If the snapshot is absent
/// (never installed, or `package-lock=false`) we treat deps as stale and let the
/// caller install.
fn deps_stale(web: &Path) -> bool {
    let snapshot = web.join("node_modules").join(".package-lock.json");
    let Ok(snapshot_mtime) = std::fs::metadata(&snapshot).and_then(|m| m.modified()) else {
        return true;
    };
    ["package.json", "package-lock.json"]
        .iter()
        .any(|manifest| {
            std::fs::metadata(web.join(manifest))
                .and_then(|m| m.modified())
                .map(|mtime| mtime > snapshot_mtime)
                .unwrap_or(false)
        })
}

/// Whether `index.html` is a notice page this script wrote earlier, rather than a
/// real build. `gzip_dist` compresses `index.html` in place, so the sentinel has
/// to be looked for in the inflated bytes as well as the raw ones.
fn is_not_built_notice(dist_index: &Path) -> bool {
    let Ok(bytes) = std::fs::read(dist_index) else {
        return false;
    };
    let bytes = if bytes.starts_with(&[0x1f, 0x8b]) {
        use std::io::Read;

        use flate2::read::GzDecoder;

        let mut out = Vec::new();
        if GzDecoder::new(&bytes[..]).read_to_end(&mut out).is_err() {
            return false;
        }
        out
    } else {
        bytes
    };
    String::from_utf8_lossy(&bytes).contains(NOT_BUILT_SENTINEL)
}

fn gzip_dist(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            gzip_dist(&path);
            continue;
        }
        let is_compressible = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| COMPRESSIBLE.contains(&e))
            .unwrap_or(false);
        if !is_compressible {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        // Already gzipped (e.g. a dist kept from a prior failed build) → skip.
        if bytes.starts_with(&[0x1f, 0x8b]) {
            continue;
        }
        let mut encoder = GzEncoder::new(Vec::new(), Compression::best());
        if encoder.write_all(&bytes).is_ok()
            && let Ok(compressed) = encoder.finish()
        {
            let _ = std::fs::write(&path, compressed);
        }
    }
}

fn run(dir: &Path, cmd: &str, args: &[&str]) -> Option<()> {
    Command::new(cmd)
        .args(args)
        .current_dir(dir)
        .status()
        .ok()
        .filter(|s| s.success())
        .map(|_| ())
}
