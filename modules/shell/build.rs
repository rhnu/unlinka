//! Links the release resource compiled by the packaging task without mutating a signed binary.

fn main() {
    println!("cargo:rerun-if-changed=shell.def");
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        let definition = std::path::PathBuf::from(
            std::env::var_os("CARGO_MANIFEST_DIR")
                .expect("Cargo provides the shell manifest directory"),
        )
        .join("shell.def");
        println!("cargo:rustc-link-arg-cdylib=/DEF:{}", definition.display());
    }
    println!("cargo:rerun-if-env-changed=UNLINKA_DLL_RESOURCE");
    if let Some(resource) = std::env::var_os("UNLINKA_DLL_RESOURCE") {
        let path = std::path::PathBuf::from(resource);
        assert!(
            path.is_file(),
            "UNLINKA_DLL_RESOURCE must name an existing resource file"
        );
        println!("cargo:rerun-if-changed={}", path.display());
        println!("cargo:rustc-link-arg-cdylib={}", path.display());
    }
}
