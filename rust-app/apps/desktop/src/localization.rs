use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum UiLocale {
    #[default]
    System,
    English,
    Russian,
    Ukrainian,
    Spanish,
    PortugueseBrazil,
    German,
    French,
    Italian,
    Korean,
}

impl UiLocale {
    pub(crate) const ALL: [Self; 10] = [
        Self::System,
        Self::English,
        Self::Russian,
        Self::Ukrainian,
        Self::Spanish,
        Self::PortugueseBrazil,
        Self::German,
        Self::French,
        Self::Italian,
        Self::Korean,
    ];

    pub(crate) const fn code(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::English => "en",
            Self::Russian => "ru",
            Self::Ukrainian => "uk",
            Self::Spanish => "es",
            Self::PortugueseBrazil => "pt-BR",
            Self::German => "de",
            Self::French => "fr",
            Self::Italian => "it",
            Self::Korean => "ko",
        }
    }

    pub(crate) const fn display_name(self) -> &'static str {
        match self {
            Self::System => "System language",
            Self::English => "English",
            Self::Russian => "Русский",
            Self::Ukrainian => "Українська",
            Self::Spanish => "Español",
            Self::PortugueseBrazil => "Português (Brasil)",
            Self::German => "Deutsch",
            Self::French => "Français",
            Self::Italian => "Italiano",
            Self::Korean => "한국어",
        }
    }

    pub(crate) fn from_code(code: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|locale| locale.code().eq_ignore_ascii_case(code))
    }

    pub(crate) fn resolved_code(self, system_locale: &str) -> &'static str {
        if self != Self::System {
            return self.code();
        }
        resolve_system_locale(system_locale).code()
    }
}

pub(crate) fn resolve_system_locale(locale: &str) -> UiLocale {
    let normalized = locale.trim().replace('_', "-").to_ascii_lowercase();
    let language = normalized.split('-').next().unwrap_or_default();
    match language {
        "ru" => UiLocale::Russian,
        "uk" => UiLocale::Ukrainian,
        "es" => UiLocale::Spanish,
        "pt" => UiLocale::PortugueseBrazil,
        "de" => UiLocale::German,
        "fr" => UiLocale::French,
        "it" => UiLocale::Italian,
        "ko" => UiLocale::Korean,
        _ => UiLocale::English,
    }
}

pub(crate) fn active_locale(locale: UiLocale) -> UiLocale {
    if locale == UiLocale::System {
        resolve_system_locale(&rust_platform_windows::user_locale_name())
    } else {
        locale
    }
}

include!(concat!(env!("OUT_DIR"), "/ui_text_catalog.rs"));

pub(crate) fn render(locale: UiLocale, message: UiText, values: &[(&str, &str)]) -> String {
    values.iter().fold(
        text(active_locale(locale), message).to_owned(),
        |value, (name, replacement)| value.replace(&format!("{{{name}}}"), replacement),
    )
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use super::{UiLocale, resolve_system_locale};

    #[test]
    fn system_locale_maps_only_to_the_approved_ui_catalogs() {
        assert_eq!(resolve_system_locale("ru-RU"), UiLocale::Russian);
        assert_eq!(resolve_system_locale("uk_UA"), UiLocale::Ukrainian);
        assert_eq!(resolve_system_locale("pt-PT"), UiLocale::PortugueseBrazil);
        assert_eq!(resolve_system_locale("ja-JP"), UiLocale::English);
        assert_eq!(resolve_system_locale("ar-SA"), UiLocale::English);
    }

    #[test]
    fn persisted_codes_round_trip() {
        for locale in UiLocale::ALL {
            assert_eq!(UiLocale::from_code(locale.code()), Some(locale));
        }
    }

    #[test]
    fn system_selection_resolves_at_application_time() {
        assert_eq!(UiLocale::System.resolved_code("de-DE"), "de");
        assert_eq!(UiLocale::French.resolved_code("de-DE"), "fr");
    }

    fn slint_message_ids() -> BTreeSet<String> {
        [
            include_str!("../ui/app.slint"),
            include_str!("../ui/controls.slint"),
            include_str!("../ui/pages_forms.slint"),
            include_str!("../ui/pages_instruments.slint"),
            include_str!("../ui/pages_lists.slint"),
            include_str!("../ui/parts.slint"),
            include_str!("../ui/recording_visual.slint"),
            include_str!("../ui/settings.slint"),
        ]
        .into_iter()
        .flat_map(|source| {
            source
                .split("@tr(\"")
                .skip(1)
                .filter_map(|tail| tail.split_once('"').map(|(message, _)| message.to_owned()))
        })
        .collect()
    }

    fn catalog(source: &str) -> BTreeMap<String, String> {
        let mut entries = BTreeMap::new();
        let mut lines = source.lines();
        while let Some(line) = lines.next() {
            let Some(id) = line
                .strip_prefix("msgid \"")
                .and_then(|id| id.strip_suffix('"'))
            else {
                continue;
            };
            if id.is_empty() {
                continue;
            }
            let value = lines
                .next()
                .and_then(|line| line.strip_prefix("msgstr \""))
                .and_then(|value| value.strip_suffix('"'))
                .unwrap_or_default();
            entries.insert(id.to_owned(), value.to_owned());
        }
        entries
    }

    #[test]
    fn every_approved_catalog_contains_every_slint_message() {
        let expected = slint_message_ids();
        let catalogs = [
            (
                "ru",
                include_str!("../translations/ru/LC_MESSAGES/sayflex-desktop.po"),
            ),
            (
                "uk",
                include_str!("../translations/uk/LC_MESSAGES/sayflex-desktop.po"),
            ),
            (
                "es",
                include_str!("../translations/es/LC_MESSAGES/sayflex-desktop.po"),
            ),
            (
                "pt-BR",
                include_str!("../translations/pt-BR/LC_MESSAGES/sayflex-desktop.po"),
            ),
            (
                "de",
                include_str!("../translations/de/LC_MESSAGES/sayflex-desktop.po"),
            ),
            (
                "fr",
                include_str!("../translations/fr/LC_MESSAGES/sayflex-desktop.po"),
            ),
            (
                "it",
                include_str!("../translations/it/LC_MESSAGES/sayflex-desktop.po"),
            ),
            (
                "ko",
                include_str!("../translations/ko/LC_MESSAGES/sayflex-desktop.po"),
            ),
        ];
        for (language, source) in catalogs {
            let entries = catalog(source);
            let missing: Vec<_> = expected
                .iter()
                .filter(|message| entries.get(*message).is_none_or(String::is_empty))
                .collect();
            assert!(missing.is_empty(), "{language} is missing {missing:?}");
            assert_ne!(entries.get("Tools").map(String::as_str), Some("Tools"));
        }
    }

    #[test]
    fn every_catalog_starts_with_a_contiguous_gettext_header() {
        for (language, source) in [
            (
                "ru",
                include_str!("../translations/ru/LC_MESSAGES/sayflex-desktop.po"),
            ),
            (
                "uk",
                include_str!("../translations/uk/LC_MESSAGES/sayflex-desktop.po"),
            ),
            (
                "es",
                include_str!("../translations/es/LC_MESSAGES/sayflex-desktop.po"),
            ),
            (
                "pt-BR",
                include_str!("../translations/pt-BR/LC_MESSAGES/sayflex-desktop.po"),
            ),
            (
                "de",
                include_str!("../translations/de/LC_MESSAGES/sayflex-desktop.po"),
            ),
            (
                "fr",
                include_str!("../translations/fr/LC_MESSAGES/sayflex-desktop.po"),
            ),
            (
                "it",
                include_str!("../translations/it/LC_MESSAGES/sayflex-desktop.po"),
            ),
            (
                "ko",
                include_str!("../translations/ko/LC_MESSAGES/sayflex-desktop.po"),
            ),
        ] {
            let expected = format!("msgid \"\"\nmsgstr \"\"\n\"Language: {language}\\n\"\n");
            assert!(
                source.replace("\r\n", "\n").starts_with(&expected),
                "{language}"
            );
        }
    }

    #[test]
    fn every_bundled_locale_can_construct_the_complete_settings_surface() {
        i_slint_backend_testing::init_no_event_loop();
        // The generated component registers its bundled catalogs when the
        // first instance is created; Slint deliberately rejects selection
        // before that registration exists.
        let seed = crate::SettingsWindow::new().expect("seed settings surface");
        for locale in UiLocale::ALL
            .into_iter()
            .filter(|locale| *locale != UiLocale::System)
        {
            slint::select_bundled_translation(locale.code())
                .unwrap_or_else(|error| panic!("{} bundle failed: {error}", locale.code()));
            let window = crate::SettingsWindow::new()
                .unwrap_or_else(|error| panic!("{} settings failed: {error}", locale.code()));
            drop(window);
        }
        drop(seed);
    }

    #[test]
    fn settings_navigation_exposes_actions_and_is_keyboard_traversable() {
        use i_slint_backend_testing::{AccessibleRole, ElementHandle};
        use slint::{
            ComponentHandle, ModelRc, VecModel,
            platform::{Key, WindowEvent},
        };

        i_slint_backend_testing::init_no_event_loop();
        let settings = crate::SettingsWindow::new().expect("settings surface");
        settings.show().expect("show headless settings surface");

        let history = ElementHandle::find_by_accessible_label(&settings, "History")
            .next()
            .expect("History navigation item in accessibility tree");
        assert_eq!(history.accessible_role(), Some(AccessibleRole::Tab));
        assert_eq!(history.accessible_enabled(), Some(true));
        history.invoke_accessible_default_action();
        assert_eq!(settings.get_active_page(), 6);

        history.mock_single_click(slint::platform::PointerEventButton::Left);
        assert_eq!(settings.get_active_page(), 6);
        settings
            .window()
            .dispatch_event(WindowEvent::WindowActiveChanged(true));
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::ZERO);
        for event in [
            WindowEvent::KeyPressed {
                text: Key::Tab.into(),
            },
            WindowEvent::KeyReleased {
                text: Key::Tab.into(),
            },
            WindowEvent::KeyPressed {
                text: Key::Return.into(),
            },
            WindowEvent::KeyReleased {
                text: Key::Return.into(),
            },
        ] {
            settings.window().dispatch_event(event);
            i_slint_backend_testing::mock_elapsed_time(std::time::Duration::ZERO);
        }
        assert_eq!(settings.get_active_page(), 7);

        settings.set_history_rows(ModelRc::new(VecModel::from(
            (0..40)
                .map(|index| crate::HistoryRow {
                    text: format!("Entry {index}").into(),
                    detail: "details".into(),
                    origin: "local".into(),
                })
                .collect::<Vec<_>>(),
        )));
        settings.set_active_page(6);
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::ZERO);
        let rail = ElementHandle::find_by_accessible_label(&settings, "Scroll position")
            .next()
            .expect("visible history scroll rail in accessibility tree");
        assert_eq!(rail.accessible_role(), Some(AccessibleRole::Slider));
        assert_eq!(rail.accessible_enabled(), Some(true));
        let before = settings.get_history_scroll_y();
        rail.invoke_accessible_increment_action();
        assert!(settings.get_history_scroll_y() < before);
    }

    #[test]
    fn custom_menu_options_are_reachable_through_accessibility_actions() {
        use std::{cell::RefCell, rc::Rc};

        use i_slint_backend_testing::{AccessibleRole, ElementHandle};
        use slint::{
            ComponentHandle,
            platform::{Key, PointerEventButton, WindowEvent},
        };

        i_slint_backend_testing::init_no_event_loop();
        let seed = crate::SettingsWindow::new().expect("seed settings surface");
        slint::select_bundled_translation("en").expect("select English bundle");
        let settings = crate::SettingsWindow::new().expect("settings surface");
        settings.show().expect("show headless settings surface");

        let selected = Rc::new(RefCell::new(String::new()));
        let selected_from_callback = Rc::clone(&selected);
        settings.on_set_ui_locale(move |locale| {
            *selected_from_callback.borrow_mut() = locale.to_string();
        });

        let language = ElementHandle::find_by_accessible_label(&settings, "System language")
            .next()
            .expect("language combobox in accessibility tree");
        assert_eq!(language.accessible_role(), Some(AccessibleRole::Combobox));
        language.invoke_accessible_default_action();

        let russian = ElementHandle::find_by_accessible_label(&settings, "Русский")
            .next()
            .expect("Russian option in open language menu");
        assert_eq!(russian.accessible_role(), Some(AccessibleRole::ListItem));
        russian.invoke_accessible_default_action();

        assert_eq!(selected.borrow().as_str(), "ru");

        settings.set_ui_locale("ru".into());
        settings.set_ui_locale_label("Русский".into());
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::ZERO);
        selected.borrow_mut().clear();
        language.mock_single_click(PointerEventButton::Left);
        for event in [
            WindowEvent::KeyPressed {
                text: Key::DownArrow.into(),
            },
            WindowEvent::KeyReleased {
                text: Key::DownArrow.into(),
            },
            WindowEvent::KeyPressed {
                text: Key::Return.into(),
            },
            WindowEvent::KeyReleased {
                text: Key::Return.into(),
            },
        ] {
            settings.window().dispatch_event(event);
            i_slint_backend_testing::mock_elapsed_time(std::time::Duration::ZERO);
        }
        assert_eq!(selected.borrow().as_str(), "uk");
        drop(seed);
    }
}
