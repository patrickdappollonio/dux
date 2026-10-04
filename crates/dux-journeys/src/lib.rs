//! The container journey harness: the real `dux` binary in Docker, driven the
//! way a person drives it, over HTTP with a cookie jar, WebSockets and a real
//! browser, with nginx, Caddy and a stand-in Tailscale where a journey needs
//! them.
//!
//! Every journey starts its own containers and removes them when it ends, pass
//! or fail. Nothing here touches the host's dux: the binary is built on the host
//! and mounted read-only into a container whose config, sessions and repos live
//! inside that container.
//!
//! The pieces:
//!
//! - [`image`] finds or builds the runtime image (the preview environment's
//!   Dockerfile, with its journey tools on) and finds the host-built binary.
//! - [`dux::Dux`] is one running dux, started from [`dux::DuxOptions`]: how it
//!   listens, whether a stand-in Tailscale answers, which ports relay where, and
//!   which seed hooks shape its config before it starts.
//! - [`client::Client`] is a person at a browser without the browser: a cookie
//!   jar, an `Origin` on every mutation, and the plain responses.
//! - [`ws`] opens the events and PTY sockets with that person's cookie.
//! - [`sidecars`] runs nginx and Caddy in front of a dux.
//! - [`browser::Browser`] is Chromium over WebDriver, in a container of its own.
//! - [`api`] is the handful of REST calls the journeys share (add a project,
//!   create an agent and wait for it).
#![cfg(all(target_os = "linux", feature = "journeys"))]

pub mod api;
pub mod browser;
pub mod client;
pub mod dux;
pub mod image;
pub mod sidecars;
pub mod util;
pub mod ws;

pub use client::{Client, Response};
pub use dux::{Bind, Dux, DuxOptions, Launch};
pub use util::{eventually, journey};

/// The port dux listens on inside every journey container. Fixed, because the
/// container has a network namespace of its own; the host sees whatever port
/// Docker published it on.
pub const DUX_PORT: u16 = 3890;

/// The address the stand-in Tailscale reports and gives the container's
/// loopback. Obviously fake, inside Tailscale's CGNAT range.
pub const TAILNET_IP: &str = "100.101.102.103";

/// A password every journey that sets one uses: long and random-looking enough
/// to clear the default minimum length and strength score.
pub const STRONG_PASSWORD: &str = "orbit-velvet-quarry-71-lantern";

/// A second strong password, for the journeys that change it.
pub const OTHER_STRONG_PASSWORD: &str = "harbor-cinnamon-glacier-38-tundra";
