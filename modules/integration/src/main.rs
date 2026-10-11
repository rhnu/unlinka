#![cfg(windows)]
use std::{fmt::Write as _, io, path::PathBuf};

use clap::{Parser, Subcommand};
use unlinka_i18n::{Translator, select_locale, windows_languages};
use unlinka_integration::{
    maintenance::{self, UninstallOutcome},
    read_settings,
};
use unlinka_ui::{CANCEL, Dialog, Verification, message_box};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};

#[derive(Parser)]
#[command(version, about = "Current-user Unlinka installation maintenance")]
struct Cli {
    #[command(subcommand)]
    command: Action,
}

#[derive(Subcommand)]
enum Action {
    Install {
        #[arg(long)]
        source: PathBuf,
        #[arg(long)]
        launcher: PathBuf,
        #[arg(long)]
        maintenance: PathBuf,
        #[arg(long, value_parser = ["auto", "zh-CN", "en-US"])]
        language: Option<String>,
        #[arg(long, action = clap::ArgAction::Set)]
        menus: Option<bool>,
        #[arg(long = "path", action = clap::ArgAction::Set)]
        path_enabled: Option<bool>,
    },
    Repair {
        #[arg(long)]
        source: Option<PathBuf>,
    },
    Configure {
        #[arg(long, value_parser = ["auto", "zh-CN", "en-US"])]
        language: Option<String>,
        #[arg(long, action = clap::ArgAction::Set)]
        menus: Option<bool>,
        #[arg(long = "path", action = clap::ArgAction::Set)]
        path_enabled: Option<bool>,
    },
    Uninstall {
        #[arg(long)]
        purge_settings: bool,
        #[arg(long)]
        confirm: bool,
    },
    Status,
}

fn main() {
    // SAFETY: maintenance runs synchronous WinRT deployment in an initialized MTA.
    let com = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    if let Err(error) = com.ok() {
        std::process::exit(failure(&io::Error::other(error.to_string())));
    }
    let mut cli = Cli::parse();
    if matches!(cli.command, Action::Uninstall { .. }) {
        let root = match unlinka_integration::install_root() {
            Ok(root) => root,
            Err(error) => std::process::exit(failure(&error)),
        };
        if std::env::current_exe()
            .is_ok_and(|exe| exe == root.join("maintenance/unlinka-maintenance.exe"))
        {
            use std::os::windows::fs::MetadataExt;
            let uninstaller = root.join("unins000.exe");
            let validated = unlinka_integration::Installation::current().and_then(|installation| {
                if installation.is_none() {
                    return Err(io::Error::other("installation is missing"));
                }
                let metadata = std::fs::symlink_metadata(&uninstaller)?;
                if !metadata.is_file() || metadata.file_attributes() & 0x400 != 0 {
                    return Err(io::Error::other(
                        "uninstaller is not a regular installed file",
                    ));
                }
                Ok(())
            });
            if let Err(error) = validated {
                std::process::exit(failure(&error));
            }
            if let Err(error) = std::process::Command::new(uninstaller).spawn() {
                std::process::exit(failure(&error));
            }
            std::process::exit(0);
        }
    }
    if !matches!(cli.command, Action::Status) {
        let args = std::env::args_os().skip(1).collect::<Vec<_>>();
        match maintenance::runtime_copy(&args) {
            Ok(Some(code)) => std::process::exit(code),
            Ok(None) => {},
            Err(error) => std::process::exit(failure(&error)),
        }
    }
    if let Action::Uninstall {
        purge_settings,
        confirm,
    } = &mut cli.command
        && !*confirm
    {
        let pending_purge = match maintenance::uninstall_purge_pending() {
            Ok(purge) => purge,
            Err(error) => std::process::exit(failure(&error)),
        };
        match confirm_uninstall(*purge_settings || pending_purge) {
            Ok(Some(purge)) => {
                *purge_settings = purge;
                *confirm = true;
            },
            Ok(None) => std::process::exit(5),
            Err(error) => std::process::exit(failure(&error)),
        }
    }
    loop {
        match run(&cli.command) {
            Ok(code) => std::process::exit(code),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                match wait_retry(&error.to_string()) {
                    Ok(true) => {},
                    Ok(false) => std::process::exit(3),
                    Err(error) => std::process::exit(failure(&error)),
                }
            },
            Err(error) => std::process::exit(failure(&error)),
        }
    }
}

fn wait_retry(reason: &str) -> io::Result<bool> {
    let tr = translator();
    let result = Dialog {
        title: "Unlinka",
        instruction: &tr.text("maintenance-wait-title"),
        content: &format!("{}\n\n{reason}", tr.text("maintenance-wait-content")),
        buttons: &[
            (1, &tr.text("maintenance-retry")),
            (CANCEL, &tr.text("button-cancel")),
        ],
        default_button: CANCEL,
        details: None,
        verification: None,
    }
    .show()?;
    Ok(result.button == 1)
}

fn run(action: &Action) -> io::Result<i32> {
    match action {
        Action::Install {
            source,
            launcher,
            maintenance,
            language,
            menus,
            path_enabled,
        } => {
            let mut settings = read_settings()?;
            if let Some(value) = language {
                settings.language.clone_from(value);
            }
            if let Some(value) = menus {
                settings.menus_enabled = *value;
            }
            if let Some(value) = path_enabled {
                settings.path_enabled = *value;
            }
            maintenance::install(source, launcher, maintenance, settings)?;
        },
        Action::Repair { source } => maintenance::repair(source.as_deref())?,
        Action::Configure {
            language,
            menus,
            path_enabled,
        } => {
            let mut settings = read_settings()?;
            if let Some(value) = language {
                settings.language.clone_from(value);
            }
            if let Some(value) = menus {
                settings.menus_enabled = *value;
            }
            if let Some(value) = path_enabled {
                settings.path_enabled = *value;
            }
            maintenance::configure(&settings)?;
        },
        Action::Uninstall { purge_settings, .. } => {
            match maintenance::uninstall(*purge_settings)? {
                UninstallOutcome::RemovalPending => {
                    show(&translator().text("maintenance-removal-pending"), false);
                    return Ok(4);
                },
                UninstallOutcome::CompleteWithRetainedFiles => show(
                    &translator().format(
                        "maintenance-retained-files",
                        &[(
                            "path",
                            unlinka_integration::install_root()?
                                .to_string_lossy()
                                .as_ref()
                                .into(),
                        )],
                    ),
                    false,
                ),
                UninstallOutcome::Complete => {},
            }
        },
        Action::Status => println!("{}", maintenance::integration_status()?),
    }
    Ok(0)
}

fn failure(error: &io::Error) -> i32 {
    let translator = translator();
    let mut message =
        translator.format("maintenance-error", &[("reason", error.to_string().into())]);
    if let Ok(path) = maintenance::log_diagnostic(&message) {
        let _format = write!(
            &mut message,
            "\n\n{}",
            translator.format(
                "maintenance-diagnostic-log",
                &[("path", path.to_string_lossy().as_ref().into())]
            )
        );
    }
    if error.kind() == io::ErrorKind::WouldBlock {
        let _format = write!(&mut message, "\n{}", translator.text("maintenance-busy"));
    }
    eprintln!("Unlinka: {message}");
    show(&message, true);
    if error.kind() == io::ErrorKind::WouldBlock {
        3
    } else {
        2
    }
}

fn show(message: &str, error: bool) { drop(message_box("Unlinka", message, error)); }

fn translator() -> Translator {
    let saved = read_settings().ok().map(|s| s.language);
    Translator::new(select_locale(
        None,
        None,
        saved.as_deref(),
        &windows_languages(),
    ))
}

fn confirm_uninstall(purge: bool) -> io::Result<Option<bool>> {
    let tr = translator();
    let result = Dialog {
        title: "Unlinka",
        instruction: &tr.text("maintenance-uninstall-title"),
        content: &tr.text("maintenance-uninstall-content"),
        buttons: &[
            (1, &tr.text("maintenance-uninstall-button")),
            (CANCEL, &tr.text("button-cancel")),
        ],
        default_button: CANCEL,
        details: None,
        verification: Some(Verification {
            label: &tr.text("maintenance-purge-settings"),
            checked: purge,
        }),
    }
    .show()?;
    Ok((result.button == 1).then_some(result.checked))
}
