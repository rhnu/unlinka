//! Cargo release configuration for the immutable installer binaries.
use std::{env, io, path::Path, process::Command};

use super::PackageBuild;

pub(super) fn configure(
    build: &mut Command,
    context: &PackageBuild<'_>,
    target: &Path,
    application_resource: &Path,
    dll_resource: &Path,
) -> io::Result<()> {
    let flags = static_crt_flags(
        read_flags("CARGO_ENCODED_RUSTFLAGS")?.as_deref(),
        read_flags("RUSTFLAGS")?.as_deref(),
        read_flags("CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS")?.as_deref(),
    );
    build
        .args([
            "build",
            "--release",
            "--locked",
            "--target",
            "x86_64-pc-windows-msvc",
            "-p",
            "unlinka",
            "-p",
            "unlinka-desktop",
            "-p",
            "unlinka-integration",
            "-p",
            "unlinka-launcher",
            "-p",
            "unlinka-shell",
        ])
        .current_dir(context.root)
        .env("CARGO_ENCODED_RUSTFLAGS", flags)
        .env("CARGO_TARGET_DIR", target)
        .env(
            "UNLINKA_SIGNED_BUILD",
            if context.options.signed() { "1" } else { "0" },
        )
        .env("UNLINKA_PACKAGE_VERSION", context.version)
        .env("UNLINKA_WINDOWS_RESOURCE", application_resource)
        .env("UNLINKA_DLL_RESOURCE", dll_resource);
    if context.options.signed() {
        build.env("UNLINKA_SIGNER_SUBJECT", context.options.publisher());
    } else {
        build.env_remove("UNLINKA_SIGNER_SUBJECT");
    }
    Ok(())
}

fn read_flags(name: &str) -> io::Result<Option<String>> {
    env::var_os(name)
        .map(|value| {
            value.into_string().map_err(|_non_unicode| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("{name} must be Unicode"),
                )
            })
        })
        .transpose()
}

/// Preserve explicit Cargo flag precedence and make every distributed binary self-contained.
fn static_crt_flags(encoded: Option<&str>, plain: Option<&str>, target: Option<&str>) -> String {
    let mut flags: Vec<&str> = match encoded {
        Some(value) => value
            .split('\u{1f}')
            .filter(|flag| !flag.is_empty())
            .collect(),
        None => plain
            .or(target)
            .unwrap_or_default()
            .split_whitespace()
            .collect(),
    };
    // The final target-feature setting wins over an inherited dynamic CRT request.
    flags.extend(["-C", "target-feature=+crt-static"]);
    flags.join("\u{1f}")
}

#[cfg(test)]
mod tests;
