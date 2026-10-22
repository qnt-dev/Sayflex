use rust_protocol::{
    TranslationAttemptReport, TranslationFailureKind, TranslationFailureReport,
    TranslationModelSummary, TranslationResult,
};

use crate::localization::{UiLocale, UiText, render, text};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TranslatorSeverity {
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TranslatorNotice {
    pub(crate) severity: TranslatorSeverity,
    pub(crate) headline: String,
    pub(crate) detail: String,
}

impl TranslatorNotice {
    pub(crate) fn message(&self) -> String {
        match self.severity {
            TranslatorSeverity::Warning if !self.detail.trim().is_empty() => {
                format!("{} {}", self.headline, self.detail)
            }
            TranslatorSeverity::Warning | TranslatorSeverity::Error => self.headline.clone(),
        }
    }
}

/// The warning shown under a translation whose numbers, dates or links still
/// differ from the original after the masked retry.
pub(crate) fn protected_values_notice(locale: UiLocale, result: &TranslationResult) -> String {
    if result.protected_values_changed {
        text(locale, UiText::TranslationCheckNumbers).to_owned()
    } else {
        String::new()
    }
}

pub(crate) fn completion_notice(
    locale: UiLocale,
    result: &TranslationResult,
) -> Option<TranslatorNotice> {
    let Some(attempt) = result.attempts.last() else {
        return result.protected_values_changed.then(|| TranslatorNotice {
            severity: TranslatorSeverity::Warning,
            headline: protected_values_notice(locale, result),
            detail: String::new(),
        });
    };
    let final_route = result.provider.as_deref().map_or_else(
        || text(locale, UiText::TranslationFallbackLocal).to_owned(),
        |provider| {
            render(
                locale,
                UiText::TranslationFallbackProvider,
                &[("provider", provider_name(provider))],
            )
        },
    );
    Some(TranslatorNotice {
        severity: TranslatorSeverity::Warning,
        headline: attempt_headline(locale, attempt),
        detail: final_route,
    })
}

pub(crate) fn failure_notice(
    locale: UiLocale,
    failure: &TranslationFailureReport,
    models: &[TranslationModelSummary],
) -> TranslatorNotice {
    if failure.kind == TranslationFailureKind::LocalModelMissing
        && let Some(model) = failure
            .required_catalog_id
            .as_deref()
            .and_then(|required| models.iter().find(|model| model.catalog_id == required))
    {
        let name = crate::settings_projection::translation_model_title_localized(model, locale);
        return TranslatorNotice {
            severity: TranslatorSeverity::Error,
            headline: render(
                locale,
                UiText::TranslationPackageMissing,
                &[("model", &name)],
            ),
            // The catalog gives a localized, actionable name. Technical worker
            // details remain in diagnostics rather than the translation card.
            detail: String::new(),
        };
    }
    let headline = match failure.kind {
        TranslationFailureKind::NoText => text(locale, UiText::EnterTextToTranslate).to_owned(),
        TranslationFailureKind::NoProviderAvailable => {
            text(locale, UiText::NoTranslationProviderAvailable).to_owned()
        }
        TranslationFailureKind::LocalModelMissing => {
            text(locale, UiText::LocalTranslationModelUnavailable).to_owned()
        }
        TranslationFailureKind::Internal => {
            text(locale, UiText::TranslationFailedTryAgain).to_owned()
        }
        _ => failure.provider_id.as_deref().map_or_else(
            || text(locale, UiText::TranslationFailedTryAgain).to_owned(),
            |provider_id| {
                attempt_headline(
                    locale,
                    &TranslationAttemptReport {
                        provider_id: provider_id.to_owned(),
                        kind: failure.kind.clone(),
                        retry_after_ms: failure.retry_after_ms,
                        detail: failure.detail.clone(),
                    },
                )
            },
        ),
    };
    TranslatorNotice {
        severity: TranslatorSeverity::Error,
        headline,
        detail: failure.detail.clone(),
    }
}

fn attempt_headline(locale: UiLocale, attempt: &TranslationAttemptReport) -> String {
    let message = match attempt.kind {
        TranslationFailureKind::Credential => UiText::TranslationProviderCredential,
        TranslationFailureKind::Misconfigured => UiText::TranslationProviderMisconfigured,
        TranslationFailureKind::QuotaExhausted => UiText::TranslationProviderQuota,
        TranslationFailureKind::RateLimited => UiText::TranslationProviderRateLimited,
        TranslationFailureKind::Timeout => UiText::TranslationProviderTimeout,
        TranslationFailureKind::Unavailable => UiText::TranslationProviderUnavailable,
        TranslationFailureKind::UnsupportedRequest => UiText::TranslationProviderUnsupported,
        TranslationFailureKind::Rejected => UiText::TranslationProviderRejected,
        TranslationFailureKind::EmptyResponse => UiText::TranslationProviderEmpty,
        TranslationFailureKind::NoText
        | TranslationFailureKind::NoProviderAvailable
        | TranslationFailureKind::LocalModelMissing
        | TranslationFailureKind::Internal => UiText::TranslationFailedTryAgain,
    };
    render(
        locale,
        message,
        &[("provider", provider_name(&attempt.provider_id))],
    )
}

fn provider_name(provider_id: &str) -> &str {
    match provider_id {
        "deepl" => "DeepL",
        "azure" => "Microsoft Azure",
        "local" => "SayFlex",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use rust_protocol::{
        RequestId, TranslationAttemptReport, TranslationFailureKind, TranslationFailureReport,
        TranslationLanguage, TranslationModelSummary, TranslationResult, TranslationTier,
    };

    use super::{TranslatorSeverity, completion_notice, failure_notice};
    use crate::localization::UiLocale;

    fn result(attempts: Vec<TranslationAttemptReport>) -> TranslationResult {
        TranslationResult {
            request_id: RequestId::new(),
            source_text: "hello".to_owned(),
            translated_text: "привет".to_owned(),
            source_language: TranslationLanguage::English,
            target_language: TranslationLanguage::Russian,
            tier: Some(TranslationTier::Tiny),
            provider: None,
            attempts,
            processing_duration_ms: 1,
            first_token_duration_ms: 1,
            backend_request_count: 1,
            tokenize_duration_ms: 1,
            chunk_count: 1,
            protected_values_changed: false,
        }
    }

    #[test]
    fn successful_quota_fallback_names_both_routes_in_russian() {
        let notice = completion_notice(
            UiLocale::Russian,
            &result(vec![TranslationAttemptReport {
                provider_id: "deepl".to_owned(),
                kind: TranslationFailureKind::QuotaExhausted,
                retry_after_ms: Some(3_600_000),
                detail: String::new(),
            }]),
        )
        .unwrap();

        assert_eq!(notice.severity, TranslatorSeverity::Warning);
        assert_eq!(notice.headline, "Квота DeepL закончилась.");
        assert_eq!(notice.detail, "Перевод завершён локально.");
    }

    #[test]
    fn unsupported_request_is_actionable_without_exposing_raw_detail() {
        let notice = failure_notice(
            UiLocale::English,
            &TranslationFailureReport {
                kind: TranslationFailureKind::UnsupportedRequest,
                provider_id: Some("azure".to_owned()),
                retry_after_ms: None,
                required_catalog_id: None,
                detail: "raw provider response".to_owned(),
                attempts: Vec::new(),
            },
            &[],
        );

        assert_eq!(notice.severity, TranslatorSeverity::Error);
        assert_eq!(
            notice.headline,
            "Microsoft Azure does not support this translation request."
        );
        assert_eq!(notice.detail, "raw provider response");
        assert_eq!(
            notice.message(),
            "Microsoft Azure does not support this translation request."
        );
    }

    #[test]
    fn every_provider_failure_kind_has_specific_user_copy() {
        let cases = [
            (
                TranslationFailureKind::Credential,
                "DeepL rejected the API key. Check or replace it in Providers.",
            ),
            (
                TranslationFailureKind::Misconfigured,
                "DeepL needs different settings. Check it in Providers.",
            ),
            (
                TranslationFailureKind::QuotaExhausted,
                "DeepL has no quota left.",
            ),
            (
                TranslationFailureKind::RateLimited,
                "DeepL is temporarily rate limited.",
            ),
            (
                TranslationFailureKind::Timeout,
                "DeepL did not answer in time.",
            ),
            (TranslationFailureKind::Unavailable, "DeepL is unavailable."),
            (
                TranslationFailureKind::UnsupportedRequest,
                "DeepL does not support this translation request.",
            ),
            (
                TranslationFailureKind::Rejected,
                "DeepL rejected this translation request.",
            ),
            (
                TranslationFailureKind::EmptyResponse,
                "DeepL returned an empty translation.",
            ),
        ];

        for (kind, expected) in cases {
            let notice = failure_notice(
                UiLocale::English,
                &TranslationFailureReport {
                    kind,
                    provider_id: Some("deepl".to_owned()),
                    retry_after_ms: None,
                    required_catalog_id: None,
                    detail: "technical detail".to_owned(),
                    attempts: Vec::new(),
                },
                &[],
            );
            assert_eq!(notice.headline, expected);
            assert_eq!(notice.message(), expected);
        }
    }

    #[test]
    fn local_and_request_failures_are_not_conflated() {
        let cases = [
            (TranslationFailureKind::NoText, "Enter text to translate."),
            (
                TranslationFailureKind::NoProviderAvailable,
                "No translation provider is available. Enable a local model or a cloud provider in Settings.",
            ),
            (
                TranslationFailureKind::LocalModelMissing,
                "The local translation model is not installed or ready. Choose a model in Settings.",
            ),
            (
                TranslationFailureKind::Internal,
                "Translation failed. Try again or check Providers.",
            ),
        ];

        for (kind, expected) in cases {
            let notice = failure_notice(
                UiLocale::English,
                &TranslationFailureReport {
                    kind,
                    provider_id: None,
                    retry_after_ms: None,
                    required_catalog_id: None,
                    detail: String::new(),
                    attempts: Vec::new(),
                },
                &[],
            );
            assert_eq!(notice.headline, expected);
        }
    }

    #[test]
    fn missing_language_package_names_the_required_package_in_russian() {
        let models = [TranslationModelSummary {
            catalog_id: "firefox-fr-en-v3".to_owned(),
            display_name: "French language pack".to_owned(),
            description: String::new(),
            tier: TranslationTier::Compact,
            languages: vec![TranslationLanguage::French, TranslationLanguage::English],
            size_bytes: 0,
            installed: false,
            selected: false,
            recommended: false,
            license: "MPL-2.0".to_owned(),
        }];
        let notice = failure_notice(
            UiLocale::Russian,
            &TranslationFailureReport {
                kind: TranslationFailureKind::LocalModelMissing,
                provider_id: None,
                retry_after_ms: None,
                required_catalog_id: Some("firefox-fr-en-v3".to_owned()),
                detail: "translation package firefox-fr-en-v3 is not installed".to_owned(),
                attempts: Vec::new(),
            },
            &models,
        );

        assert!(notice.message().contains("Французский ↔ Английский"));
        assert!(!notice.message().contains("firefox-fr-en-v3"));
        assert!(notice.detail.is_empty());
        assert!(
            !notice
                .message()
                .contains("Локальная модель перевода не установлена")
        );
    }

    #[test]
    fn no_provider_is_not_reported_as_an_internal_failure() {
        let notice = failure_notice(
            UiLocale::English,
            &TranslationFailureReport {
                kind: TranslationFailureKind::NoProviderAvailable,
                provider_id: None,
                retry_after_ms: None,
                required_catalog_id: None,
                detail: String::new(),
                attempts: Vec::new(),
            },
            &[],
        );

        assert_eq!(
            notice.headline,
            "No translation provider is available. Enable a local model or a cloud provider in Settings."
        );
        assert_eq!(notice.message(), notice.headline);
    }
}
