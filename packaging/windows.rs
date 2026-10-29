//! Shared executable manifests for Windows builds.
use std::{env, fs, path::PathBuf};

pub fn embed(binary: &str) {
    println!("cargo:rerun-if-env-changed=UNLINKA_WINDOWS_RESOURCE");
    println!("cargo:rerun-if-env-changed=UNLINKA_PACKAGE_VERSION");
    println!("cargo:rerun-if-changed=../../packaging/windows.rs");
    println!("cargo:rerun-if-changed=../../packaging/unlinka.manifest");
    if let Ok(version) = env::var("UNLINKA_PACKAGE_VERSION") {
        println!("cargo:rustc-env=UNLINKA_PACKAGE_VERSION={version}");
    }
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    assert_eq!(
        env::var("CARGO_CFG_TARGET_ENV").as_deref(),
        Ok("msvc"),
        "Unlinka currently requires a Windows MSVC target"
    );
    if let Some(resource) = env::var_os("UNLINKA_WINDOWS_RESOURCE") {
        println!(
            "cargo:rustc-link-arg-bin={binary}={}",
            PathBuf::from(resource).display()
        );
    }
    let version = env::var("UNLINKA_PACKAGE_VERSION")
        .unwrap_or_else(|_| format!("{}.0", env!("CARGO_PKG_VERSION")));
    assert!(
        version.split('.').count() == 4
            && version.split('.').all(|part| !part.is_empty()
                && part.bytes().all(|ch| ch.is_ascii_digit())
                && part.parse::<u16>().is_ok()),
        "UNLINKA_PACKAGE_VERSION must be four bounded numeric components"
    );
    let template =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("Cargo manifest directory"))
            .join("../../packaging/unlinka.manifest");
    let manifest = fs::read_to_string(template)
        .expect("read application manifest")
        .replace("@BINARY_NAME@", binary)
        .replace("@PACKAGE_VERSION@", &version);
    let path = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo OUT_DIR"))
        .join(format!("{binary}.manifest"));
    fs::write(&path, manifest).expect("write generated application manifest");
    println!("cargo:rustc-link-arg-bin={binary}=/MANIFEST:EMBED");
    println!(
        "cargo:rustc-link-arg-bin={binary}=/MANIFESTINPUT:{}",
        path.display()
    );
}
