use std::io::Write;
use std::path::Path;
use std::process::Command;

use flate2::Compression;
use flate2::write::GzEncoder;

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

    // Always (re)build when this script runs. The `rerun-if-changed` lines above
    // gate WHEN cargo re-runs it — the first build and whenever the web sources
    // change — so the embedded site is never stale, while Rust-only rebuilds skip
    // this entirely. node_modules persists, so steady-state cost is one fast Vite
    // build only when the frontend actually changed.
    let dist = web.join("dist");
    let dist_index = dist.join("index.html");
    // Install dependencies when the lockfile is out of sync with what's on disk
    // — not only when `node_modules` is missing entirely. A `git pull` that adds
    // a frontend dependency leaves the existing `node_modules` in place but
    // stale; without this the next `tsc`/Vite build fails with "cannot find
    // module" and only the placeholder page gets embedded. `npm ci` is clean and
    // reproducible; fall back to `npm install` if the lockfile itself is stale.
    if deps_stale(web) {
        let _ = run(web, "npm", &["ci"]).or_else(|| run(web, "npm", &["install"]));
    }
    if run(web, "npm", &["run", "build"]).is_none() {
        // npm unavailable (offline / no node). Keep any existing dist so the binary
        // still embeds the last good build; only write a placeholder if none exists.
        if !dist_index.exists() {
            std::fs::create_dir_all(&dist).ok();
            std::fs::write(
                &dist_index,
                "<!doctype html><title>dux</title><div id=\"root\">web assets not built — run npm run build in crates/dux-web/web</div>",
            )
            .ok();
        }
        println!(
            "cargo:warning=dux-web: frontend build failed; embedded the existing/placeholder page. \
             Run `npm ci` in crates/dux-web/web, or `cargo build -vv` to see the npm/tsc/Vite error."
        );
    }

    // Gzip the text assets IN PLACE so rust-embed bakes the compressed bytes into
    // the binary (and `web_assets` serves them with `Content-Encoding: gzip`).
    // Runs after the Vite build (which writes raw files); idempotent via the gzip
    // magic-byte check, so a kept-from-last-time dist isn't double-compressed.
    gzip_dist(&dist);
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
