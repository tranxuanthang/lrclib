use ahash::RandomState as AHashRandomState;
use moka::future::Cache;
use sha2::{Digest, Sha256};

pub(crate) use lrclib_core::text::{has_alphanumeric, process_param};

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
