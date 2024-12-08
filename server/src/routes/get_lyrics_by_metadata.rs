use axum::{extract::{Query, State}, Json};
use rusqlite::Connection;
use serde::{Deserialize,Serialize};
use std::sync::Arc;
use crate::{
    entities::{missing_track::MissingTrack, track::SimpleTrack},
    errors::ApiError,
    repositories::track_repository::get_track_by_metadata,
    utils::process_param,
    AppState,
};
use axum_macros::debug_handler;
use validator::Validate;
use anyhow::Result;
use crossbeam_queue::ArrayQueue;

#[derive(Clone, Validate, Deserialize)]
pub struct QueryParams {
  #[validate(length(min = 1, message = "cannot be empty"))]
  track_name: String,
  #[validate(length(min = 1, message = "cannot be empty"))]
  artist_name: String,
  album_name: Option<String>,
  #[validate(range(min = 1.0, max = 3600.0, message = "must be between 1 and 3600"))]
  duration: Option<f64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackResponse {
  id: i64,
  name: Option<String>,
  track_name: Option<String>,
  artist_name: Option<String>,
  album_name: Option<String>,
  duration: Option<f64>,
  instrumental: bool,
  plain_lyrics: Option<String>,
  synced_lyrics: Option<String>,
}

#[debug_handler]
pub async fn route(Query(params): Query<QueryParams>, State(state): State<Arc<AppState>>) -> Result<Json<TrackResponse>, ApiError> {
  params.validate().map_err(|e| ApiError::ValidationError(e.to_string()))?;

  // Process input parameters once
  let track_name_lower = process_param(Some(params.track_name.as_str()));
  let artist_name_lower = process_param(Some(params.artist_name.as_str()));
  let album_name_lower = process_param(params.album_name.as_deref());

  let mut conn = state.pool.get()?;

  if let (Some(track_name_lower), Some(artist_name_lower)) = (track_name_lower, artist_name_lower) {
    // Attempt to fetch the track with all provided metadata
    if let Some(track) = fetch_track(&track_name_lower, &artist_name_lower, album_name_lower.as_deref(), params.duration, &mut conn).await? {
      return Ok(Json(create_response(track)));
    }

    // If not found, handle missing track logic
    if let Err(e) = handle_missing_track(&params, &track_name_lower, &artist_name_lower, album_name_lower.as_deref(), &state).await {
      tracing::error!(message = "failed to handle missing track", error = e.to_string());
    }

    // Retry fetching the track without the album name
    // if album_name_lower.is_some() {
    //   if let Some(track) = fetch_track_without_album(&track_name_lower, &artist_name_lower, params.duration, &mut conn).await? {
    //     return Ok(Json(create_response(track)));
    //   }
    // }
  }

  Err(ApiError::TrackNotFoundError)
}

async fn fetch_track(track_name_lower: &str, artist_name_lower: &str, album_name_lower: Option<&str>, duration: Option<f64>, conn: &mut Connection) -> Result<Option<SimpleTrack>> {
  get_track_by_metadata(
    track_name_lower,
    artist_name_lower,
    album_name_lower,
    duration,
    conn,
  )
}

// async fn fetch_track_without_album(track_name_lower: &str, artist_name_lower: &str, duration: Option<f64>, conn: &mut Connection) -> Result<Option<SimpleTrack>> {
//   get_track_by_metadata(
//     track_name_lower,
//     artist_name_lower,
//     None,
//     duration,
//     conn,
//   )
// }

async fn handle_missing_track(
  params: &QueryParams,
  track_name_lower: &str,
  artist_name_lower: &str,
  album_name_lower: Option<&str>,
  state: &Arc<AppState>,
) -> Result<()> {
  if let (Some(album_name), Some(album_name_lower), Some(duration)) = (
    params.album_name.as_deref(),
    album_name_lower,
    params.duration,
  ) {
    let missing_track = MissingTrack {
      name: params.track_name.trim().to_owned(),
      artist_name: params.artist_name.trim().to_owned(),
      album_name: album_name.trim().to_owned(),
      duration,
    };

    let cache_key = format!("missing_track:{}:{}:{}:{}", track_name_lower, artist_name_lower, album_name_lower, duration);
    if !state.get_cache.contains_key(&cache_key) {
      state.get_cache.insert(cache_key, "1".to_owned()).await;
      send_to_queue(missing_track, &state.queue);
    }
  }

  Ok(())
}

fn create_response(track: SimpleTrack) -> TrackResponse {
  let plain_lyrics = match track.last_lyrics {
    Some(ref lyrics) => lyrics.plain_lyrics.to_owned(),
    None => None
  };

  let synced_lyrics = match track.last_lyrics {
    Some(ref lyrics) => lyrics.synced_lyrics.to_owned(),
    None => None
  };

  let instrumental = match track.last_lyrics {
    Some(ref lyrics) => lyrics.instrumental.to_owned(),
    None => false
  };

  TrackResponse {
    id: track.id,
    name: track.name.to_owned(),
    track_name: track.name.to_owned(),
    artist_name: track.artist_name.to_owned(),
    album_name: track.album_name.to_owned(),
    duration: track.duration,
    instrumental,
    plain_lyrics,
    synced_lyrics,
  }
}

fn send_to_queue(missing_track: MissingTrack, queue: &ArrayQueue<MissingTrack>) {
  match queue.push(missing_track.clone()) {
    Ok(_) => tracing::debug!(
      message = "sent missing track to queue",
      track_name = missing_track.name,
      artist_name = missing_track.artist_name,
      album_name = missing_track.album_name,
      duration = missing_track.duration,
    ),
    Err(missing_track) => tracing::debug!(
      message = "failed to push to queue",
      track_name = missing_track.name,
      artist_name = missing_track.artist_name,
      album_name = missing_track.album_name,
      duration = missing_track.duration,
    ),
  }
}
