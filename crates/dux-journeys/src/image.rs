//! The images and the host-built binary every journey uses, and the one-time
//! setup that gets them ready before any journey's clock starts.
//!
//! The journey image is the preview environment's (`tools/preview-env/Dockerfile`),
//! built with its journey tools on and its agent CLIs off, so the journeys exercise
//! the same entrypoint, fake provider and seeded config a person previewing the UI
//! gets. Its tag is a hash of everything that decides what is in it: the files it
//! is built from, the pinned base image, and the host's glibc (the binary built on
//! this machine must run inside it). `DUX_JOURNEY_IMAGE` names an image to use
//! instead (CI builds it in a cached step of its own).
//!
//! The third-party images (the base, nginx, Caddy) are pinned by digest in
//! `tools/preview-env/journey-images.env`, which the CI workflow reads too.
//!
//! The binary is NOT built here: a test that runs cargo inside cargo test fights
//! over the build lock. Build it first with
//! `cargo build --profile journeys --bin dux` (or point `DUX_JOURNEY_BIN` at one).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use sha2::{Digest, Sha256};
use tokio::sync::OnceCell;

/// The repository name every journey image is tagged under.
pub const IMAGE_NAME: &str = "dux-journeys";

/// The label every container and network a journey creates carries.
pub const LABEL: &str = "dux-journeys";

/// The label naming the test process that created a container, so a later run
/// can tell a leftover of a dead run from a container a live run still uses.
pub const PID_LABEL: &str = "dux-journeys.pid";

/// The pinned third-party images, read from the file the CI workflow reads.
const PINNED: &str = include_str!("../../../tools/preview-env/journey-images.env");

/// The files the image is built from, relative to `tools/preview-env`. Every one
/// of them feeds the tag, so changing any of them builds a new image.
const IMAGE_INPUTS: &[&str] = &[
    "Dockerfile",
    "entrypoint.sh",
    "fake-agent.sh",
    "tailscale-stand-in.sh",
    "tui-driver.js",
    "tui-journey.example.js",
    "journey-images.env",
];

/// How long the one-time setup (an image build, the pulls) may take. Generous on
/// purpose: a first build on a slow connection downloads Chromium.
const SETUP_DEADLINE: Duration = Duration::from_secs(30 * 60);

/// One pinned image reference (`name:tag@sha256:...`) from journey-images.env.
pub fn pinned(key: &str) -> String {
    PINNED
        .lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .find_map(|l| l.strip_prefix(key)?.strip_prefix('='))
        .unwrap_or_else(|| panic!("journey-images.env names no {key}"))
        .trim()
        .to_string()
}

/// A pinned reference split the way testcontainers takes it: the name, and the
/// rest (`tag@sha256:...`), which it puts back together with a colon.
pub fn pinned_parts(key: &str) -> (String, String) {
    let full = pinned(key);
    let (name, rest) = full
        .split_once(':')
        .unwrap_or_else(|| panic!("{key} is not name:tag@digest: {full}"));
    (name.to_string(), rest.to_string())
}

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

/// The host's glibc, which the mounted binary was linked against.
fn host_glibc() -> String {
    Command::new("getconf")
        .arg("GNU_LIBC_VERSION")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

/// The image tag for this tree and this host.
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
    hasher.update(pinned("JOURNEY_BASE_IMAGE").as_bytes());
    hasher.update([0]);
    hasher.update(host_glibc().as_bytes());
    let digest = hasher.finalize();
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    format!("j-{}", &hex[..16])
}

static SETUP: OnceCell<(String, String)> = OnceCell::const_new();

/// Everything a journey needs that is not its own containers, done once per
/// test process before any journey's deadline starts: the leftovers of dead runs
/// removed, the journey image present, the pinned proxy images pulled. Returns
/// the journey image as `(name, tag)`.
pub async fn setup() -> (String, String) {
    SETUP
        .get_or_init(|| async {
            let work = tokio::task::spawn_blocking(|| {
                sweep_dead_runs();
                let image = match std::env::var("DUX_JOURNEY_IMAGE") {
                    Ok(full) => full
                        .rsplit_once(':')
                        .map(|(n, t)| (n.to_string(), t.to_string()))
                        .unwrap_or((full.clone(), "latest".to_string())),
                    Err(_) => {
                        let tag = content_tag();
                        ensure_built(&tag);
                        (IMAGE_NAME.to_string(), tag)
                    }
                };
                for key in ["JOURNEY_NGINX_IMAGE", "JOURNEY_CADDY_IMAGE"] {
                    ensure_pulled(&pinned(key));
                }
                image
            });
            match tokio::time::timeout(SETUP_DEADLINE, work).await {
                Ok(result) => result.expect("the journey setup"),
                Err(_) => {
                    panic!("the journey setup (image build and pulls) took over {SETUP_DEADLINE:?}")
                }
            }
        })
        .await
        .clone()
}

/// The journey image, after [`setup`].
pub async fn journey_image() -> (String, String) {
    setup().await
}

fn docker(args: &[&str]) -> std::process::Output {
    Command::new("docker")
        .args(args)
        .output()
        .expect("run docker (is Docker installed and running?)")
}

fn image_present(reference: &str) -> bool {
    docker(&["image", "inspect", "--format", "{{.Id}}", reference])
        .status
        .success()
}

fn ensure_pulled(reference: &str) {
    if image_present(reference) {
        return;
    }
    let status = Command::new("docker")
        .args(["pull", "--quiet", reference])
        .status()
        .expect("run docker pull");
    assert!(status.success(), "docker pull {reference} failed");
}

/// Build the journey image unless it exists. A lock file held across the build
/// makes concurrent test processes (two worktrees, say) wait for one build
/// instead of racing their own.
fn ensure_built(tag: &str) {
    let full = format!("{IMAGE_NAME}:{tag}");
    if image_present(&full) {
        return;
    }
    let lock_path = std::env::temp_dir().join("dux-journeys-image-build.lock");
    let lock = std::fs::File::create(&lock_path)
        .unwrap_or_else(|err| panic!("create {}: {err}", lock_path.display()));
    lock.lock().expect("take the image build lock");
    if image_present(&full) {
        return;
    }
    eprintln!("dux-journeys: building image {full} from tools/preview-env (once per change)");
    let base = format!("BASE_IMAGE={}", pinned("JOURNEY_BASE_IMAGE"));
    let status = Command::new("docker")
        .args([
            "build",
            "--build-arg",
            &base,
            "--build-arg",
            "AGENT_CLIS=0",
            "--build-arg",
            "JOURNEY_TOOLS=1",
            "--label",
            &format!("{LABEL}=1"),
            "-t",
            &full,
        ])
        .arg(preview_env())
        .status()
        .expect("run docker build");
    assert!(status.success(), "docker build of {full} failed");
    prune_older_images(tag);
}

/// Remove the journey images an older tree built. An image a running container
/// still uses is refused by Docker and left alone.
fn prune_older_images(keep: &str) {
    let listed = docker(&["image", "ls", IMAGE_NAME, "--format", "{{.Tag}}"]);
    for tag in String::from_utf8_lossy(&listed.stdout).lines() {
        if tag.starts_with("j-") && tag != keep {
            let _ = docker(&["image", "rm", &format!("{IMAGE_NAME}:{tag}")]);
        }
    }
}

/// Remove the containers and networks of journey runs whose test process is
/// gone (killed mid-run, say). A live run's are left alone: they carry the pid
/// of a process that still exists.
fn sweep_dead_runs() {
    let alive = |pid: &str| {
        pid.parse::<u32>()
            .is_ok_and(|p| Path::new(&format!("/proc/{p}")).exists())
    };
    let listed = docker(&[
        "ps",
        "-a",
        "--filter",
        &format!("label={PID_LABEL}"),
        "--format",
        &format!("{{{{.ID}}}} {{{{.Label \"{PID_LABEL}\"}}}}"),
    ]);
    for line in String::from_utf8_lossy(&listed.stdout).lines() {
        if let Some((id, pid)) = line.split_once(' ')
            && !alive(pid)
        {
            let _ = docker(&["rm", "-f", "-v", id]);
        }
    }
    let networks = docker(&[
        "network",
        "ls",
        "--filter",
        &format!("label={PID_LABEL}"),
        "--format",
        &format!("{{{{.Name}}}} {{{{.Label \"{PID_LABEL}\"}}}}"),
    ]);
    for line in String::from_utf8_lossy(&networks.stdout).lines() {
        if let Some((name, pid)) = line.split_once(' ')
            && !alive(pid)
        {
            let _ = docker(&["network", "rm", name]);
        }
    }
}

/// Removes a container by name when dropped, whatever state it reached. It is
/// created BEFORE the container is, so a journey cancelled between Docker
/// creating the container and the harness getting a handle on it (a deadline, a
/// failed copy or start) still removes it.
pub struct Reaper(pub String);

impl Drop for Reaper {
    fn drop(&mut self) {
        let _ = Command::new("docker")
            .args(["rm", "-f", "-v", &self.0])
            .output();
    }
}

/// A Docker network of the journey's own, labelled with this process's pid and
/// removed when dropped. Declare it before the containers that join it, so it
/// outlives them.
pub struct JourneyNetwork {
    name: String,
}

impl JourneyNetwork {
    pub fn create() -> JourneyNetwork {
        let name = format!("dux-journeys-net-{}", crate::util::suffix());
        let output = docker(&[
            "network",
            "create",
            "--label",
            &format!("{LABEL}=1"),
            "--label",
            &format!("{PID_LABEL}={}", std::process::id()),
            &name,
        ]);
        assert!(
            output.status.success(),
            "docker network create {name}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        JourneyNetwork { name }
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

impl Drop for JourneyNetwork {
    fn drop(&mut self) {
        let _ = docker(&["network", "rm", &self.name]);
    }
}
