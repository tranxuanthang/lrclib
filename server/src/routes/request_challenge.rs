use axum::{extract::State, Json};
use rand::{distributions::Alphanumeric, Rng};
use serde::Serialize;
use std::sync::{atomic::Ordering, Arc};
use anyhow::Result;
use crate::{errors::ApiError, AppState};
use num_bigint::BigUint;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Challenge {
  prefix: String,
  target: String
}

pub async fn route(
  State(state): State<Arc<AppState>>
) -> Result<Json<Challenge>, ApiError> {
  let challenge = generate_challenge(&state).await?;

  state.challenge_cache.insert(format!("challenge:{}", challenge.prefix), challenge.target.to_owned()).await;

  Ok(Json(challenge))
}

async fn generate_challenge(state: &Arc<AppState>) -> Result<Challenge> {
  let prefix: String = rand::thread_rng()
    .sample_iter(&Alphanumeric)
    .take(32)
    .map(char::from)
    .collect();
  let last_10_mins_lyrics_count = state.recent_lyrics_count.load(Ordering::Relaxed);
  let base_target = b"000000FF00000000000000000000000000000000000000000000000000000000".to_owned();
  let base_submit_count = 100;
  let base_target_big_uint = BigUint::parse_bytes(&base_target, 16).unwrap();
  let target_big_uint = if last_10_mins_lyrics_count > base_submit_count {
    base_target_big_uint * base_submit_count as u64 / last_10_mins_lyrics_count as u64
  } else {
    base_target_big_uint
  };
  let target: String = format!("{:064X}", target_big_uint);
  Ok(Challenge {
    prefix,
    target
  })
}
