use std::env;

#[path = "../../packaging/windows.rs"]
mod windows;

fn xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Only explicit signed builds embed the package identity supplied by packaging.
fn main() {
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
    let identity = if signed {
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
        Some(format!(
            "<msix xmlns=\"urn:schemas-microsoft-com:msix.v1\" publisher=\"{}\" packageName=\"{}\" applicationId=\"{}\" />",
            values[1], values[0], values[2]
        ))
    } else {
        None
    };
    windows::embed("unlinka-desktop", identity.as_deref());
}
