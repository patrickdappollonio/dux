//! Builds the React frontend in `web/` and leaves the result in `web/dist` for
//! `rust_embed` to bake into the binary.
//!
//! ## Failure policy
//!
//! A frontend build that is ATTEMPTED and fails is FATAL: this script panics and
//! `cargo build` fails. It used to write a placeholder page, print a
//! `cargo:warning` and succeed, which meant a release could ship four platform
//! binaries containing "web assets not built" with every check green. There was
//! also a fallback that silently re-embedded a previously built `dist/`, so a
//! broken build shipped a stale UI instead. Both paths are gone: nothing here
//! turns a failure into a successful build.
//!
//! ## Escape hatch
//!
//! Rust-only contributors (and machines with no Node toolchain) set
//! [`DISABLE_ENV`] to any non-empty value. The frontend build is then not
//! attempted at all and the Rust build succeeds. Skipping deliberately is
//! supported; failing silently is not.
//!
//! When the hatch is set and there is no previously built `dist/`, this script
//! writes a plain notice page (see `NOT_BUILT_PAGE`) and sets
//! `cargo:rustc-env=DUX_UI_BUILD_SKIPPED=1` so the Rust side knows the embedded
//! page is not a real build. `web_assets::ui_build_skipped` reads that back, the
//! `dux server` startup banner turns it into a warning row, and the static
//! serving tests use it to SKIP with a printed reason instead of passing on a
//! page that is not a build.

use std::io::Write;
use std::path::Path;
use std::process::Command;

use flate2::Compression;
use flate2::write::GzEncoder;

/// Set this to any non-empty value to skip the frontend build entirely.
const DISABLE_ENV: &str = "DUX_DISABLE_UI_BUILD";

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
/// is no previously built `dist/` to embed. Deliberately NOT the SPA shell: it
/// carries no `id="root"` and no hashed asset reference, so neither a browser nor
/// a test can mistake it for a real build. A user who reaches the server in a
/// browser must be told what happened and how to fix it rather than staring at a
/// blank page.
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
    // Without this, cargo caches the build-script result and toggling the hatch
    // appears to do nothing: the stale `DUX_UI_BUILD_SKIPPED` (or its absence)
    // sticks across builds. Verified by probe, not assumed.
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
/// than embedding one that may be a few commits stale, and the warning says so.
/// With no `dist/` at all, write the notice page and tell the Rust side that the
/// embedded page is not a build.
fn skip_frontend_build(dist: &Path, dist_index: &Path) {
    if dist_index.exists() && !is_not_built_notice(dist_index) {
        println!(
            "cargo:warning=dux-web: {DISABLE_ENV} is set, so the frontend build was skipped. \
             Embedding the existing web/dist, which may be stale. Unset {DISABLE_ENV} to rebuild it."
        );
        return;
    }
    std::fs::create_dir_all(dist)
        .and_then(|()| std::fs::write(dist_index, NOT_BUILT_PAGE))
        .unwrap_or_else(|err| {
            panic!("dux-web: could not write the notice page to {dist_index:?}: {err}")
        });
    println!("cargo:rustc-env=DUX_UI_BUILD_SKIPPED=1");
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
