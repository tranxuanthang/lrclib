use aho_corasick::AhoCorasick;
use collapse::collapse;
use lazy_static::lazy_static;
use secular::lower_lay_string;

lazy_static! {
    static ref PATTERNS_TO_SPACE: Vec<&'static str> = vec![
        "`", "~", "!", "@", "#", "$", "%", "^", "&", "*", "(", ")", "_", "|", "+", "-", "=", "?",
        ";", ":", "\"", ",", ".", "<", ">", "{", "}", "[", "]", "\\", "/", "\x00", "\n",
    ];
    static ref SPACE_REPLACEMENTS: Vec<&'static str> = vec![" "; PATTERNS_TO_SPACE.len()];
    static ref TO_SPACE: AhoCorasick = AhoCorasick::new(PATTERNS_TO_SPACE.iter()).unwrap();
    static ref PATTERNS_TO_REMOVE: Vec<&'static str> = vec!["'", "\u{2019}"];
    static ref EMPTY_REPLACEMENTS: Vec<&'static str> = vec!["", ""];
    static ref TO_REMOVE: AhoCorasick = AhoCorasick::new(PATTERNS_TO_REMOVE.iter()).unwrap();
}

pub fn prepare_input(input: &str) -> String {
    let prepared_input = lower_lay_string(input);
    let prepared_input = TO_SPACE.replace_all(&prepared_input, &SPACE_REPLACEMENTS);
    let prepared_input = TO_REMOVE.replace_all(&prepared_input, &EMPTY_REPLACEMENTS);
    let prepared_input = prepared_input.to_lowercase();
    let prepared_input = collapse(&prepared_input);

    // Punctuation-only values normalize to an empty string, which would make
    // them indistinguishable from a missing parameter. Keep the literal value
    // so metadata lookups can match it and the search index can tokenize it.
    if prepared_input.is_empty() {
        input.trim().to_lowercase()
    } else {
        prepared_input
    }
}

/// Whether text contains at least one character the search tokenizer would
/// emit as part of a token.
pub fn has_alphanumeric(input: &str) -> bool {
    input.chars().any(char::is_alphanumeric)
}

pub fn process_param(param: Option<&str>) -> Option<String> {
    param
        .as_ref()
        .map(|s| prepare_input(s))
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.to_owned())
}

#[cfg(test)]
mod tests {
    use super::{prepare_input, process_param};

    #[test]
    fn prepare_input_keeps_punctuation_only_values() {
        assert_eq!(prepare_input("?"), "?");
        assert_eq!(prepare_input("!!!"), "!!!");
        assert_eq!(prepare_input("  ?  "), "?");
        assert_eq!(prepare_input("'"), "'");
    }

    #[test]
    fn prepare_input_still_normalizes_mixed_values() {
        assert_eq!(prepare_input("Don't Stop!"), "dont stop");
        assert_eq!(prepare_input("AC/DC"), "ac dc");
    }

    #[test]
    fn process_param_keeps_punctuation_only_values() {
        assert_eq!(process_param(Some("?")).as_deref(), Some("?"));
        assert_eq!(process_param(Some("   ")), None);
    }
}
