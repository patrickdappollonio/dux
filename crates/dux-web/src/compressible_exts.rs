// The one list of asset extensions that travel Brotli-compressed through the
// embed, shared by the two sides that must agree on it:
//
// * `build.rs` includes this file (`include!`) and Brotli-compresses every
//   staged file whose extension is listed while mirroring `web/dist` into
//   `$OUT_DIR/ui`.
// * `web_assets.rs` uses it as a module and serves those same paths with
//   `Content-Encoding: br` (decompressing on the fly for a client that does
//   not accept it).
//
// Brotli has NO magic bytes (unlike the gzip scheme this replaced), so the
// server cannot sniff compressed-ness from the payload; the extension IS the
// contract, which is why the list must live in exactly one place. build.rs
// guarantees the contract holds for every staged file: whatever path it takes
// (real build, escape hatch, stale reuse, the not-built notice page), staging
// goes through the one `copy_tree`, which compresses by this list.

/// Text asset extensions worth compressing. Binary assets (fonts, images,
/// wasm) are already compressed, so they're left raw.
pub const COMPRESSIBLE: &[&str] = &[
    "js",
    "css",
    "html",
    "json",
    "svg",
    "webmanifest",
    "txt",
    "map",
];

/// Whether a request/staging path names an asset the embed stores
/// Brotli-compressed, decided by its extension (see the module docs: there is
/// no magic-byte fallback, this predicate is the contract).
pub fn compressible_path(path: &str) -> bool {
    std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| COMPRESSIBLE.contains(&e))
}
