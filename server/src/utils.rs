use moka::future::Cache;
use sha2::{Digest, Sha256};
use secular::lower_lay_string;
use regex::Regex;
use collapse::collapse;

pub fn prepare_input(input: &str) -> String {
  let mut prepared_input = lower_lay_string(&input);

  let re = Regex::new(r#"[`~!@#$%^&*()_|+\-=?;:",.<>\{\}\[\]\\\/]"#).unwrap();
  prepared_input = re.replace_all(&prepared_input, " ").to_string();

  let re = Regex::new(r#"['’]"#).unwrap();
  prepared_input = re.replace_all(&prepared_input, "").to_string();

  prepared_input = prepared_input.to_lowercase();
  prepared_input = collapse(&prepared_input);

  prepared_input
}

pub fn strip_timestamp(synced_lyrics: &str) -> String {
  let re = Regex::new(r"^\[(.*)\] *").unwrap();
  let plain_lyrics = re.replace_all(synced_lyrics, "");
  plain_lyrics.to_string()
}

// tokens

pub async fn is_valid_publish_token(publish_token: &str, challenge_cache: &Cache<String, String>) -> bool {
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
        challenge_cache.remove(&format!("challenge:{}", prefix)).await;
        true
      } else {
        false
      }
    },
    None => {
      false
    }
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
