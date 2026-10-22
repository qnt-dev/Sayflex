//! Shared, allocation-free rules for recognizing protected text values.

/// A bounded screen block containing a number but no word needs no language
/// translation. Keep the original bytes, including signs, spacing and IDs.
#[must_use]
pub fn is_numeric_only_text(text: &str) -> bool {
    if text.len() > rust_protocol::MAX_SCREEN_BLOCK_TEXT_BYTES {
        return false;
    }
    let mut has_digit = false;
    for character in text.chars() {
        if character.is_alphabetic() {
            return false;
        }
        has_digit |= character.is_ascii_digit();
    }
    has_digit
}

/// Whether an OCR comma separates a named-month date's day and year, as in
/// `February 10,2026`, rather than joining one numeric value. Only complete
/// English month names, days 1–31, and four-digit years with a safe boundary
/// match. The caller must still compare the day/year as counted exact tokens.
/// Indices are UTF-8 byte offsets; invalid offsets or a non-comma return false.
#[must_use]
pub fn named_month_date_comma(text: &str, number_start: usize, comma: usize) -> bool {
    const MONTHS: [&str; 12] = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    let (Some(day), Some(prefix)) = (text.get(number_start..comma), text.get(..number_start))
    else {
        return false;
    };
    if text.as_bytes().get(comma) != Some(&b',')
        || !(1..=2).contains(&day.len())
        || !day.bytes().all(|byte| byte.is_ascii_digit())
        || !day.parse::<u8>().is_ok_and(|day| (1..=31).contains(&day))
        || !prefix.chars().next_back().is_some_and(char::is_whitespace)
    {
        return false;
    }
    let Some(month) = prefix.split_whitespace().next_back() else {
        return false;
    };
    if !MONTHS
        .iter()
        .any(|candidate| month.eq_ignore_ascii_case(candidate))
    {
        return false;
    }
    let Some(year_and_tail) = comma.checked_add(1).and_then(|start| text.get(start..)) else {
        return false;
    };
    let Some(year) = year_and_tail.get(..4) else {
        return false;
    };
    if !year.bytes().all(|byte| byte.is_ascii_digit()) {
        return false;
    }
    let mut tail = year_and_tail[4..].chars();
    match tail.next() {
        None => true,
        Some(character) if character.is_whitespace() => true,
        Some('.' | ',') => tail.next().is_none_or(|next| !next.is_alphanumeric()),
        Some(character) => {
            !character.is_alphanumeric()
                && !matches!(
                    character,
                    '+' | '-' | '−' | '﹣' | '－' | '﹢' | '＋' | '±' | '∓'
                )
                && !matches!(character, '_' | ':' | '/' | '%')
        }
    }
}

#[cfg(test)]
mod tests {
    use super::named_month_date_comma;

    #[test]
    fn numeric_passthrough_requires_bounded_digits_without_any_language_words() {
        for text in [
            "198-5130", "002-03", "35%", "+25", "−25", "1.234,56", " 30 30 ",
        ] {
            assert!(super::is_numeric_only_text(text), "{text}");
        }
        for text in [
            "",
            " \t",
            "---",
            "Call 198-5130",
            "Урон 25",
            "伤害25",
            "id002-03",
            "25s",
        ] {
            assert!(!super::is_numeric_only_text(text), "{text}");
        }
        assert!(!super::is_numeric_only_text(
            &"1".repeat(rust_protocol::MAX_SCREEN_BLOCK_TEXT_BYTES + 1)
        ));
    }

    #[test]
    fn named_month_date_comma_requires_date_context_and_safe_year_boundary() {
        for text in [
            "February 10,2026",
            "February 10,2026.",
            "February 10,2026; next",
        ] {
            assert!(named_month_date_comma(text, 9, 11), "{text}");
        }
        for text in [
            "February 10,2026%",
            "February 10,2026s",
            "February 10,2026/1",
            "February 10,2026.1",
            "February 10,2026+1",
            "February 10,2026−1",
            "February 10,2026_1",
            "February 10,20261",
        ] {
            assert!(!named_month_date_comma(text, 9, 11), "{text}");
        }
        assert!(!named_month_date_comma("Cost 10,2026", 5, 7));
    }

    #[test]
    fn named_month_date_comma_rejects_invalid_byte_offsets_without_panicking() {
        let text = "February 10,2026";
        for (start, comma) in [(12, 11), (9, 12), (usize::MAX, 11), (9, usize::MAX)] {
            assert!(!named_month_date_comma(text, start, comma));
        }
        assert!(!named_month_date_comma("Февраль 10,2026", 1, 16));
    }
}
