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
//! Old journey images are never removed during a run, because another worktree's
//! run may be using one right now. The documented cleanup removes the ones no
//! container uses that are over a day old.
//!
//! The third-party images (the base, nginx, Caddy) are pinned by digest in
//! `tools/preview-env/journey-images.env`, which the CI workflow reads too.
//!
//! The binary is NOT built here: a test that runs cargo inside cargo test fights
//! over the build lock. Build it first with
//! `cargo build --profile journeys --bin dux` (or point `DUX_JOURNEY_BIN` at one).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};
use tokio::sync::OnceCell;

use crate::run::{cache_dir, run_id, sweep_dead_runs};

/// The repository name every journey image is tagged under.
pub const IMAGE_NAME: &str = "dux-journeys";

/// The label every container, network and image the suite creates carries.
pub const LABEL: &str = "dux-journeys";

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

/// Split an image reference into the `(name, tag)` pair testcontainers puts back
/// together with a colon. The tag is after the last colon only when that colon
/// comes after the last slash (a registry's port is not a tag), defaults to
/// `latest`, and keeps a digest (`tag@sha256:...`) so the pin survives.
pub fn split_reference(reference: &str) -> (String, String) {
    let (named, digest) = match reference.split_once('@') {
        Some((named, digest)) => (named, Some(digest)),
        None => (reference, None),
    };
    let last_slash = named.rfind('/').map_or(0, |i| i + 1);
    let (name, tag) = match named[last_slash..].rfind(':') {
        Some(colon) => (
            &named[..last_slash + colon],
            &named[last_slash + colon + 1..],
        ),
        None => (named, "latest"),
    };
    let tag = match digest {
        Some(digest) => format!("{tag}@{digest}"),
        None => tag.to_string(),
    };
    (name.to_string(), tag)
}

/// A pinned reference split for testcontainers.
pub fn pinned_parts(key: &str) -> (String, String) {
    split_reference(&pinned(key))
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

/// Where cargo writes this workspace's builds: `CARGO_TARGET_DIR` (relative to
/// the workspace root when relative) or `target/`.
fn target_dir() -> PathBuf {
    match std::env::var_os("CARGO_TARGET_DIR").filter(|v| !v.is_empty()) {
        Some(dir) => {
            let dir = PathBuf::from(dir);
            if dir.is_absolute() {
                dir
            } else {
                workspace_root().join(dir)
            }
        }
        None => workspace_root().join("target"),
    }
}

/// The newest modification time among the files that go into the dux binary:
/// every crate's sources and manifest and the web UI's sources, this crate and
/// build output excluded.
fn newest_source(root: &Path) -> Option<(SystemTime, PathBuf)> {
    fn walk(dir: &Path, newest: &mut Option<(SystemTime, PathBuf)>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if matches!(
                name.as_ref(),
                "target" | "node_modules" | "dist" | "dux-journeys" | ".git"
            ) {
                continue;
            }
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                walk(&path, newest);
            } else if let Ok(modified) = meta.modified()
                && newest.as_ref().is_none_or(|(t, _)| modified > *t)
            {
                *newest = Some((modified, path));
            }
        }
    }
    let mut newest = None;
    // The workspace's Cargo.lock is left out on purpose: this crate's own
    // dependencies move it without changing a byte of dux.
    walk(&root.join("crates"), &mut newest);
    newest
}

/// The host-built dux binary to mount, or a panic saying how to build one. A
/// binary older than the newest source is refused too (a journey run against
/// yesterday's dux passes or fails for the wrong reason), unless
/// `DUX_JOURNEY_ALLOW_STALE=1` says that is intended.
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
    let path = target_dir().join("journeys/dux");
    assert!(
        path.is_file(),
        "no dux binary at {}. Build it first: cargo build --profile journeys --bin dux \
         (or set DUX_JOURNEY_BIN to a dux binary built on this machine)",
        path.display()
    );
    if std::env::var("DUX_JOURNEY_ALLOW_STALE").as_deref() != Ok("1") {
        let built = std::fs::metadata(&path)
            .and_then(|m| m.modified())
            .expect("the binary's modification time");
        if let Some((newest, source)) = newest_source(&workspace_root())
            && newest > built
        {
            panic!(
                "{} is older than {}. Rebuild it: cargo build --profile journeys --bin dux \
                 (or set DUX_JOURNEY_ALLOW_STALE=1 to run the journeys against it anyway)",
                path.display(),
                source.display()
            );
        }
    }
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
/// test process before any journey's deadline starts: this run started (its
/// heartbeat and Ctrl-C cleanup), dead runs' leftovers removed, the journey
/// image present, the pinned proxy images pulled. Returns the journey image as
/// `(name, tag)`.
pub async fn setup() -> (String, String) {
    SETUP
        .get_or_init(|| async {
            let work = tokio::task::spawn_blocking(|| {
                run_id();
                sweep_dead_runs();
                let image = match std::env::var("DUX_JOURNEY_IMAGE") {
                    Ok(full) => split_reference(&full),
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

/// The lock that makes concurrent test processes (two worktrees, say) wait for
/// one image build instead of racing their own: a per-user file under the
/// cache directory, so it does not move with `TMPDIR`. `None`, with a warning,
/// when it cannot be opened (a cache directory another user owns): the build
/// then runs unguarded, which costs at most a duplicate build.
fn build_lock() -> Option<std::fs::File> {
    let dir = cache_dir();
    let path = dir.join("image-build.lock");
    let opened = std::fs::create_dir_all(&dir).and_then(|()| {
        std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
    });
    match opened {
        Ok(file) => match file.lock() {
            Ok(()) => Some(file),
            Err(err) => {
                eprintln!(
                    "dux-journeys: cannot lock {} ({err}); building without the lock",
                    path.display()
                );
                None
            }
        },
        Err(err) => {
            eprintln!(
                "dux-journeys: cannot open the image build lock {} ({err}); is the directory \
                 owned by another user? Building without the lock",
                path.display()
            );
            None
        }
    }
}

/// Build the journey image unless it exists.
fn ensure_built(tag: &str) {
    let full = format!("{IMAGE_NAME}:{tag}");
    if image_present(&full) {
        return;
    }
    let _lock = build_lock();
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
}

/// Removes a container by name when dropped, whatever state it reached. It is
/// created BEFORE the container is, so a journey cancelled between Docker
/// creating the container and the harness getting a handle on it (a deadline, a
/// failed copy or start) still removes it. A container already gone is fine.
pub struct Reaper(pub String);

impl Drop for Reaper {
    fn drop(&mut self) {
        let _ = Command::new("docker")
            .args(["rm", "-f", "-v", &self.0])
            .output();
    }
}

/// A Docker network of the journey's own, labelled with this run and removed
/// when dropped. Whoever holds it must drop it after the containers on it.
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
            &format!("{}={}", crate::run::RUN_LABEL, run_id()),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reference_splits_into_name_and_tag_the_way_docker_reads_it() {
        let cases = [
            ("dux-journeys:ci", ("dux-journeys", "ci")),
            ("dux-journeys", ("dux-journeys", "latest")),
            (
                "localhost:5000/dux-journeys",
                ("localhost:5000/dux-journeys", "latest"),
            ),
            (
                "localhost:5000/team/dux:j-1",
                ("localhost:5000/team/dux", "j-1"),
            ),
            (
                "nginx:1-alpine@sha256:abc",
                ("nginx", "1-alpine@sha256:abc"),
            ),
            (
                "registry:5000/nginx@sha256:abc",
                ("registry:5000/nginx", "latest@sha256:abc"),
            ),
        ];
        for (reference, (name, tag)) in cases {
            assert_eq!(
                split_reference(reference),
                (name.to_string(), tag.to_string()),
                "{reference}"
            );
        }
    }

    #[test]
    fn the_pinned_images_are_all_pinned_by_digest() {
        for key in [
            "JOURNEY_BASE_IMAGE",
            "JOURNEY_NGINX_IMAGE",
            "JOURNEY_CADDY_IMAGE",
        ] {
            assert!(pinned(key).contains("@sha256:"), "{key}");
        }
    }
}
