use std::path::Path;
use std::process::Command;

fn main() {
    let web = Path::new("web");
    println!("cargo:rerun-if-changed=web/src");
    println!("cargo:rerun-if-changed=web/index.html");
    println!("cargo:rerun-if-changed=web/package.json");
    println!("cargo:rerun-if-changed=web/vite.config.ts");

    // Always (re)build when this script runs. The `rerun-if-changed` lines above
    // gate WHEN cargo re-runs it — the first build and whenever the web sources
    // change — so the embedded site is never stale, while Rust-only rebuilds skip
    // this entirely. node_modules persists, so steady-state cost is one fast Vite
    // build only when the frontend actually changed.
    let dist_index = web.join("dist/index.html");
    if !web.join("node_modules").exists() {
        let _ = run(web, "npm", &["ci"]).or_else(|| run(web, "npm", &["install"]));
    }
    if run(web, "npm", &["run", "build"]).is_none() {
        // npm unavailable (offline / no node). Keep any existing dist so the binary
        // still embeds the last good build; only write a placeholder if none exists.
        if !dist_index.exists() {
            std::fs::create_dir_all(web.join("dist")).ok();
            std::fs::write(
                &dist_index,
                "<!doctype html><title>dux</title><div id=\"root\">web assets not built — run npm run build in crates/dux-web/web</div>",
            )
            .ok();
        }
        println!(
            "cargo:warning=dux-web: frontend build failed; embedded the existing/placeholder page"
        );
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
