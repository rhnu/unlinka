#[cfg(windows)]
mod benchmark;

mod assets;

#[cfg(windows)]
mod packaging;

use std::process::ExitCode;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(about = "Unlinka development tasks")]
struct Cli {
    #[command(subcommand)]
    task: Task,
}

#[derive(Subcommand)]
enum Task {
    /// Generate Windows PNG and ICO resources from assets/Unlinka.svg.
    Assets {
        /// Check that committed resources match the SVG without writing files.
        #[arg(long)]
        check: bool,
    },
    /// Build a current-user x64 installer; signing also enables the Windows 11 menu.
    Pack {
        /// Certificate Subject; omit all signing options for an unsigned installer.
        #[arg(long, requires_all = ["certificate_thumbprint", "timestamp_url"])]
        publisher: Option<String>,
        /// Certificate in CurrentUser/My. Private keys never travel in arguments.
        #[arg(long, requires_all = ["publisher", "timestamp_url"])]
        certificate_thumbprint: Option<String>,
        /// RFC3161 timestamp endpoint required for a signed release.
        #[arg(long, requires_all = ["publisher", "certificate_thumbprint"])]
        timestamp_url: Option<String>,
        #[arg(long, default_value_t = 0)]
        build_number: u16,
        #[arg(long)]
        iscc: Option<std::path::PathBuf>,
        #[arg(long)]
        sdk_bin: Option<std::path::PathBuf>,
    },
    /// Compare fresh plain-file deletion on Windows and save JSON evidence.
    Benchmark {
        /// Existing unlinka executable; otherwise build the locked release binary.
        #[arg(long)]
        binary: Option<std::path::PathBuf>,
        #[arg(long, default_value_t = 12000, value_parser = clap::value_parser!(u32).range(1000..=1_000_000))]
        file_count: u32,
        #[arg(long, default_value_t = 3, value_parser = clap::value_parser!(u32).range(1..=10))]
        rounds: u32,
        /// Dataset shapes (comma-separated, or repeated).
        #[arg(long, value_delimiter = ',', default_value = "flat,tree", value_parser = ["flat", "tree"], num_args = 1..)]
        shapes: Vec<String>,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.task {
        Task::Assets { check } => assets::run(check),
        #[cfg(windows)]
        Task::Pack {
            publisher,
            certificate_thumbprint,
            timestamp_url,
            build_number,
            iscc,
            sdk_bin,
        } => packaging::run(&packaging::Options {
            publisher,
            certificate_thumbprint,
            timestamp_url,
            build_number,
            iscc,
            sdk_bin,
        }),
        #[cfg(windows)]
        Task::Benchmark {
            binary,
            file_count,
            rounds,
            shapes,
        } => benchmark::run(binary, file_count, rounds, &shapes),
        #[cfg(not(windows))]
        _ => Err(std::io::Error::other(
            "packaging and the deletion benchmark require Windows",
        )),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        },
    }
}
