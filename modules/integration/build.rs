use std::{env, fmt::Write, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=UNLINKA_WINDOWS_RESOURCE");
    println!("cargo:rerun-if-env-changed=UNLINKA_PACKAGE_VERSION");
    println!("cargo:rerun-if-env-changed=UNLINKA_PACKAGE_PUBLISHER");
    println!("cargo:rerun-if-env-changed=UNLINKA_SIGNED_BUILD");
    let signed = match env::var("UNLINKA_SIGNED_BUILD") {
        Ok(value) if value == "1" => true,
        Ok(value) if value == "0" => false,
        Err(env::VarError::NotPresent) => false,
        _ => panic!("UNLINKA_SIGNED_BUILD must be 0 or 1"),
    };
    if signed {
        let publisher = env::var("UNLINKA_PACKAGE_PUBLISHER")
            .expect("signed builds require UNLINKA_PACKAGE_PUBLISHER");
        assert!(
            !publisher.trim().is_empty(),
            "signed builds require a nonempty UNLINKA_PACKAGE_PUBLISHER"
        );
        println!("cargo:rustc-env=UNLINKA_PACKAGE_PUBLISHER={publisher}");
    }
    if let Some(resource) = env::var_os("UNLINKA_WINDOWS_RESOURCE") {
        println!(
            "cargo:rustc-link-arg-bin=unlinka-maintenance={}",
            PathBuf::from(resource).display()
        );
    }
    if let Ok(version) = env::var("UNLINKA_PACKAGE_VERSION") {
        println!("cargo:rustc-env=UNLINKA_PACKAGE_VERSION={version}");
    }
    println!("cargo:rerun-if-changed=../../packaging/identity.json");
    let identity: serde_json::Value = serde_json::from_slice(
        &fs::read("../../packaging/identity.json").expect("read product identity"),
    )
    .expect("valid product identity");
    let mut constants = String::new();
    writeln!(constants, "pub const SIGNED_BUILD: bool = {signed};").expect("write build mode");
    for (name, key, braces) in [
        ("CLASSIC_CLSID", "classic_clsid", true),
        ("MODERN_CLSID", "modern_clsid", true),
        ("ACTION_GUID", "action_guid", true),
        ("PACKAGE_NAME", "package_name", false),
        ("INSTALL_NAME", "install_name", false),
    ] {
        let value = identity[key].as_str().expect("identity field");
        let value = if braces {
            format!("{{{value}}}")
        } else {
            value.to_owned()
        };
        writeln!(constants, "pub const {name}: &str = {value:?};").expect("write identity");
    }
    let namespace = identity["install_name"].as_str().expect("install name");
    writeln!(constants, "pub(crate) const DATA_NAME: &str = {namespace:?};\npub(crate) const INSTALL_KEY: &str = {:?};\npub(crate) const SETTINGS_KEY: &str = {:?};", format!("Software\\{namespace}\\Installation"), format!("Software\\{namespace}\\Settings")).expect("write registry identity");
    fs::write(
        PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR")).join("identity.rs"),
        constants,
    )
    .expect("write identity constants");
}
