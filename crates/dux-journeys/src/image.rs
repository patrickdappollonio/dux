//! The runtime image and the host-built binary every journey mounts into it.
//!
//! The image is the preview environment's (`tools/preview-env/Dockerfile`), built
//! with its journey tools on and its agent CLIs off, so the journeys exercise the
//! same entrypoint, fake provider and seeded config a person previewing the UI
//! gets. It is tagged by a hash of everything that goes into it, so an edit to
//! the Dockerfile or the entrypoint builds a fresh image and an unchanged tree
//! reuses the one already built. `DUX_JOURNEY_IMAGE` names an image to use
//! instead (CI builds it in a cached step of its own).
//!
//! The binary is NOT built here: a test that runs cargo inside cargo test fights
//! over the build lock. Build it first with
//! `cargo build --profile journeys --bin dux` (or point `DUX_JOURNEY_BIN` at one).

use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest, Sha256};
use tokio::sync::OnceCell;

/// The repository name every journey image is tagged under.
pub const IMAGE_NAME: &str = "dux-journeys";

/// The files the image is built from, relative to `tools/preview-env`. Every one
/// of them feeds the tag, so changing any of them builds a new image.
const IMAGE_INPUTS: &[&str] = &[
    "Dockerfile",
    "entrypoint.sh",
    "fake-agent.sh",
    "tailscale-stand-in.sh",
    "tui-driver.js",
    "tui-journey.example.js",
];

/// The repository root, found from this crate's own manifest.
pub fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the workspace root exists")
}

fn preview_env() -> PathBuf {
    workspace_root().join("tools/preview-env")
}

/// The host-built dux binary to mount, or a panic saying how to build one.
pub fn dux_binary() -> PathBuf {
    if let Some(path) = std::env::var_os("DUX_JOURNEY_BIN") {
        let path = PathBuf::from(path);
        assert!(
            path.is_file(),
            "DUX_JOURNEY_BIN points at {}, which is not a file",
            path.display()
        );
        return path;
    }
    let path = workspace_root().join("target/journeys/dux");
    assert!(
        path.is_file(),
        "no dux binary at {}. Build it first: cargo build --profile journeys --bin dux \
         (or set DUX_JOURNEY_BIN to a dux binary built on this machine)",
        path.display()
    );
    path
}

/// The image tag for the current tree: a hash of every input file.
fn content_tag() -> String {
    let mut hasher = Sha256::new();
    for name in IMAGE_INPUTS {
        let path = preview_env().join(name);
        let bytes = std::fs::read(&path)
            .unwrap_or_else(|err| panic!("cannot read image input {}: {err}", path.display()));
        hasher.update(name.as_bytes());
        hasher.update([0]);
        hasher.update(&bytes);
        hasher.update([0]);
    }
    let digest = hasher.finalize();
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    format!("j-{}", &hex[..16])
}

static IMAGE: OnceCell<(String, String)> = OnceCell::const_new();

/// The `(name, tag)` of the journey image, building it once per test process
/// when it does not exist yet. Concurrent journeys wait on the one build.
pub async fn journey_image() -> (String, String) {
    IMAGE
        .get_or_init(|| async {
            if let Ok(full) = std::env::var("DUX_JOURNEY_IMAGE") {
                let (name, tag) = full
                    .rsplit_once(':')
                    .map(|(n, t)| (n.to_string(), t.to_string()))
                    .unwrap_or((full.clone(), "latest".to_string()));
                return (name, tag);
            }
            let tag = content_tag();
            tokio::task::spawn_blocking(move || {
                ensure_built(&tag);
                (IMAGE_NAME.to_string(), tag)
            })
            .await
            .expect("the image build task")
        })
        .await
        .clone()
}

fn ensure_built(tag: &str) {
    let full = format!("{IMAGE_NAME}:{tag}");
    let present = Command::new("docker")
        .args(["image", "inspect", "--format", "{{.Id}}", &full])
        .output()
        .expect("run docker (is Docker installed and running?)")
        .status
        .success();
    if present {
        return;
    }
    eprintln!("dux-journeys: building image {full} from tools/preview-env (once per change)");
    let status = Command::new("docker")
        .args([
            "build",
            "--build-arg",
            "AGENT_CLIS=0",
            "--build-arg",
            "JOURNEY_TOOLS=1",
            "--label",
            "dux-journeys=1",
            "-t",
            &full,
        ])
        .arg(preview_env())
        .status()
        .expect("run docker build");
    assert!(status.success(), "docker build of {full} failed");
}
