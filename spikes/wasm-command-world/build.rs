//! Builds the WASM guest components the spike's tests run.
//!
//! The guests live in `guests/` and are their own one-package workspaces, so
//! they are invisible to the host workspace's `cargo build`/`clippy` and can
//! target `wasm32-wasip2` without a per-target toolchain dance. Rust emits a
//! component (not a core module) for that target directly, so no
//! `cargo component` or `wasm-tools component new` step is needed.
//!
//! When the `wasm32-wasip2` standard library is not installed the guests cannot
//! be built; the script then emits `cfg(no_wasm_guests)` and the tests that
//! need a component compile out rather than fail.

use std::path::{Path, PathBuf};
use std::process::Command;

const GUESTS: [&str; 2] = ["marker", "looper"];
const TARGET: &str = "wasm32-wasip2";

fn main() {
    println!("cargo::rustc-check-cfg=cfg(no_wasm_guests)");
    println!("cargo::rerun-if-changed=guests");
    println!("cargo::rerun-if-changed=../../wit");

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
            "cargo::rustc-env=SPIKE_GUEST_{}={}",
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
    let guest_dir = manifest_dir.join("guests").join(guest);
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
        .join(format!("spike_guest_{guest}.wasm"));
    assert!(
        component.is_file(),
        "guest {guest} produced no component at {}",
        component.display()
    );
    component
}
