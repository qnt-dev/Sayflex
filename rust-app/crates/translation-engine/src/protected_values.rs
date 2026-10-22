#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ProtectedValue {
    Exact(String),
    Decimal { raw: String, canonical: String },
}

impl ProtectedValue {
    fn raw(&self) -> &str {
        match self {
            Self::Exact(raw) | Self::Decimal { raw, .. } => raw,
        }
    }
}

/// Compares protected values without rewriting either text. Exact spellings are
/// matched first; only unambiguous decimal punctuation can compare equivalently.
pub fn protected_values_match(source: &str, translated: &str) -> bool {
    let mut expected = protected_values(source);
    let mut actual = protected_values(translated);
    // Consume exact spellings first, including unchanged values next to units
    // whose spacing changed. Sorting puts protected IDs before flexible decimals.
    expected.retain(|value| {
        if let Some(index) = actual
            .iter()
            .position(|candidate| value.raw() == candidate.raw())
        {
            actual.remove(index);
            false
        } else {
            true
        }
    });
    let decimals = |values: Vec<ProtectedValue>| {
        let mut values = values
            .into_iter()
            .map(|value| match value {
                ProtectedValue::Decimal { canonical, .. } => Some(canonical),
                ProtectedValue::Exact(_) => None,
            })
            .collect::<Option<Vec<_>>>()?;
        values.sort();
        Some(values)
    };
    match (decimals(expected), decimals(actual)) {
        (Some(expected), Some(actual)) => expected == actual,
        _ => false,
    }
}

fn protected_number(text: &str, start: usize, end: usize) -> ProtectedValue {
    let value = &text[start..end];
    let exact = || ProtectedValue::Exact(value.to_owned());
    let attached_prefix = text[..start].chars().next_back().is_some_and(|character| {
        character.is_alphanumeric()
            || is_numeric_sign(character)
            || matches!(character, '_' | '.' | ',' | ':' | '/')
    });
    let mut suffix = text[end..].chars();
    let attached_suffix = suffix.next().is_some_and(|character| {
        character.is_alphanumeric()
            || is_numeric_sign(character)
            || matches!(character, '_' | ':' | '/')
            || (character == '.' && suffix.next().is_some_and(char::is_alphanumeric))
    });
    let attached_seconds = !value.ends_with('%') && is_complete_seconds_suffix(&text[end..]);
    if attached_prefix || (attached_suffix && !attached_seconds) {
        return exact();
    }
    let number = value.strip_suffix('%').unwrap_or(value);
    let unsigned = number.strip_prefix(is_numeric_sign).unwrap_or(number);
    let Some((integer, fraction)) = unsigned.split_once(['.', ',']) else {
        return exact();
    };
    // A single separator with one or two fractional digits is a decimal, not
    // a thousands group. Keep grouped values, multi-part dates and IDs exact.
    // No arithmetic conversion: signs, every digit, percent and count survive.
    if integer.is_empty()
        || !integer.bytes().all(|byte| byte.is_ascii_digit())
        || !(1..=2).contains(&fraction.len())
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
    {
        return exact();
    }
    ProtectedValue::Decimal {
        raw: value.to_owned(),
        canonical: value.replace(',', "."),
    }
}

fn is_numeric_sign(character: char) -> bool {
    matches!(
        character,
        '+' | '-' | '−' | '﹣' | '－' | '﹢' | '＋' | '±' | '∓'
    )
}

fn is_complete_seconds_suffix(suffix: &str) -> bool {
    let Some(remainder) = suffix
        .strip_prefix('s')
        .or_else(|| suffix.strip_prefix('с'))
    else {
        return false;
    };
    // Only the complete seconds abbreviation is a unit. Do not reinterpret
    // serials, identifiers, paths, or other arbitrary alphanumeric suffixes.
    let mut boundary = remainder.chars();
    match boundary.next() {
        None => true,
        Some(character) if character.is_whitespace() => true,
        Some('.') => boundary.next().is_none_or(|character| {
            character.is_whitespace() || matches!(character, ')' | ']' | '}' | '"' | '\'')
        }),
        Some(character) => matches!(character, ',' | ';' | '!' | '?' | ')' | ']' | '}'),
    }
}

pub(crate) fn protected_values(text: &str) -> Vec<ProtectedValue> {
    let mut values = Vec::new();
    let characters = text.char_indices().collect::<Vec<_>>();
    let mut index = 0;
    while index < characters.len() {
        let (byte, character) = characters[index];
        let signed_number = is_numeric_sign(character)
            && characters
                .get(index + 1)
                .is_some_and(|(_, next)| next.is_ascii_digit());
        if character.is_ascii_digit() || signed_number {
            let mut end = byte + character.len_utf8();
            index += 1;
            while index < characters.len() {
                let (next_byte, next) = characters[index];
                let numeric_separator = matches!(next, '.' | ',' | ':' | '-' | '/')
                    && characters
                        .get(index + 1)
                        .is_some_and(|(_, following)| following.is_ascii_digit())
                    && !(next == ','
                        && rust_app_core::text_values::named_month_date_comma(
                            text, byte, next_byte,
                        ));
                if next.is_ascii_digit() || numeric_separator || next == '%' {
                    end = next_byte + next.len_utf8();
                    index += 1;
                    if next == '%' {
                        break;
                    }
                } else {
                    break;
                }
            }
            values.push(protected_number(text, byte, end));
            continue;
        }
        if character == '{'
            && let Some(relative_end) = text[byte..].find('}')
        {
            let end = byte + relative_end + 1;
            values.push(ProtectedValue::Exact(text[byte..end].to_owned()));
            while index < characters.len() && characters[index].0 < end {
                index += 1;
            }
            continue;
        }
        if matches!(character, '™' | '©' | '®') {
            values.push(ProtectedValue::Exact(character.to_string()));
        }
        index += 1;
    }
    for token in text.split_whitespace() {
        let token = token.trim_matches(|character: char| {
            matches!(
                character,
                '(' | ')' | '[' | ']' | '<' | '>' | ',' | ';' | '!' | '?'
            )
        });
        if token.starts_with("https://") || token.starts_with("http://") {
            values.push(ProtectedValue::Exact(token.to_owned()));
        }
    }
    values.sort();
    values
}

#[cfg(test)]
mod tests {
    use super::protected_values_match;

    #[test]
    fn named_month_date_keeps_day_and_year_when_ocr_omits_comma_space() {
        assert!(protected_values_match(
            "February 10,2026. Collect within 30 days.",
            "10 февраля 2026. Заберите в течение 30 дней."
        ));
        for month in [
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
        ] {
            let compact = format!("Date: {month} 10,2026");
            let spaced = format!("Date: {month} 10, 2026");
            assert!(protected_values_match(&compact, &spaced), "{month}");
            assert!(protected_values_match(&spaced, &compact), "{month}");
        }
    }

    #[test]
    fn named_month_date_does_not_relax_numbers_identifiers_or_token_counts() {
        for translated in [
            "11 февраля 2026, срок 30 дней",
            "10 февраля 2027, срок 30 дней",
            "10 февраля 2026, срок 31 день",
            "10 февраля 2026, срок дней",
            "10 февраля 2026, срок 30 и 30 дней",
        ] {
            assert!(!protected_values_match(
                "February 10,2026, within 30 days",
                translated
            ));
        }
        for exact in [
            "10,2026",
            "Cost 10,2026",
            "February 1,234",
            "February 10,2026%",
            "February 10,2026s",
            "February 10,2026.1",
            "February 10,2026/1",
            "February 10,2026,1",
            "February -10,2026",
            "February 0,2026",
            "February 32,2026",
            "February10,2026",
            "id-February 10,2026",
            "https://example.test/10,2026",
            "{February 10,2026}",
        ] {
            assert!(
                !protected_values_match(exact, &exact.replace(',', ", ")),
                "{exact}"
            );
        }
        assert!(!protected_values_match(
            "February 10,2026 https://example.test/a {count} 30",
            "10 февраля 2026 https://example.test/b {count} 30"
        ));
    }

    #[test]
    fn unicode_numeric_signs_cannot_be_lost_or_replaced() {
        for sign in ['−', '﹣', '－', '﹢', '＋', '±', '∓'] {
            for number in ["25", "0.2s", "6.5%"] {
                let source = format!("Change {sign}{number}");
                assert!(protected_values_match(&source, &source));
                assert!(
                    !protected_values_match(&source, &format!("Изменение {number}")),
                    "{source}"
                );
                assert!(
                    !protected_values_match(&source, &format!("Изменение +{number}")),
                    "{source}"
                );
            }
        }
    }

    #[test]
    fn numeric_signs_are_protected_when_attached_to_labels_or_adjacent_values() {
        for (source, changed) in [
            ("Damage:−25", "Урон:25"),
            ("id−25", "id25"),
            ("10−25", "10 25"),
            ("10＋25", "10 25"),
        ] {
            assert!(!protected_values_match(source, changed), "{source}");
        }
    }

    #[test]
    fn unicode_signs_preserve_decimal_locale_rules_without_relaxing_ids() {
        assert!(protected_values_match("Change −0.2s", "Изменение −0,2 с"));
        for source in [
            "id−0.2s",
            "id＋0.2s",
            "https://example.test/−0.2s",
            "{delay_−0.2s}",
        ] {
            assert!(
                !protected_values_match(source, &source.replace('.', ",")),
                "{source}"
            );
        }
    }

    #[test]
    fn attached_seconds_accept_equivalent_decimal_punctuation_at_complete_unit_boundaries() {
        for (source, translated) in [
            (
                "lasts 4.6 seconds. 0.2s after",
                "длится 4,6 секунды. Через 0,2 с",
            ),
            ("Delay 0.2s", "Задержка 0,2с"),
            ("Delay 0.2с", "Задержка 0,2s"),
            ("Delay (0.2s).", "Задержка (0,2с)."),
            ("Delay 0.2s, then", "Задержка 0,2 с, затем"),
            ("Delay 0.2s. Then", "Задержка 0,2с. Затем"),
            (
                "Delta -0.2s and +1.25s, chance 6.5%",
                "Изменение -0,2 с и +1,25с, шанс 6,5%",
            ),
            ("Twice 0.2s and 0.2s", "Дважды 0,2с и 0,2 с"),
        ] {
            assert!(
                protected_values_match(source, translated),
                "{source:?} -> {translated:?}"
            );
        }
    }

    #[test]
    fn attached_seconds_keep_numeric_digits_signs_counts_and_percent_exact() {
        for translated in [
            "0,3 с",
            "0,20 с",
            "-0,2 с",
            "+0,2 с",
            "0,2%",
            "0,2 с 0,2 с",
            "ноль секунд",
        ] {
            assert!(
                !protected_values_match("0.2s", translated),
                "{translated:?}"
            );
        }
        assert!(!protected_values_match("-0.2s", "0,2 с"));
        assert!(!protected_values_match("0.2s 0.2s", "0,2 с"));
        assert!(!protected_values_match("0.2%", "0,2 с"));
    }

    #[test]
    fn locale_normalization_never_reclassifies_identifiers_or_ambiguous_numbers_as_seconds() {
        for source in [
            "v0.2s",
            "id_0.2s",
            "id:0.2s",
            "id/0.2s",
            "id-0.2s",
            "id+0.2s",
            "0.2serial",
            "0.2s_id",
            "0.2s1",
            "0.2s-id",
            "0.2s/id",
            "0.2s:id",
            "0.2s+id",
            "0.2s.id",
            "0.2ms",
            "0.2S",
            "0.2С",
            "0.2%s",
            "0.2x",
            "0.2сек",
            "0.2seconds",
            "1.234s",
            "1.234.56s",
            "1,234.56s",
            "2026.10.04s",
            "https://example.test/0.2s",
            "{delay_0.2s}",
        ] {
            let changed = source.replace('.', ",");
            assert!(!protected_values_match(source, &changed), "{source:?}");
        }
    }

    #[test]
    fn exact_numeric_spellings_remain_valid_when_unit_spacing_changes() {
        for (source, translated) in [
            ("0.2s", "0.2 секунды"),
            ("0.2ms", "0.2 мс"),
            ("6.5s 6.5", "6.5 секунды 6.5с"),
            ("-0.2s and 35%", "-0.2 с и 35%"),
        ] {
            assert!(protected_values_match(source, translated));
        }
    }

    #[test]
    fn unchanged_exact_values_do_not_hide_a_changed_url_placeholder_or_repeated_number() {
        for (source, translated) in [
            (
                "0.2s https://example.test/a {delay}",
                "0,2 с https://example.test/b {delay}",
            ),
            (
                "0.2s https://example.test/a {delay}",
                "0,2 с https://example.test/a {other}",
            ),
            ("0.2s 25 25", "0,2 с 25"),
            ("0.2s ™ © ®", "0,2 с ™ ©"),
        ] {
            assert!(!protected_values_match(source, translated));
        }
    }
}
