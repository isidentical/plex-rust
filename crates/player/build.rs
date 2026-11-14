//! Links the static libmpv from `vendor/libmpv` and what it depends on.

use std::path::PathBuf;
use std::process::Command;

fn main() {
    if let Err(error) = pkg_config::Config::new().statik(true).probe("mpv") {
        panic!("libmpv not found, run scripts/build-libmpv.sh first: {error}");
    }
    // FreeType names zlib by file path, which the pkg-config crate skips.
    println!("cargo:rustc-link-lib=z");
    // libplacebo has C++ sources but its pkg-config file does not say so.
    println!("cargo:rustc-link-lib=c++");

    // libmpv contains Swift objects. They name the Swift libraries they need
    // themselves; the linker only has to be told where those live.
    let sdk = xcrun(&["--show-sdk-path"]);
    let toolchain = xcrun(&["--find", "swiftc"]).join("../../lib/swift/macosx");
    println!(
        "cargo:rustc-link-search=native={}",
        sdk.join("usr/lib/swift").display()
    );
    println!("cargo:rustc-link-search=native={}", toolchain.display());
}

fn xcrun(args: &[&str]) -> PathBuf {
    let output = Command::new("xcrun")
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("xcrun {args:?} did not start: {error}"));
    assert!(output.status.success(), "xcrun {args:?} failed");
    PathBuf::from(String::from_utf8_lossy(&output.stdout).trim())
}
