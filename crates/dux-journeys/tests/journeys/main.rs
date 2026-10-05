//! dux's container journeys: the real binary in Docker, driven like a person.
//!
//! Every journey's doc comment is written STAR: the Situation it starts from,
//! the Task the person has, the Action they take, and the Result they must see.
//! A journey starts its own containers and removes them when it ends.
//!
//! Run them (Docker required, Linux only):
//!
//! ```text
//! cargo build --profile journeys --bin dux
//! cargo test -p dux-journeys --features journeys -- --test-threads=2
//! cargo test -p dux-journeys --features journeys -- --test-threads=2 smoke   # one journey
//! cargo test -p dux-journeys --features auth -- --test-threads=2            # the login journeys too
//! ```
//!
//! The journeys about the web password login run only with `--features auth`.
//! Without it they compile and are listed as ignored with the reason. They were
//! written before the login was built, as its definition of done.
#![cfg(target_os = "linux")]

mod auth_browser;
mod auth_cli;
mod auth_lifecycle;
mod auth_login;
mod auth_network;
mod auth_proxies;
mod smoke;
