use std::env;

fn main() {
    println!("cargo:rerun-if-env-changed=DUX_RELEASE_BUILD");

    let display_version = if env::var("DUX_RELEASE_BUILD").as_deref() == Ok("1") {
        format!("v{}", env!("CARGO_PKG_VERSION"))
    } else {
        "development".to_string()
    };

    println!("cargo:rustc-env=DUX_DISPLAY_VERSION={display_version}");
}
