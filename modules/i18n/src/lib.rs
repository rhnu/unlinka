//! Embedded product translations and deterministic language negotiation.
pub mod presentation;

pub use fluent_bundle::FluentValue;
use fluent_bundle::{FluentArgs, FluentBundle, FluentResource};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Locale {
    EnUs,
    ZhCn,
}

impl Locale {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::EnUs => "en-US",
            Self::ZhCn => "zh-CN",
        }
    }
}

/// Maps supported English and simplified Chinese variants without treating
/// traditional Chinese as simplified Chinese.
pub fn supported_language(value: &str) -> Option<Locale> {
    let tag: unic_langid::LanguageIdentifier = value.parse().ok()?;
    match tag.language.as_str() {
        "en" => Some(Locale::EnUs),
        "zh" if tag.script.is_some_and(|script| script.as_str() == "Hans") => Some(Locale::ZhCn),
        "zh" if tag.script.is_none()
            && tag
                .region
                .is_some_and(|region| matches!(region.as_str(), "CN" | "SG")) =>
        {
            Some(Locale::ZhCn)
        },
        _ => None,
    }
}

pub fn select_locale(
    explicit: Option<&str>,
    environment: Option<&str>,
    saved: Option<&str>,
    preferred: &[String],
) -> Locale {
    for value in [explicit, environment, saved].into_iter().flatten() {
        if value == "auto" {
            break;
        }
        if let Some(locale) = supported_language(value) {
            return locale;
        }
    }
    preferred
        .iter()
        .find_map(|value| supported_language(value))
        .unwrap_or(Locale::EnUs)
}

/// Reads the user's preferred Windows display language list in priority order.
pub fn windows_languages() -> Vec<String> {
    #[cfg(windows)]
    {
        use windows::{
            Win32::Globalization::{GetUserPreferredUILanguages, MUI_LANGUAGE_NAME},
            core::PWSTR,
        };
        let mut count = 0;
        let mut length = 0;
        {
            // SAFETY: Null buffer requests its required size; count and length are writable.
            if unsafe {
                GetUserPreferredUILanguages(
                    MUI_LANGUAGE_NAME,
                    &raw mut count,
                    None,
                    &raw mut length,
                )
            }
            .is_err()
            {
                return Vec::new();
            }
            let mut buffer = vec![0; length as usize];
            // SAFETY: The allocated buffer contains the queried number of UTF-16 units.
            if unsafe {
                GetUserPreferredUILanguages(
                    MUI_LANGUAGE_NAME,
                    &raw mut count,
                    Some(PWSTR(buffer.as_mut_ptr())),
                    &raw mut length,
                )
            }
            .is_err()
            {
                return Vec::new();
            }
            buffer
                .split(|unit| *unit == 0)
                .filter(|part| !part.is_empty())
                .map(String::from_utf16_lossy)
                .collect()
        }
    }
    #[cfg(not(windows))]
    Vec::new()
}

pub struct Translator {
    locale: Locale,
    bundle: FluentBundle<FluentResource>,
}

impl Translator {
    /// Creates a formatter using compiled product resources.
    ///
    /// # Panics
    /// Panics if the embedded resources are invalid or contain duplicate identifiers.
    pub fn new(locale: Locale) -> Self {
        let source = match locale {
            Locale::EnUs => include_str!("../locales/en-US.ftl"),
            Locale::ZhCn => include_str!("../locales/zh-CN.ftl"),
        };
        let resource = FluentResource::try_new(source.to_owned())
            .expect("embedded Fluent resource must parse");
        let mut bundle = FluentBundle::new(vec![locale.as_str().parse().expect("known locale")]);
        bundle.set_use_isolating(false);
        bundle
            .add_resource(resource)
            .expect("embedded Fluent message identifiers must be unique");
        Self { locale, bundle }
    }

    pub const fn locale(&self) -> Locale { self.locale }

    pub fn text(&self, id: &str) -> String { self.format(id, &[]) }

    pub fn count(&self, id: &str, count: u64) -> String {
        self.format(id, &[("count", FluentValue::from(count))])
    }

    pub fn operation(&self, operation: &str) -> String {
        let key = format!("operation-{operation}");
        if self.bundle.get_message(&key).is_some() {
            self.text(&key)
        } else {
            operation.into()
        }
    }

    pub fn format(&self, id: &str, values: &[(&str, FluentValue<'_>)]) -> String {
        let Some(pattern) = self
            .bundle
            .get_message(id)
            .and_then(|message| message.value())
        else {
            return id.to_owned();
        };
        let mut args = FluentArgs::new();
        for (key, value) in values {
            args.set(*key, value.clone());
        }
        let mut errors = Vec::new();
        self.bundle
            .format_pattern(pattern, Some(&args), &mut errors)
            .into_owned()
    }
}

#[cfg(test)]
mod tests;
