# syntax=docker/dockerfile:1.7
#
# Self-contained build + runtime image for the dux browser-client. Users
# with only Docker installed can `docker build -t dux-web .` and get a
# working static host at port 80; no Rust, clang, or wasm-pack needed on
# the host machine.
#
# The browser loaded from this image speaks iroh QUIC (relayed over
# WebSocket) directly to the dux host — the container never sees
# plaintext RemoteMessage traffic. It just serves the SPA shell + WASM.
#
# WASM is architecture-independent: the `dux_web_browser_bg.wasm` bytes
# produced in the builder stage are identical regardless of the host
# arch. What differs per arch is the runtime (nginx:alpine) layer, which
# is why buildx's `--platform linux/amd64,linux/arm64` produces a real
# multi-arch manifest.

# ── Stage 1: build the static bundle ──────────────────────────────
# Pin a recent stable Rust + Debian base. `slim-bookworm` keeps the
# builder small while still having apt available for clang.
FROM rust:1.90-slim-bookworm AS builder

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
         clang \
         curl \
         ca-certificates \
         pkg-config \
         libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Install the wasm target and wasm-pack once, cached in the builder layer.
RUN rustup target add wasm32-unknown-unknown
RUN cargo install wasm-pack --locked --version 0.14.0

WORKDIR /src

# Copy only what the WASM build needs. Keeping the copy narrow keeps the
# Docker cache hot when unrelated files change (README, CI, etc.).
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates

# Produce crates/dux-web-browser/dist/ — same bytes on every arch.
RUN crates/dux-web-browser/build.sh

# ── Stage 2: serve the bundle ─────────────────────────────────────
# nginx:alpine's default mime.types maps .wasm → `application/wasm` with
# no charset parameter, so Chromium's `WebAssembly.instantiateStreaming`
# accepts the response on the fast path (no fallback warning, no
# double-fetch).  It's also a tiny image and well-understood by anyone
# self-hosting a static site.
FROM nginx:1.27-alpine

COPY --from=builder /src/crates/dux-web-browser/dist/ /usr/share/nginx/html/

# Two small nginx overrides:
#
# 1. Re-assert `application/wasm` for `.wasm` — the default mime.types
#    already does this, but being explicit keeps the
#    `WebAssembly.instantiateStreaming` fast path working if a user
#    layers a custom config on top.
#
# 2. Turn on gzip for the SPA bundle.  nginx:alpine ships gzip off by
#    default and, even when on, does not include `application/wasm` or
#    `application/javascript` in `gzip_types`.  For this app the 2.5 MB
#    raw wasm compresses to ~1.05 MB — that's what users actually
#    download.  Enabling gzip here means the wire cost drops ~58%
#    without any build-time changes.
RUN printf 'types { application/wasm wasm; }\n' \
      > /etc/nginx/conf.d/wasm-mime.conf \
 && printf 'gzip on;\n\
gzip_comp_level 6;\n\
gzip_min_length 1024;\n\
gzip_proxied any;\n\
gzip_vary on;\n\
gzip_types\n\
    application/wasm\n\
    application/javascript\n\
    text/css\n\
    text/html\n\
    text/plain;\n' \
      > /etc/nginx/conf.d/gzip.conf

EXPOSE 80
