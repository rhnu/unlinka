use std::{env, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=UNLINKA_WINDOWS_RESOURCE");
    println!("cargo:rerun-if-env-changed=UNLINKA_PACKAGE_VERSION");
    if let Some(resource) = env::var_os("UNLINKA_WINDOWS_RESOURCE") {
        println!("cargo:rustc-link-arg={}", PathBuf::from(resource).display());
    }
    if let Ok(version) = env::var("UNLINKA_PACKAGE_VERSION") {
        println!("cargo:rustc-env=UNLINKA_PACKAGE_VERSION={version}");
    }
}
