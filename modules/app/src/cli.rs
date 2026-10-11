use std::path::PathBuf;

use clap::{ArgAction, CommandFactory, Parser, ValueEnum};
use unlinka_i18n::Translator;

/// Command-line choices for permanent deletion and its confirmation boundary.
#[derive(Debug, Parser)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "Clap exposes independent command-line switches directly"
)]
#[command(
    name = "unlinka",
    version,
    about = "Permanently delete Windows files and directories",
    long_about = "Permanently delete Windows files and directories. Deleted items do not enter the Recycle Bin. Directory contents require --recursive. Reparse points are removed as links without following their targets."
)]
pub(crate) struct Cli {
    /// Files, directories, or links to delete; use -- before paths beginning with -.
    #[arg(value_name = "PATH", required_unless_present = "integration_status", num_args = 1..)]
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

    #[arg(long, value_parser = ["auto", "zh-CN", "en-US"])]
    pub(crate) lang: Option<String>,

    #[arg(long)]
    pub(crate) json: bool,

    #[arg(long, conflicts_with_all = ["paths", "recursive", "confirm", "yes", "dry_run", "threads"])]
    pub(crate) integration_status: bool,
}

/// Localizes clap's declarative help before it renders any parser result.
pub(crate) fn command(translator: &Translator) -> clap::Command {
    let mut command = Cli::command()
        .disable_help_flag(true)
        .disable_version_flag(true)
        .arg(clap::Arg::new("help").short('h').long("help").action(ArgAction::Help))
        .arg(clap::Arg::new("version").short('V').long("version").action(ArgAction::Version))
        .about(translator.text("cli-about"))
        .long_about(translator.text("cli-long-about"))
        .help_template(format!("{{name}} {{version}}\n{{about-with-newline}}\n{} {{usage}}\n\n{{all-args}}{{after-help}}", translator.text("cli-usage")));
    for (argument, message) in [
        ("paths", "help-paths"),
        ("recursive", "help-recursive"),
        ("confirm", "help-confirm"),
        ("yes", "help-yes"),
        ("dry_run", "help-dry-run"),
        ("threads", "help-threads"),
        ("verbose", "help-verbose"),
        ("lang", "help-lang"),
        ("json", "help-json"),
        ("integration_status", "help-status"),
        ("help", "help-help"),
        ("version", "help-version"),
    ] {
        command = command.mut_arg(argument, |arg| {
            let arg = if arg.get_action().takes_values() {
                arg.hide_possible_values(true).hide_default_value(true)
            } else {
                arg
            };
            arg.help(translator.text(message))
                .help_heading(if argument == "paths" {
                    translator.text("cli-arguments")
                } else {
                    translator.text("cli-options")
                })
        });
    }
    command
}

/// Bootstrap options stop at `--`, retaining filename semantics.
pub(crate) fn bootstrap(arguments: &[std::ffi::OsString]) -> (Option<String>, bool) {
    let mut language = None;
    let json = arguments
        .iter()
        .skip(1)
        .take_while(|argument| *argument != "--")
        .any(|argument| argument == "--json");
    let mut arguments = arguments.iter().skip(1);
    while let Some(argument) = arguments.next() {
        if argument == "--" {
            break;
        }
        if argument == "--lang" {
            language = arguments
                .next()
                .map(|value| value.to_string_lossy().into_owned());
        } else if let Some(value) = argument
            .to_str()
            .and_then(|arg| arg.strip_prefix("--lang="))
        {
            language = Some(value.to_owned());
        }
    }
    (language, json)
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
