//! What every container a journey starts has in common: its labels, its name
//! reaper, and how its ports are published.
//!
//! PORTS ARE PUBLISHED ON THE HOST'S LOOPBACK ONLY. testcontainers' default
//! publishes every exposed port on every host interface, and Docker's own
//! firewall rules sit in front of the host's, so for the length of a run an
//! unauthenticated dux, the relays and chromedriver would be reachable from the
//! local network. Every published port here binds `127.0.0.1` on an ephemeral
//! host port instead, and [`loopback_publish`] is the one place that says so.

use std::collections::HashMap;

use testcontainers::bollard::models::{HostConfig, PortBinding};
use testcontainers::core::ContainerRequest;
use testcontainers::{GenericImage, ImageExt};

use crate::image::{LABEL, PID_LABEL, Reaper};
use crate::util::{LogBuffer, container_logs, suffix};

/// A container name for this journey, its reaper, and its log buffer
/// (recorded against the running journey).
pub fn identity(kind: &str) -> (String, Reaper, LogBuffer) {
    let name = format!("dux-journeys-{kind}-{}", suffix());
    let reaper = Reaper(name.clone());
    let logs = container_logs(&name);
    (name, reaper, logs)
}

/// Name, label and log a container request the way every journey container is.
pub fn labelled(
    request: ContainerRequest<GenericImage>,
    name: &str,
    logs: &LogBuffer,
) -> ContainerRequest<GenericImage> {
    request
        .with_container_name(name)
        .with_label(LABEL, "1")
        .with_label(PID_LABEL, std::process::id().to_string())
        .with_log_consumer(logs.clone())
}

/// A host-config modifier that publishes exactly `ports`, each on
/// `127.0.0.1` with an ephemeral host port, and nothing else.
pub fn loopback_publish(ports: Vec<u16>) -> impl Fn(&mut HostConfig) + Send + Sync + 'static {
    move |host: &mut HostConfig| {
        host.publish_all_ports = Some(false);
        let bindings: HashMap<String, Option<Vec<PortBinding>>> = ports
            .iter()
            .map(|port| {
                (
                    format!("{port}/tcp"),
                    Some(vec![PortBinding {
                        host_ip: Some("127.0.0.1".to_string()),
                        host_port: Some(String::new()),
                    }]),
                )
            })
            .collect();
        host.port_bindings = Some(bindings);
    }
}

/// A host-config modifier that joins another container's network namespace and
/// publishes nothing (a namespace that is not its own cannot publish).
pub fn share_namespace_of(container_id: &str) -> impl Fn(&mut HostConfig) + Send + Sync + 'static {
    let namespace = format!("container:{container_id}");
    move |host: &mut HostConfig| {
        host.network_mode = Some(namespace.clone());
        host.publish_all_ports = Some(false);
        host.port_bindings = None;
    }
}

/// The host side of every port binding of a container, as Docker reports it:
/// `(container port, host ip, host port)`. For the test that proves nothing is
/// published beyond loopback.
pub async fn published_bindings(container_id: &str) -> Vec<(String, String, String)> {
    let output = tokio::process::Command::new("docker")
        .args([
            "container",
            "inspect",
            "--format",
            "{{json .NetworkSettings.Ports}}",
            container_id,
        ])
        .output()
        .await
        .expect("docker container inspect");
    let ports: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("the inspected ports are JSON");
    let mut bindings = Vec::new();
    if let Some(map) = ports.as_object() {
        for (port, list) in map {
            for binding in list.as_array().into_iter().flatten() {
                bindings.push((
                    port.clone(),
                    binding["HostIp"].as_str().unwrap_or_default().to_string(),
                    binding["HostPort"].as_str().unwrap_or_default().to_string(),
                ));
            }
        }
    }
    bindings
}
