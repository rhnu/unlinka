#[cfg(windows)]
mod benchmark;

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
    #[cfg(windows)]
    let result = match cli.task {
        Task::Benchmark {
            binary,
            file_count,
            rounds,
            shapes,
        } => benchmark::run(binary, file_count, rounds, &shapes),
    };
    #[cfg(not(windows))]
    let result: std::io::Result<()> = {
        let _ = cli;
        Err(std::io::Error::other(
            "the deletion benchmark requires Windows",
        ))
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        },
    }
}
