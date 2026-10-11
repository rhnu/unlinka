use std::{env, fs, path::PathBuf};

fn xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Only explicit signed builds embed the package identity supplied by packaging.
fn main() {
    println!("cargo:rerun-if-env-changed=UNLINKA_WINDOWS_RESOURCE");
    println!("cargo:rerun-if-env-changed=UNLINKA_PACKAGE_VERSION");
    println!("cargo:rerun-if-env-changed=UNLINKA_SIGNED_BUILD");
    let signed = match env::var("UNLINKA_SIGNED_BUILD") {
        Ok(value) if value == "1" => true,
        Ok(value) if value == "0" => false,
        Err(env::VarError::NotPresent) => false,
        _ => panic!("UNLINKA_SIGNED_BUILD must be 0 or 1"),
    };
    for key in [
        "UNLINKA_PACKAGE_NAME",
        "UNLINKA_PACKAGE_PUBLISHER",
        "UNLINKA_PACKAGE_APP_ID",
    ] {
        println!("cargo:rerun-if-env-changed={key}");
    }
    println!("cargo:rerun-if-changed=../app/unlinka.manifest");
    let mut manifest =
        fs::read_to_string("../app/unlinka.manifest").expect("native application manifest");
    let version = env::var("UNLINKA_PACKAGE_VERSION")
        .unwrap_or_else(|_| format!("{}.0", env!("CARGO_PKG_VERSION")));
    assert!(
        version.split('.').count() == 4
            && version.split('.').all(|part| !part.is_empty()
                && part.bytes().all(|ch| ch.is_ascii_digit())
                && part.parse::<u16>().is_ok()),
        "UNLINKA_PACKAGE_VERSION must be four bounded numeric components"
    );
    manifest = manifest.replace("@PACKAGE_VERSION@", &version);
    if signed {
        let values: Vec<_> = [
            "UNLINKA_PACKAGE_NAME",
            "UNLINKA_PACKAGE_PUBLISHER",
            "UNLINKA_PACKAGE_APP_ID",
        ]
        .into_iter()
        .map(|key| {
            let value = env::var(key).unwrap_or_else(|_| panic!("signed builds require {key}"));
            assert!(
                !value.trim().is_empty(),
                "signed builds require a nonempty {key}"
            );
            xml(&value)
        })
        .collect();
        let identity = format!(
            "<msix xmlns=\"urn:schemas-microsoft-com:msix.v1\" publisher=\"{}\" packageName=\"{}\" applicationId=\"{}\" />",
            values[1], values[0], values[2]
        );
        manifest = manifest.replace("</assembly>", &format!("{identity}\n</assembly>"));
    }
    let path = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo OUT_DIR"))
        .join("unlinka-desktop.manifest");
    fs::write(&path, manifest).expect("write generated manifest");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        if let Some(resource) = env::var_os("UNLINKA_WINDOWS_RESOURCE") {
            println!(
                "cargo:rustc-link-arg-bin=unlinka-desktop={}",
                PathBuf::from(resource).display()
            );
        }
        println!("cargo:rustc-link-arg-bin=unlinka-desktop=/MANIFEST:EMBED");
        println!(
            "cargo:rustc-link-arg-bin=unlinka-desktop=/MANIFESTINPUT:{}",
            path.display()
        );
    }
}
