use std::{ffi::OsString, io};

use unlinka_i18n::Translator;
use unlinka_integration::{Installation, maintenance, read_settings};
use unlinka_ui::CANCEL;

use crate::task;

/// Stages all choices locally, then delegates one atomic maintenance transaction.
pub(crate) fn show(tr: &Translator) -> io::Result<u8> {
    Installation::current()?.ok_or_else(|| io::Error::other(tr.text("settings-portable")))?;
    let settings = read_settings()?;
    let selected = task::presentation(
        tr,
        &tr.text("settings-title"),
        &format!(
            "{}: {}\n{}: {}\n{}: {}",
            tr.text("settings-language"),
            settings.language,
            tr.text("settings-menus"),
            tr.text(if settings.menus_enabled {
                "settings-enable"
            } else {
                "settings-disable"
            }),
            tr.text("settings-path"),
            tr.text(if settings.path_enabled {
                "settings-enable"
            } else {
                "settings-disable"
            })
        ),
        &[
            (1001, tr.text("settings-change")),
            (1002, tr.text("button-repair")),
            (CANCEL, tr.text("button-cancel")),
        ],
        CANCEL,
        None,
    )?;
    if selected == CANCEL {
        return Ok(3);
    }
    let arguments: Vec<OsString> = if selected == 1002 {
        vec!["repair".into()]
    } else {
        let Some(updated) = choose_settings(tr, settings)? else {
            return Ok(3);
        };
        [
            "configure",
            "--language",
            &updated.language,
            "--menus",
            if updated.menus_enabled {
                "true"
            } else {
                "false"
            },
            "--path",
            if updated.path_enabled {
                "true"
            } else {
                "false"
            },
        ]
        .into_iter()
        .map(OsString::from)
        .collect()
    };
    // Do not hold an AppLease: maintenance needs the exclusive lock itself.
    let code = maintenance::run_installed(&arguments)?;
    if code == 3 {
        // The helper's wait dialog returns this code when maintenance is cancelled.
        return Ok(3);
    }
    if code == 0 {
        task::message(tr, &tr.text("settings-title"), &tr.text("settings-saved"))?;
        Ok(0)
    } else {
        task::message(
            tr,
            &tr.text("error-maintenance"),
            &format!("{} ({code})", tr.text("error-maintenance")),
        )?;
        Ok(1)
    }
}

fn choose_settings(
    tr: &Translator,
    mut settings: unlinka_integration::Settings,
) -> io::Result<Option<unlinka_integration::Settings>> {
    let language = task::presentation(
        tr,
        &tr.text("settings-language"),
        &settings.language,
        &[
            (1001, tr.text("settings-auto")),
            (1002, "简体中文".into()),
            (1003, "English".into()),
            (CANCEL, tr.text("button-cancel")),
        ],
        match settings.language.as_str() {
            "zh-CN" => 1002,
            "en-US" => 1003,
            _ => 1001,
        },
        None,
    )?;
    settings.language = match language {
        1001 => "auto",
        1002 => "zh-CN",
        1003 => "en-US",
        _ => return Ok(None),
    }
    .into();
    let menus = enabled(tr, "settings-menus", settings.menus_enabled)?;
    let Some(menus) = menus else {
        return Ok(None);
    };
    let path = enabled(tr, "settings-path", settings.path_enabled)?;
    let Some(path) = path else {
        return Ok(None);
    };
    let save = task::presentation(
        tr,
        &tr.text("settings-title"),
        &format!(
            "{}: {}\n{}: {}\n{}: {}",
            tr.text("settings-language"),
            settings.language,
            tr.text("settings-menus"),
            tr.text(if menus {
                "settings-enable"
            } else {
                "settings-disable"
            }),
            tr.text("settings-path"),
            tr.text(if path {
                "settings-enable"
            } else {
                "settings-disable"
            })
        ),
        &[
            (1001, tr.text("button-save")),
            (CANCEL, tr.text("button-cancel")),
        ],
        CANCEL,
        None,
    )?;
    if save != 1001 {
        return Ok(None);
    }
    settings.menus_enabled = menus;
    settings.path_enabled = path;
    Ok(Some(settings))
}

fn enabled(tr: &Translator, title: &str, current: bool) -> io::Result<Option<bool>> {
    let selected = task::presentation(
        tr,
        &tr.text(title),
        "",
        &[
            (1001, tr.text("settings-enable")),
            (1002, tr.text("settings-disable")),
            (CANCEL, tr.text("button-cancel")),
        ],
        if current { 1001 } else { 1002 },
        None,
    )?;
    Ok(match selected {
        1001 => Some(true),
        1002 => Some(false),
        _ => None,
    })
}
