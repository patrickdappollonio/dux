//! Whether dux runs inside a container.
//!
//! The web server asks this for one reason: a Tailscale running OUTSIDE the
//! container (on the host, or in a sidecar that does not share this
//! container's network) is invisible from in here, so when dux sees no
//! Tailscale it cannot tell whether something outside publishes its port, and
//! it says so once at start.

use std::path::Path;

/// Whether this process runs inside a container. Linux only (a macOS process
/// is never in one dux can tell apart); read once from the real root and
/// environment and remembered, since it cannot change while dux runs.
pub fn running_in_container() -> bool {
    static IN_CONTAINER: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *IN_CONTAINER.get_or_init(|| {
        cfg!(target_os = "linux")
            && container_signals_in(
                Path::new("/"),
                std::env::var_os("KUBERNETES_SERVICE_HOST").is_some(),
            )
    })
}

/// [`running_in_container`] against a root directory, so the signals can be
/// exercised with fake files. Each is cheap (a `stat` or one small read) and
/// each is the runtime's own:
///
/// - `/.dockerenv`, which Docker writes into every container;
/// - `/run/.containerenv`, which Podman writes;
/// - `KUBERNETES_SERVICE_HOST`, which Kubernetes sets in every pod;
/// - PID 1's cgroup path naming a runtime (`docker`, `kubepods`, `lxc`,
///   `libpod`, `containerd`), which cgroup v1 shows; on a host PID 1 sits at the
///   root or in `init.scope`;
/// - the root filesystem being `overlay`, or a file bind-mounted from a
///   runtime's container store (Docker's `/etc/hostname` from
///   `/docker/containers/`, Podman's from `/containers/storage/`), which is what
///   still shows on cgroup v2, where the cgroup path is only `/`.
///
/// A host whose own root is an overlay (a live image) reads as a container;
/// that only adds a warning.
pub fn container_signals_in(root: &Path, kubernetes_env: bool) -> bool {
    let read = |path: &str| std::fs::read_to_string(root.join(path)).ok();
    kubernetes_env
        || root.join(".dockerenv").exists()
        || root.join("run/.containerenv").exists()
        || read("proc/1/cgroup").is_some_and(|text| cgroup_names_a_runtime(&text))
        || read("proc/self/mountinfo").is_some_and(|text| mountinfo_says_container(&text))
}

/// Whether a `/proc/<pid>/cgroup` text places the process under a container
/// runtime.
fn cgroup_names_a_runtime(text: &str) -> bool {
    text.lines().any(|line| {
        let path = line.splitn(3, ':').nth(2).unwrap_or("");
        ["docker", "kubepods", "lxc", "libpod", "containerd"]
            .iter()
            .any(|runtime| path.contains(runtime))
    })
}

/// Whether a `/proc/self/mountinfo` text shows an overlay root or a file bind
/// mounted from a runtime's container store.
fn mountinfo_says_container(text: &str) -> bool {
    text.lines().any(|line| {
        let fields: Vec<&str> = line.split_whitespace().collect();
        let mount_point = fields.get(4).copied().unwrap_or("");
        let fs_type = fields
            .iter()
            .position(|field| *field == "-")
            .and_then(|dash| fields.get(dash + 1))
            .copied()
            .unwrap_or("");
        let overlay_root = mount_point == "/" && fs_type == "overlay";
        let store_bind = fields.get(3).is_some_and(|source| {
            source.contains("/docker/containers/") || source.contains("/containers/storage/")
        });
        overlay_root || store_bind
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root_with(files: &[(&str, &str)]) -> crate::test_scratch::ScratchDir {
        let dir = crate::test_scratch::ScratchDir::new();
        for (path, text) in files {
            let path = dir.path().join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        dir
    }

    /// A host's own files: a cgroup v2 root and an ordinary root filesystem.
    const HOST_CGROUP: &str = "0::/user.slice/user-1000.slice/session-2.scope\n";
    const HOST_MOUNTINFO: &str = "\
26 1 259:2 / / rw,relatime shared:1 - ext4 /dev/nvme0n1p2 rw\n\
27 26 0:6 / /dev rw,nosuid shared:2 - devtmpfs devtmpfs rw\n";

    #[test]
    fn a_plain_host_is_not_a_container() {
        let empty = root_with(&[]);
        assert!(!container_signals_in(empty.path(), false));
        let host = root_with(&[
            ("proc/1/cgroup", HOST_CGROUP),
            ("proc/self/mountinfo", HOST_MOUNTINFO),
        ]);
        assert!(!container_signals_in(host.path(), false));
    }

    #[test]
    fn each_engines_own_marker_says_container() {
        // Docker writes /.dockerenv, Podman /run/.containerenv.
        for marker in [".dockerenv", "run/.containerenv"] {
            let root = root_with(&[(marker, "")]);
            assert!(container_signals_in(root.path(), false), "{marker}");
        }
        // Kubernetes sets this in every pod.
        assert!(container_signals_in(root_with(&[]).path(), true));
    }

    #[test]
    fn a_cgroup_v1_path_naming_a_container_runtime_says_container() {
        for line in [
            "12:pids:/docker/3f2a9c\n",
            "11:memory:/kubepods/burstable/pod1/abc\n",
            "5:cpu:/lxc/web\n",
            "3:devices:/system.slice/containerd.service/kubepods-x\n",
            "1:name=systemd:/machine.slice/libpod-0123.scope\n",
        ] {
            let root = root_with(&[("proc/1/cgroup", line)]);
            assert!(container_signals_in(root.path(), false), "{line}");
        }
    }

    #[test]
    fn an_overlay_root_or_a_runtime_bind_mount_says_container() {
        let overlay = "\
600 500 0:52 / / rw,relatime master:300 - overlay overlay rw,lowerdir=/var/lib/docker/overlay2/l/A\n";
        let bind = format!(
            "{HOST_MOUNTINFO}\
610 600 259:2 /var/lib/docker/containers/abc/hostname /etc/hostname rw - ext4 /dev/sda1 rw\n"
        );
        for mountinfo in [overlay.to_string(), bind] {
            let root = root_with(&[
                ("proc/1/cgroup", HOST_CGROUP),
                ("proc/self/mountinfo", &mountinfo),
            ]);
            assert!(container_signals_in(root.path(), false), "{mountinfo}");
        }
    }
}
