//! What to send to each service, and how to read what comes back.
//!
//! Building the request and reading the answer are separated from performing
//! it. Everything specific to a service — its address, how it carries the key,
//! whether it wants the audio raw or in a form field, where the text sits in
//! the reply — lives here as plain data and is covered by tests. The part that
//! actually opens a socket knows none of it.

use crate::{
    failure::{ProviderFailure, classify_response},
    provider::{ProviderEntry, ProviderId},
};

/// A body to upload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestBody {
    /// The audio bytes on their own, with a content type.
    Raw {
        content_type: &'static str,
        bytes: Vec<u8>,
    },
    /// A multipart form: the audio as a file part plus text fields.
    Form {
        file_name: &'static str,
        file_content_type: &'static str,
        audio: Vec<u8>,
        fields: Vec<(String, String)>,
    },
}

/// Everything needed to perform one transcription request.
///
/// `Debug` is written by hand so a plan can be logged while investigating a
/// failure without printing the key it carries.
#[derive(Clone, PartialEq, Eq)]
pub struct RequestPlan {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: RequestBody,
}

impl std::fmt::Debug for RequestPlan {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let headers: Vec<_> = self
            .headers
            .iter()
            .map(|(name, value)| {
                if is_secret_header(name) {
                    (name.as_str(), "<redacted>")
                } else {
                    (name.as_str(), value.as_str())
                }
            })
            .collect();
        formatter
            .debug_struct("RequestPlan")
            .field("url", &self.url)
            .field("headers", &headers)
            .field(
                "body",
                &match &self.body {
                    RequestBody::Raw { bytes, .. } => format!("{} raw bytes", bytes.len()),
                    RequestBody::Form { audio, .. } => format!("{} bytes as a form", audio.len()),
                },
            )
            .finish()
    }
}

fn is_secret_header(name: &str) -> bool {
    name.eq_ignore_ascii_case("authorization")
        || name.eq_ignore_ascii_case("ocp-apim-subscription-key")
}

/// Why a request could not even be built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanError {
    /// The provider needs something the user has not filled in, such as a
    /// Cloudflare account identifier or a custom endpoint.
    MissingSetting(&'static str),
    /// This build has no adapter for that identifier.
    Unsupported,
}

const OPENAI_DEFAULT_MODEL: &str = "whisper-large-v3-turbo";
const CLOUDFLARE_DEFAULT_MODEL: &str = "@cf/openai/whisper-large-v3-turbo";
const DEEPGRAM_DEFAULT_MODEL: &str = "nova-3";

/// Builds the request for one provider.
///
/// `language` is a hint, not a demand: every service here detects the language
/// on its own, and forcing the wrong one is worse than letting it decide.
pub fn plan_request(
    entry: &ProviderEntry,
    secret: &str,
    wav: Vec<u8>,
    language: Option<&str>,
) -> Result<RequestPlan, PlanError> {
    match entry.id.as_str() {
        ProviderId::GROQ => Ok(openai_compatible(
            entry
                .endpoint
                .as_deref()
                .unwrap_or("https://api.groq.com/openai/v1"),
            entry.model.as_deref().unwrap_or(OPENAI_DEFAULT_MODEL),
            secret,
            wav,
            language,
        )),
        ProviderId::CUSTOM => {
            let endpoint = entry
                .endpoint
                .as_deref()
                .filter(|endpoint| !endpoint.trim().is_empty())
                .ok_or(PlanError::MissingSetting("endpoint"))?;
            Ok(openai_compatible(
                endpoint,
                entry.model.as_deref().unwrap_or(OPENAI_DEFAULT_MODEL),
                secret,
                wav,
                language,
            ))
        }
        ProviderId::CLOUDFLARE => {
            let account = entry
                .account
                .as_deref()
                .filter(|account| !account.trim().is_empty())
                .ok_or(PlanError::MissingSetting("account"))?;
            let model = entry.model.as_deref().unwrap_or(CLOUDFLARE_DEFAULT_MODEL);
            Ok(RequestPlan {
                url: format!(
                    "https://api.cloudflare.com/client/v4/accounts/{account}/ai/run/{model}"
                ),
                headers: vec![
                    ("Authorization".to_owned(), format!("Bearer {secret}")),
                    ("Content-Type".to_owned(), "audio/wav".to_owned()),
                ],
                body: RequestBody::Raw {
                    content_type: "audio/wav",
                    bytes: wav,
                },
            })
        }
        ProviderId::AZURE => {
            let region = entry
                .account
                .as_deref()
                .filter(|region| !region.trim().is_empty())
                .ok_or(PlanError::MissingSetting("region"))?;
            let mut url = format!(
                "https://{region}.stt.speech.microsoft.com/speech/recognition/conversation/cognitiveservices/v1"
            );
            // Azure is the one service that will not detect the language for
            // us on this endpoint, so the hint becomes a parameter.
            url.push_str(&format!("?language={}", language.unwrap_or("en-US")));
            Ok(RequestPlan {
                url,
                headers: vec![
                    ("Ocp-Apim-Subscription-Key".to_owned(), secret.to_owned()),
                    (
                        "Content-Type".to_owned(),
                        "audio/wav; codecs=audio/pcm; samplerate=16000".to_owned(),
                    ),
                    ("Accept".to_owned(), "application/json".to_owned()),
                ],
                body: RequestBody::Raw {
                    content_type: "audio/wav",
                    bytes: wav,
                },
            })
        }
        ProviderId::DEEPGRAM => {
            let model = entry.model.as_deref().unwrap_or(DEEPGRAM_DEFAULT_MODEL);
            let mut url =
                format!("https://api.deepgram.com/v1/listen?model={model}&smart_format=true");
            if let Some(language) = language {
                url.push_str(&format!("&language={language}"));
            }
            Ok(RequestPlan {
                url,
                headers: vec![
                    ("Authorization".to_owned(), format!("Token {secret}")),
                    ("Content-Type".to_owned(), "audio/wav".to_owned()),
                ],
                body: RequestBody::Raw {
                    content_type: "audio/wav",
                    bytes: wav,
                },
            })
        }
        _ => Err(PlanError::Unsupported),
    }
}

fn openai_compatible(
    endpoint: &str,
    model: &str,
    secret: &str,
    wav: Vec<u8>,
    language: Option<&str>,
) -> RequestPlan {
    let mut fields = vec![
        ("model".to_owned(), model.to_owned()),
        ("response_format".to_owned(), "json".to_owned()),
    ];
    if let Some(language) = language {
        fields.push(("language".to_owned(), language.to_owned()));
    }
    RequestPlan {
        url: format!("{}/audio/transcriptions", endpoint.trim_end_matches('/')),
        headers: vec![("Authorization".to_owned(), format!("Bearer {secret}"))],
        body: RequestBody::Form {
            file_name: "dictation.wav",
            file_content_type: "audio/wav",
            audio: wav,
            fields,
        },
    }
}

/// Reads the transcript out of a successful reply, or explains the failure.
///
/// A reply that arrived with a success status but carries no text is treated
/// as a rejection rather than as an empty dictation: silently inserting
/// nothing is indistinguishable, to the user, from the application losing
/// their words.
pub fn read_transcript(
    id: &ProviderId,
    status: u16,
    retry_after: Option<std::time::Duration>,
    body: &str,
) -> Result<String, ProviderFailure> {
    if !(200..300).contains(&status) {
        return Err(classify_response(status, retry_after, body));
    }
    let document: serde_json::Value = serde_json::from_str(body)
        .map_err(|error| ProviderFailure::Rejected(format!("the reply was not JSON: {error}")))?;

    let text = match id.as_str() {
        ProviderId::CLOUDFLARE => {
            // Cloudflare answers 200 with success:false for its own errors.
            if document["success"] == serde_json::Value::Bool(false) {
                let detail = document["errors"].to_string();
                return Err(cloudflare_failure(&detail));
            }
            document["result"]["text"].as_str()
        }
        ProviderId::AZURE => {
            let status_text = document["RecognitionStatus"].as_str().unwrap_or_default();
            match status_text {
                "Success" => document["DisplayText"].as_str(),
                // Azure reports "nothing was said" as a normal result.
                "NoMatch" | "InitialSilenceTimeout" | "BabbleTimeout" => Some(""),
                other => {
                    return Err(ProviderFailure::Rejected(format!(
                        "recognition status {other}"
                    )));
                }
            }
        }
        ProviderId::DEEPGRAM => {
            document["results"]["channels"][0]["alternatives"][0]["transcript"].as_str()
        }
        // Groq and anything OpenAI-compatible.
        _ => document["text"].as_str(),
    };

    text.map(str::to_owned)
        .ok_or_else(|| ProviderFailure::Rejected("the reply carried no transcript".to_owned()))
}

/// Cloudflare returns its quota and authentication problems inside a 200, so
/// the code that would normally decide has to read the message instead.
fn cloudflare_failure(detail: &str) -> ProviderFailure {
    let lowercase = detail.to_ascii_lowercase();
    if lowercase.contains("authentication") || lowercase.contains("unauthorized") {
        ProviderFailure::Credential(detail.to_owned())
    } else if lowercase.contains("neuron")
        || lowercase.contains("limit")
        || lowercase.contains("quota")
    {
        ProviderFailure::QuotaExhausted { retry_after: None }
    } else {
        ProviderFailure::Rejected(detail.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{PlanError, RequestBody, plan_request, read_transcript};
    use crate::{
        failure::ProviderFailure,
        provider::{ProviderEntry, ProviderId},
    };

    fn entry(id: &str) -> ProviderEntry {
        ProviderEntry::new(id, true)
    }

    #[test]
    fn groq_uploads_a_form_to_the_openai_compatible_route() {
        let plan = plan_request(&entry(ProviderId::GROQ), "gsk_key", vec![1, 2, 3], None).unwrap();

        assert_eq!(
            plan.url,
            "https://api.groq.com/openai/v1/audio/transcriptions"
        );
        assert_eq!(
            plan.headers,
            [("Authorization".to_owned(), "Bearer gsk_key".to_owned())]
        );
        let RequestBody::Form { fields, audio, .. } = plan.body else {
            panic!("the OpenAI route takes a form");
        };
        assert_eq!(audio, [1, 2, 3]);
        assert!(fields.contains(&("model".to_owned(), "whisper-large-v3-turbo".to_owned())));
    }

    #[test]
    fn a_custom_endpoint_is_required_and_then_used_verbatim() {
        assert_eq!(
            plan_request(&entry(ProviderId::CUSTOM), "key", vec![], None),
            Err(PlanError::MissingSetting("endpoint"))
        );

        let mut custom = entry(ProviderId::CUSTOM);
        custom.endpoint = Some("https://speech.example.test/v1/".to_owned());
        custom.model = Some("my-model".to_owned());
        let plan = plan_request(&custom, "key", vec![], None).unwrap();

        assert_eq!(
            plan.url,
            "https://speech.example.test/v1/audio/transcriptions"
        );
        let RequestBody::Form { fields, .. } = plan.body else {
            panic!("a custom endpoint speaks the OpenAI dialect");
        };
        assert!(fields.contains(&("model".to_owned(), "my-model".to_owned())));
    }

    #[test]
    fn cloudflare_needs_an_account_and_posts_the_audio_raw() {
        assert_eq!(
            plan_request(&entry(ProviderId::CLOUDFLARE), "token", vec![], None),
            Err(PlanError::MissingSetting("account"))
        );

        let mut cloudflare = entry(ProviderId::CLOUDFLARE);
        cloudflare.account = Some("abc123".to_owned());
        let plan = plan_request(&cloudflare, "token", vec![9], None).unwrap();

        assert_eq!(
            plan.url,
            "https://api.cloudflare.com/client/v4/accounts/abc123/ai/run/@cf/openai/whisper-large-v3-turbo"
        );
        assert!(matches!(plan.body, RequestBody::Raw { .. }));
    }

    #[test]
    fn azure_carries_its_key_in_its_own_header_and_takes_the_language() {
        let mut azure = entry(ProviderId::AZURE);
        azure.account = Some("westeurope".to_owned());

        let plan = plan_request(&azure, "subscription", vec![], Some("ru-RU")).unwrap();

        assert!(
            plan.url
                .starts_with("https://westeurope.stt.speech.microsoft.com/")
        );
        assert!(plan.url.ends_with("?language=ru-RU"));
        assert!(plan.headers.iter().any(|(name, value)| name
            == "Ocp-Apim-Subscription-Key"
            && value == "subscription"));
    }

    #[test]
    fn deepgram_uses_its_token_scheme() {
        let plan = plan_request(&entry(ProviderId::DEEPGRAM), "dg_key", vec![], None).unwrap();

        assert!(
            plan.url
                .starts_with("https://api.deepgram.com/v1/listen?model=nova-3")
        );
        assert_eq!(
            plan.headers[0],
            ("Authorization".to_owned(), "Token dg_key".to_owned())
        );
    }

    #[test]
    fn the_local_engine_and_unknown_services_have_no_request() {
        assert_eq!(
            plan_request(&entry(ProviderId::LOCAL), "", vec![], None),
            Err(PlanError::Unsupported)
        );
        assert_eq!(
            plan_request(&entry("something-else"), "", vec![], None),
            Err(PlanError::Unsupported)
        );
    }

    #[test]
    fn a_plan_never_prints_the_key_it_carries() {
        let plan = plan_request(&entry(ProviderId::GROQ), "gsk_secret", vec![], None).unwrap();

        let printed = format!("{plan:?}");

        assert!(!printed.contains("gsk_secret"));
        assert!(printed.contains("<redacted>"));
    }

    #[test]
    fn each_service_has_its_transcript_read_from_its_own_shape() {
        let groq = read_transcript(
            &ProviderId::new(ProviderId::GROQ),
            200,
            None,
            r#"{"text":"hello there"}"#,
        );
        let cloudflare = read_transcript(
            &ProviderId::new(ProviderId::CLOUDFLARE),
            200,
            None,
            r#"{"success":true,"result":{"text":"hello there"}}"#,
        );
        let azure = read_transcript(
            &ProviderId::new(ProviderId::AZURE),
            200,
            None,
            r#"{"RecognitionStatus":"Success","DisplayText":"hello there"}"#,
        );
        let deepgram = read_transcript(
            &ProviderId::new(ProviderId::DEEPGRAM),
            200,
            None,
            r#"{"results":{"channels":[{"alternatives":[{"transcript":"hello there"}]}]}}"#,
        );

        for result in [groq, cloudflare, azure, deepgram] {
            assert_eq!(result.as_deref(), Ok("hello there"));
        }
    }

    #[test]
    fn azure_reports_silence_as_an_empty_transcript_not_a_failure() {
        let result = read_transcript(
            &ProviderId::new(ProviderId::AZURE),
            200,
            None,
            r#"{"RecognitionStatus":"NoMatch"}"#,
        );

        assert_eq!(result.as_deref(), Ok(""));
    }

    #[test]
    fn cloudflare_failures_hidden_inside_a_success_status_are_still_read() {
        let spent = read_transcript(
            &ProviderId::new(ProviderId::CLOUDFLARE),
            200,
            None,
            r#"{"success":false,"errors":[{"message":"You have exceeded your daily Neuron limit"}]}"#,
        );
        let refused = read_transcript(
            &ProviderId::new(ProviderId::CLOUDFLARE),
            200,
            None,
            r#"{"success":false,"errors":[{"message":"Authentication error"}]}"#,
        );

        assert_eq!(
            spent,
            Err(ProviderFailure::QuotaExhausted { retry_after: None })
        );
        assert!(matches!(refused, Err(ProviderFailure::Credential(_))));
    }

    #[test]
    fn an_error_status_is_classified_before_the_body_is_parsed() {
        let result = read_transcript(
            &ProviderId::new(ProviderId::GROQ),
            429,
            Some(Duration::from_secs(2)),
            "slow down",
        );

        assert_eq!(
            result,
            Err(ProviderFailure::RateLimited {
                retry_after: Duration::from_secs(2)
            })
        );
    }

    #[test]
    fn a_success_without_a_transcript_is_not_treated_as_an_empty_dictation() {
        let result = read_transcript(
            &ProviderId::new(ProviderId::GROQ),
            200,
            None,
            r#"{"unexpected":true}"#,
        );

        assert!(matches!(result, Err(ProviderFailure::Rejected(_))));
    }
}
