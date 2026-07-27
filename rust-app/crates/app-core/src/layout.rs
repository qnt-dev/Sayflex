#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutDirection {
    ToCyrillic,
    ToLatin,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutCandidate {
    pub direction: LayoutDirection,
    pub text: String,
}

const PAIRS: &[(char, char)] = &[
    ('`', 'ё'),
    ('q', 'й'),
    ('w', 'ц'),
    ('e', 'у'),
    ('r', 'к'),
    ('t', 'е'),
    ('y', 'н'),
    ('u', 'г'),
    ('i', 'ш'),
    ('o', 'щ'),
    ('p', 'з'),
    ('[', 'х'),
    (']', 'ъ'),
    ('a', 'ф'),
    ('s', 'ы'),
    ('d', 'в'),
    ('f', 'а'),
    ('g', 'п'),
    ('h', 'р'),
    ('j', 'о'),
    ('k', 'л'),
    ('l', 'д'),
    (';', 'ж'),
    ('\'', 'э'),
    ('z', 'я'),
    ('x', 'ч'),
    ('c', 'с'),
    ('v', 'м'),
    ('b', 'и'),
    ('n', 'т'),
    ('m', 'ь'),
    (',', 'б'),
    ('.', 'ю'),
    ('/', '.'),
];

#[must_use]
pub fn build_layout_candidates(source: &str) -> Vec<LayoutCandidate> {
    let mut candidates = Vec::with_capacity(2);
    let cyrillic = convert_layout(source, LayoutDirection::ToCyrillic);
    if cyrillic != source && cyrillic.chars().any(is_cyrillic) {
        candidates.push(LayoutCandidate {
            direction: LayoutDirection::ToCyrillic,
            text: cyrillic,
        });
    }
    let latin = convert_layout(source, LayoutDirection::ToLatin);
    if latin != source
        && latin
            .chars()
            .any(|character| character.is_ascii_alphabetic())
    {
        candidates.push(LayoutCandidate {
            direction: LayoutDirection::ToLatin,
            text: latin,
        });
    }
    candidates
}

#[must_use]
pub fn convert_layout(source: &str, direction: LayoutDirection) -> String {
    let characters = source.chars().collect::<Vec<_>>();
    let mut output = String::with_capacity(source.len());
    for (index, character) in characters.iter().copied().enumerate() {
        let converted = convert_character(character, direction);
        if should_preserve_punctuation(&characters, index, character, converted) {
            output.push(character);
        } else {
            output.push(converted);
        }
    }
    output
}

fn convert_character(character: char, direction: LayoutDirection) -> char {
    let lower = character.to_lowercase().next().unwrap_or(character);
    for &(latin, cyrillic) in PAIRS {
        let (source, target) = match direction {
            LayoutDirection::ToCyrillic => (latin, cyrillic),
            LayoutDirection::ToLatin => (cyrillic, latin),
        };
        if lower == source {
            return if character.is_uppercase() {
                target.to_uppercase().next().unwrap_or(target)
            } else {
                target
            };
        }
    }
    character
}

fn should_preserve_punctuation(
    source: &[char],
    index: usize,
    character: char,
    converted: char,
) -> bool {
    if character.is_alphabetic() || !converted.is_alphabetic() {
        return false;
    }
    if character == '[' && source[index + 1..].contains(&']') {
        return true;
    }
    if character == ']' && source[..index].contains(&'[') {
        return true;
    }
    if !matches!(character, ',' | '.' | ';' | '/') {
        return false;
    }
    source.get(index + 1).is_none_or(|next| {
        next.is_whitespace() || matches!(next, ')' | ']' | '}' | '!' | '?' | ':' | ';')
    })
}

fn is_cyrillic(character: char) -> bool {
    ('\u{0400}'..='\u{04ff}').contains(&character)
}

#[cfg(test)]
mod tests {
    use super::{LayoutDirection, build_layout_candidates, convert_layout};

    #[test]
    fn converts_multiline_selection_in_both_directions() {
        assert_eq!(
            convert_layout("ghbdtn vbh", LayoutDirection::ToCyrillic),
            "привет мир"
        );
        assert_eq!(
            convert_layout("Привет мир", LayoutDirection::ToLatin),
            "Ghbdtn vbh"
        );
    }

    #[test]
    fn preserves_structural_and_trailing_punctuation() {
        assert_eq!(
            convert_layout("[ghbdtn], vbh.", LayoutDirection::ToCyrillic),
            "[привет], мир."
        );
    }

    #[test]
    fn returns_only_changed_meaningful_candidates() {
        let candidates = build_layout_candidates("ghbdtn");
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].text, "привет");
    }
}
