use std::path::PathBuf;

use clap::{ArgAction, Parser, ValueEnum};

/// Command-line choices for permanent deletion and its confirmation boundary.
#[derive(Debug, Parser)]
#[command(
    name = "unlinka",
    version,
    about = "Permanently delete Windows files and directories",
    long_about = "Permanently delete Windows files and directories. Deleted items do not enter the Recycle Bin. Directory contents require --recursive. Reparse points are removed as links without following their targets."
)]
pub(crate) struct Cli {
    /// Files, directories, or links to delete; use -- before paths beginning with -.
    #[arg(value_name = "PATH", required = true, num_args = 1..)]
    pub(crate) paths: Vec<PathBuf>,

    /// Delete directory contents recursively.
    #[arg(short = 'r', long)]
    pub(crate) recursive: bool,

    /// Confirmation interface (default: native).
    #[arg(long, value_enum, conflicts_with = "yes")]
    pub(crate) confirm: Option<Confirmation>,

    /// Skip confirmation and permanently delete the selected targets.
    #[arg(short = 'y', long)]
    pub(crate) yes: bool,

    /// Scan and report counts without deleting anything or asking for confirmation.
    #[arg(long)]
    pub(crate) dry_run: bool,

    /// Total thread budget, including scanning, from 1 to 64.
    #[arg(long, value_name = "COUNT", default_value_t = default_threads(), value_parser = parse_threads)]
    pub(crate) threads: usize,

    /// Print additional preparation details.
    #[arg(short = 'v', long, action = ArgAction::Count)]
    pub(crate) verbose: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(crate) enum Confirmation {
    Native,
    Console,
}

impl Cli {
    pub(crate) fn confirmation(&self) -> Confirmation {
        self.confirm.unwrap_or(Confirmation::Native)
    }
}

/// Stable process exit codes for scripts and interactive callers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub(crate) enum ExitStatus {
    Success            = 0,
    ExecutionFailed    = 1,
    InvalidInput       = 2,
    Cancelled          = 3,
    ConfirmationFailed = 4,
}

impl ExitStatus {
    pub(crate) const fn for_outcome(cancelled: bool, failed: u64) -> Self {
        if cancelled {
            Self::Cancelled
        } else if failed > 0 {
            Self::ExecutionFailed
        } else {
            Self::Success
        }
    }
}

fn default_threads() -> usize {
    std::thread::available_parallelism()
        .map_or(1, std::num::NonZeroUsize::get)
        .min(4)
}

fn parse_threads(value: &str) -> Result<usize, String> {
    let threads = value
        .parse::<usize>()
        .map_err(|error| format!("thread budget must be an integer from 1 to 64: {error}"))?;
    if (1..=64).contains(&threads) {
        Ok(threads)
    } else {
        Err("thread budget must be an integer from 1 to 64".to_owned())
    }
}

#[cfg(test)]
mod tests;
