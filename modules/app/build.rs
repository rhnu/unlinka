use std::{env, fs, path::PathBuf};

/// Embeds the CLI's Windows manifest and optional packaged resources.
fn main() {
    println!("cargo:rerun-if-env-changed=UNLINKA_WINDOWS_RESOURCE");
    println!("cargo:rerun-if-env-changed=UNLINKA_PACKAGE_VERSION");
    println!("cargo:rerun-if-changed=unlinka.manifest");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        if let Some(resource) = env::var_os("UNLINKA_WINDOWS_RESOURCE") {
            println!(
                "cargo:rustc-link-arg-bin=unlinka={}",
                PathBuf::from(resource).display()
            );
        }
        assert_eq!(
            env::var("CARGO_CFG_TARGET_ENV").as_deref(),
            Ok("msvc"),
            "Unlinka currently requires a Windows MSVC target"
        );
        let template = PathBuf::from(
            env::var_os("CARGO_MANIFEST_DIR")
                .expect("Cargo provides the package manifest directory"),
        )
        .join("unlinka.manifest");
        let version = env::var("UNLINKA_PACKAGE_VERSION")
            .unwrap_or_else(|_| format!("{}.0", env!("CARGO_PKG_VERSION")));
        assert!(
            version.split('.').count() == 4
                && version.split('.').all(|part| !part.is_empty()
                    && part.bytes().all(|ch| ch.is_ascii_digit())
                    && part.parse::<u16>().is_ok()),
            "UNLINKA_PACKAGE_VERSION must be four bounded numeric components"
        );
        let source = fs::read_to_string(template)
            .expect("read application manifest")
            .replace("@PACKAGE_VERSION@", &version);
        let manifest =
            PathBuf::from(env::var_os("OUT_DIR").expect("Cargo OUT_DIR")).join("unlinka.manifest");
        fs::write(&manifest, source).expect("write generated application manifest");
        println!("cargo:rustc-link-arg-bin=unlinka=/MANIFEST:EMBED");
        println!(
            "cargo:rustc-link-arg-bin=unlinka=/MANIFESTINPUT:{}",
            manifest.display()
        );
    }
}
