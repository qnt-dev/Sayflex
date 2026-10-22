//! Latin and Cyrillic letters that look the same.
//!
//! A recognizer trained on both alphabets sometimes reads one letter of a word
//! in the other script: "Windоws" with a Cyrillic "о", "°С" with a Cyrillic
//! "С". The text looks right and then fails search, copy and translation. A
//! word written in one script is rewritten so that its look-alike letters use
//! that script too.

/// Look-alike pairs: (Latin, Cyrillic).
const PAIRS: [(char, char); 22] = [
    ('a', 'а'),
    ('c', 'с'),
    ('e', 'е'),
    ('i', 'і'),
    ('j', 'ј'),
    ('o', 'о'),
    ('p', 'р'),
    ('s', 'ѕ'),
    ('x', 'х'),
    ('y', 'у'),
    ('A', 'А'),
    ('B', 'В'),
    ('C', 'С'),
    ('E', 'Е'),
    ('H', 'Н'),
    ('I', 'І'),
    ('K', 'К'),
    ('M', 'М'),
    ('O', 'О'),
    ('P', 'Р'),
    ('T', 'Т'),
    ('X', 'Х'),
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Script {
    Latin,
    Cyrillic,
}

fn script(character: char) -> Option<Script> {
    if character.is_ascii_alphabetic() || ('\u{00C0}'..='\u{024F}').contains(&character) {
        Some(Script::Latin)
    } else if ('\u{0400}'..='\u{04FF}').contains(&character) {
        Some(Script::Cyrillic)
    } else {
        None
    }
}

fn is_lookalike(character: char) -> bool {
    PAIRS
        .iter()
        .any(|(latin, cyrillic)| *latin == character || *cyrillic == character)
}

fn to_script(character: char, target: Script) -> char {
    PAIRS
        .iter()
        .find_map(|(latin, cyrillic)| match target {
            Script::Latin if *cyrillic == character => Some(*latin),
            Script::Cyrillic if *latin == character => Some(*cyrillic),
            _ => None,
        })
        .unwrap_or(character)
}

/// The script of a word's letters that have no look-alike, when they agree.
fn dominant_script(word: &[char]) -> Option<Script> {
    let mut latin = 0;
    let mut cyrillic = 0;
    for character in word
        .iter()
        .copied()
        .filter(|character| !is_lookalike(*character))
    {
        match script(character) {
            Some(Script::Latin) => latin += 1,
            Some(Script::Cyrillic) => cyrillic += 1,
            None => {}
        }
    }
    match (latin, cyrillic) {
        (0, 0) => None,
        (latin, cyrillic) if latin > cyrillic => Some(Script::Latin),
        (latin, cyrillic) if cyrillic > latin => Some(Script::Cyrillic),
        _ => None,
    }
}

fn normalize_word(word: &[char], previous: Option<char>, output: &mut String) {
    let has_latin = word.iter().any(|c| script(*c) == Some(Script::Latin));
    let has_cyrillic = word.iter().any(|c| script(*c) == Some(Script::Cyrillic));
    // A unit after a degree sign is Latin: °C, °F.
    let degree_unit = previous == Some('°') && word.len() == 1;
    let target = if degree_unit {
        Some(Script::Latin)
    } else if has_latin && has_cyrillic {
        dominant_script(word)
    } else {
        None
    };
    match target {
        Some(target) => output.extend(word.iter().map(|c| to_script(*c, target))),
        None => output.extend(word.iter()),
    }
}

/// Rewrites look-alike letters of mixed-script words into the word's script.
#[must_use]
pub fn normalize_homoglyphs(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut word = Vec::new();
    let mut before_word = None;
    let mut previous = None;
    for character in text.chars() {
        if script(character).is_some() {
            if word.is_empty() {
                before_word = previous;
            }
            word.push(character);
        } else {
            if !word.is_empty() {
                normalize_word(&word, before_word, &mut output);
                word.clear();
            }
            output.push(character);
        }
        previous = Some(character);
    }
    if !word.is_empty() {
        normalize_word(&word, before_word, &mut output);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::normalize_homoglyphs;

    #[test]
    fn a_latin_word_gets_its_latin_letters_back() {
        assert_eq!(
            normalize_homoglyphs("Обновите Windоws 11"),
            "Обновите Windows 11"
        );
        assert_eq!(normalize_homoglyphs("Ехсel"), "Excel");
    }

    #[test]
    fn a_cyrillic_word_gets_its_cyrillic_letters_back() {
        assert_eq!(normalize_homoglyphs("Пpивет"), "Привет");
        assert_eq!(normalize_homoglyphs("готoво"), "готово");
    }

    #[test]
    fn degree_units_are_latin() {
        assert_eq!(normalize_homoglyphs("−12 °С"), "−12 °C");
    }

    #[test]
    fn single_script_and_ambiguous_words_are_left_alone() {
        for text in [
            "Привет мир",
            "Hello world",
            "ВАС",
            "BAC",
            "Status: готово — build 42",
        ] {
            assert_eq!(normalize_homoglyphs(text), text, "{text}");
        }
    }
}
