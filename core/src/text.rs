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
    collapse(&prepared_input)
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
    fn prepare_input_normalizes_mixed_values() {
        assert_eq!(prepare_input("Don't Stop!"), "dont stop");
        assert_eq!(prepare_input("AC/DC"), "ac dc");
    }

    #[test]
    fn process_param_drops_empty_values() {
        assert_eq!(process_param(Some("   ")), None);
        assert_eq!(process_param(Some("Track")), Some("track".to_owned()));
    }
}
