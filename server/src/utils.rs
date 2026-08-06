use ahash::RandomState as AHashRandomState;
use aho_corasick::AhoCorasick;
use collapse::collapse;
use lazy_static::lazy_static;
use moka::future::Cache;
use secular::lower_lay_string;
use sha2::{Digest, Sha256};

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
    let prepared_input = lower_lay_string(&input);
    let prepared_input = TO_SPACE.replace_all(&prepared_input, &SPACE_REPLACEMENTS);
    let prepared_input = TO_REMOVE.replace_all(&prepared_input, &EMPTY_REPLACEMENTS);
    let prepared_input = prepared_input.to_lowercase();
    collapse(&prepared_input)
}

// tokens

pub async fn is_valid_publish_token(
    publish_token: &str,
    challenge_cache: &Cache<String, String, AHashRandomState>,
) -> bool {
    let publish_token_parts = publish_token.split(":").collect::<Vec<&str>>();

    if publish_token_parts.len() != 2 {
        return false;
    }

    let prefix = publish_token_parts[0];
    let nonce = publish_token_parts[1];
    let target = challenge_cache.get(&format!("challenge:{}", prefix)).await;

    match target {
        Some(target) => {
            let result = verify_answer(prefix, &target, nonce);

            if result {
                challenge_cache
                    .remove(&format!("challenge:{}", prefix))
                    .await;
                true
            } else {
                false
            }
        }
        None => false,
    }
}

pub fn verify_answer(prefix: &str, target: &str, nonce: &str) -> bool {
    let input = format!("{}{}", prefix, nonce);
    let mut hasher = Sha256::new();
    hasher.update(input);
    let hashed_bytes = hasher.finalize();

    let target_bytes = match hex::decode(target) {
        Ok(bytes) => bytes,
        Err(_) => return false,
    };

    if target_bytes.len() != hashed_bytes.len() {
        return false;
    }

    for (hashed_byte, target_byte) in hashed_bytes.iter().zip(target_bytes.iter()) {
        if hashed_byte > target_byte {
            return false;
        }
        if hashed_byte < target_byte {
            break;
        }
    }

    true
}

pub fn process_param(param: Option<&str>) -> Option<String> {
    param
        .as_ref()
        .map(|s| prepare_input(s))
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.to_owned())
}

