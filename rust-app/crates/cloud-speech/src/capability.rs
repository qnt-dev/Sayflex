//! What each service can actually be asked to do.
//!
//! Providers are not interchangeable. Azure's short-audio endpoint refuses
//! anything past a minute and cannot translate; Groq caps an upload at
//! twenty-five megabytes; only the OpenAI-compatible route has a translation
//! endpoint at all. Sending a request a service is known to refuse spends a
//! round trip, sometimes an allowance, and then reports a failure that says
//! nothing about the service being unhealthy.
//!
//! So the walk asks this first, and a provider that cannot serve *this*
//! recording is stepped over rather than tried and blamed.

use crate::provider::ProviderId;

/// The language the audio is expected to be in.
///
/// Kept as a BCP-47 tag because that is the widest form: services that want a
/// bare ISO-639-1 code take the primary subtag, which is a prefix of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanguageHint(String);

impl LanguageHint {
    #[must_use]
    pub fn new(tag: &str) -> Option<Self> {
        let tag = tag.trim();
        (!tag.is_empty()).then(|| Self(tag.replace('_', "-")))
    }

    /// `ru-RU` — what Azure and Deepgram expect.
    #[must_use]
    pub fn tag(&self) -> &str {
        &self.0
    }

    /// `ru` — what the OpenAI-compatible route expects.
    #[must_use]
    pub fn primary(&self) -> &str {
        self.0.split('-').next().unwrap_or(&self.0)
    }
}

/// One recording, described in the terms a provider is chosen by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptionRequest {
    pub audio_duration_ms: u64,
    pub upload_bytes: u64,
    /// The user asked for English out of non-English speech.
    pub translate_to_english: bool,
    pub language: Option<LanguageHint>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    pub max_audio_ms: Option<u64>,
    pub max_upload_bytes: Option<u64>,
    pub translates_to_english: bool,
    /// False when the service needs to be told the language and will not work
    /// it out on its own.
    pub detects_language: bool,
}

impl Capabilities {
    /// Why this provider cannot serve the request, in words for the user.
    #[must_use]
    pub fn refusal(&self, request: &TranscriptionRequest) -> Option<&'static str> {
        if request.translate_to_english && !self.translates_to_english {
            return Some("cannot translate to English");
        }
        if self
            .max_audio_ms
            .is_some_and(|limit| request.audio_duration_ms > limit)
        {
            return Some("recording is longer than this service accepts");
        }
        if self
            .max_upload_bytes
            .is_some_and(|limit| request.upload_bytes > limit)
        {
            return Some("recording is larger than this service accepts");
        }
        if !self.detects_language && request.language.is_none() {
            return Some("needs a language and none is known");
        }
        None
    }
}

/// The limits each supported service is known to have.
///
/// Conservative on purpose: a limit that turns out to be wrong costs one
/// skipped provider, while a missing limit costs a failed request and a
/// misleading diagnosis.
#[must_use]
pub fn capabilities(id: &ProviderId) -> Capabilities {
    match id.as_str() {
        // The worker owns the local engine and knows its own model's limits.
        ProviderId::LOCAL => Capabilities {
            max_audio_ms: None,
            max_upload_bytes: None,
            translates_to_english: true,
            detects_language: true,
        },
        ProviderId::GROQ | ProviderId::CUSTOM => Capabilities {
            max_audio_ms: None,
            // Groq refuses an upload past 25 MB. A custom endpoint is the
            // user's own, and the same shape, so it inherits the assumption.
            max_upload_bytes: Some(25 * 1024 * 1024),
            translates_to_english: true,
            detects_language: true,
        },
        ProviderId::CLOUDFLARE => Capabilities {
            max_audio_ms: None,
            max_upload_bytes: None,
            // The Whisper model offered there transcribes only.
            translates_to_english: false,
            detects_language: true,
        },
        ProviderId::AZURE => Capabilities {
            // The short-audio REST endpoint stops at sixty seconds.
            max_audio_ms: Some(60_000),
            max_upload_bytes: None,
            translates_to_english: false,
            // It will not guess: the request carries the language or fails.
            detects_language: false,
        },
        ProviderId::DEEPGRAM => Capabilities {
            max_audio_ms: None,
            max_upload_bytes: None,
            translates_to_english: false,
            // Detection has to be asked for explicitly, which the adapter does.
            detects_language: true,
        },
        _ => Capabilities {
            max_audio_ms: None,
            max_upload_bytes: None,
            translates_to_english: false,
            detects_language: true,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{LanguageHint, TranscriptionRequest, capabilities};
    use crate::provider::ProviderId;

    fn request() -> TranscriptionRequest {
        TranscriptionRequest {
            audio_duration_ms: 4_000,
            upload_bytes: 128_000,
            translate_to_english: false,
            language: LanguageHint::new("ru-RU"),
        }
    }

    #[test]
    fn a_language_hint_serves_both_shapes_services_ask_for() {
        let hint = LanguageHint::new("ru_RU").unwrap();

        assert_eq!(hint.tag(), "ru-RU");
        assert_eq!(hint.primary(), "ru");
        assert_eq!(LanguageHint::new("  "), None);
    }

    #[test]
    fn an_ordinary_dictation_is_accepted_everywhere() {
        for id in ProviderId::KNOWN {
            assert_eq!(
                capabilities(&ProviderId::new(id)).refusal(&request()),
                None,
                "{id} should take an ordinary clip"
            );
        }
    }

    #[test]
    fn azure_is_stepped_over_for_a_recording_past_its_minute() {
        let long = TranscriptionRequest {
            audio_duration_ms: 61_000,
            ..request()
        };

        assert!(
            capabilities(&ProviderId::new(ProviderId::AZURE))
                .refusal(&long)
                .is_some()
        );
        assert_eq!(
            capabilities(&ProviderId::new(ProviderId::GROQ)).refusal(&long),
            None,
            "a minute is nothing to the others"
        );
    }

    #[test]
    fn only_the_openai_route_is_offered_a_translation() {
        let translating = TranscriptionRequest {
            translate_to_english: true,
            ..request()
        };

        assert_eq!(
            capabilities(&ProviderId::new(ProviderId::GROQ)).refusal(&translating),
            None
        );
        for id in [
            ProviderId::AZURE,
            ProviderId::CLOUDFLARE,
            ProviderId::DEEPGRAM,
        ] {
            assert!(
                capabilities(&ProviderId::new(id))
                    .refusal(&translating)
                    .is_some(),
                "{id} cannot translate"
            );
        }
    }

    #[test]
    fn azure_is_stepped_over_when_the_language_is_unknown() {
        let unknown = TranscriptionRequest {
            language: None,
            ..request()
        };

        assert!(
            capabilities(&ProviderId::new(ProviderId::AZURE))
                .refusal(&unknown)
                .is_some()
        );
        assert_eq!(
            capabilities(&ProviderId::new(ProviderId::DEEPGRAM)).refusal(&unknown),
            None,
            "detection is asked for explicitly instead"
        );
    }

    #[test]
    fn an_upload_past_the_openai_limit_is_stepped_over() {
        let large = TranscriptionRequest {
            upload_bytes: 26 * 1024 * 1024,
            ..request()
        };

        assert!(
            capabilities(&ProviderId::new(ProviderId::GROQ))
                .refusal(&large)
                .is_some()
        );
        assert_eq!(
            capabilities(&ProviderId::new(ProviderId::CLOUDFLARE)).refusal(&large),
            None
        );
    }
}
