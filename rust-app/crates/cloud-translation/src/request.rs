//! Turning one translation into the request a particular service takes.
//!
//! Three services, three shapes. Nothing here talks to the network: a plan is
//! data, so what would be sent can be examined in a test without a key and
//! without a connection.

use serde_json::{Value, json};

use crate::{ProviderEntry, ProviderId};

/// A request built for one provider, and how to read its answer back.
///
/// `Debug` is written by hand and redacts. The derived one printed the
/// Authorization header, the key Google carries in its query string, and the
/// user's own text; nothing logged it, but a struct that spills all three the
/// first time somebody adds a `{plan:?}` is a struct waiting to.
#[derive(Clone, PartialEq, Eq)]
pub struct RequestPlan {
    /// Who this is for. The transport does not care, but the reader of the
    /// answer does: the three services disagree about what a 403 means, and
    /// telling them apart needs to know which one replied.
    pub provider: ProviderId,
    pub url: String,
    /// Header name and value. The key travels here for two of the three; for
    /// Google it is in the query string, because that is the only form the v2
    /// endpoint takes.
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

/// Drops a URL's query string, which is where Google's key travels.
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
    /// `{"data":{"translations":[{"translatedText":"…"}]}}`
    Google,
}

/// Why a plan could not be built. All three are the user's to fix, and all
/// three are worth saying plainly rather than sending a request that will fail.
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
        ProviderId::GOOGLE => Ok(plan_google(
            secret,
            text,
            source.map(google_language),
            google_language(target),
        )),
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

fn google_language(code: &str) -> &str {
    match code {
        "pt-BR" => "pt",
        "zh-Hans" => "zh-CN",
        "zh-Hant" => "zh-TW",
        other => other,
    }
}

/// Azure carries the key and the region in headers, and the languages in the
/// query string. Without the region header the service answers 403, which is
/// why the region is required before a request is built at all.
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

/// Google's v2 endpoint takes the key in the query string. That is the only
/// form it accepts — v3 wants a service account, which is a different and much
/// heavier thing to ask a user for.
fn plan_google(secret: &str, text: &str, source: Option<&str>, target: &str) -> RequestPlan {
    let mut body = json!({
        "q": text,
        "target": target,
        // Without this the service treats the text as HTML and escapes
        // punctuation on the way back.
        "format": "text",
    });
    if let Some(source) = source {
        body["source"] = Value::String(source.to_owned());
    }
    RequestPlan {
        provider: ProviderId::new(ProviderId::GOOGLE),
        url: format!(
            "https://translation.googleapis.com/language/translate/v2?key={}",
            urlencode(secret)
        ),
        headers: vec![("Content-Type".to_owned(), "application/json".to_owned())],
        body: body.to_string(),
        shape: ResponseShape::Google,
    }
}

/// Enough escaping for an API key in a query string. Keys are alphanumeric with
/// dashes and underscores, but a key pasted with a stray character should be
/// rejected by the service rather than change the URL's meaning.
fn urlencode(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.' | '~') {
                character.to_string()
            } else {
                format!("%{:02X}", character as u32)
            }
        })
        .collect()
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
/// meant one broken reply from DeepL stopped the walk before Azure, Google or
/// the local engine were asked.
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
        ResponseShape::Google => value
            .get("data")
            .and_then(|value| value.get("translations"))
            .and_then(|value| value.get(0))
            .and_then(|value| value.get("translatedText")),
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
        ResponseShape::Google => value
            .get("data")?
            .get("translations")?
            .get(0)?
            .get("translatedText")?
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

        let google = format!(
            "{:?}",
            plan_request(
                &entry(ProviderId::GOOGLE),
                "another-key",
                "hello",
                None,
                "ru"
            )
            .unwrap()
        );

        assert!(
            !google.contains("another-key"),
            "Google's key rides in the query string: {google}"
        );
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
    fn a_google_key_is_escaped_into_the_query_string() {
        let plan =
            plan_request(&entry(ProviderId::GOOGLE), "abc 123&x", "hello", None, "ru").unwrap();

        assert!(plan.url.contains("key=abc%20123%26x"));
        assert!(
            plan.body.contains("\"format\":\"text\""),
            "without this the answer comes back HTML-escaped"
        );
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
        assert_eq!(
            read_translation(
                ResponseShape::Google,
                r#"{"data":{"translations":[{"translatedText":"привет"}]}}"#
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

        let plan = plan_request(
            &entry(ProviderId::GOOGLE),
            "key",
            "text",
            Some("zh-Hant"),
            "zh-Hans",
        )
        .unwrap();
        assert!(plan.body.contains("\"source\":\"zh-TW\""));
        assert!(plan.body.contains("\"target\":\"zh-CN\""));
    }

    #[test]
    fn newly_supported_deepl_languages_are_not_blocked_by_a_stale_client_list() {
        let plan =
            plan_request(&entry(ProviderId::DEEPL), "key", "परीक्षण", Some("hi"), "en").unwrap();
        assert!(plan.body.contains("\"source_lang\":\"HI\""));
        assert!(plan.body.contains("\"target_lang\":\"EN-GB\""));
    }
}
