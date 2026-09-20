pub fn paragraph_normalize(text: &str) -> String {
    let line_normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    line_normalized
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        .trim_matches('\n')
        .to_owned()
}

pub fn normalized_cer(expected: &str, actual: &str) -> f64 {
    let expected = paragraph_normalize(expected);
    let actual = paragraph_normalize(actual);
    let reference = expected.chars().collect::<Vec<_>>();
    let hypothesis = actual.chars().collect::<Vec<_>>();

    if reference.is_empty() {
        return if hypothesis.is_empty() { 0.0 } else { 1.0 };
    }

    levenshtein_distance(&reference, &hypothesis) as f64 / reference.len() as f64
}

fn levenshtein_distance(reference: &[char], hypothesis: &[char]) -> usize {
    let mut previous = (0..=hypothesis.len()).collect::<Vec<_>>();
    let mut current = vec![0; hypothesis.len() + 1];

    for (row, expected) in reference.iter().enumerate() {
        current[0] = row + 1;
        for (column, actual) in hypothesis.iter().enumerate() {
            let substitution = previous[column] + usize::from(expected != actual);
            let insertion = current[column] + 1;
            let deletion = previous[column + 1] + 1;
            current[column + 1] = substitution.min(insertion).min(deletion);
        }
        std::mem::swap(&mut previous, &mut current);
    }

    previous[hypothesis.len()]
}

#[cfg(test)]
mod tests {
    use super::{normalized_cer, paragraph_normalize};

    #[test]
    fn exact_text_has_zero_character_error_rate() {
        assert_eq!(normalized_cer("abc", "abc"), 0.0);
    }

    #[test]
    fn one_substitution_is_one_third_error() {
        assert_eq!(normalized_cer("abc", "axc"), 1.0 / 3.0);
    }

    #[test]
    fn paragraph_normalization_preserves_lines_but_removes_transport_noise() {
        assert_eq!(paragraph_normalize("A\r\nB  "), "A\nB");
    }

    #[test]
    fn empty_reference_is_well_defined() {
        assert_eq!(normalized_cer("", ""), 0.0);
        assert_eq!(normalized_cer("", "noise"), 1.0);
    }
}
