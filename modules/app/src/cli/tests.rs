use clap::{Parser, error::ErrorKind};

use super::{Cli, Confirmation, ExitStatus};

#[test]
fn requires_at_least_one_path() {
    let error = Cli::try_parse_from(["unlinka"]).expect_err("a target path is required");
    assert_eq!(error.kind(), ErrorKind::MissingRequiredArgument);
}

#[test]
fn accepts_multiple_paths_and_recursive_deletion() {
    let cli = Cli::try_parse_from(["unlinka", "-r", "-v", "first", "second"])
        .expect("multiple paths with recursion are valid");
    assert_eq!(
        cli.paths,
        [std::path::PathBuf::from("first"), "second".into()]
    );
    assert!(cli.recursive);
    assert_eq!(cli.verbose, 1);
    assert_eq!(cli.confirmation(), Confirmation::Native);
}

#[test]
fn yes_works_without_an_explicit_confirmation_option() {
    let cli = Cli::try_parse_from(["unlinka", "-y", "file"])
        .expect("yes without an explicit confirmation mode is valid");
    assert!(cli.yes);
}

#[test]
fn yes_conflicts_with_every_explicit_confirmation_mode() {
    for mode in ["native", "console"] {
        let error = Cli::try_parse_from(["unlinka", "--yes", "--confirm", mode, "file"])
            .expect_err("yes conflicts with an explicit confirmation mode");
        assert_eq!(error.kind(), ErrorKind::ArgumentConflict);
    }
}

#[test]
fn rejects_unknown_confirmation_modes() {
    let error = Cli::try_parse_from(["unlinka", "--confirm", "other", "file"])
        .expect_err("unknown confirmation modes are invalid");
    assert_eq!(error.kind(), ErrorKind::InvalidValue);
}

#[test]
fn accepts_console_and_dry_run() {
    let cli = Cli::try_parse_from(["unlinka", "--confirm", "console", "--dry-run", "file"])
        .expect("dry-run accepts the console mode without opening confirmation");
    assert_eq!(cli.confirmation(), Confirmation::Console);
    assert!(cli.dry_run);
}

#[test]
fn restricts_thread_budgets_to_the_inclusive_range() {
    for count in ["1", "64"] {
        let cli = Cli::try_parse_from(["unlinka", "--threads", count, "file"])
            .expect("inclusive thread budget boundaries are valid");
        assert_eq!(cli.threads.to_string(), count);
    }
    for count in ["0", "65", "abc", "1.5"] {
        let error = Cli::try_parse_from(["unlinka", "--threads", count, "file"])
            .expect_err("out-of-range and noninteger thread budgets are invalid");
        assert_eq!(error.kind(), ErrorKind::ValueValidation);
    }
}

#[test]
fn default_thread_budget_is_bounded() {
    let cli = Cli::try_parse_from(["unlinka", "file"]).expect("a single target path is valid");
    assert!((1..=4).contains(&cli.threads));
}

#[test]
fn accepts_dash_prefixed_paths_after_the_option_terminator() {
    let cli = Cli::try_parse_from(["unlinka", "--", "-file"])
        .expect("an option terminator permits dash-prefixed paths");
    assert_eq!(cli.paths, [std::path::PathBuf::from("-file")]);
}

#[test]
fn help_and_version_are_successful_parser_exits() {
    for option in ["--help", "--version"] {
        let error = Cli::try_parse_from(["unlinka", option])
            .expect_err("help and version return a parser display result");
        assert_eq!(error.exit_code(), 0);
    }
}

#[test]
fn execution_outcomes_preserve_failure_and_cancellation() {
    assert_eq!(ExitStatus::for_outcome(false, 0), ExitStatus::Success);
    assert_eq!(
        ExitStatus::for_outcome(false, 2),
        ExitStatus::ExecutionFailed
    );
    assert_eq!(ExitStatus::for_outcome(true, 0), ExitStatus::Cancelled);
    assert_eq!(ExitStatus::for_outcome(true, 2), ExitStatus::Cancelled);
}

#[test]
fn script_exit_codes_are_stable() {
    assert_eq!(ExitStatus::Success as u8, 0);
    assert_eq!(ExitStatus::ExecutionFailed as u8, 1);
    assert_eq!(ExitStatus::InvalidInput as u8, 2);
    assert_eq!(ExitStatus::Cancelled as u8, 3);
    assert_eq!(ExitStatus::ConfirmationFailed as u8, 4);
}

#[test]
fn bootstrap_stops_before_filename_language_and_json_options() {
    let arguments = ["unlinka", "--lang=zh-CN", "--", "--lang", "en-US", "--json"]
        .map(std::ffi::OsString::from);
    assert_eq!(super::bootstrap(&arguments), (Some("zh-CN".into()), false));
}

#[test]
fn malformed_language_still_requests_json_parameter_error() {
    let arguments = ["unlinka", "--lang", "--json"].map(std::ffi::OsString::from);
    assert!(super::bootstrap(&arguments).1);
}

#[test]
fn integration_status_is_an_option_and_mutually_exclusive_with_deletion() {
    let cli = Cli::try_parse_from(["unlinka", "--integration-status", "--json"]).unwrap();
    assert!(cli.integration_status);
    assert!(Cli::try_parse_from(["unlinka", "--integration-status", "target"]).is_err());
    assert!(Cli::try_parse_from(["unlinka", "--integration-status", "--threads", "4"]).is_err());
    let cli = Cli::try_parse_from(["unlinka", "integration"]).unwrap();
    assert_eq!(cli.paths, [std::path::PathBuf::from("integration")]);
}

#[test]
fn localized_command_renders_help_before_parsing() {
    let translator = unlinka_i18n::Translator::new(unlinka_i18n::Locale::ZhCn);
    let mut command = super::command(&translator);
    let help = command.render_long_help().to_string();
    assert!(help.contains("永久删除"));
    assert!(help.contains("--integration-status"));
    assert!(!help.contains("Print help"));
    assert!(
        command
            .try_get_matches_from(["unlinka", "--lang", "zh-CN", "--json", "file"])
            .is_ok()
    );
}
