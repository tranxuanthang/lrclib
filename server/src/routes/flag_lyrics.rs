use anyhow::Result;
use axum::{
  extract::State,
  http::{
    StatusCode,
    HeaderMap,
  },
  Json,
};
use serde::Deserialize;
use moka::future::Cache;
use std::sync::Arc;
use crate::{errors::ApiError, repositories::track_repository, AppState};
use sha2::{Digest, Sha256};
use hex;
use axum_macros::debug_handler;

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct FlagLyricsRequest {
    track_id: i64,
    content: Option<String>,
}

#[debug_handler]
pub async fn route(
  headers: HeaderMap,
  State(state): State<Arc<AppState>>,
  Json(payload): Json<FlagLyricsRequest>,
) -> Result<StatusCode, ApiError> {
  match headers.get("X-Publish-Token") {
    Some(publish_token) => {
      let is_valid = is_valid_publish_token(publish_token.to_str()?, &state.challenge_cache).await;

      if is_valid {
        let content = payload.content.unwrap_or("".to_string());
        let track_id = payload.track_id;
        let mut conn = state.pool.get()?;
        track_repository::flag_track_last_lyrics(track_id, &content, &mut conn)?;

        Ok(StatusCode::CREATED)
      } else {
        Err(ApiError::IncorrectPublishTokenError)
      }
    },
    None => Err(ApiError::IncorrectPublishTokenError)
  }
}

async fn is_valid_publish_token(publish_token: &str, challenge_cache: &Cache<String, String>) -> bool {
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
