use std::env;
use std::path::PathBuf;

fn main() {
    // Tell rustc/clippy that this is an expected cfg value.
    println!("cargo:rustc-check-cfg=cfg(vendored_bwrap_available)");
    println!("cargo:rerun-if-env-changed=COLOSSAL_BWRAP_SOURCE_DIR");

    // Rebuild if the vendored bwrap sources change.
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap_or_default());
    let vendor_dir = manifest_dir.join("../../vendor/bubblewrap");
    for source_file in &["bubblewrap.c", "bind-mount.c", "network.c", "utils.c"] {
        println!(
            "cargo:rerun-if-changed={}",
            vendor_dir.join(source_file).display()
        );
    }

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "linux" {
        return;
    }

    match try_build_vendored_bwrap(&vendor_dir) {
        Ok(()) => {}
        Err(err) => {
            // Don't hard-fail the build if libcap is missing or bwrap sources
            // aren't available. The runtime will fall back to system bwrap or
            // landlock (with a clear error if neither works).
            eprintln!(
                "cargo:warning=Could not compile vendored bubblewrap: {err}. \
                 Sandbox will require system bubblewrap (bwrap) to be installed."
            );
        }
    }
}

fn try_build_vendored_bwrap(vendor_dir: &PathBuf) -> Result<(), String> {
    let out_dir = PathBuf::from(env::var("OUT_DIR").map_err(|err| err.to_string())?);

    if !vendor_dir.exists() {
        return Err(format!(
            "vendored bubblewrap sources not found at {}",
            vendor_dir.display()
        ));
    }

    let libcap = pkg_config::Config::new()
        .probe("libcap")
        .map_err(|err| format!("libcap not available via pkg-config: {err}"))?;

    let config_h = out_dir.join("config.h");
    std::fs::write(
        &config_h,
        r#"#pragma once
#define PACKAGE_STRING "bubblewrap built at nite build-time"
"#,
    )
    .map_err(|err| format!("failed to write {}: {err}", config_h.display()))?;

    let mut build = cc::Build::new();
    build
        .file(vendor_dir.join("bubblewrap.c"))
        .file(vendor_dir.join("bind-mount.c"))
        .file(vendor_dir.join("network.c"))
        .file(vendor_dir.join("utils.c"))
        .include(&out_dir)
        .include(vendor_dir)
        .define("_GNU_SOURCE", None)
        // Rename `main` so we can call it via FFI.
        .define("main", Some("bwrap_main"));
    for include_path in libcap.include_paths {
        build.flag(format!("-idirafter{}", include_path.display()));
    }

    build.compile("build_time_bwrap");
    println!("cargo:rustc-cfg=vendored_bwrap_available");
    Ok(())
}
