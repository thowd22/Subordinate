//! Builds the WASM guest components the runtime tests run against.
//!
//! The guests live in `tests/guests/` and are their own one-package
//! workspaces, so they are invisible to the host workspace's `cargo build` and
//! `cargo clippy` and can target `wasm32-wasip2` without a per-target
//! toolchain dance. Rust emits a component (not a core module) for that target
//! directly, so no `cargo component` or `wasm-tools component new` step is
//! needed.
//!
//! Building them is not free, and nothing outside this crate's own tests wants
//! them, so the work is gated on the `test-guests` feature, which the crate
//! turns on for itself through a dev-dependency. An ordinary
//! `cargo build -p sub-plugin` compiles no WASM at all.
//!
//! When the `wasm32-wasip2` standard library is not installed the guests
//! cannot be built; the script then emits `cfg(no_wasm_guests)` and the tests
//! that need a component compile out rather than fail.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The guests, by directory name under `tests/guests`.
const GUESTS: [&str; 6] = ["looper", "hog", "counter", "editor", "tinter", "toolbox"];
const TARGET: &str = "wasm32-wasip2";

fn main() {
    println!("cargo::rustc-check-cfg=cfg(no_wasm_guests)");
    println!("cargo::rerun-if-changed=tests/guests");
    println!("cargo::rerun-if-changed=../../wit");

    if std::env::var_os("CARGO_FEATURE_TEST_GUESTS").is_none() {
        // Nothing in a plain build of the library needs a guest component.
        println!("cargo::rustc-cfg=no_wasm_guests");
        return;
    }

    let manifest_dir = PathBuf::from(env("CARGO_MANIFEST_DIR"));
    let out_dir = PathBuf::from(env("OUT_DIR"));

    if !target_installed() {
        println!("cargo::warning=the {TARGET} target is not installed; skipping the WASM guests");
        println!("cargo::rustc-cfg=no_wasm_guests");
        return;
    }

    for guest in GUESTS {
        let component = build_guest(&manifest_dir, &out_dir, guest);
        println!(
            "cargo::rustc-env=SUB_PLUGIN_GUEST_{}={}",
            guest.to_uppercase(),
            component.display()
        );
    }
}

/// Reads a build-script environment variable that Cargo always sets.
fn env(key: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| panic!("cargo did not set {key}"))
}

/// Returns true if the `wasm32-wasip2` standard library is available.
fn target_installed() -> bool {
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_owned());
    Command::new(rustc)
        .args(["--print", "target-libdir", "--target", TARGET])
        .output()
        .is_ok_and(|out| out.status.success())
}

/// Builds one guest and returns the path of the component it produced.
fn build_guest(manifest_dir: &Path, out_dir: &Path, guest: &str) -> PathBuf {
    let guest_dir = manifest_dir.join("tests").join("guests").join(guest);
    let target_dir = out_dir.join("guest-target");

    let mut cmd = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned()));
    cmd.current_dir(&guest_dir)
        .args(["build", "--release", "--target", TARGET])
        .arg("--target-dir")
        .arg(&target_dir);

    // A nested cargo must not inherit the outer build's flags, wrappers or
    // jobserver, or it silently builds for the wrong target or deadlocks.
    for key in [
        "RUSTFLAGS",
        "CARGO_ENCODED_RUSTFLAGS",
        "RUSTC_WORKSPACE_WRAPPER",
        "RUSTC_WRAPPER",
        "CARGO_BUILD_TARGET",
        "CARGO_BUILD_RUSTFLAGS",
        "CARGO_MAKEFLAGS",
        "CARGO_UNSTABLE_BUILD_STD",
    ] {
        cmd.env_remove(key);
    }

    let status = cmd
        .status()
        .unwrap_or_else(|err| panic!("failed to run cargo for guest {guest}: {err}"));
    assert!(status.success(), "cargo build failed for guest {guest}");

    let component = target_dir
        .join(TARGET)
        .join("release")
        .join(format!("sub_plugin_guest_{guest}.wasm"));
    assert!(
        component.is_file(),
        "guest {guest} produced no component at {}",
        component.display()
    );
    component
}
