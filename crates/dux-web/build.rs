use std::path::Path;
use std::process::Command;

fn main() {
    let web = Path::new("web");
    println!("cargo:rerun-if-changed=web/src");
    println!("cargo:rerun-if-changed=web/index.html");
    println!("cargo:rerun-if-changed=web/package.json");
    println!("cargo:rerun-if-changed=web/vite.config.ts");

    let dist_index = web.join("dist/index.html");
    if dist_index.exists() {
        return; // already built; rerun-if-changed re-triggers when web sources change
    }
    if !web.join("node_modules").exists() {
        let _ = run(web, "npm", &["ci"]).or_else(|| run(web, "npm", &["install"]));
    }
    if run(web, "npm", &["run", "build"]).is_none() {
        std::fs::create_dir_all(web.join("dist")).ok();
        std::fs::write(
            &dist_index,
            "<!doctype html><title>dux</title><div id=\"root\">web assets not built — run npm run build in crates/dux-web/web</div>",
        )
        .ok();
        println!("cargo:warning=dux-web: frontend build unavailable; embedded a placeholder page");
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
