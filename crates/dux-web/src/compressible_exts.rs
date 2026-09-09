// The one list of asset extensions that travel Brotli-compressed through the
// embed. `build.rs` `include!`s this file and compresses by it while staging
// `web/dist`; `web_assets.rs` uses it as a module and serves those paths with
// `Content-Encoding: br`. Brotli has no magic bytes, so the server cannot sniff
// compressed-ness and the extension is the whole contract: keep the list here
// alone, and keep every staging path going through `copy_tree`.

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
