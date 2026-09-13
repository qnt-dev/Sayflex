//! Build-configured support destination.
//!
//! A missing or unsafe value produces no UI at all. Opening is kept behind a
//! small injectable boundary so failure handling can be tested without
//! launching the user's browser.

use std::io;

pub(crate) fn validated_support_url(value: Option<&str>) -> Option<&str> {
    let candidate = value?.trim();
    let parsed = url::Url::parse(candidate).ok()?;
    (parsed.scheme() == "https"
        && parsed.host_str().is_some()
        && parsed.username().is_empty()
        && parsed.password().is_none())
    .then_some(candidate)
}

pub(crate) fn configured_support_url() -> Option<String> {
    validated_support_url(option_env!("SAYFLEX_SUPPORT_URL")).map(str::to_owned)
}

pub(crate) fn open_support_with(
    configured: Option<&str>,
    opener: impl FnOnce(&str) -> io::Result<()>,
) -> io::Result<bool> {
    let Some(url) = validated_support_url(configured) else {
        return Ok(false);
    };
    opener(url)?;
    Ok(true)
}

pub(crate) fn open_configured_support(url: &str) -> io::Result<()> {
    open_support_with(Some(url), rust_platform_windows::open_https_url).and_then(|opened| {
        opened
            .then_some(())
            .ok_or_else(|| io::Error::other("support page is not configured"))
    })
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, io};

    use super::{open_support_with, validated_support_url};

    #[test]
    fn support_url_policy_hides_missing_insecure_and_malformed_values() {
        assert_eq!(validated_support_url(None), None);
        assert_eq!(validated_support_url(Some("")), None);
        assert_eq!(
            validated_support_url(Some("http://buymeacoffee.com/sayflex")),
            None
        );
        assert_eq!(validated_support_url(Some("https://")), None);
        assert_eq!(
            validated_support_url(Some("https://user:secret@example.com/support")),
            None
        );
    }

    #[test]
    fn a_plain_https_destination_is_visible_and_opened_verbatim() {
        let opened = Cell::new(false);
        let url = "https://buymeacoffee.com/example";

        assert_eq!(validated_support_url(Some(url)), Some(url));
        assert!(
            open_support_with(Some(url), |actual| {
                assert_eq!(actual, url);
                opened.set(true);
                Ok(())
            })
            .unwrap()
        );
        assert!(opened.get());
    }

    #[test]
    fn shell_failure_is_returned_to_the_ui_owner() {
        let error = open_support_with(Some("https://example.com/support"), |_| {
            Err(io::Error::other("shell refused the target"))
        })
        .unwrap_err();

        assert_eq!(error.to_string(), "shell refused the target");
    }
}
