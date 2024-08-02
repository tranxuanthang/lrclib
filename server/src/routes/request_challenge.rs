use axum::{extract::State, Json};
use rand::{distributions::Alphanumeric, Rng};
use serde::Serialize;
use std::sync::Arc;
use crate::{errors::ApiError, AppState};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Challenge {
  prefix: String,
  target: String
}

pub async fn route(
  State(state): State<Arc<AppState>>
) -> Result<Json<Challenge>, ApiError> {
  let challenge = generate_challenge();

  state.challenge_cache.insert(format!("challenge:{}", challenge.prefix), challenge.target.to_owned()).await;

  Ok(Json(challenge))
}

fn generate_challenge() -> Challenge {
  let prefix: String = rand::thread_rng()
    .sample_iter(&Alphanumeric)
    .take(32)
    .map(char::from)
    .collect();
  let target = "000000FF00000000000000000000000000000000000000000000000000000000".to_owned();

  Challenge {
    prefix,
    target
  }
}
