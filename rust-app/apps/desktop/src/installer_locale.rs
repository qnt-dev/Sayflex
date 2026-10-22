//! Durable installer language requests. A new installation ID applies once;
//! legacy plain language files only seed a profile that does not exist yet.

use std::{fs::File, io::Read, path::Path};

use serde::Deserialize;

use crate::localization::UiLocale;

pub(crate) const SEED_FILE_NAME: &str = "installer-language";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum InstallerLanguage {
    Legacy(UiLocale),
    Request {
        locale: UiLocale,
        installation_id: String,
    },
}

impl InstallerLanguage {
    pub(crate) fn locale(&self) -> UiLocale {
        match self {
            Self::Legacy(locale) | Self::Request { locale, .. } => *locale,
        }
    }
}

pub(crate) fn load() -> Option<InstallerLanguage> {
    let executable = std::env::current_exe().ok()?;
    load_from_directory(executable.parent()?)
}

pub(crate) fn load_from_directory(directory: &Path) -> Option<InstallerLanguage> {
    // Bound reads and treat malformed/foreign files as absent. IDs come from
    // CoCreateGuid in the installer and are canonicalized for comparisons.
    let file = File::open(directory.join(SEED_FILE_NAME)).ok()?;
    let mut contents = String::new();
    file.take(257).read_to_string(&mut contents).ok()?;
    if contents.len() > 256 {
        return None;
    }
    let contents = contents.trim().trim_start_matches('\u{feff}').trim();
    if contents.starts_with('{') {
        #[derive(Deserialize)]
        struct Request {
            language: String,
            installation_id: String,
        }
        let request: Request = serde_json::from_str(contents).ok()?;
        if !valid_installation_id(&request.installation_id) {
            return None;
        }
        return Some(InstallerLanguage::Request {
            locale: language_name(&request.language)?,
            installation_id: request.installation_id.to_ascii_lowercase(),
        });
    }
    Some(InstallerLanguage::Legacy(language_name(contents)?))
}

pub(crate) fn valid_installation_id(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| {
            if [8, 13, 18, 23].contains(&index) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}

fn language_name(name: &str) -> Option<UiLocale> {
    match name.trim().to_ascii_lowercase().as_str() {
        "english" => Some(UiLocale::English),
        "russian" => Some(UiLocale::Russian),
        "ukrainian" => Some(UiLocale::Ukrainian),
        "spanish" => Some(UiLocale::Spanish),
        "brazilianportuguese" => Some(UiLocale::PortugueseBrazil),
        "german" => Some(UiLocale::German),
        "french" => Some(UiLocale::French),
        "italian" => Some(UiLocale::Italian),
        "korean" => Some(UiLocale::Korean),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{InstallerLanguage, SEED_FILE_NAME, load_from_directory};
    use crate::localization::UiLocale;

    #[test]
    fn installer_locale_maps_every_installer_language() {
        let installation = tempfile::tempdir().unwrap();
        for (name, expected) in [
            ("english", UiLocale::English),
            ("russian", UiLocale::Russian),
            ("ukrainian", UiLocale::Ukrainian),
            ("spanish", UiLocale::Spanish),
            ("brazilianportuguese", UiLocale::PortugueseBrazil),
            ("german", UiLocale::German),
            ("french", UiLocale::French),
            ("italian", UiLocale::Italian),
            ("korean", UiLocale::Korean),
        ] {
            fs::write(installation.path().join(SEED_FILE_NAME), name).unwrap();
            assert_eq!(
                load_from_directory(installation.path()),
                Some(InstallerLanguage::Legacy(expected))
            );
        }
    }

    #[test]
    fn installer_locale_normalizes_bom_whitespace_and_case() {
        let installation = tempfile::tempdir().unwrap();
        fs::write(
            installation.path().join(SEED_FILE_NAME),
            "\u{feff}  RuSsIaN\r\n",
        )
        .unwrap();
        assert_eq!(
            load_from_directory(installation.path()),
            Some(InstallerLanguage::Legacy(UiLocale::Russian))
        );
    }

    #[test]
    fn installer_locale_ignores_missing_invalid_and_oversized_seeds() {
        let installation = tempfile::tempdir().unwrap();
        assert_eq!(load_from_directory(installation.path()), None);
        for value in ["", "system", "not-a-language", "russian\ngerman"] {
            fs::write(installation.path().join(SEED_FILE_NAME), value).unwrap();
            assert_eq!(load_from_directory(installation.path()), None);
        }
        fs::write(
            installation.path().join(SEED_FILE_NAME),
            " ".repeat(4096) + "russian",
        )
        .unwrap();
        assert_eq!(load_from_directory(installation.path()), None);
    }

    #[test]
    fn installer_request_parses_every_language_with_a_unique_installation_id() {
        let directory = tempfile::tempdir().unwrap();
        let id = "01234567-89ab-cdef-0123-456789abcdef";
        for (name, locale) in [
            ("english", UiLocale::English),
            ("russian", UiLocale::Russian),
            ("ukrainian", UiLocale::Ukrainian),
            ("spanish", UiLocale::Spanish),
            ("brazilianportuguese", UiLocale::PortugueseBrazil),
            ("german", UiLocale::German),
            ("french", UiLocale::French),
            ("italian", UiLocale::Italian),
            ("korean", UiLocale::Korean),
        ] {
            let contents = serde_json::json!({"language":name,"installation_id":id}).to_string();
            fs::write(
                directory.path().join(SEED_FILE_NAME),
                format!("\u{feff}{contents}\r\n"),
            )
            .unwrap();
            assert_eq!(
                load_from_directory(directory.path()),
                Some(InstallerLanguage::Request {
                    locale,
                    installation_id: id.into(),
                })
            );
        }
    }

    #[test]
    fn installer_request_rejects_malformed_ids_languages_and_more_than_256_bytes() {
        let directory = tempfile::tempdir().unwrap();
        for value in [
            serde_json::json!({"language":"russian"}),
            serde_json::json!({"language":"russian","installation_id":""}),
            serde_json::json!({"language":"russian","installation_id":"../settings.json"}),
            serde_json::json!({"language":"russian","installation_id":"z1234567-89ab-cdef-0123-456789abcdef"}),
            serde_json::json!({"language":"system","installation_id":"01234567-89ab-cdef-0123-456789abcdef"}),
        ] {
            fs::write(directory.path().join(SEED_FILE_NAME), value.to_string()).unwrap();
            assert_eq!(load_from_directory(directory.path()), None);
        }
        let mut contents = serde_json::json!({"language":"russian","installation_id":"01234567-89ab-cdef-0123-456789abcdef"}).to_string();
        contents.push_str(&" ".repeat(256 - contents.len()));
        fs::write(directory.path().join(SEED_FILE_NAME), &contents).unwrap();
        assert!(matches!(
            load_from_directory(directory.path()),
            Some(InstallerLanguage::Request { .. })
        ));
        contents.push(' ');
        fs::write(directory.path().join(SEED_FILE_NAME), contents).unwrap();
        assert_eq!(load_from_directory(directory.path()), None);
    }
}
