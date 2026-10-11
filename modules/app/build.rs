use std::{env, path::PathBuf};

/// Embeds Common Controls v6 so `TaskDialogIndirect` uses the native Windows UI.
fn main() {
    println!("cargo:rerun-if-changed=unlinka.manifest");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        assert_eq!(
            env::var("CARGO_CFG_TARGET_ENV").as_deref(),
            Ok("msvc"),
            "Unlinka currently requires a Windows MSVC target"
        );
        let manifest = PathBuf::from(
            env::var_os("CARGO_MANIFEST_DIR")
                .expect("Cargo provides the package manifest directory"),
        )
        .join("unlinka.manifest");
        println!("cargo:rustc-link-arg-bin=unlinka=/MANIFEST:EMBED");
        println!(
            "cargo:rustc-link-arg-bin=unlinka=/MANIFESTINPUT:{}",
            manifest.display()
        );
    }
}
