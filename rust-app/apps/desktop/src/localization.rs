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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // Consumed by the Task 8 capture/OCR state machine.
pub(crate) enum ScreenTranslationOutcome {
    CaptureUnavailable,
    ProtectedContent,
    EmptySelection,
    NoText,
    ModelUnavailable,
    LanguageUnavailable,
    WorkerFailed,
    Cancelled,
}

#[allow(dead_code)] // Consumed by the Task 8 capture/OCR state machine.
pub(crate) fn screen_translation_outcome_text(
    locale: UiLocale,
    outcome: ScreenTranslationOutcome,
) -> &'static str {
    let message = match outcome {
        ScreenTranslationOutcome::CaptureUnavailable => UiText::ScreenCaptureUnavailable,
        ScreenTranslationOutcome::ProtectedContent => UiText::ScreenCaptureProtected,
        ScreenTranslationOutcome::EmptySelection => UiText::ScreenSelectionEmpty,
        ScreenTranslationOutcome::NoText => UiText::ScreenOcrNoText,
        ScreenTranslationOutcome::ModelUnavailable => UiText::ScreenOcrModelUnavailable,
        ScreenTranslationOutcome::LanguageUnavailable => UiText::ScreenOcrLanguageUnavailable,
        ScreenTranslationOutcome::WorkerFailed => UiText::ScreenOcrWorkerFailed,
        ScreenTranslationOutcome::Cancelled => UiText::ScreenTranslationCancelled,
    };
    text(active_locale(locale), message)
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

    #[test]
    fn about_page_keeps_framework_credit_small_and_out_of_the_product_identity() {
        let source = include_str!("../ui/pages_instruments.slint");
        assert!(source.contains("about-identity := Rectangle"));
        assert!(source.contains("technical-details := Rectangle"));
        assert!(source.contains("THIRD_PARTY_NOTICES.md"));
        assert!(!source.contains("AboutSlint"));
        assert!(source.contains("text: \"Made with Slint\""));
        assert!(source.contains("font-size: 9px"));
        assert!(!source.contains("licenses-panel"));
    }

    #[test]
    fn settings_shell_owns_quick_translation_but_not_process_exit_or_duplicate_page_number() {
        let settings = include_str!("../ui/settings.slint");
        let tray = include_str!("../ui/app.slint");

        assert!(settings.contains("label: @tr(\"Quick translation\")"));
        assert!(settings.contains("clicked => { root.open-translator(); }"));
        let quick_translation = settings
            .find("label: @tr(\"Quick translation\")")
            .expect("quick translation action");
        let workspace = settings
            .find("// ── workspace ──")
            .expect("workspace boundary");
        assert!(quick_translation < workspace);
        assert_eq!(
            settings.matches("version: root.app-version").count(),
            1,
            "the version belongs only to About"
        );
        assert!(!settings.contains("text: \"SayFlex\";"));
        assert!(!settings.contains("callback quit-requested();"));
        assert!(!settings.contains("root.quit-requested()"));
        assert!(!settings.contains("page-numbers"));
        assert!(tray.contains("callback quit-requested();"));
        assert!(tray.contains("title: @tr(\"Quit\")"));
    }

    #[test]
    fn ready_status_uses_the_configured_shortcut_in_the_active_language() {
        assert_eq!(
            super::render(
                UiLocale::Russian,
                super::UiText::ReadyHoldShortcut,
                &[("shortcut", "Alt + F9")],
            ),
            "Готово · удерживайте Alt + F9, чтобы говорить."
        );
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
    fn settings_copy_is_semantic_in_every_supported_language() {
        let catalogs = [
            (
                "ru",
                include_str!("../translations/ru/LC_MESSAGES/sayflex-desktop.po"),
                [
                    ("Locally", "Локально"),
                    ("Fastest available", "Самый быстрый из доступных"),
                    ("Modern GPU", "Современная видеокарта"),
                    ("Custom endpoint", "Свой сервер"),
                    ("Providers", "Провайдеры"),
                    ("SOURCE TEXT", "ИСХОДНЫЙ ТЕКСТ"),
                    (
                        "Runs on this computer · nothing leaves it",
                        "Работает локально · данные никуда не отправляются",
                    ),
                    ("TEXT LEAVES DEVICE", "ТЕКСТ ОТПРАВЛЯЕТСЯ В ОБЛАКО"),
                    ("EVERYTHING STAYS HERE", "ВСЁ ОБРАБАТЫВАЕТСЯ ЛОКАЛЬНО"),
                    (
                        "Nothing can transcribe right now",
                        "Диктовка сейчас недоступна",
                    ),
                    (
                        "Nothing can transcribe · text leaves this device for {text}",
                        "Диктовка недоступна · текст отправляется в {text}",
                    ),
                    (
                        "SPEECH — WHAT TRANSCRIBES A RECORDING",
                        "ДИКТОВКА — СЕРВИСЫ РАСПОЗНАВАНИЯ РЕЧИ",
                    ),
                    (
                        "TEXT — WHAT TRANSLATES A SELECTION",
                        "ПЕРЕВОД — СЕРВИСЫ ПЕРЕВОДА ТЕКСТА",
                    ),
                    ("DICTATION LANGUAGE", "ЯЗЫК ДИКТОВКИ"),
                    ("TRANSCRIPT LANGUAGE", "ЯЗЫК РЕЗУЛЬТАТА"),
                    ("PROCESSING MODE", "РЕЖИМ ОБРАБОТКИ"),
                    ("Fast response", "Быстрый ответ"),
                    ("Lower resource use", "Экономия ресурсов"),
                    ("Interface language", "Язык интерфейса"),
                    ("Follow Windows settings", "Как в Windows"),
                    ("ACTION", "ДЕЙСТВИЕ"),
                    ("Insert latest dictation", "Вставить последнюю диктовку"),
                    ("Act on selection", "Обработать выделение"),
                    ("Correct keyboard layout", "Исправить раскладку клавиатуры"),
                    ("Never reached", "Пропускается"),
                    (
                        "No compatible speech backend is active",
                        "Нет совместимого движка распознавания речи",
                    ),
                ],
            ),
            (
                "uk",
                include_str!("../translations/uk/LC_MESSAGES/sayflex-desktop.po"),
                [
                    ("Locally", "Локально"),
                    ("Fastest available", "Найшвидший із доступних"),
                    ("Modern GPU", "Сучасна відеокарта"),
                    ("Custom endpoint", "Власний сервер"),
                    ("Providers", "Провайдери"),
                    ("SOURCE TEXT", "ВИХІДНИЙ ТЕКСТ"),
                    (
                        "Runs on this computer · nothing leaves it",
                        "Працює локально · дані нікуди не надсилаються",
                    ),
                    ("TEXT LEAVES DEVICE", "ТЕКСТ НАДСИЛАЄТЬСЯ В ХМАРУ"),
                    ("EVERYTHING STAYS HERE", "УСЕ ОБРОБЛЯЄТЬСЯ ЛОКАЛЬНО"),
                    (
                        "Nothing can transcribe right now",
                        "Диктування зараз недоступне",
                    ),
                    (
                        "Nothing can transcribe · text leaves this device for {text}",
                        "Диктування недоступне · текст надсилається до {text}",
                    ),
                    (
                        "SPEECH — WHAT TRANSCRIBES A RECORDING",
                        "ДИКТУВАННЯ — СЕРВІСИ РОЗПІЗНАВАННЯ МОВЛЕННЯ",
                    ),
                    (
                        "TEXT — WHAT TRANSLATES A SELECTION",
                        "ПЕРЕКЛАД — СЕРВІСИ ПЕРЕКЛАДУ ТЕКСТУ",
                    ),
                    ("DICTATION LANGUAGE", "МОВА ДИКТУВАННЯ"),
                    ("TRANSCRIPT LANGUAGE", "МОВА РЕЗУЛЬТАТУ"),
                    ("PROCESSING MODE", "РЕЖИМ ОБРОБКИ"),
                    ("Fast response", "Швидка відповідь"),
                    ("Lower resource use", "Економія ресурсів"),
                    ("Interface language", "Мова інтерфейсу"),
                    ("Follow Windows settings", "Як у Windows"),
                    ("ACTION", "ДІЯ"),
                    ("Insert latest dictation", "Вставити останнє диктування"),
                    ("Act on selection", "Обробити виділення"),
                    ("Correct keyboard layout", "Виправити розкладку клавіатури"),
                    ("Never reached", "Пропускається"),
                    (
                        "No compatible speech backend is active",
                        "Немає сумісного рушія розпізнавання мовлення",
                    ),
                ],
            ),
            (
                "es",
                include_str!("../translations/es/LC_MESSAGES/sayflex-desktop.po"),
                [
                    ("Locally", "En local"),
                    ("Fastest available", "La opción más rápida"),
                    ("Modern GPU", "GPU moderna"),
                    ("Custom endpoint", "Servidor propio"),
                    ("Providers", "Proveedores"),
                    ("SOURCE TEXT", "TEXTO ORIGINAL"),
                    (
                        "Runs on this computer · nothing leaves it",
                        "Funciona de forma local · no se envían datos",
                    ),
                    ("TEXT LEAVES DEVICE", "EL TEXTO SE ENVÍA A LA NUBE"),
                    ("EVERYTHING STAYS HERE", "TODO SE PROCESA DE FORMA LOCAL"),
                    (
                        "Nothing can transcribe right now",
                        "El dictado no está disponible ahora",
                    ),
                    (
                        "Nothing can transcribe · text leaves this device for {text}",
                        "Dictado no disponible · el texto se envía a {text}",
                    ),
                    (
                        "SPEECH — WHAT TRANSCRIBES A RECORDING",
                        "DICTADO — SERVICIOS DE RECONOCIMIENTO DE VOZ",
                    ),
                    (
                        "TEXT — WHAT TRANSLATES A SELECTION",
                        "TRADUCCIÓN — SERVICIOS DE TRADUCCIÓN DE TEXTO",
                    ),
                    ("DICTATION LANGUAGE", "IDIOMA DE DICTADO"),
                    ("TRANSCRIPT LANGUAGE", "IDIOMA DEL RESULTADO"),
                    ("PROCESSING MODE", "MODO DE PROCESAMIENTO"),
                    ("Fast response", "Respuesta rápida"),
                    ("Lower resource use", "Menor uso de recursos"),
                    ("Interface language", "Idioma de la interfaz"),
                    ("Follow Windows settings", "Como en Windows"),
                    ("ACTION", "ACCIÓN"),
                    ("Insert latest dictation", "Insertar el último dictado"),
                    ("Act on selection", "Procesar la selección"),
                    (
                        "Correct keyboard layout",
                        "Corregir la distribución del teclado",
                    ),
                    ("Never reached", "Omitido"),
                    (
                        "No compatible speech backend is active",
                        "No hay ningún motor de reconocimiento de voz compatible activo",
                    ),
                ],
            ),
            (
                "pt-BR",
                include_str!("../translations/pt-BR/LC_MESSAGES/sayflex-desktop.po"),
                [
                    ("Locally", "Localmente"),
                    ("Fastest available", "A opção mais rápida"),
                    ("Modern GPU", "GPU moderna"),
                    ("Custom endpoint", "Servidor personalizado"),
                    ("Providers", "Provedores"),
                    ("SOURCE TEXT", "TEXTO ORIGINAL"),
                    (
                        "Runs on this computer · nothing leaves it",
                        "Funciona localmente · nenhum dado é enviado",
                    ),
                    ("TEXT LEAVES DEVICE", "O TEXTO É ENVIADO À NUVEM"),
                    ("EVERYTHING STAYS HERE", "TUDO É PROCESSADO LOCALMENTE"),
                    (
                        "Nothing can transcribe right now",
                        "O ditado não está disponível agora",
                    ),
                    (
                        "Nothing can transcribe · text leaves this device for {text}",
                        "Ditado indisponível · o texto é enviado para {text}",
                    ),
                    (
                        "SPEECH — WHAT TRANSCRIBES A RECORDING",
                        "DITADO — SERVIÇOS DE RECONHECIMENTO DE VOZ",
                    ),
                    (
                        "TEXT — WHAT TRANSLATES A SELECTION",
                        "TRADUÇÃO — SERVIÇOS DE TRADUÇÃO DE TEXTO",
                    ),
                    ("DICTATION LANGUAGE", "IDIOMA DO DITADO"),
                    ("TRANSCRIPT LANGUAGE", "IDIOMA DO RESULTADO"),
                    ("PROCESSING MODE", "MODO DE PROCESSAMENTO"),
                    ("Fast response", "Resposta rápida"),
                    ("Lower resource use", "Menor uso de recursos"),
                    ("Interface language", "Idioma da interface"),
                    ("Follow Windows settings", "Como no Windows"),
                    ("ACTION", "AÇÃO"),
                    ("Insert latest dictation", "Inserir o último ditado"),
                    ("Act on selection", "Processar a seleção"),
                    ("Correct keyboard layout", "Corrigir o layout do teclado"),
                    ("Never reached", "Ignorado"),
                    (
                        "No compatible speech backend is active",
                        "Nenhum mecanismo de reconhecimento de fala compatível está ativo",
                    ),
                ],
            ),
            (
                "de",
                include_str!("../translations/de/LC_MESSAGES/sayflex-desktop.po"),
                [
                    ("Locally", "Lokal"),
                    ("Fastest available", "Schnellste verfügbare Option"),
                    ("Modern GPU", "Moderne GPU"),
                    ("Custom endpoint", "Eigener Server"),
                    ("Providers", "Anbieter"),
                    ("SOURCE TEXT", "AUSGANGSTEXT"),
                    (
                        "Runs on this computer · nothing leaves it",
                        "Läuft lokal · es werden keine Daten gesendet",
                    ),
                    ("TEXT LEAVES DEVICE", "TEXT WIRD AN DIE CLOUD GESENDET"),
                    ("EVERYTHING STAYS HERE", "ALLES WIRD LOKAL VERARBEITET"),
                    (
                        "Nothing can transcribe right now",
                        "Diktat ist derzeit nicht verfügbar",
                    ),
                    (
                        "Nothing can transcribe · text leaves this device for {text}",
                        "Diktat nicht verfügbar · Text wird an {text} gesendet",
                    ),
                    (
                        "SPEECH — WHAT TRANSCRIBES A RECORDING",
                        "DIKTAT — DIENSTE ZUR SPRACHERKENNUNG",
                    ),
                    (
                        "TEXT — WHAT TRANSLATES A SELECTION",
                        "ÜBERSETZUNG — DIENSTE FÜR TEXTÜBERSETZUNG",
                    ),
                    ("DICTATION LANGUAGE", "DIKTATSPRACHE"),
                    ("TRANSCRIPT LANGUAGE", "SPRACHE DER AUSGABE"),
                    ("PROCESSING MODE", "VERARBEITUNGSMODUS"),
                    ("Fast response", "Schnelle Antwort"),
                    ("Lower resource use", "Ressourcensparend"),
                    ("Interface language", "Sprache der Benutzeroberfläche"),
                    ("Follow Windows settings", "Wie in Windows"),
                    ("ACTION", "AKTION"),
                    ("Insert latest dictation", "Letztes Diktat einfügen"),
                    ("Act on selection", "Auswahl verarbeiten"),
                    ("Correct keyboard layout", "Tastaturlayout korrigieren"),
                    ("Never reached", "Übersprungen"),
                    (
                        "No compatible speech backend is active",
                        "Keine kompatible Spracherkennungs-Engine ist aktiv",
                    ),
                ],
            ),
            (
                "fr",
                include_str!("../translations/fr/LC_MESSAGES/sayflex-desktop.po"),
                [
                    ("Locally", "En local"),
                    ("Fastest available", "Option la plus rapide"),
                    ("Modern GPU", "GPU récente"),
                    ("Custom endpoint", "Serveur personnalisé"),
                    ("Providers", "Services"),
                    ("SOURCE TEXT", "TEXTE SOURCE"),
                    (
                        "Runs on this computer · nothing leaves it",
                        "Fonctionne en local · aucune donnée n’est envoyée",
                    ),
                    ("TEXT LEAVES DEVICE", "LE TEXTE EST ENVOYÉ DANS LE CLOUD"),
                    ("EVERYTHING STAYS HERE", "TOUT EST TRAITÉ LOCALEMENT"),
                    (
                        "Nothing can transcribe right now",
                        "La dictée est indisponible pour le moment",
                    ),
                    (
                        "Nothing can transcribe · text leaves this device for {text}",
                        "Dictée indisponible · le texte est envoyé à {text}",
                    ),
                    (
                        "SPEECH — WHAT TRANSCRIBES A RECORDING",
                        "DICTÉE — SERVICES DE RECONNAISSANCE VOCALE",
                    ),
                    (
                        "TEXT — WHAT TRANSLATES A SELECTION",
                        "TRADUCTION — SERVICES DE TRADUCTION DE TEXTE",
                    ),
                    ("DICTATION LANGUAGE", "LANGUE DE DICTÉE"),
                    ("TRANSCRIPT LANGUAGE", "LANGUE DU RÉSULTAT"),
                    ("PROCESSING MODE", "MODE DE TRAITEMENT"),
                    ("Fast response", "Réponse rapide"),
                    ("Lower resource use", "Économie de ressources"),
                    ("Interface language", "Langue de l’interface"),
                    ("Follow Windows settings", "Comme dans Windows"),
                    ("ACTION", "ACTION"),
                    ("Insert latest dictation", "Insérer la dernière dictée"),
                    ("Act on selection", "Agir sur la sélection"),
                    (
                        "Correct keyboard layout",
                        "Corriger la disposition du clavier",
                    ),
                    ("Never reached", "Ignoré"),
                    (
                        "No compatible speech backend is active",
                        "Aucun moteur de reconnaissance vocale compatible n’est actif",
                    ),
                ],
            ),
            (
                "it",
                include_str!("../translations/it/LC_MESSAGES/sayflex-desktop.po"),
                [
                    ("Locally", "In locale"),
                    ("Fastest available", "Opzione più veloce"),
                    ("Modern GPU", "GPU moderna"),
                    ("Custom endpoint", "Server personalizzato"),
                    ("Providers", "Provider"),
                    ("SOURCE TEXT", "TESTO ORIGINALE"),
                    (
                        "Runs on this computer · nothing leaves it",
                        "Funziona in locale · nessun dato viene inviato",
                    ),
                    ("TEXT LEAVES DEVICE", "IL TESTO VIENE INVIATO AL CLOUD"),
                    ("EVERYTHING STAYS HERE", "TUTTO VIENE ELABORATO IN LOCALE"),
                    (
                        "Nothing can transcribe right now",
                        "La dettatura non è disponibile al momento",
                    ),
                    (
                        "Nothing can transcribe · text leaves this device for {text}",
                        "Dettatura non disponibile · il testo viene inviato a {text}",
                    ),
                    (
                        "SPEECH — WHAT TRANSCRIBES A RECORDING",
                        "DETTATURA — SERVIZI DI RICONOSCIMENTO VOCALE",
                    ),
                    (
                        "TEXT — WHAT TRANSLATES A SELECTION",
                        "TRADUZIONE — SERVIZI DI TRADUZIONE DEL TESTO",
                    ),
                    ("DICTATION LANGUAGE", "LINGUA DI DETTATURA"),
                    ("TRANSCRIPT LANGUAGE", "LINGUA DEL RISULTATO"),
                    ("PROCESSING MODE", "MODALITÀ DI ELABORAZIONE"),
                    ("Fast response", "Risposta rapida"),
                    ("Lower resource use", "Risparmio di risorse"),
                    ("Interface language", "Lingua dell’interfaccia"),
                    ("Follow Windows settings", "Come in Windows"),
                    ("ACTION", "AZIONE"),
                    ("Insert latest dictation", "Inserisci l’ultima dettatura"),
                    ("Act on selection", "Agisci sulla selezione"),
                    (
                        "Correct keyboard layout",
                        "Correggi il layout della tastiera",
                    ),
                    ("Never reached", "Saltato"),
                    (
                        "No compatible speech backend is active",
                        "Non è attivo alcun motore di riconoscimento vocale compatibile",
                    ),
                ],
            ),
            (
                "ko",
                include_str!("../translations/ko/LC_MESSAGES/sayflex-desktop.po"),
                [
                    ("Locally", "로컬"),
                    ("Fastest available", "사용 가능한 최고 속도"),
                    ("Modern GPU", "최신 GPU"),
                    ("Custom endpoint", "사용자 지정 서버"),
                    ("Providers", "서비스"),
                    ("SOURCE TEXT", "원문"),
                    (
                        "Runs on this computer · nothing leaves it",
                        "로컬에서 실행 · 데이터를 전송하지 않음",
                    ),
                    ("TEXT LEAVES DEVICE", "텍스트를 클라우드로 전송"),
                    ("EVERYTHING STAYS HERE", "모두 로컬에서 처리"),
                    (
                        "Nothing can transcribe right now",
                        "현재 받아쓰기를 사용할 수 없음",
                    ),
                    (
                        "Nothing can transcribe · text leaves this device for {text}",
                        "받아쓰기 사용 불가 · 텍스트 전송 대상: {text}",
                    ),
                    (
                        "SPEECH — WHAT TRANSCRIBES A RECORDING",
                        "받아쓰기 — 음성 인식 서비스",
                    ),
                    (
                        "TEXT — WHAT TRANSLATES A SELECTION",
                        "번역 — 텍스트 번역 서비스",
                    ),
                    ("DICTATION LANGUAGE", "받아쓰기 언어"),
                    ("TRANSCRIPT LANGUAGE", "결과 언어"),
                    ("PROCESSING MODE", "처리 모드"),
                    ("Fast response", "빠른 응답"),
                    ("Lower resource use", "리소스 절약"),
                    ("Interface language", "인터페이스 언어"),
                    ("Follow Windows settings", "Windows 설정 따르기"),
                    ("ACTION", "동작"),
                    ("Insert latest dictation", "최근 받아쓰기 삽입"),
                    ("Act on selection", "선택 영역 처리"),
                    ("Correct keyboard layout", "키보드 레이아웃 수정"),
                    ("Never reached", "건너뜀"),
                    (
                        "No compatible speech backend is active",
                        "호환되는 음성 인식 엔진이 활성화되어 있지 않음",
                    ),
                ],
            ),
        ];

        for (language, source, expected) in catalogs {
            let entries = catalog(source);
            for (message, translation) in expected {
                assert_eq!(
                    entries.get(message).map(String::as_str),
                    Some(translation),
                    "{language}: {message}"
                );
            }
        }
    }

    #[test]
    fn technical_labels_remain_stable_in_every_catalog() {
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
            for label in ["CPU", "CUDA", "Vulkan", "PIN", "ORIG", "DUAL", "ON IMAGE", "DEL"] {
                assert_eq!(
                    entries.get(label).map(String::as_str),
                    Some(label),
                    "{language}: {label} is a product label, not prose"
                );
            }
        }
    }

    #[test]
    fn catalogs_do_not_define_the_same_message_twice() {
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
            let mut seen = BTreeSet::new();
            let duplicates = source
                .lines()
                .filter_map(|line| {
                    line.strip_prefix("msgid \"")
                        .and_then(|value| value.strip_suffix('"'))
                })
                .filter(|message| !message.is_empty())
                .filter(|message| !seen.insert((*message).to_owned()))
                .collect::<Vec<_>>();
            assert!(
                duplicates.is_empty(),
                "{language}: duplicate message ids {duplicates:?}"
            );
        }
    }

    #[test]
    fn typed_catalog_messages_preserve_named_placeholders() {
        let placeholders = |value: &str| {
            let mut names = Vec::new();
            let mut rest = value;
            while let Some(start) = rest.find('{') {
                let tail = &rest[start + 1..];
                let Some(end) = tail.find('}') else {
                    break;
                };
                names.push(tail[..end].to_owned());
                rest = &tail[end + 1..];
            }
            names.sort();
            names
        };
        let typed_messages = include_str!("../ui_texts.txt")
            .lines()
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .filter_map(|line| line.split_once('|').map(|(_, message)| message))
            .collect::<Vec<_>>();

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
            let entries = catalog(source);
            for message in &typed_messages {
                assert_eq!(
                    placeholders(message),
                    placeholders(&entries[*message]),
                    "{language}: placeholders changed in {message:?}"
                );
            }
        }
    }

    #[test]
    fn screen_translation_outcomes_are_translated_in_every_shipped_locale() {
        let messages = [
            "Screen capture is unavailable. Try again.",
            "This content is protected and cannot be captured.",
            "Select an area that contains text.",
            "No readable text was found in the selected area.",
            "The local screen-reading model is not installed. Install it in Models.",
            "This language is not available for local screen reading.",
            "Screen text recognition stopped unexpectedly. Try again.",
            "Screen translation was cancelled.",
        ];
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
            let entries = catalog(source);
            for message in messages {
                let translated = entries.get(message).unwrap_or_else(|| {
                    panic!("{language}: missing screen translation outcome {message:?}")
                });
                assert_ne!(
                    translated, message,
                    "{language}: screen translation outcome leaked English"
                );
            }
        }
    }

    #[test]
    fn typed_screen_translation_outcomes_select_specific_user_copy() {
        use super::{ScreenTranslationOutcome, screen_translation_outcome_text};

        assert_eq!(
            screen_translation_outcome_text(
                UiLocale::Russian,
                ScreenTranslationOutcome::ProtectedContent,
            ),
            "Защищённое содержимое нельзя захватить."
        );
        assert_eq!(
            screen_translation_outcome_text(UiLocale::Korean, ScreenTranslationOutcome::NoText),
            "선택한 영역에서 읽을 수 있는 텍스트를 찾지 못했습니다."
        );
    }

    #[test]
    fn interface_language_control_lives_on_the_about_page() {
        use i_slint_backend_testing::ElementHandle;
        use slint::ComponentHandle;

        i_slint_backend_testing::init_no_event_loop();
        let seed = crate::SettingsWindow::new().expect("seed settings surface");
        slint::select_bundled_translation("en").expect("select English bundle");
        let settings = crate::SettingsWindow::new().expect("settings surface");
        settings.show().expect("show headless settings surface");

        settings.set_active_page(0);
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::ZERO);
        assert!(
            ElementHandle::find_by_accessible_label(&settings, "System language")
                .next()
                .is_none(),
            "the global header must not carry an application setting"
        );

        settings.set_active_page(10);
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::ZERO);
        assert!(
            ElementHandle::find_by_accessible_label(&settings, "System language")
                .next()
                .is_some(),
            "the About page must expose the interface-language setting"
        );
        drop(seed);
    }

    #[test]
    fn every_settings_page_reports_the_real_eleven_page_count() {
        use i_slint_backend_testing::ElementHandle;
        use slint::ComponentHandle;

        i_slint_backend_testing::init_no_event_loop();
        let seed = crate::SettingsWindow::new().expect("seed settings surface");
        slint::select_bundled_translation("en").expect("select English bundle");
        let settings = crate::SettingsWindow::new().expect("settings surface");
        settings.show().expect("show headless settings surface");

        for page in 0..11 {
            settings.set_active_page(page);
            i_slint_backend_testing::mock_elapsed_time(std::time::Duration::ZERO);
            let expected = format!("{:02} / 11", page + 1);
            assert!(
                ElementHandle::find_by_accessible_label(&settings, &expected)
                    .next()
                    .is_some(),
                "page {page} must expose {expected}"
            );
        }
        drop(seed);
    }

    #[test]
    fn resources_page_explains_that_unloaded_models_return_automatically_in_every_locale() {
        let sources = [
            "SayFlex total",
            "Unloading only frees working memory. The selected models remain installed. The next time you use a function, SayFlex loads its model automatically and the function continues normally.",
            "SayFlex may unload idle engines to protect system resources. Active work is never interrupted.",
        ];
        for (locale, po) in [
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
            let entries = catalog(po);
            for source in sources {
                let translated = entries
                    .get(source)
                    .unwrap_or_else(|| panic!("{locale}: missing resources-page copy: {source}"));
                assert!(
                    !translated.trim().is_empty(),
                    "{locale}: empty copy: {source}"
                );
                assert_ne!(
                    translated, source,
                    "{locale}: copy is not localized: {source}"
                );
            }
        }
    }

    #[test]
    fn translated_engine_tiles_do_not_overlap() {
        use i_slint_backend_testing::ElementHandle;
        use slint::ComponentHandle;

        i_slint_backend_testing::init_no_event_loop();
        let seed = crate::SettingsWindow::new().expect("seed settings surface");
        for (locale, source) in [
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
            slint::select_bundled_translation(locale).expect("select translated bundle");
            let settings = crate::SettingsWindow::new().expect("settings surface");
            settings.show().expect("show headless settings surface");
            settings.set_active_page(1);
            i_slint_backend_testing::mock_elapsed_time(std::time::Duration::ZERO);
            let entries = catalog(source);

            let find = |label: &str| {
                ElementHandle::find_by_accessible_label(&settings, label)
                    .next()
                    .unwrap_or_else(|| panic!("{locale}: missing engine tile {label}"))
            };
            let auto = find(&entries["Auto"]);
            let cuda = find("CUDA");
            let vulkan = find("Vulkan");
            let cpu = find("CPU");

            for tile in [&auto, &cuda, &vulkan, &cpu] {
                assert!(
                    tile.size().height >= 68.0,
                    "{locale}: translated tile is too short"
                );
            }
            let separated = |first: &ElementHandle, second: &ElementHandle| {
                let a = first.absolute_position();
                let b = second.absolute_position();
                let a_size = first.size();
                let b_size = second.size();
                a.x + a_size.width <= b.x
                    || b.x + b_size.width <= a.x
                    || a.y + a_size.height <= b.y
                    || b.y + b_size.height <= a.y
            };
            assert!(separated(&auto, &cuda), "{locale}: top row overlaps");
            assert!(separated(&auto, &vulkan), "{locale}: left column overlaps");
            assert!(separated(&cuda, &cpu), "{locale}: right column overlaps");
            assert!(separated(&vulkan, &cpu), "{locale}: bottom row overlaps");
        }
        drop(seed);
    }

    #[test]
    fn translated_visualizer_tiles_keep_a_readable_width() {
        use i_slint_backend_testing::ElementHandle;
        use slint::ComponentHandle;

        i_slint_backend_testing::init_no_event_loop();
        let seed = crate::SettingsWindow::new().expect("seed settings surface");
        for (locale, source) in [
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
            slint::select_bundled_translation(locale).expect("select translated bundle");
            let entries = catalog(source);
            let settings = crate::SettingsWindow::new().expect("settings surface");
            settings.show().expect("show headless settings surface");
            settings.set_active_page(5);
            i_slint_backend_testing::mock_elapsed_time(std::time::Duration::ZERO);

            let companion =
                ElementHandle::find_by_accessible_label(&settings, &entries["Companion orb"])
                    .next()
                    .unwrap_or_else(|| panic!("{locale}: companion visualizer tile"));
            assert!(
                companion.size().width >= 88.0,
                "{locale}: visualizer tile is only {}px wide",
                companion.size().width,
            );
        }
        drop(seed);
    }

    #[test]
    fn retired_visualizers_are_absent_from_the_visuals_page() {
        use i_slint_backend_testing::ElementHandle;
        use slint::ComponentHandle;

        i_slint_backend_testing::init_no_event_loop();
        let settings = crate::SettingsWindow::new().expect("settings surface");
        settings.show().expect("show headless settings surface");
        settings.set_active_page(5);
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::ZERO);

        for retired in ["Voice orb", "Spectrum", "Fluid ribbon"] {
            assert!(
                ElementHandle::find_by_accessible_label(&settings, retired)
                    .next()
                    .is_none(),
                "retired visualizer remains selectable: {retired}"
            );
        }
        for released in [
            "Broadcast mic",
            "Circular",
            "Flow sphere",
            "Writing slate",
            "Companion orb",
            "Pulse matrix",
            "Off",
        ] {
            assert!(
                ElementHandle::find_by_accessible_label(&settings, released)
                    .next()
                    .is_some(),
                "released visualizer is missing: {released}"
            );
        }
    }

    #[test]
    fn translated_provider_actions_fit_side_by_side() {
        use i_slint_backend_testing::ElementHandle;
        use slint::{ComponentHandle, ModelRc, VecModel};

        i_slint_backend_testing::init_no_event_loop();
        let seed = crate::SettingsWindow::new().expect("seed settings surface");
        for (locale, source) in [
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
            slint::select_bundled_translation(locale).expect("select translated bundle");
            let entries = catalog(source);
            let settings = crate::SettingsWindow::new().expect("settings surface");
            settings.set_provider_rows(ModelRc::new(VecModel::from(vec![crate::ProviderRow {
                id: "speech:groq".into(),
                section: "SPEECH".into(),
                title: "Groq".into(),
                detail: "Key ••••9c07 · Cloud speech · availability follows the configured account and service region".into(),
                enabled: true,
                available: true,
                needs_key: true,
                key_hint: "".into(),
                field_label: "".into(),
                field_value: "".into(),
                state: entries["Key refused"].clone().into(),
                tone: "off".into(),
                unreachable: false,
            }])));
            settings.show().expect("show headless settings surface");
            settings.set_active_page(4);
            i_slint_backend_testing::mock_elapsed_time(std::time::Duration::ZERO);

            let only =
                ElementHandle::find_by_accessible_label(&settings, &entries["Only this service"])
                    .next()
                    .unwrap_or_else(|| panic!("{locale}: exclusive-provider action"));
            let add = ElementHandle::find_by_accessible_label(&settings, &entries["Add key"])
                .next()
                .unwrap_or_else(|| panic!("{locale}: credential action"));
            let only_position = only.absolute_position();
            let add_position = add.absolute_position();
            let list = ElementHandle::find_by_element_id(&settings, "ProvidersPage::list")
                .next()
                .unwrap_or_else(|| panic!("{locale}: provider list"));
            let provider_row = ElementHandle::find_by_element_type_name(&settings, "ListRow")
                .next()
                .unwrap_or_else(|| panic!("{locale}: provider row"));
            assert!(
                only_position.x + only.size().width <= add_position.x,
                "{locale}: translated provider actions overlap: only_right={} add_left={}",
                only_position.x + only.size().width,
                add_position.x,
            );
            let list_position = list.absolute_position();
            let row_position = provider_row.absolute_position();
            assert!(
                add_position.x + add.size().width <= list_position.x + list.size().width,
                "{locale}: translated provider actions leave the page: action_right={} list_right={} row_left={} row_width={}",
                add_position.x + add.size().width,
                list_position.x + list.size().width,
                row_position.x,
                provider_row.size().width,
            );
        }
        drop(seed);
    }

    #[test]
    fn every_provider_privacy_state_stays_inside_the_visible_header() {
        use i_slint_backend_testing::{ElementHandle, ElementRoot};
        use slint::ComponentHandle;

        i_slint_backend_testing::init_no_event_loop();
        let seed = crate::SettingsWindow::new().expect("seed settings surface");
        for locale in UiLocale::ALL
            .into_iter()
            .filter(|locale| *locale != UiLocale::System)
        {
            slint::select_bundled_translation(locale.code())
                .unwrap_or_else(|error| panic!("{} bundle failed: {error}", locale.code()));
            let settings = crate::SettingsWindow::new().expect("settings surface");
            let claims = [
                super::text(locale, super::UiText::EverythingStays).to_owned(),
                super::render(
                    locale,
                    super::UiText::NothingCanTranscribeTextLeaves,
                    &[("text", "DeepL")],
                ),
                super::render(
                    locale,
                    super::UiText::DifferentServices,
                    &[("audio", "Microsoft Azure Speech"), ("text", "DeepL")],
                ),
            ];

            settings.show().expect("show headless settings surface");
            settings.set_active_page(4);
            for claim in claims {
                settings.set_provider_claim(claim.clone().into());
                settings.set_audio_leaves_device(true);
                i_slint_backend_testing::mock_elapsed_time(std::time::Duration::ZERO);

                let element = ElementHandle::find_by_accessible_label(&settings, &claim)
                    .next()
                    .unwrap_or_else(|| panic!("{}: missing provider claim {claim}", locale.code()));
                let surface = settings.root_element().size();
                let position = element.absolute_position();
                let size = element.size();
                assert!(
                    position.x >= -0.5 && position.x + size.width <= surface.width + 0.5,
                    "{}: claim leaves the window: x={} width={} surface={} text={claim}",
                    locale.code(),
                    position.x,
                    size.width,
                    surface.width,
                );
                assert!(
                    position.y >= -0.5 && position.y + size.height <= surface.height + 0.5,
                    "{}: claim leaves the window vertically: y={} height={} surface={} text={claim}",
                    locale.code(),
                    position.y,
                    size.height,
                    surface.height,
                );
            }
        }
        drop(seed);
    }

    #[test]
    fn provider_page_state_matrix_keeps_controls_and_group_identity_intact() {
        use i_slint_backend_testing::{AccessibleRole, ElementHandle, ElementQuery, ElementRoot};
        use slint::{ComponentHandle, ModelRc, VecModel};

        i_slint_backend_testing::init_no_event_loop();
        let seed = crate::SettingsWindow::new().expect("seed settings surface");
        for (locale, source) in [
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
            slint::select_bundled_translation(locale).expect("select translated bundle");
            let entries = catalog(source);
            let row = |id: &str,
                       section: &str,
                       title: &str,
                       enabled: bool,
                       needs_key: bool,
                       key_hint: &str,
                       field_label: &str,
                       state: &str,
                       tone: &str| crate::ProviderRow {
                id: id.into(),
                section: section.into(),
                title: title.into(),
                detail: entries["Cloud speech · availability follows your Azure account"]
                    .clone()
                    .into(),
                enabled,
                available: enabled && (!needs_key || !key_hint.is_empty()),
                needs_key,
                key_hint: key_hint.into(),
                field_label: field_label.into(),
                field_value: "westeurope".into(),
                state: state.into(),
                tone: tone.into(),
                unreachable: false,
            };
            let settings = crate::SettingsWindow::new().expect("settings surface");
            settings.set_provider_rows(ModelRc::new(VecModel::from(vec![
                row(
                    "speech:local",
                    "SPEECH",
                    &entries["This computer"],
                    false,
                    false,
                    "",
                    "",
                    &entries["Switched off"],
                    "off",
                ),
                row(
                    "speech:groq",
                    "SPEECH",
                    "Groq",
                    false,
                    true,
                    "••••9c07",
                    "",
                    &entries["Switched off"],
                    "off",
                ),
                row(
                    "speech:azure",
                    "SPEECH",
                    "Microsoft Azure Speech",
                    true,
                    true,
                    "••••1f30",
                    &entries["Region"],
                    &entries["Cooling down"],
                    "working",
                ),
                row(
                    "speech:custom",
                    "SPEECH",
                    &entries["Custom endpoint"],
                    true,
                    true,
                    "",
                    &entries["Endpoint"],
                    &entries["No key stored"],
                    "off",
                ),
                row(
                    "text:local",
                    "TEXT",
                    &entries["This computer"],
                    false,
                    false,
                    "",
                    "",
                    &entries["Switched off"],
                    "off",
                ),
                row(
                    "text:deepl",
                    "TEXT",
                    "DeepL",
                    true,
                    true,
                    "••••7b22",
                    "",
                    &entries["In use"],
                    "active",
                ),
            ])));
            let locale = UiLocale::from_code(locale).expect("known locale");
            let claim = super::render(
                locale,
                super::UiText::NothingCanTranscribeTextLeaves,
                &[("text", "DeepL")],
            );
            settings.set_provider_claim(claim.into());
            settings.set_audio_leaves_device(true);
            settings.set_provider_editing("speech:groq".into());
            settings.set_provider_problem_for("speech:groq".into());
            settings.set_provider_problem(
                "The service did not answer. The existing key was kept unchanged.".into(),
            );
            settings.show().expect("show headless settings surface");
            settings.set_active_page(4);
            i_slint_backend_testing::mock_elapsed_time(std::time::Duration::ZERO);

            assert!(
                ElementHandle::find_by_accessible_label(
                    &settings,
                    &entries["SPEECH — WHAT TRANSCRIBES A RECORDING"],
                )
                .next()
                .is_some(),
                "{locale:?}: speech providers must keep the speech heading",
            );
            for label in ["Locally", "Only this service"] {
                assert!(
                    ElementHandle::find_by_accessible_label(&settings, &entries[label])
                        .next()
                        .is_some(),
                    "{locale:?}: provider state is missing the semantic action {label}",
                );
            }
            for label in ["Save", "Cancel", "Remove key"] {
                assert!(
                    ElementHandle::find_by_accessible_label(&settings, &entries[label])
                        .next()
                        .is_some(),
                    "{locale:?}: open credential state is missing {label}",
                );
            }
            let rail =
                ElementHandle::find_by_accessible_label(&settings, &entries["Scroll position"])
                    .next()
                    .unwrap_or_else(|| panic!("{locale:?}: provider scroll rail is missing"));
            assert_eq!(
                rail.accessible_enabled(),
                Some(true),
                "{locale:?}: the expanded provider state must remain scrollable",
            );

            let surface = settings.root_element().size();
            let controls = ElementQuery::from_root(&settings)
                .match_predicate(|element| {
                    matches!(
                        element.accessible_role(),
                        Some(
                            AccessibleRole::Button
                                | AccessibleRole::Checkbox
                                | AccessibleRole::Combobox
                                | AccessibleRole::RadioButton
                                | AccessibleRole::Slider
                                | AccessibleRole::Spinbox
                                | AccessibleRole::Switch
                                | AccessibleRole::TextInput
                        )
                    )
                })
                .find_all();
            for control in controls {
                let position = control.absolute_position();
                let size = control.size();
                if size.width <= 0.0 || size.height <= 0.0 {
                    continue;
                }
                assert!(
                    position.x >= -0.5 && position.x + size.width <= surface.width + 0.5,
                    "{locale:?}: control {:?} at x={} width={} leaves {}px surface",
                    control.accessible_label(),
                    position.x,
                    size.width,
                    surface.width,
                );
            }
        }
        drop(seed);
    }

    #[test]
    fn a_localized_model_cancel_still_invokes_the_cancel_action() {
        use std::{cell::Cell, rc::Rc};

        use i_slint_backend_testing::{ElementHandle, ElementRoot};
        use slint::{ComponentHandle, ModelRc, VecModel};

        i_slint_backend_testing::init_no_event_loop();
        let seed = crate::SettingsWindow::new().expect("seed settings surface");
        for (locale, source) in [
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
            slint::select_bundled_translation(locale).expect("select translated bundle");
            let entries = catalog(source);
            let settings = crate::SettingsWindow::new().expect("settings surface");
            settings.set_model_rows(ModelRc::new(VecModel::from(vec![crate::ModelRow {
                catalog_id: "whisper-large-v3-turbo".into(),
                section: entries["Speech"].clone().into(),
                title: "Whisper Large v3 Turbo".into(),
                detail: entries["Preparing model"].clone().into(),
                size: "1.6 GB".into(),
                state: "50 %".into(),
                tone: "working".into(),
                action: entries["Cancel"].clone().into(),
                action_key: "cancel".into(),
                removable: false,
                remove_label: entries["Remove"].clone().into(),
                remove_question: "Remove model?".into(),
                removing: false,
                selected: false,
                installed: false,
                progress: 0.5,
            }])));
            let chose = Rc::new(Cell::new(false));
            let cancelled = Rc::new(Cell::new(false));
            let chose_from_callback = Rc::clone(&chose);
            settings.on_choose_model(move |_| chose_from_callback.set(true));
            let cancelled_from_callback = Rc::clone(&cancelled);
            settings.on_cancel_model_install(move |_| cancelled_from_callback.set(true));
            settings.show().expect("show headless settings surface");
            settings.set_active_page(3);
            i_slint_backend_testing::mock_elapsed_time(std::time::Duration::ZERO);

            let cancel = ElementHandle::find_by_accessible_label(&settings, &entries["Cancel"])
                .next()
                .unwrap_or_else(|| panic!("{locale}: localized cancel action"));
            let position = cancel.absolute_position();
            let surface = settings.root_element().size();
            assert!(
                position.x + cancel.size().width <= surface.width + 0.5,
                "{locale}: localized cancel action leaves the window",
            );
            cancel.invoke_accessible_default_action();
            assert!(cancelled.get(), "{locale}: localized Cancel must cancel");
            assert!(!chose.get(), "{locale}: localized Cancel must not choose");
        }
        drop(seed);
    }

    #[test]
    fn localized_model_use_and_remove_flow_fit_and_dispatch_the_right_actions() {
        use std::{cell::RefCell, rc::Rc};

        use i_slint_backend_testing::{ElementHandle, ElementRoot};
        use slint::{ComponentHandle, ModelRc, VecModel};

        i_slint_backend_testing::init_no_event_loop();
        let seed = crate::SettingsWindow::new().expect("seed settings surface");
        for (locale, source) in [
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
            slint::select_bundled_translation(locale).expect("select translated bundle");
            let entries = catalog(source);
            let settings = crate::SettingsWindow::new().expect("settings surface");
            settings.set_model_rows(ModelRc::new(VecModel::from(vec![crate::ModelRow {
                catalog_id: "whisper-medium".into(),
                section: entries["Speech"].clone().into(),
                title: "Whisper Medium".into(),
                detail: "Installed model".into(),
                size: "1.5 GB".into(),
                state: entries["Installed"].clone().into(),
                tone: "resting".into(),
                action: entries["Use"].clone().into(),
                action_key: "use".into(),
                removable: true,
                remove_label: entries["Remove"].clone().into(),
                remove_question: format!("{} · Whisper Medium · 1.5 GB", entries["Remove"]).into(),
                removing: false,
                selected: false,
                installed: true,
                progress: 0.0,
            }])));
            let calls = Rc::new(RefCell::new(Vec::new()));
            let chose = Rc::clone(&calls);
            settings.on_choose_model(move |id| chose.borrow_mut().push(format!("use:{id}")));
            let removed = Rc::clone(&calls);
            settings.on_remove_model(move |id| removed.borrow_mut().push(format!("remove:{id}")));
            settings.show().expect("show headless settings surface");
            settings.set_active_page(3);
            i_slint_backend_testing::mock_elapsed_time(std::time::Duration::ZERO);

            let surface = settings.root_element().size();
            for label in [&entries["Use"], &entries["Remove"]] {
                let control = ElementHandle::find_by_accessible_label(&settings, label)
                    .next()
                    .unwrap_or_else(|| panic!("{locale}: missing model action {label}"));
                let position = control.absolute_position();
                assert!(
                    position.x >= -0.5 && position.x + control.size().width <= surface.width + 0.5,
                    "{locale}: model action {label:?} leaves the client area",
                );
            }

            ElementHandle::find_by_accessible_label(&settings, &entries["Use"])
                .next()
                .expect("Use action")
                .invoke_accessible_default_action();
            ElementHandle::find_by_accessible_label(&settings, &entries["Remove"])
                .next()
                .expect("Remove action")
                .invoke_accessible_default_action();
            i_slint_backend_testing::mock_elapsed_time(std::time::Duration::ZERO);
            ElementHandle::find_by_accessible_label(&settings, &entries["Yes"])
                .next()
                .unwrap_or_else(|| panic!("{locale}: missing removal confirmation"))
                .invoke_accessible_default_action();

            assert_eq!(
                calls.borrow().as_slice(),
                ["use:whisper-medium", "remove:whisper-medium"],
                "{locale}: model actions must keep stable identities",
            );
        }
        drop(seed);
    }

    #[test]
    fn companion_help_is_contextual_and_names_the_complete_gesture() {
        i_slint_backend_testing::init_no_event_loop();
        let settings = crate::SettingsWindow::new().expect("settings surface");
        settings.set_ui_locale("en".into());

        crate::settings_view::sync_visualizer_copy(
            &settings,
            crate::visualizer::VisualizerStyle::CompanionOrb,
        );
        let companion_help = settings.get_visuals_note().to_string();
        assert!(companion_help.contains("Ctrl + Alt"));
        assert!(companion_help.contains("left mouse button"));
        assert!(companion_help.contains("position is saved"));

        crate::settings_view::sync_visualizer_copy(
            &settings,
            crate::visualizer::VisualizerStyle::BroadcastMic,
        );
        assert_ne!(settings.get_visuals_note().as_str(), companion_help);
    }

    #[test]
    fn a_localized_custom_endpoint_editor_keeps_its_endpoint_hint() {
        use i_slint_backend_testing::ElementHandle;
        use slint::{ComponentHandle, ModelRc, VecModel};

        i_slint_backend_testing::init_no_event_loop();
        let seed = crate::SettingsWindow::new().expect("seed settings surface");
        for (locale, field_label) in [
            ("ru", "Адрес сервера"),
            ("uk", "Адреса сервера"),
            ("es", "Dirección del servidor"),
            ("pt-BR", "Endereço do servidor"),
            ("de", "Serveradresse"),
            ("fr", "Adresse du serveur"),
            ("it", "Indirizzo del server"),
            ("ko", "서버 주소"),
        ] {
            slint::select_bundled_translation(locale).expect("select translated bundle");
            let settings = crate::SettingsWindow::new().expect("settings surface");
            settings.set_provider_rows(ModelRc::new(VecModel::from(vec![crate::ProviderRow {
                id: "speech:custom".into(),
                section: "SPEECH".into(),
                title: "Custom endpoint".into(),
                detail: "OpenAI-compatible endpoint".into(),
                enabled: true,
                available: false,
                needs_key: true,
                key_hint: "".into(),
                field_label: field_label.into(),
                field_value: "".into(),
                state: "No key".into(),
                tone: "off".into(),
                unreachable: false,
            }])));
            settings.set_provider_editing("speech:custom".into());
            settings.show().expect("show headless settings surface");
            settings.set_active_page(4);
            i_slint_backend_testing::mock_elapsed_time(std::time::Duration::ZERO);

            assert!(
                ElementHandle::find_by_accessible_label(
                    &settings,
                    "https://speech.example.com/v1",
                )
                .next()
                .is_some(),
                "{locale}: endpoint hint disappeared after translating its field label",
            );
        }
        drop(seed);
    }

    #[test]
    fn localized_hotkey_capture_state_keeps_its_prompt_and_actions_visible() {
        use std::{cell::Cell, rc::Rc};

        use i_slint_backend_testing::{AccessibleRole, ElementHandle, ElementQuery, ElementRoot};
        use slint::{ComponentHandle, ModelRc, VecModel};

        i_slint_backend_testing::init_no_event_loop();
        let seed = crate::SettingsWindow::new().expect("seed settings surface");
        for (locale, source) in [
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
            slint::select_bundled_translation(locale).expect("select translated bundle");
            let entries = catalog(source);
            let settings = crate::SettingsWindow::new().expect("settings surface");
            settings.set_hotkey_rows(ModelRc::new(VecModel::from(vec![crate::HotkeyRow {
                action: "translate".into(),
                keys: "Ctrl + Alt + F".into(),
                name: entries["Act on selection"].clone().into(),
                detail: entries["On selected text"].clone().into(),
                state: entries["Ready"].clone().into(),
                tone: "resting".into(),
                problem: false,
                enabled: true,
                toggle_enabled: true,
                is_default: false,
                locked_reason: "".into(),
                mode: "text-action".into(),
                mode_value: entries["Decide from the text"].clone().into(),
                mode_key: "adaptive".into(),
            }])));
            settings.set_hotkey_capture_index(0);
            settings.set_hotkey_capture_label(entries["Act on selection"].clone().into());
            settings.set_hotkey_capture_value("Ctrl + Alt + G".into());
            settings.set_hotkey_capture_valid(true);
            let saved = Rc::new(Cell::new(false));
            let saved_from_callback = Rc::clone(&saved);
            settings.on_save_hotkey_capture(move || saved_from_callback.set(true));
            settings.show().expect("show headless settings surface");
            settings.set_active_page(8);
            i_slint_backend_testing::mock_elapsed_time(std::time::Duration::ZERO);

            let cancel = ElementHandle::find_by_accessible_label(&settings, &entries["Cancel"])
                .next()
                .unwrap_or_else(|| panic!("{locale}: capture cancel is missing"));
            assert_eq!(cancel.accessible_role(), Some(AccessibleRole::Button));
            let save = ElementHandle::find_by_accessible_label(&settings, &entries["Save"])
                .next()
                .unwrap_or_else(|| panic!("{locale}: capture save is missing"));
            assert_eq!(save.accessible_enabled(), Some(true));
            save.invoke_accessible_default_action();
            assert!(
                saved.get(),
                "{locale}: Save does not submit the captured chord"
            );
            assert!(
                ElementHandle::find_by_accessible_label(&settings, "Ctrl + Alt + G")
                    .next()
                    .is_some(),
                "{locale}: captured shortcut is not visible",
            );
            let surface = settings.root_element().size();
            let controls = ElementQuery::from_root(&settings)
                .match_predicate(|element| {
                    matches!(
                        element.accessible_role(),
                        Some(
                            AccessibleRole::Button
                                | AccessibleRole::Combobox
                                | AccessibleRole::Switch
                        )
                    )
                })
                .find_all();
            for control in controls {
                let position = control.absolute_position();
                let size = control.size();
                if size.width <= 0.0 || size.height <= 0.0 {
                    continue;
                }
                assert!(
                    position.x >= -0.5 && position.x + size.width <= surface.width + 0.5,
                    "{locale}: capture control {:?} leaves the window",
                    control.accessible_label(),
                );
            }
        }
        drop(seed);
    }

    #[test]
    fn text_action_mode_value_uses_the_row_value_column() {
        use i_slint_backend_testing::ElementHandle;
        use slint::{ComponentHandle, ModelRc, VecModel};

        i_slint_backend_testing::init_no_event_loop();
        let settings = crate::SettingsWindow::new().expect("settings surface");
        settings.set_hotkey_rows(ModelRc::new(VecModel::from(vec![crate::HotkeyRow {
            action: "translate".into(),
            keys: "Ctrl + Alt + F".into(),
            name: "Text actions".into(),
            detail: "On selected text".into(),
            state: "Ready".into(),
            tone: "resting".into(),
            problem: false,
            enabled: true,
            toggle_enabled: true,
            is_default: true,
            locked_reason: "".into(),
            mode: "text-action".into(),
            mode_value: "Decide from the text".into(),
            mode_key: "adaptive".into(),
        }])));
        settings.show().expect("show headless settings surface");
        settings.set_active_page(8);
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::ZERO);

        let mode = ElementHandle::find_by_accessible_label(&settings, "Decide from the text")
            .next()
            .expect("text action value selector");
        let row = ElementHandle::find_by_element_type_name(&settings, "ListRow")
            .next()
            .expect("text action row");
        let mode_right = mode.absolute_position().x + mode.size().width;
        let row_right = row.absolute_position().x + row.size().width;
        assert!(
            (row_right - mode_right).abs() <= 1.0,
            "mode value belongs at the value edge: mode_right={mode_right}, row_right={row_right}",
        );
    }

    #[test]
    fn translated_interactive_controls_stay_inside_the_settings_window() {
        use i_slint_backend_testing::{AccessibleRole, ElementQuery, ElementRoot};
        use slint::ComponentHandle;

        i_slint_backend_testing::init_no_event_loop();
        let seed = crate::SettingsWindow::new().expect("seed settings surface");
        for locale in ["ru", "uk", "es", "pt-BR", "de", "fr", "it", "ko"] {
            slint::select_bundled_translation(locale).expect("select translated bundle");
            let settings = crate::SettingsWindow::new().expect("settings surface");
            settings.show().expect("show headless settings surface");
            let surface = settings.root_element().size();

            for page in 0..11 {
                settings.set_active_page(page);
                i_slint_backend_testing::mock_elapsed_time(std::time::Duration::ZERO);
                let controls = ElementQuery::from_root(&settings)
                    .match_predicate(|element| {
                        matches!(
                            element.accessible_role(),
                            Some(
                                AccessibleRole::Button
                                    | AccessibleRole::Checkbox
                                    | AccessibleRole::Combobox
                                    | AccessibleRole::RadioButton
                                    | AccessibleRole::Slider
                                    | AccessibleRole::Spinbox
                                    | AccessibleRole::Switch
                                    | AccessibleRole::TextInput
                            )
                        )
                    })
                    .find_all();

                for control in controls {
                    let position = control.absolute_position();
                    let size = control.size();
                    if size.width <= 0.0 || size.height <= 0.0 {
                        continue;
                    }
                    assert!(
                        position.x >= -0.5 && position.x + size.width <= surface.width + 0.5,
                        "{locale} page {page}: control {:?} at x={} width={} leaves {}px surface",
                        control.accessible_label(),
                        position.x,
                        size.width,
                        surface.width,
                    );
                }
            }
        }
        drop(seed);
    }

    #[test]
    fn translated_visible_content_stays_inside_the_horizontal_viewport() {
        use i_slint_backend_testing::{ElementQuery, ElementRoot};
        use slint::ComponentHandle;

        i_slint_backend_testing::init_no_event_loop();
        let seed = crate::SettingsWindow::new().expect("seed settings surface");
        for locale in ["ru", "uk", "es", "pt-BR", "de", "fr", "it", "ko"] {
            slint::select_bundled_translation(locale).expect("select translated bundle");
            let settings = crate::SettingsWindow::new().expect("settings surface");
            settings.show().expect("show headless settings surface");
            let surface = settings.root_element().size();

            for page in 0..11 {
                settings.set_active_page(page);
                i_slint_backend_testing::mock_elapsed_time(std::time::Duration::ZERO);
                let labelled = ElementQuery::from_root(&settings)
                    .match_predicate(|element| {
                        element
                            .accessible_label()
                            .is_some_and(|label| !label.trim().is_empty())
                    })
                    .find_all();
                for element in labelled {
                    let position = element.absolute_position();
                    let size = element.size();
                    if size.width <= 0.0
                        || size.height <= 0.0
                        || position.y >= surface.height
                        || position.y + size.height <= 0.0
                    {
                        continue;
                    }
                    assert!(
                        position.x >= -0.5 && position.x + size.width <= surface.width + 0.5,
                        "{locale} page {page}: visible {:?} at x={} width={} leaves {}px surface",
                        element.accessible_label(),
                        position.x,
                        size.width,
                        surface.width,
                    );
                }
            }
        }
        drop(seed);
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
        settings.set_active_page(10);
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::ZERO);

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

    #[test]
    fn translator_controls_fit_and_remain_addressable_in_every_locale() {
        use i_slint_backend_testing::{ElementHandle, ElementRoot};
        use slint::{ComponentHandle, PhysicalSize};

        i_slint_backend_testing::init_no_event_loop();
        let seed = crate::TranslatorWindow::new().expect("seed translator surface");
        for (locale, source) in [
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
            slint::select_bundled_translation(locale).expect("select translated bundle");
            let entries = catalog(source);
            let translator = crate::TranslatorWindow::new().expect("translator surface");
            translator.set_source_text("A sentence to translate".into());
            translator.set_result_text("Translated sentence".into());
            translator.window().set_size(PhysicalSize::new(620, 460));
            translator.show().expect("show headless translator surface");
            i_slint_backend_testing::mock_elapsed_time(std::time::Duration::ZERO);
            let surface = translator.root_element().size();

            for label in [
                &entries["Source text"],
                &entries["Swap text"],
                &entries["Copy result"],
                &entries["Translate (Ctrl+Enter)"],
            ] {
                let control = ElementHandle::find_by_accessible_label(&translator, label)
                    .next()
                    .unwrap_or_else(|| panic!("{locale}: missing translator control {label}"));
                let position = control.absolute_position();
                let size = control.size();
                assert!(
                    position.x >= -0.5
                        && position.y >= -0.5
                        && position.x + size.width <= surface.width + 0.5
                        && position.y + size.height <= surface.height + 0.5,
                    "{locale}: translator control {label:?} leaves the client area"
                );
            }

            let copy =
                ElementHandle::find_by_accessible_label(&translator, &entries["Copy result"])
                    .next()
                    .expect("copy action");
            let translate = ElementHandle::find_by_accessible_label(
                &translator,
                &entries["Translate (Ctrl+Enter)"],
            )
            .next()
            .expect("translate action");
            assert_eq!(
                copy.size().height,
                translate.size().height,
                "{locale}: translator actions must share one height"
            );
        }
        drop(seed);
    }

    #[test]
    fn first_run_choice_and_confirmation_fit_in_every_locale() {
        use i_slint_backend_testing::{ElementHandle, ElementRoot};
        use slint::ComponentHandle;

        i_slint_backend_testing::init_no_event_loop();
        let seed = crate::FirstRunWindow::new().expect("seed first-run surface");
        for (locale, source) in [
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
            slint::select_bundled_translation(locale).expect("select translated bundle");
            let entries = catalog(source);
            let first_run = crate::FirstRunWindow::new().expect("first-run surface");
            crate::first_run::configure_first_run_window(&first_run, true).expect("catalog model");
            first_run.show().expect("show headless first-run surface");
            i_slint_backend_testing::mock_elapsed_time(std::time::Duration::ZERO);
            let surface = first_run.root_element().size();

            for label in [
                &entries["Download the recommended local model"],
                &entries["Continue"],
            ] {
                let control = ElementHandle::find_by_accessible_label(&first_run, label)
                    .next()
                    .unwrap_or_else(|| panic!("{locale}: missing first-run control {label}"));
                let position = control.absolute_position();
                let size = control.size();
                assert!(
                    position.x >= -0.5
                        && position.y >= -0.5
                        && position.x + size.width <= surface.width + 0.5
                        && position.y + size.height <= surface.height + 0.5,
                    "{locale}: first-run control {label:?} leaves the client area"
                );
            }
        }
        drop(seed);
    }

    #[test]
    fn support_action_is_always_available_and_fits_in_every_locale() {
        use std::{cell::Cell, rc::Rc};

        use i_slint_backend_testing::{ElementHandle, ElementRoot};
        use slint::ComponentHandle;

        i_slint_backend_testing::init_no_event_loop();
        let seed = crate::SettingsWindow::new().expect("seed settings surface");
        for (locale, source) in [
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
            slint::select_bundled_translation(locale).expect("select translated bundle");
            let entries = catalog(source);
            let settings = crate::SettingsWindow::new().expect("settings surface");
            settings.set_active_page(9);
            settings.show().expect("show headless settings surface");
            i_slint_backend_testing::mock_elapsed_time(std::time::Duration::ZERO);

            let label = &entries["Support the developer"];
            let activations = Rc::new(Cell::new(0));
            let activations_from_callback = Rc::clone(&activations);
            settings.on_open_support(move || {
                activations_from_callback.set(activations_from_callback.get() + 1);
            });
            let action = ElementHandle::find_by_accessible_label(&settings, label)
                .next()
                .unwrap_or_else(|| panic!("{locale}: support action is missing"));
            let position = action.absolute_position();
            let size = action.size();
            let surface = settings.root_element().size();
            assert!(
                position.x >= -0.5
                    && position.y >= -0.5
                    && position.x + size.width <= surface.width + 0.5
                    && position.y + size.height <= surface.height + 0.5,
                "{locale}: support action leaves the settings surface"
            );
            action.invoke_accessible_default_action();
            assert_eq!(
                activations.get(),
                1,
                "{locale}: support action did not fire"
            );
        }
        drop(seed);
    }

    #[test]
    fn support_action_is_aligned_with_its_coffee_icon() {
        use i_slint_backend_testing::ElementHandle;
        use slint::ComponentHandle;

        i_slint_backend_testing::init_no_event_loop();
        let seed = crate::SettingsWindow::new().expect("seed settings surface");
        slint::select_bundled_translation("ru").expect("select translated bundle");
        let entries = catalog(include_str!(
            "../translations/ru/LC_MESSAGES/sayflex-desktop.po"
        ));
        let settings = crate::SettingsWindow::new().expect("settings surface");
        settings.set_active_page(9);
        settings.show().expect("show headless settings surface");
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::ZERO);

        let action =
            ElementHandle::find_by_accessible_label(&settings, &entries["Support the developer"])
                .next()
                .expect("configured support action");
        let icon = ElementHandle::find_by_element_type_name(&settings, "Image")
            .next()
            .expect("coffee icon");
        let action_center = action.absolute_position().y + action.size().height / 2.0;
        let icon_center = icon.absolute_position().y + icon.size().height / 2.0;
        let action_to_icon_gap =
            icon.absolute_position().x - (action.absolute_position().x + action.size().width);

        let optical_cup_center = icon_center + 5.0;
        assert!(
            (action_center - optical_cup_center).abs() <= 0.5,
            "support action center {action_center} differs from coffee cup optical center {optical_cup_center}"
        );
        assert!(
            (action_to_icon_gap - 10.0).abs() <= 0.5,
            "support action gap is {action_to_icon_gap}, expected 10"
        );
        assert_eq!(icon.size().width, 48.0, "coffee icon width");
        assert_eq!(icon.size().height, 48.0, "coffee icon height");
        drop(seed);
    }

    #[test]
    fn support_wallet_window_exposes_both_network_specific_copy_actions() {
        use std::{cell::Cell, rc::Rc};

        use i_slint_backend_testing::{ElementHandle, ElementRoot};
        use slint::{ComponentHandle, Image};

        i_slint_backend_testing::init_no_event_loop();
        let seed = crate::SupportWindow::new().expect("seed support surface");
        for (locale, source) in [
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
            slint::select_bundled_translation(locale).expect("select translated bundle");
            let entries = catalog(source);
            let support = crate::SupportWindow::new().expect("support surface");
            let destinations = crate::support_action::support_destinations();
            support.set_bnb_address(destinations[0].address.into());
            support.set_tron_address(destinations[1].address.into());
            support.set_bnb_qr(Image::from_rgba8(
                crate::support_action::render_qr_pixels(destinations[0].address).expect("BNB QR"),
            ));
            support.set_tron_qr(Image::from_rgba8(
                crate::support_action::render_qr_pixels(destinations[1].address).expect("TRON QR"),
            ));

            let bnb_copies = Rc::new(Cell::new(0));
            let bnb_copies_from_callback = Rc::clone(&bnb_copies);
            support.on_copy_bnb(move || {
                bnb_copies_from_callback.set(bnb_copies_from_callback.get() + 1)
            });
            let tron_copies = Rc::new(Cell::new(0));
            let tron_copies_from_callback = Rc::clone(&tron_copies);
            support.on_copy_tron(move || {
                tron_copies_from_callback.set(tron_copies_from_callback.get() + 1)
            });

            support.show().expect("show headless support surface");
            i_slint_backend_testing::mock_elapsed_time(std::time::Duration::ZERO);
            let surface = support.root_element().size();
            for (label, expected_address, copy_count) in [
                (
                    &entries["Copy BNB Smart Chain address"],
                    destinations[0].address,
                    &bnb_copies,
                ),
                (
                    &entries["Copy TRON address"],
                    destinations[1].address,
                    &tron_copies,
                ),
            ] {
                let action = ElementHandle::find_by_accessible_label(&support, label)
                    .next()
                    .unwrap_or_else(|| panic!("{locale}: missing {label:?}"));
                let position = action.absolute_position();
                let size = action.size();
                assert!(
                    position.x >= -0.5
                        && position.y >= -0.5
                        && position.x + size.width <= surface.width + 0.5
                        && position.y + size.height <= surface.height + 0.5,
                    "{locale}: {label:?} leaves the support surface"
                );
                assert!(
                    ElementHandle::find_by_accessible_label(&support, expected_address)
                        .next()
                        .is_some(),
                    "{locale}: public address is not exposed to accessibility"
                );
                action.invoke_accessible_default_action();
                assert_eq!(copy_count.get(), 1, "{locale}: {label:?} did not fire");
            }
        }
        drop(seed);
    }

    #[test]
    fn translator_surface_uses_one_material_and_explicit_interaction_states() {
        let source = include_str!("../ui/app.slint");

        assert!(source.contains("in-out property <int> presentation-state"));
        assert!(source.contains("source-input.has-focus ? Sheet.signal"));
        assert!(source.contains("background: Sheet.raised"));
        assert!(source.contains("callback submit-shortcut();"));
        assert!(source.contains("source-input.clear-focus();"));
        assert!(source.contains("source: @image-url(\"../assets/action-copy.svg\")"));
        assert!(source.contains("source: @image-url(\"../assets/action-swap.svg\")"));
        assert!(source.contains("label: @tr(\"Copy result\")"));
        assert!(source.contains("label: @tr(\"Swap text\")"));
        assert!(!source.contains("import { ListView, TextEdit }"));
    }

    #[test]
    fn translation_preview_toolbar_contains_only_approved_controls() {
        let source = include_str!("../ui/app.slint");
        let preview = source
            .split("export component TranslationPreview")
            .nth(1)
            .expect("translation preview component");

        assert!(preview.contains("text: @tr(\"ORIG\")"));
        assert!(preview.contains("text: @tr(\"DUAL\")"));
        assert!(preview.contains("text: @tr(\"PIN\")"));
        assert!(
            preview.contains("visible: root.processing;"),
            "a terminal preview must remove the Translating subtree from accessibility"
        );
        for forbidden in [
            "source-language-label",
            "target-language-label",
            "retry-translation",
            "text: \"⇄\"",
            "text: \"↻\"",
        ] {
            assert!(
                !preview.contains(forbidden),
                "unapproved toolbar token: {forbidden}"
            );
        }
    }

    #[test]
    fn layout_chooser_is_compact_and_exposes_candidate_and_close_actions() {
        use std::{cell::Cell, rc::Rc};

        use i_slint_backend_testing::{ElementHandle, ElementRoot};
        use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

        i_slint_backend_testing::init_no_event_loop();
        let chooser = crate::SelectionChooser::new().expect("layout chooser");
        chooser.set_source_text("ghbdtn vbh".into());
        chooser.set_source_panel_height(38.0);
        chooser.set_candidate_labels(ModelRc::new(VecModel::from(vec![SharedString::from(
            "English keys → Cyrillic layout",
        )])));
        chooser.set_candidate_texts(ModelRc::new(VecModel::from(vec![SharedString::from(
            "привет мир",
        )])));

        let chosen = Rc::new(Cell::new(None));
        let chosen_from_callback = Rc::clone(&chosen);
        chooser.on_choose_candidate(move |index| chosen_from_callback.set(Some(index)));
        let cancelled = Rc::new(Cell::new(false));
        let cancelled_from_callback = Rc::clone(&cancelled);
        chooser.on_cancel(move || cancelled_from_callback.set(true));

        chooser.show().expect("show layout chooser");
        chooser.set_prepared(true);
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::ZERO);

        let surface = chooser.root_element().size();
        assert!(
            surface.width <= 480.0,
            "layout chooser regressed to a wide dialog"
        );
        assert!(
            surface.height <= 205.0,
            "one correction must fit in a compact transient card"
        );

        let candidate =
            ElementHandle::find_by_accessible_label(&chooser, "English keys → Cyrillic layout")
                .next()
                .expect("correction candidate action");
        candidate.invoke_accessible_default_action();
        assert_eq!(chosen.get(), Some(0));

        let close = ElementHandle::find_by_accessible_label(&chooser, "Close")
            .next()
            .expect("icon close action");
        close.invoke_accessible_default_action();
        assert!(cancelled.get());
        assert!(
            ElementHandle::find_by_accessible_label(&chooser, "CANCEL")
                .next()
                .is_none(),
            "the legacy footer button must not return"
        );
    }

    #[test]
    fn translator_shortcut_callback_invokes_translate_once_and_releases_focus() {
        use std::{cell::Cell, rc::Rc};

        use i_slint_backend_testing::ElementHandle;
        use slint::{ComponentHandle, platform::PointerEventButton};

        i_slint_backend_testing::init_no_event_loop();
        let translator = crate::TranslatorWindow::new().expect("translator surface");
        translator.show().expect("show translator surface");
        let calls = Rc::new(Cell::new(0));
        let calls_from_callback = Rc::clone(&calls);
        translator.on_translate(move || calls_from_callback.set(calls_from_callback.get() + 1));

        let source = ElementHandle::find_by_accessible_label(&translator, "Source text")
            .next()
            .expect("source field in accessibility tree");
        source.mock_single_click(PointerEventButton::Left);
        assert!(translator.get_source_focused());
        translator.invoke_submit_shortcut();
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::ZERO);

        assert_eq!(calls.get(), 1);
        assert!(!translator.get_source_focused());
    }
}
