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
use std::sync::Arc;
use crate::{errors::ApiError, repositories::track_repository, AppState};
use axum_macros::debug_handler;
use crate::utils::is_valid_publish_token;

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
