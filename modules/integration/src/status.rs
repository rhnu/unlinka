use std::ffi::OsString;

use clap::{
    Arg, ArgAction, Command,
    error::{ContextKind, ErrorKind},
};
use serde_json::Value;
use unlinka_i18n::{Translator, select_locale, windows_languages};

/// Status-only fallback used after uninstall has already removed the active CLI.
///
/// Reports parse and status-query failures through the returned exit code.
pub fn query(arguments: &[OsString]) -> i32 {
    let (explicit, json) = bootstrap(arguments);
    let environment = std::env::var("UNLINKA_LANG").ok();
    let settings = crate::read_settings().ok();
    let translator = Translator::new(select_locale(
        explicit.as_deref(),
        environment.as_deref(),
        settings.as_ref().map(|s| s.language.as_str()),
        &windows_languages(),
    ));
    if arguments
        .iter()
        .take_while(|a| *a != "--")
        .any(|a| a == "--help" || a == "-h")
    {
        println!(
            "{}",
            translator.format(
                "maintenance-launcher-help",
                &[(
                    "reason",
                    translator.text("maintenance-installation-missing").into()
                )]
            )
        );
        return 0;
    }
    if arguments
        .iter()
        .take_while(|a| *a != "--")
        .any(|a| a == "--version" || a == "-V")
    {
        println!("unlinka {}", env!("CARGO_PKG_VERSION"));
        return 0;
    }
    let mut args = vec![OsString::from("unlinka")];
    args.extend_from_slice(arguments);
    if let Err(error) = query_command().try_get_matches_from(args) {
        query_error(&translator, &error, json);
        return 2;
    }
    match crate::maintenance::integration_status() {
        Ok(value) => {
            if json {
                println!("{}", document(0, Some(value), None));
            } else {
                println!("{}", format(&translator, &value));
            }
            0
        },
        Err(error) => {
            eprintln!(
                "[integration_status_failed] {}: {error}",
                translator.text("error-installation")
            );
            if json {
                println!(
                    "{}",
                    document(
                        1,
                        None,
                        Some(("integration_status_failed", "integration_status"))
                    )
                );
            }
            1
        },
    }
}

fn query_command() -> Command {
    Command::new("unlinka")
        .disable_help_flag(true)
        .disable_version_flag(true)
        .arg(
            Arg::new("integration_status")
                .long("integration-status")
                .action(ArgAction::SetTrue)
                .required(true)
                .conflicts_with_all(["paths", "recursive", "yes", "dry_run", "threads"]),
        )
        .arg(Arg::new("json").long("json").action(ArgAction::SetTrue))
        .arg(
            Arg::new("lang")
                .long("lang")
                .value_parser(["auto", "en-US", "zh-CN"]),
        )
        .arg(
            Arg::new("verbose")
                .short('v')
                .long("verbose")
                .action(ArgAction::Count),
        )
        .arg(Arg::new("paths").num_args(1..))
        .arg(
            Arg::new("recursive")
                .short('r')
                .long("recursive")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("yes")
                .short('y')
                .long("yes")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("dry_run")
                .long("dry-run")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("threads")
                .long("threads")
                .value_parser(clap::value_parser!(u8).range(1..=64)),
        )
}

fn bootstrap(arguments: &[OsString]) -> (Option<String>, bool) {
    let json = arguments
        .iter()
        .take_while(|a| *a != "--")
        .any(|a| a == "--json");
    let mut explicit = None;
    let mut remaining = arguments.iter();
    while let Some(argument) = remaining.next() {
        if argument == "--" {
            break;
        }
        if argument == "--lang" {
            explicit = remaining
                .next()
                .map(|value| value.to_string_lossy().into_owned());
        } else if let Some(value) = argument
            .to_str()
            .and_then(|value| value.strip_prefix("--lang="))
        {
            explicit = Some(value.to_owned());
        }
    }
    (explicit, json)
}

fn document(code: i32, integration: Option<Value>, diagnostic: Option<(&str, &str)>) -> Value {
    let diagnostics: Vec<_> = diagnostic.into_iter().map(|(code, operation)| serde_json::json!({"code":code,"operation":operation,"path_utf16":null,"os_error":null})).collect();
    let mut document = serde_json::json!({"schema_version":1,"outcome":match code {0=>"success",2=>"invalid_input",_=>"execution_failed"},"exit_code":code,"dry_run":false,"files":0,"directories":0,"links":0,"failed":0,"cancelled":false,"elapsed_seconds":0.0,"paths_utf16":[],"diagnostics":diagnostics});
    if let Some(value) = integration {
        document["integration"] = value;
    }
    document
}

#[cfg(test)]
mod tests;

/// Converts the shared machine status into localized, readable query output.
#[must_use]
pub fn format(tr: &Translator, value: &Value) -> String {
    if value.get("installed").and_then(Value::as_bool) == Some(false) {
        return tr.text("status-portable");
    }
    let mut lines = vec![tr.text("status-installed")];
    if let Some(object) = value.as_object() {
        for (key, value) in object {
            if matches!(key.as_str(), "installed" | "root_utf16") {
                continue;
            }
            let label = match key.as_str() {
                "root" | "root_display" => "status-root",
                "active_version" => "status-version",
                "state" => "status-state",
                "programs" => "status-programs",
                "classic_menu" => "status-classic",
                "signed" => "status-signed",
                "signer_subject" => "status-signer",
                "path_present" => "settings-path",
                "path_owned" => "status-path-owned",
                "pending_cleanup" | "pending_cleanup_utf16" => "status-cleanup",
                "transaction_pending" => "status-transaction",
                "settings" => "settings-title",
                _ => key,
            };
            if let Some(object) = value.as_object() {
                lines.push(format!("{}:", tr.text(label)));
                for (key, value) in object {
                    let nested = match key.as_str() {
                        "language" => tr.text("settings-language"),
                        "menus_enabled" => tr.text("settings-menus"),
                        "path_enabled" => tr.text("settings-path"),
                        _ => key.to_owned(),
                    };
                    lines.push(format!("  {nested}: {}", display(tr, value)));
                }
            } else {
                lines.push(format!("{}: {}", tr.text(label), display(tr, value)));
            }
        }
    }
    lines.join("\n")
}

fn display(tr: &Translator, value: &Value) -> String {
    match value {
        Value::Bool(value) => tr.text(if *value { "status-yes" } else { "status-no" }),
        Value::Null => tr.text("status-not-applicable"),
        Value::String(value) => match value.as_str() {
            "Installing" => tr.text("status-installing"),
            "Ready" => tr.text("status-ready"),
            "Updating" => tr.text("status-updating"),
            "RepairRequired" => tr.text("status-repair"),
            "Removing" => tr.text("status-removing"),
            "RemovalPending" => tr.text("status-removal-pending"),
            "auto" => tr.text("settings-auto"),
            _ => value.clone(),
        },
        Value::Array(values) => values.len().to_string(),
        _ => value.to_string(),
    }
}

fn query_error(translator: &Translator, error: &clap::Error, json: bool) {
    let argument = error
        .get(ContextKind::InvalidArg)
        .map(ToString::to_string)
        .unwrap_or_default();
    let value = error
        .get(ContextKind::InvalidValue)
        .map(ToString::to_string)
        .unwrap_or_default();
    let id = match error.kind() {
        ErrorKind::UnknownArgument => "error-unknown",
        ErrorKind::MissingRequiredArgument => "error-missing",
        ErrorKind::ArgumentConflict => "error-conflict",
        ErrorKind::InvalidValue
        | ErrorKind::ValueValidation
        | ErrorKind::TooFewValues
        | ErrorKind::TooManyValues => "error-value",
        _ => "error-arguments",
    };
    eprintln!(
        "[invalid_arguments] {}\n{}",
        translator.format(
            id,
            &[
                ("argument", argument.as_str().into()),
                ("value", value.as_str().into())
            ]
        ),
        translator.text("error-hint")
    );
    if json {
        println!(
            "{}",
            document(2, None, Some(("invalid_arguments", "parse_arguments")))
        );
    }
}
