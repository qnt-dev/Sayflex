//! Turning one translation into the request a particular service takes.
//!
//! Two services, two shapes. Nothing here talks to the network: a plan is
//! data, so what would be sent can be examined in a test without a key and
//! without a connection.

use serde_json::{Value, json};

use crate::{ProviderEntry, ProviderId};

/// A request built for one provider, and how to read its answer back.
///
/// `Debug` is written by hand and redacts. The derived one printed the
/// Authorization header and the user's own text; nothing logged it, but a
/// struct that spills both the
/// first time somebody adds a `{plan:?}` is a struct waiting to.
#[derive(Clone, PartialEq, Eq)]
pub struct RequestPlan {
    /// Who this is for. The transport does not care, but the reader of the
    /// answer does: the services disagree about what a 403 means, and
    /// telling them apart needs to know which one replied.
    pub provider: ProviderId,
    pub url: String,
    /// Header name and value. Both supported services carry their key here.
    pub headers: Vec<(String, String)>,
    pub body: String,
    pub shape: ResponseShape,
}

impl std::fmt::Debug for RequestPlan {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RequestPlan")
            .field("provider", &self.provider)
            .field("url", &redact_query(&self.url))
            .field("headers", &self.headers.len())
            .field("body_bytes", &self.body.len())
            .field("shape", &self.shape)
            .finish()
    }
}

/// Drops a URL's query string so future query parameters cannot leak through
/// diagnostics by accident.
fn redact_query(url: &str) -> String {
    url.split_once('?')
        .map_or_else(|| url.to_owned(), |(head, _)| format!("{head}?…"))
}

/// Where the translated text sits in each service's answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseShape {
    /// `{"translations":[{"text":"…"}]}`
    DeepL,
    /// `[{"translations":[{"text":"…"}]}]`
    Azure,
}

/// Why a plan could not be built. These are worth saying plainly rather than
/// sending a request that will fail.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlanError {
    #[error("{0} is needed as well as the key")]
    MissingSetting(&'static str),
    #[error("no target language was given")]
    NoTargetLanguage,
    #[error("this build does not know how to talk to {0}")]
    Unsupported(String),
}

/// Builds the request one provider takes for one translation.
///
/// `target` and `source` are the short codes the application uses internally —
/// "en", "ru". Each service wants them in its own case and spelling, which is
/// this function's job and nobody else's.
pub fn plan_request(
    entry: &ProviderEntry,
    secret: &str,
    text: &str,
    source: Option<&str>,
    target: &str,
) -> Result<RequestPlan, PlanError> {
    if target.is_empty() {
        return Err(PlanError::NoTargetLanguage);
    }
    match entry.id.as_str() {
        ProviderId::DEEPL => Ok(plan_deepl(secret, text, source, target)),
        ProviderId::AZURE => plan_azure(
            entry,
            secret,
            text,
            source.map(azure_language),
            azure_language(target),
        ),
        other => Err(PlanError::Unsupported(other.to_owned())),
    }
}

/// DeepL wants upper-case language codes and its own header scheme, and it
/// tells us which host to use through the key itself: a free key ends in `:fx`
/// and is only accepted by the free host. Sending a free key to the paid host
/// answers 403, which would look exactly like a dead key.
fn plan_deepl(secret: &str, text: &str, source: Option<&str>, target: &str) -> RequestPlan {
    let host = if secret.trim_end().ends_with(":fx") {
        "https://api-free.deepl.com"
    } else {
        "https://api.deepl.com"
    };
    let mut body = json!({
        "text": [text],
        "target_lang": deepl_target(target),
    });
    if let Some(source) = source {
        body["source_lang"] = Value::String(deepl_source(source));
    }
    RequestPlan {
        provider: ProviderId::new(ProviderId::DEEPL),
        url: format!("{host}/v2/translate"),
        headers: vec![
            (
                "Authorization".to_owned(),
                format!("DeepL-Auth-Key {secret}"),
            ),
            ("Content-Type".to_owned(), "application/json".to_owned()),
        ],
        body: body.to_string(),
        shape: ResponseShape::DeepL,
    }
}

/// DeepL's name for a language it is asked to translate *into*.
///
/// Upper case, and a regional variant where one exists: plain "EN" is accepted
/// as a target but deprecated in favour of "EN-GB" or "EN-US", so pick one
/// rather than let the service choose.
fn deepl_target(code: &str) -> String {
    match code.to_ascii_lowercase().as_str() {
        "en" => "EN-GB".to_owned(),
        "pt" | "pt-br" => "PT-BR".to_owned(),
        "zh-hans" => "ZH-HANS".to_owned(),
        "zh-hant" => "ZH-HANT".to_owned(),
        other => other.to_ascii_uppercase(),
    }
}

/// DeepL's name for a language it is told the text already *is*.
///
/// The regional variants are targets only — `source_lang=EN-GB` is not a
/// language DeepL knows, and the request is refused. One mapping was doing both
/// jobs, so every English selection was announced as EN-GB and turned down.
fn deepl_source(code: &str) -> String {
    match code.to_ascii_lowercase().as_str() {
        "pt" | "pt-br" => "PT".to_owned(),
        "zh-hans" | "zh-hant" => "ZH".to_owned(),
        other => other.to_ascii_uppercase(),
    }
}

fn azure_language(code: &str) -> &str {
    code
}

/// Azure carries the key and the region in headers, and the languages in the
/// query string. A global resource has no region; regional and multi-service
/// resources carry their region in a header.
fn plan_azure(
    entry: &ProviderEntry,
    secret: &str,
    text: &str,
    source: Option<&str>,
    target: &str,
) -> Result<RequestPlan, PlanError> {
    let region = entry
        .region
        .as_deref()
        .map(str::trim)
        .filter(|region| !region.is_empty());
    let mut url = format!(
        "https://api.cognitive.microsofttranslator.com/translate?api-version=3.0&to={target}"
    );
    if let Some(source) = source {
        url.push_str(&format!("&from={source}"));
    }
    // The region header is sent only when there is a region. A global
    // Translator resource does not have one and is refused if it is sent an
    // empty one; a regional or multi-service resource does and is refused
    // without it. The user's own key decides which, and this carries whichever
    // they gave.
    let mut headers = vec![
        ("Ocp-Apim-Subscription-Key".to_owned(), secret.to_owned()),
        ("Content-Type".to_owned(), "application/json".to_owned()),
    ];
    if let Some(region) = region {
        headers.push(("Ocp-Apim-Subscription-Region".to_owned(), region.to_owned()));
    }
    Ok(RequestPlan {
        provider: ProviderId::new(ProviderId::AZURE),
        url,
        headers,
        body: json!([{ "Text": text }]).to_string(),
        shape: ResponseShape::Azure,
    })
}

/// Reads the translated text out of a successful answer.
///
/// An answer that parses but carries nothing is not an error to pass down the
/// list: the same nothing would come back from everyone.
/// What was in a service's answer.
///
/// Three outcomes, not two. An answer that could not be parsed at all — a proxy
/// login page, a WAF block, a schema the service changed — is not the same as a
/// service saying it has no translation for this text, and treating them alike
/// meant one broken reply from DeepL stopped the walk before Azure or the local
/// engine were asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadAnswer {
    Translated(String),
    /// Parsed, understood, and carrying no text.
    Empty,
    /// Not the shape this service is supposed to answer in.
    Malformed(String),
}

/// Reads a service's answer, saying which of the three it is.
pub fn read_answer(shape: ResponseShape, body: &str) -> ReadAnswer {
    let Ok(value) = serde_json::from_str::<Value>(body) else {
        return ReadAnswer::Malformed("the answer was not JSON".to_owned());
    };
    let found = match shape {
        ResponseShape::DeepL => value
            .get("translations")
            .and_then(|value| value.get(0))
            .and_then(|value| value.get("text")),
        ResponseShape::Azure => value
            .get(0)
            .and_then(|value| value.get("translations"))
            .and_then(|value| value.get(0))
            .and_then(|value| value.get("text")),
    };
    match found.and_then(Value::as_str) {
        None => ReadAnswer::Malformed(
            "the answer did not carry a translation where this service puts one".to_owned(),
        ),
        Some(text) if text.trim().is_empty() => ReadAnswer::Empty,
        Some(text) => ReadAnswer::Translated(text.to_owned()),
    }
}

pub fn read_translation(shape: ResponseShape, body: &str) -> Option<String> {
    let value: Value = serde_json::from_str(body).ok()?;
    let text = match shape {
        ResponseShape::DeepL => value
            .get("translations")?
            .get(0)?
            .get("text")?
            .as_str()?
            .to_owned(),
        ResponseShape::Azure => value
            .get(0)?
            .get("translations")?
            .get(0)?
            .get("text")?
            .as_str()?
            .to_owned(),
    };
    (!text.trim().is_empty()).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::{PlanError, ResponseShape, plan_request, read_translation};
    use crate::{ProviderEntry, ProviderId};

    fn entry(id: &str) -> ProviderEntry {
        ProviderEntry::new(ProviderId::new(id))
    }

    #[test]
    fn deepl_is_told_a_variant_to_translate_into_and_a_plain_language_it_is_reading() {
        // `source_lang=EN-GB` is not a language DeepL knows and the request is
        // refused; `target_lang=EN` is deprecated. One mapping cannot do both.
        let plan = plan_request(
            &entry(ProviderId::DEEPL),
            "key",
            "Проверка",
            Some("ru"),
            "en",
        )
        .unwrap();

        assert!(
            plan.body.contains("\"target_lang\":\"EN-GB\""),
            "{}",
            plan.body
        );
        assert!(
            plan.body.contains("\"source_lang\":\"RU\""),
            "{}",
            plan.body
        );

        let outward = plan_request(
            &entry(ProviderId::DEEPL),
            "key",
            "The quick brown fox",
            Some("en"),
            "ru",
        )
        .unwrap();

        assert!(
            outward.body.contains("\"source_lang\":\"EN\""),
            "a source is named plainly: {}",
            outward.body
        );
    }

    #[test]
    fn a_plan_prints_nothing_it_was_trusted_with() {
        let mut azure = entry(ProviderId::AZURE);
        azure.region = Some("westeurope".to_owned());
        let printed = format!(
            "{:?}",
            plan_request(
                &azure,
                "super-secret-key",
                "confidential sentence",
                None,
                "ru"
            )
            .unwrap()
        );

        assert!(!printed.contains("super-secret-key"), "{printed}");
        assert!(!printed.contains("confidential sentence"), "{printed}");
    }

    #[test]
    fn a_free_deepl_key_goes_to_the_free_host() {
        // A free key sent to the paid host answers 403, which reads exactly
        // like a dead key. The key says which host it belongs to, so nobody has
        // to be asked.
        let plan = plan_request(
            &entry(ProviderId::DEEPL),
            "279a2e9d-83b3-c416-7e2d-f721593e42a0:fx",
            "привет",
            Some("ru"),
            "en",
        )
        .unwrap();

        assert!(plan.url.starts_with("https://api-free.deepl.com/"));
        assert!(plan.body.contains("\"target_lang\":\"EN-GB\""));
        assert!(plan.body.contains("\"source_lang\":\"RU\""));
        assert!(
            plan.headers.iter().any(
                |(name, value)| name == "Authorization" && value.starts_with("DeepL-Auth-Key ")
            )
        );
    }

    #[test]
    fn a_paid_deepl_key_goes_to_the_paid_host() {
        let plan = plan_request(
            &entry(ProviderId::DEEPL),
            "279a2e9d-83b3-c416-7e2d-f721593e42a0",
            "hello",
            None,
            "ru",
        )
        .unwrap();

        assert!(plan.url.starts_with("https://api.deepl.com/"));
        assert!(
            !plan.body.contains("source_lang"),
            "an unknown source is left for the service to detect"
        );
    }

    #[test]
    fn azure_without_a_region_is_addressed_without_the_header() {
        // A global Translator resource has no region and is refused if it is
        // sent an empty one. Demanding a region here refused those keys before
        // they could be tried at all.
        let plan = plan_request(&entry(ProviderId::AZURE), "key", "hello", None, "ru")
            .expect("a global resource is addressed without a region");

        assert!(
            !plan
                .headers
                .iter()
                .any(|(name, _)| name == "Ocp-Apim-Subscription-Region"),
            "no region means no region header, not an empty one"
        );
        assert!(
            plan.headers
                .iter()
                .any(|(name, value)| name == "Ocp-Apim-Subscription-Key" && value == "key")
        );
    }

    #[test]
    fn azure_carries_its_key_and_region_in_headers() {
        let mut azure = entry(ProviderId::AZURE);
        azure.region = Some("  westeurope  ".to_owned());
        let plan = plan_request(&azure, "key", "hello", Some("en"), "ru").unwrap();

        assert!(plan.url.contains("to=ru"));
        assert!(plan.url.contains("from=en"));
        assert!(
            plan.headers.iter().any(
                |(name, value)| name == "Ocp-Apim-Subscription-Region" && value == "westeurope"
            ),
            "the region is trimmed, because a pasted value usually is not"
        );
    }

    #[test]
    fn the_removed_google_adapter_is_not_a_routing_backdoor() {
        let error = plan_request(
            &entry(ProviderId::GOOGLE),
            "legacy-key",
            "hello",
            None,
            "ru",
        )
        .expect_err("a retained settings row must not keep an undeclared transport alive");

        assert_eq!(error, PlanError::Unsupported("google".to_owned()));
    }

    #[test]
    fn each_service_is_read_out_of_its_own_shape() {
        assert_eq!(
            read_translation(
                ResponseShape::DeepL,
                r#"{"translations":[{"text":"привет"}]}"#
            ),
            Some("привет".to_owned())
        );
        assert_eq!(
            read_translation(
                ResponseShape::Azure,
                r#"[{"translations":[{"text":"привет"}]}]"#
            ),
            Some("привет".to_owned())
        );
    }

    #[test]
    fn an_answer_carrying_nothing_is_not_a_translation() {
        assert_eq!(
            read_translation(ResponseShape::DeepL, r#"{"translations":[{"text":"   "}]}"#),
            None
        );
        assert_eq!(
            read_translation(ResponseShape::DeepL, "not json at all"),
            None
        );
    }

    #[test]
    fn a_provider_this_build_does_not_know_is_refused_rather_than_guessed_at() {
        let error = plan_request(&entry("a-service-from-next-year"), "key", "hi", None, "ru")
            .expect_err("guessing at an unknown service's shape would send the key somewhere odd");

        assert!(matches!(error, PlanError::Unsupported(_)));
    }

    #[test]
    fn multilingual_codes_are_mapped_for_each_provider() {
        let plan = plan_request(
            &entry(ProviderId::DEEPL),
            "key",
            "text",
            Some("zh-Hant"),
            "pt-BR",
        )
        .unwrap();
        assert!(plan.body.contains("\"source_lang\":\"ZH\""));
        assert!(plan.body.contains("\"target_lang\":\"PT-BR\""));
    }

    #[test]
    fn newly_supported_deepl_languages_are_not_blocked_by_a_stale_client_list() {
        let plan =
            plan_request(&entry(ProviderId::DEEPL), "key", "परीक्षण", Some("hi"), "en").unwrap();
        assert!(plan.body.contains("\"source_lang\":\"HI\""));
        assert!(plan.body.contains("\"target_lang\":\"EN-GB\""));
    }

    #[test]
    fn every_product_language_has_explicit_deepl_and_azure_wire_names() {
        use rust_protocol::TranslationLanguage;

        let deepl_names = [
            ("en", "EN", "EN-GB"),
            ("ru", "RU", "RU"),
            ("uk", "UK", "UK"),
            ("es", "ES", "ES"),
            ("pt-BR", "PT", "PT-BR"),
            ("de", "DE", "DE"),
            ("fr", "FR", "FR"),
            ("it", "IT", "IT"),
            ("zh-Hans", "ZH", "ZH-HANS"),
            ("zh-Hant", "ZH", "ZH-HANT"),
            ("ja", "JA", "JA"),
            ("ko", "KO", "KO"),
            ("hi", "HI", "HI"),
            ("ar", "AR", "AR"),
        ];
        assert_eq!(TranslationLanguage::ALL.len(), deepl_names.len());

        for (language, (code, source_name, target_name)) in
            TranslationLanguage::ALL.into_iter().zip(deepl_names)
        {
            assert_eq!(language.code(), code);

            let source_plan =
                plan_request(&entry(ProviderId::DEEPL), "key", "text", Some(code), "en").unwrap();
            let source_body: serde_json::Value = serde_json::from_str(&source_plan.body).unwrap();
            assert_eq!(
                source_body["source_lang"], source_name,
                "DeepL source {code}"
            );

            let target_plan =
                plan_request(&entry(ProviderId::DEEPL), "key", "text", None, code).unwrap();
            let target_body: serde_json::Value = serde_json::from_str(&target_plan.body).unwrap();
            assert_eq!(
                target_body["target_lang"], target_name,
                "DeepL target {code}"
            );

            let azure_plan =
                plan_request(&entry(ProviderId::AZURE), "key", "text", Some(code), code).unwrap();
            assert!(
                azure_plan.url.contains(&format!("to={code}")),
                "Azure target {code}"
            );
            assert!(
                azure_plan.url.contains(&format!("from={code}")),
                "Azure source {code}"
            );
        }
    }
}
