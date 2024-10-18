use anyhow::Result;
use axum::{
  extract::State,
  http::{
    StatusCode,
    HeaderMap,
  },
  Json,
};
use rusqlite::Connection;
use serde::Deserialize;
use std::sync::Arc;
use crate::{
  errors::ApiError,
  repositories::{lyrics_repository, track_repository},
  utils::{strip_timestamp, is_valid_publish_token},
  AppState
};
use axum_macros::debug_handler;
use regex::Regex;

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PublishRequest {
    track_name: String,
    artist_name: String,
    album_name: String,
    duration: f64,
    plain_lyrics: Option<String>,
    synced_lyrics: Option<String>,
}

#[debug_handler]
pub async fn route(
  headers: HeaderMap,
  State(state): State<Arc<AppState>>,
  Json(payload): Json<PublishRequest>,
) -> Result<StatusCode, ApiError> {
  match headers.get("X-Publish-Token") {
    Some(publish_token) => {
      let is_valid = is_valid_publish_token(publish_token.to_str()?, &state.challenge_cache).await;

      if is_valid {
        {
          let mut conn = state.pool.get()?;
          publish_lyrics(&payload, &mut conn)?;
        }

        Ok(StatusCode::CREATED)
      } else {
        Err(ApiError::IncorrectPublishTokenError)
      }
    },
    None => Err(ApiError::IncorrectPublishTokenError)
  }
}

fn publish_lyrics(payload: &PublishRequest, conn: &mut Connection) -> Result<()> {
  let mut tx = conn.transaction()?;

  let existing_track = track_repository::get_track_id_by_metadata_tx(
    &payload.track_name.trim(),
    &payload.artist_name.trim(),
    &payload.album_name.trim(),
    payload.duration,
    &mut tx,
  )?;

  let track_id = match existing_track {
    Some(track_id) => track_id,
    None => track_repository::add_one_tx(
      &payload.track_name.trim(),
      &payload.artist_name.trim(),
      &payload.album_name.trim(),
      payload.duration,
      &mut tx,
    )?
  };

  let mut plain_lyrics = payload.plain_lyrics.as_ref().filter(|s| !s.is_empty()).map(|s| s.to_owned());
  let synced_lyrics = payload.synced_lyrics.as_ref().filter(|s| !s.is_empty()).map(|s| s.to_owned());

  // Generate plain_lyrics from synced_lyrics
  if plain_lyrics.is_none() && synced_lyrics.is_some() {
    plain_lyrics = Some(strip_timestamp(synced_lyrics.as_deref().unwrap()));
  }

  // Create a regex to match "[au: instrumental]" or "[au:instrumental]"
  let re = Regex::new(r"\[au:\s*instrumental\]").expect("Invalid regex");
  let is_instrumental = synced_lyrics.as_ref().map_or(false, |lyrics| re.is_match(lyrics));

  if is_instrumental {
    // Mark the track as instrumental
    lyrics_repository::add_one_tx(
      &None,
      &None,
      track_id,
      true,
      &Some("lrclib".to_owned()),
      &mut tx,
    )?;
  } else {
    lyrics_repository::add_one_tx(
      &plain_lyrics,
      &synced_lyrics,
      track_id,
      false,
      &Some("lrclib".to_owned()),
      &mut tx,
    )?;
  }

  tx.commit()?;

  Ok(())
}
