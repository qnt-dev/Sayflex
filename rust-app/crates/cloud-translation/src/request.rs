//! Turning one translation into the request a particular service takes.
//!
//! Three services, three shapes. Nothing here talks to the network: a plan is
//! data, so what would be sent can be examined in a test without a key and
//! without a connection.

use serde_json::{Value, json};

use crate::{ProviderEntry, ProviderId};

/// A request built for one provider, and how to read its answer back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestPlan {
    pub url: String,
    /// Header name and value. The key travels here for two of the three; for
    /// Google it is in the query string, because that is the only form the v2
    /// endpoint takes.
    pub headers: Vec<(String, String)>,
    pub body: String,
    pub shape: ResponseShape,
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
        ProviderId::AZURE => plan_azure(entry, secret, text, source, target),
        ProviderId::GOOGLE => Ok(plan_google(secret, text, source, target)),
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
        "target_lang": deepl_language(target),
    });
    if let Some(source) = source {
        body["source_lang"] = Value::String(deepl_language(source));
    }
    RequestPlan {
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

/// DeepL takes upper case, and distinguishes British from American English on
/// the way out. "EN" is accepted as a target but deprecated in favour of a
/// variant, so pick one rather than let the service choose.
fn deepl_language(code: &str) -> String {
    match code.to_ascii_lowercase().as_str() {
        "en" => "EN-GB".to_owned(),
        "pt" => "PT-PT".to_owned(),
        other => other.to_ascii_uppercase(),
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
        .filter(|region| !region.is_empty())
        .ok_or(PlanError::MissingSetting("Region"))?;
    let mut url = format!(
        "https://api.cognitive.microsofttranslator.com/translate?api-version=3.0&to={target}"
    );
    if let Some(source) = source {
        url.push_str(&format!("&from={source}"));
    }
    Ok(RequestPlan {
        url,
        headers: vec![
            (
                "Ocp-Apim-Subscription-Key".to_owned(),
                secret.to_owned(),
            ),
            (
                "Ocp-Apim-Subscription-Region".to_owned(),
                region.to_owned(),
            ),
            ("Content-Type".to_owned(), "application/json".to_owned()),
        ],
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
            plan.headers
                .iter()
                .any(|(name, value)| name == "Authorization" && value.starts_with("DeepL-Auth-Key "))
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
    fn azure_refuses_to_be_addressed_without_a_region() {
        let error = plan_request(&entry(ProviderId::AZURE), "key", "hello", None, "ru")
            .expect_err("a request without a region would answer 403");

        assert_eq!(error, PlanError::MissingSetting("Region"));
    }

    #[test]
    fn azure_carries_its_key_and_region_in_headers() {
        let mut azure = entry(ProviderId::AZURE);
        azure.region = Some("  westeurope  ".to_owned());
        let plan = plan_request(&azure, "key", "hello", Some("en"), "ru").unwrap();

        assert!(plan.url.contains("to=ru"));
        assert!(plan.url.contains("from=en"));
        assert!(
            plan.headers.iter().any(|(name, value)| name
                == "Ocp-Apim-Subscription-Region"
                && value == "westeurope"),
            "the region is trimmed, because a pasted value usually is not"
        );
    }

    #[test]
    fn a_google_key_is_escaped_into_the_query_string() {
        let plan = plan_request(&entry(ProviderId::GOOGLE), "abc 123&x", "hello", None, "ru")
            .unwrap();

        assert!(plan.url.contains("key=abc%20123%26x"));
        assert!(
            plan.body.contains("\"format\":\"text\""),
            "without this the answer comes back HTML-escaped"
        );
    }

    #[test]
    fn each_service_is_read_out_of_its_own_shape() {
        assert_eq!(
            read_translation(ResponseShape::DeepL, r#"{"translations":[{"text":"привет"}]}"#),
            Some("привет".to_owned())
        );
        assert_eq!(
            read_translation(ResponseShape::Azure, r#"[{"translations":[{"text":"привет"}]}]"#),
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
        assert_eq!(read_translation(ResponseShape::DeepL, "not json at all"), None);
    }

    #[test]
    fn a_provider_this_build_does_not_know_is_refused_rather_than_guessed_at() {
        let error = plan_request(&entry("a-service-from-next-year"), "key", "hi", None, "ru")
            .expect_err("guessing at an unknown service's shape would send the key somewhere odd");

        assert!(matches!(error, PlanError::Unsupported(_)));
    }
}
