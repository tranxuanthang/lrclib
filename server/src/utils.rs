use crate::{
    AppState,
    track_response::{TrackResponse},
};
use aho_corasick::AhoCorasick;
use collapse::collapse;
use moka::future::Cache;
use regex::Regex;
use secular::lower_lay_string;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::sync::Arc;

pub fn prepare_input(input: &str) -> String {
    let prepared_input = lower_lay_string(&input);

    // Normalize punctuation and control characters to spaces for matching.
    let patterns_to_space = vec![
        "`", "~", "!", "@", "#", "$", "%", "^", "&", "*", "(", ")", "_", "|", "+", "-", "=", "?",
        ";", ":", "\"", ",", ".", "<", ">", "{", "}", "[", "]", "\\", "/", "\x00", "\n",
    ];
    let to_space = AhoCorasick::new(&patterns_to_space).unwrap();
    let space_replacements = vec![" "; patterns_to_space.len()];
    let prepared_input = to_space.replace_all(&prepared_input, &space_replacements);

    // Remove apostrophes (ASCII and typographic) to reduce token variance.
    let patterns_to_remove = vec!["'", "\u{2019}"];
    let to_remove = AhoCorasick::new(&patterns_to_remove).unwrap();
    let prepared_input = to_remove.replace_all(&prepared_input, &["", ""]);

    // Collapse spacing and lowercase for stable comparisons.
    let prepared_input = prepared_input.to_lowercase();
    let prepared_input = collapse(&prepared_input);

    prepared_input
}

pub fn strip_timestamp(synced_lyrics: &str) -> String {
    let re = Regex::new(r"^\[(.*)\] *").unwrap();
    let plain_lyrics = re.replace_all(synced_lyrics, "");
    plain_lyrics.to_string()
}

// tokens

pub async fn is_valid_publish_token(
    publish_token: &str,
    challenge_cache: &Cache<String, String>,
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

fn duration_bucket(duration: f64) -> i64 {
    duration.round() as i64
}

/// Try to get cached response using multiple duration keys (±2 seconds)
pub async fn get_cached_metadata_response(
    state: &Arc<AppState>,
    track_name_lower: &str,
    artist_name_lower: &str,
    album_name_lower: Option<&str>,
    duration: f64,
) -> Option<TrackResponse> {
    let bucket = duration_bucket(duration);

    // Try cache keys for duration -2, -1, 0, +1, +2
    for offset in -2i64..=2i64 {
        let test_bucket = bucket + offset;
        let cache_key = build_get_metadata_cache_key(
            track_name_lower,
            artist_name_lower,
            album_name_lower,
            Some(test_bucket),
        );

        if let Some(cached) = state.get_metadata_cache.get(&cache_key).await {
            return Some(cached);
        }
    }

    None
}

pub fn build_get_metadata_cache_key(
    track_name_lower: &str,
    artist_name_lower: &str,
    album_name_lower: Option<&str>,
    duration_bucket: Option<i64>,
) -> String {
    let album = album_name_lower.unwrap_or("");
    let duration_value = duration_bucket.map(|d| d.to_string()).unwrap_or_default();

    format!(
        "get:v3:{}:{}:{}:{}",
        track_name_lower, artist_name_lower, album, duration_value,
    )
}

pub fn add_get_metadata_cache_index(state: &Arc<AppState>, track_id: i64, cache_key: &str) {
    let mut entry = state
        .get_metadata_index
        .entry(track_id)
        .or_insert_with(HashSet::new);
    if entry.len() < 64 {
        entry.insert(cache_key.to_owned());
    }
}

pub async fn invalidate_get_metadata_cache_for_track_id(state: &Arc<AppState>, track_id: i64) {
    let keys_to_remove = state
        .get_metadata_index
        .remove(&track_id)
        .map(|(_, keys)| keys.into_iter().collect::<Vec<_>>())
        .unwrap_or_default();

    for key in keys_to_remove {
        state.get_metadata_cache.remove(&key).await;
    }
}
